// SPDX-License-Identifier: AGPL-3.0-only

//! One handover: what it packs, what it verifies, and what it records
//! (`docs/specs/wave3-anonymize-and-bids.md`, §11).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nils_registry::schema::{Type, table};
use nils_registry::store::{Insert, Param, Store};
use nils_registry::{Registry, time::now_iso};

use super::archive::{Archiver, Par2};
use super::plan::{self, Strategy};

/// What one handover needs.
pub struct Settings<'a> {
    /// The release, by name. Its newest finished version is the one handed
    /// over, because that is the one the tree is.
    pub release: &'a str,
    /// Where the archives go, which is not where the tree is.
    pub out: &'a Path,
    pub archiver: &'a Archiver,
    /// How the archives are kept closed (record 55, ruling 35 (c)).
    pub protection: Protection<'a>,
    pub cap: i64,
    pub strategy: Strategy,
    /// `7z`'s compression level. Low by default: a released tree is mostly
    /// NIfTI that is already gzipped, and the time costs more than the bytes.
    pub level: u8,
    pub par2: Option<Par2>,
    /// Read every archive back before saying it is done.
    pub verify: bool,
    pub actor: &'a str,
}

/// How a handover's archives are kept closed (record 55, ruling 35 (c),
/// Nima 2026-10-16): each handover has a password of its own, or none for a
/// reason a person gave. Never a password shared by every handover under a
/// key, which is what a handover from before registry schema 105 has.
pub enum Protection<'a> {
    /// A password of its own, derived from this key and the handover's id
    /// ([`super::handover_password`]); never stored.
    Made { key_name: &'a str, key: &'a [u8] },
    /// A password the person handing over typed. `sealed` is it under the
    /// registry's key (`key_name`), which is all the row keeps.
    Typed {
        key_name: &'a str,
        password: &'a str,
        sealed: &'a [u8],
    },
    /// Not encrypted, and why; the row keeps the reason.
    None { why: &'a str },
}

impl Protection<'_> {
    fn kind(&self) -> super::Kind {
        match self {
            Protection::Made { .. } => super::Kind::Made,
            Protection::Typed { .. } => super::Kind::Typed,
            Protection::None { .. } => super::Kind::None,
        }
    }

    fn key_name(&self) -> &str {
        match self {
            Protection::Made { key_name, .. } | Protection::Typed { key_name, .. } => key_name,
            Protection::None { .. } => "",
        }
    }

    /// The password of the handover with this id, or none.
    fn password(&self, handover: i64) -> Option<String> {
        match self {
            Protection::Made { key, .. } => Some(super::handover_password(key, handover)),
            Protection::Typed { password, .. } => Some(password.to_string()),
            Protection::None { .. } => None,
        }
    }
}

/// What a handover says when it is done.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Report {
    pub handover_id: i64,
    pub release: String,
    /// The version of it that was packed, which is the one the tree is.
    pub version: String,
    pub root: String,
    pub out: String,
    pub tool: String,
    pub strategy: String,
    pub archives: i64,
    pub files: i64,
    /// The bytes of the tree, and the bytes of the archives, which is what a
    /// recipient has to receive.
    pub bytes: i64,
    pub packed_bytes: i64,
    pub subjects: i64,
    pub verified: i64,
    /// Files the release recorded and the tree no longer holds. Never a silent
    /// skip: a handover of a tree somebody edited is not a handover of the
    /// release.
    pub missing: i64,
    pub failed: Vec<String>,
    pub seconds: f64,
    /// `made`, `typed` or `none` (record 55, ruling 35 (c)).
    pub password: String,
}

#[derive(Debug)]
pub enum Error {
    Store(nils_registry::store::Error),
    Refused(String),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Refused(m) => f.write_str(m),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<nils_registry::store::Error> for Error {
    fn from(e: nils_registry::store::Error) -> Error {
        Error::Store(e)
    }
}

/// The version of a release that a handover packs, with its root.
pub struct Version {
    pub id: i64,
    pub dataset: i64,
    pub version: String,
    pub root: String,
}

/// Pack a release into archives, and record what was sent.
/// A handover is a job (Wave 4a §9.1), like the release it ships.
pub fn run(registry: &mut Registry, settings: &Settings) -> Result<Report, Error> {
    use nils_registry::job;
    let job_id = job::claim(
        registry.store(),
        &job::Claim {
            kind: "handover",
            name: settings.release,
            args: serde_json::json!({ "out": settings.out.display().to_string() }),
        },
    )
    .map_err(|e| match e {
        job::Error::Store(e) => Error::Store(e),
        other => Error::Refused(other.to_string()),
    })?;
    let result = run_handover(registry, settings);
    let (state, error) = match &result {
        Ok(_) => (job::State::Done, None),
        Err(e) => (job::State::Failed, Some(e.to_string())),
    };
    let _ = job::finish(registry.store(), job_id, state, error.as_deref());
    // Wave 4a §9.2: the audit row.
    if result.is_ok() {
        nils_registry::audit::record(
            registry,
            &nils_registry::audit::Entry {
                principal: settings.actor,
                action: nils_registry::audit::Action::Handover,
                scope: serde_json::json!({
                    "release": settings.release,
                    "out": settings.out.display().to_string(),
                    "handover": result.as_ref().map(|r| r.handover_id).unwrap_or_default(),
                    "encrypted": !matches!(settings.protection, Protection::None { .. }),
                    "password": settings.protection.kind().word(),
                }),
                policy: None,
                job_id: Some(job_id),
                details: match &settings.protection {
                    Protection::None { why } => Some(serde_json::json!({ "unencrypted_why": why })),
                    _ => None,
                },
            },
        )
        .map_err(Error::Store)?;
    }
    result
}

fn run_handover(registry: &mut Registry, settings: &Settings) -> Result<Report, Error> {
    let started = std::time::Instant::now();
    let found = newest(registry.store(), settings.release)?;
    let Some(found) = found else {
        return Err(Error::Refused(format!(
            "no finished release named {}",
            settings.release
        )));
    };
    if let Some(words) = not_whole(registry.store(), &found)? {
        return Err(Error::Refused(words));
    }
    // The tree as the release recorded it, and the tree as it is now. A file
    // the record names and the disk does not is reported rather than skipped:
    // a handover of a tree somebody edited is not a handover of the release.
    let root = PathBuf::from(&found.root);
    let (present, missing) = files_of(registry.store(), found.dataset, &root)?;
    if present.is_empty() && missing == 0 {
        return Err(Error::Refused(format!(
            "version {} of {} wrote no files",
            found.version, settings.release
        )));
    }

    let members = plan::members(&present);
    let chunks = plan::chunks(&members, settings.cap, settings.strategy);
    let mut report = Report {
        release: settings.release.to_string(),
        version: found.version.clone(),
        root: found.root.clone(),
        out: settings.out.display().to_string(),
        tool: settings.archiver.describe(),
        strategy: settings.strategy.name().to_string(),
        files: present.len() as i64,
        bytes: present.iter().map(|(_, b)| b).sum(),
        subjects: members.iter().filter(|m| !m.subject.is_empty()).count() as i64,
        missing,
        password: settings.protection.kind().word().to_string(),
        ..Report::default()
    };
    std::fs::create_dir_all(settings.out).map_err(Error::Io)?;
    report.handover_id = open_row(registry.store(), settings, &found, &report)?;
    // its own password, now that it has an id; held here and nowhere else
    let password = settings.protection.password(report.handover_id);

    let codes = subject_ids(registry.store())?;
    let mut rows: Vec<Vec<Param>> = Vec::new();
    let mut people: Vec<(usize, Vec<Param>)> = Vec::new();
    for chunk in &chunks {
        let name = chunk.name(settings.release);
        let path = settings.out.join(&name);
        std::fs::remove_file(&path).ok();
        let made = settings
            .archiver
            .pack(
                &root,
                &chunk.entries(),
                &path,
                password.as_deref(),
                settings.level,
            )
            .and_then(|()| match &settings.par2 {
                Some(p) => p.cover(&path),
                None => Ok(()),
            });
        let (digest, bytes) = match &made {
            Ok(()) => digest_of(&path),
            Err(_) => (String::new(), 0),
        };
        let verified = match (&made, settings.verify) {
            (Ok(()), true) => settings.archiver.verify(&path, password.as_deref()).err(),
            _ => None,
        };
        let failed = made.as_ref().err().cloned().or_else(|| verified.clone());
        match &failed {
            None => {
                report.archives += 1;
                report.packed_bytes += bytes;
                if settings.verify {
                    report.verified += 1;
                }
            }
            Some(why) => report.failed.push(format!("{name}: {why}")),
        }
        rows.push(vec![
            Param::Int(report.handover_id),
            Param::Int(chunk.ordinal as i64),
            Param::from(name.as_str()),
            Param::from(digest.as_str()),
            Param::Int(bytes),
            Param::Int(chunk.files() as i64),
            Param::Int(
                chunk
                    .members
                    .iter()
                    .filter(|m| !m.subject.is_empty())
                    .count() as i64,
            ),
            match (settings.verify, &failed) {
                (true, None) => Param::from(now_iso()),
                _ => Param::Null,
            },
            match &failed {
                Some(why) => Param::from(why.as_str()),
                None => Param::Null,
            },
        ]);
        for m in &chunk.members {
            if m.subject.is_empty() {
                continue;
            }
            people.push((
                chunk.ordinal,
                vec![
                    Param::Int(codes.get(&m.subject).copied().unwrap_or(0)),
                    Param::from(m.subject.as_str()),
                    Param::Int(m.paths.len() as i64),
                    Param::Int(m.bytes),
                ],
            ));
        }
    }

    write_archives(registry.store(), &rows)?;
    write_subjects(registry.store(), report.handover_id, &people)?;
    // The manifest beside the archives, because a recipient has the archives
    // and not the registry.
    write_manifest(
        settings.out,
        &report,
        &chunks,
        settings.release,
        password.is_some(),
    )
    .map_err(Error::Io)?;
    close_row(registry.store(), &report)?;
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

/// Read a handover back: every archive still there, still its own digest, and
/// still openable.
pub fn verify(
    registry: &mut Registry,
    handover: i64,
    archiver: &Archiver,
    password: Option<&str>,
) -> Result<Report, Error> {
    let started = std::time::Instant::now();
    let d = registry.store().dialect();
    let sql = format!(
        "SELECT root, release_id FROM {} WHERE id = {}",
        registry.store().qualified("handover"),
        d.param(1, Type::Int),
    );
    let Some(row) = registry.store().query_opt(&sql, &[Param::Int(handover)])? else {
        return Err(Error::Refused(format!("no handover {handover}")));
    };
    let out = PathBuf::from(row.text(0)?);
    let sql = format!(
        "SELECT id, name, digest, bytes FROM {} WHERE handover_id = {} ORDER BY ordinal",
        registry.store().qualified("handover_archive"),
        registry.store().dialect().param(1, Type::Int),
    );
    let archives = registry.store().query(&sql, &[Param::Int(handover)])?;
    let mut report = Report {
        handover_id: handover,
        out: out.display().to_string(),
        tool: archiver.describe(),
        ..Report::default()
    };
    let mut verified: Vec<(i64, Option<String>)> = Vec::new();
    for r in &archives {
        let name = r.text(1)?.to_string();
        let path = out.join(&name);
        report.archives += 1;
        let why = match path.is_file() {
            false => Some("it is not there".to_string()),
            true => {
                let (digest, bytes) = digest_of(&path);
                report.packed_bytes += bytes;
                match digest == r.text(2)? {
                    // The digest first, because it says the file is the file
                    // without a password; then the password, because an
                    // archive nobody can open was not handed over.
                    true => archiver.verify(&path, password).err(),
                    false => Some("its checksum is not the one recorded".to_string()),
                }
            }
        };
        match &why {
            None => report.verified += 1,
            Some(why) => report.failed.push(format!("{name}: {why}")),
        }
        verified.push((r.int(0)?, why));
    }
    let now = now_iso();
    for (id, why) in verified {
        let d = registry.store().dialect();
        let sql = format!(
            "UPDATE {} SET verified_at = {}, error = {} WHERE id = {}",
            registry.store().qualified("handover_archive"),
            d.param(1, Type::Timestamp),
            d.param(2, Type::Text),
            d.param(3, Type::Int),
        );
        let params = [
            match why {
                None => Param::from(now.as_str()),
                Some(_) => Param::Null,
            },
            match &why {
                Some(w) => Param::from(w.as_str()),
                None => Param::Null,
            },
            Param::Int(id),
        ];
        registry.store().execute(&sql, &params)?;
    }
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

/// A handover as its row keeps it: how it is kept closed, and the release
/// version it handed over with who made that version (record 55, ruling 35
/// (c): the maker may see and set its password).
#[derive(Debug, Clone)]
pub struct Kept {
    pub id: i64,
    pub kind: super::Kind,
    pub key_name: String,
    pub sealed: Option<Vec<u8>>,
    pub why: Option<String>,
    pub release: String,
    pub version: String,
    pub maker: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub archives: i64,
    pub root: String,
}

impl Kept {
    /// What a list answers: never the sealed password.
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "release": self.release,
            "version": self.version,
            "maker": self.maker,
            "root": self.root,
            "started_at": self.started_at,
            "finished_at": self.finished_at,
            "archives": self.archives,
            "encrypted": self.kind != super::Kind::None,
            "password": match self.kind { super::Kind::Key => "key", k => k.word() },
            "password_words": self.kind.words(),
            "key": if self.kind == super::Kind::Typed || self.kind == super::Kind::None { None } else { Some(self.key_name.as_str()) },
            "unencrypted_why": self.why,
        })
    }
}

/// The handovers, newest first: of one release by name, or one by its id,
/// or every one.
pub fn kept(store: &mut Store, release: Option<&str>, id: Option<i64>) -> Result<Vec<Kept>, Error> {
    let d = store.dialect();
    let h = table("handover");
    let started = d.text_of_qualified(Some("h"), h.column("started_at").expect("a column"));
    let finished = d.text_of_qualified(Some("h"), h.column("finished_at").expect("a column"));
    let mut wheres = Vec::new();
    let mut params = Vec::new();
    if let Some(name) = release {
        params.push(Param::from(name));
        wheres.push(format!("r.name = {}", d.param(params.len(), Type::Text)));
    }
    if let Some(id) = id {
        params.push(Param::Int(id));
        wheres.push(format!("h.id = {}", d.param(params.len(), Type::Int)));
    }
    let wheres = if wheres.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", wheres.join(" AND "))
    };
    let sql = format!(
        "SELECT h.id, h.password, h.key_name, h.password_sealed, h.unencrypted_why, r.name, r.version, r.actor, \
         {started}, {finished}, h.archives, h.root \
         FROM {} h JOIN {} r ON r.id = h.release_id{wheres} ORDER BY h.id DESC",
        store.qualified("handover"),
        store.qualified("release"),
    );
    store
        .query(&sql, &params)?
        .iter()
        .map(|r| {
            Ok(Kept {
                id: r.int(0)?,
                kind: super::Kind::read(r.opt_text(1)?),
                key_name: r.text(2)?.to_string(),
                sealed: r.opt_bytes(3)?.map(<[u8]>::to_vec),
                why: r.opt_text(4)?.map(str::to_string),
                release: r.text(5)?.to_string(),
                version: r.text(6)?.to_string(),
                maker: r.text(7)?.to_string(),
                started_at: r.text(8)?.to_string(),
                finished_at: r.opt_text(9)?.map(str::to_string),
                archives: r.int(10)?,
                root: r.text(11)?.to_string(),
            })
        })
        .collect()
}

/// The password that opens a handover's archives: its own, derived again;
/// a typed one, unsealed; the key's shared one for a handover from before;
/// none for one not encrypted. Read from the registry's key store, never
/// written anywhere.
pub fn password_of(registry: &Registry, kept: &Kept) -> Result<Option<String>, Error> {
    use super::Kind;
    let key = || {
        registry
            .keys()
            .read(&kept.key_name)
            .map_err(|e| Error::Refused(e.to_string()))
    };
    Ok(match kept.kind {
        Kind::None => None,
        Kind::Key => Some(super::password(&key()?)),
        Kind::Made => Some(super::handover_password(&key()?, kept.id)),
        Kind::Typed => {
            let sealed = kept.sealed.as_deref().ok_or_else(|| {
                Error::Refused(format!("handover {} keeps no sealed password", kept.id))
            })?;
            let opened = nils_registry::linkage::Subkeys::derive(&key()?)
                .open(sealed)
                .map_err(|_| {
                    Error::Refused(format!(
                        "the password of handover {} does not open under key {}",
                        kept.id, kept.key_name
                    ))
                })?;
            Some(opened)
        }
    })
}

/// Why the root of a release's newest finished version cannot be handed
/// over as it is (Wave 7b, the maintenance track), in plain words; none
/// when the folder is that version. A release lock in the folder says a
/// release is writing it, or died in it; a release newer than the version
/// into the same folder that stopped, failed or did not finish, left the
/// folder a mixture of two versions, unless it stopped before it wrote or
/// removed anything ([`crate::run::WROTE_NOTHING`]), in which case the
/// folder is the version still. The handover refuses either, and so does
/// its door before it queues one.
pub fn not_whole(store: &mut Store, found: &Version) -> Result<Option<String>, Error> {
    let root = Path::new(&found.root);
    if crate::lock::path(root).exists() {
        let who = match crate::lock::read(root) {
            Some(lock) => format!("a release (job {} on {})", lock.job, lock.host),
            None => "a release whose lock cannot be read".to_string(),
        };
        return Ok(Some(format!(
            "the folder holds a release lock: {who} is writing it, or did not finish; wait for it, \
             or run the release again, before handing it over"
        )));
    }
    let d = store.dialect();
    let sql = format!(
        "SELECT error FROM {} WHERE dataset_id = {} AND root = {} AND id > {} AND finished_at IS \
         NULL ORDER BY id DESC",
        store.qualified("release"),
        d.param(1, Type::Int),
        d.param(2, Type::Text),
        d.param(3, Type::Int),
    );
    let newer = store.query(
        &sql,
        &[
            Param::Int(found.dataset),
            Param::from(found.root.as_str()),
            Param::Int(found.id),
        ],
    )?;
    for r in newer {
        let what = match r.opt_text(0)? {
            // a stop that wrote and removed nothing left the folder as it was
            Some(error) if error.contains(crate::run::WROTE_NOTHING) => continue,
            // one that failed once its row was open says why, and the tree
            // may be a mixture as after a stop
            Some(error) => match (
                crate::run::why_failed(error),
                error.find(crate::run::STOPPED_WORDS),
            ) {
                (Some(why), _) => format!("failed ({why})"),
                (None, Some(at)) => error[at..].split(';').next().unwrap_or(error).to_string(),
                (None, None) => format!("did not finish ({error})"),
            },
            // no finish and no word: a release is at it, or died at it
            None => {
                return Ok(Some(
                    "the folder holds a release that is running or died here; wait for it or run \
                     it again before handing it over"
                        .to_string(),
                ));
            }
        };
        return Ok(Some(format!(
            "the folder holds a release that {what}; run the release again to finish it before \
             handing it over"
        )));
    }
    Ok(None)
}

/// [`not_whole`] by the release's name, for the door: none as well when the
/// release has no finished version, which the handover itself refuses.
pub fn refusal_for(store: &mut Store, name: &str) -> Result<Option<String>, Error> {
    match newest(store, name)? {
        Some(found) => not_whole(store, &found),
        None => Ok(None),
    }
}

/// The newest finished version of a release, by id: a version string sorts by
/// component and `2026.09.05.10` sorts before `.9`.
fn newest(store: &mut Store, name: &str) -> Result<Option<Version>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id, dataset_id, version, root FROM {} WHERE name = {} AND finished_at IS NOT NULL \
         ORDER BY id DESC LIMIT 1",
        store.qualified("release"),
        d.param(1, Type::Text),
    );
    let Some(r) = store.query_opt(&sql, &[Param::from(name)])? else {
        return Ok(None);
    };
    Ok(Some(Version {
        id: r.int(0)?,
        dataset: r.int(1)?,
        version: r.text(2)?.to_string(),
        root: r.text(3)?.to_string(),
    }))
}

/// Every file of the version, from the state rather than from a scan of the
/// tree (Wave 4a §4).
///
/// A stack's files are what `release_stack` says they are: a converted stack
/// is its stem plus its extensions, a DICOM stack is the directory it owns.
/// The dataset's own files, `dataset_description.json`, `participants.tsv`
/// and the `_scans.tsv` beside each session, sit at the levels no stack ever
/// occupies, so they are the files found there. A file the state names and
/// the disk does not have is counted rather than skipped; a file the disk has
/// and the state does not name is not part of the release and is not packed.
fn files_of(
    store: &mut Store,
    dataset: i64,
    root: &Path,
) -> Result<(Vec<(String, i64)>, i64), Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT dir, stem, files, extensions FROM {} WHERE dataset_id = {} ORDER BY dir, stem",
        store.qualified("release_stack"),
        d.param(1, Type::Int),
    );
    let mut present: Vec<(String, i64)> = Vec::new();
    let mut missing = 0i64;
    let mut owned: Vec<String> = Vec::new();
    for r in store.query(&sql, &[Param::Int(dataset)])? {
        let dir = r.text(0)?.to_string();
        let files = r.int(2)?;
        match r.opt_text(1)? {
            Some(stem) => {
                let extensions = r.opt_text(3)?.unwrap_or("").to_string();
                for ext in extensions.split(',').filter(|e| !e.is_empty()) {
                    // Record 55 C4 (2026-10-09): the stack's DICOM export, a
                    // directory of its own under `sourcedata/dicom/`.
                    if ext == crate::run::DICOM_EXPORT {
                        let export = crate::bids::place::dicom_dir(&dir, stem);
                        walk(root, &root.join(&export), &mut present);
                        owned.push(format!("{export}/"));
                        continue;
                    }
                    let path = format!("{dir}/{stem}{ext}");
                    match std::fs::metadata(root.join(&path)) {
                        Ok(m) if m.is_file() => present.push((path, m.len() as i64)),
                        _ => missing += 1,
                    }
                }
            }
            None => {
                let before = present.len();
                walk(root, &root.join(&dir), &mut present);
                let found = (present.len() - before) as i64;
                if found < files {
                    missing += files - found;
                }
                owned.push(format!("{dir}/"));
            }
        }
    }
    // The dataset's own files: at the root, beside a subject, beside a
    // session. Never inside a stack, and never a hidden file.
    let named: std::collections::HashSet<String> = present.iter().map(|(p, _)| p.clone()).collect();
    let mut level: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = level.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            if relative.starts_with('.') || relative.contains("/.") {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                if depth < 2
                    && !owned
                        .iter()
                        .any(|o| relative.starts_with(o.trim_end_matches('/')))
                {
                    level.push((path, depth + 1));
                }
            } else if meta.is_file()
                && !named.contains(&relative)
                && !owned.iter().any(|o| relative.starts_with(o.as_str()))
            {
                present.push((relative, meta.len() as i64));
            }
        }
    }
    present.sort();
    present.dedup();
    Ok((present, missing))
}

/// Every file under a directory, relative to the root.
fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, i64)>) {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file()
                && let Ok(relative) = path.strip_prefix(root)
            {
                out.push((
                    relative.to_string_lossy().replace('\\', "/"),
                    meta.len() as i64,
                ));
            }
        }
    }
}

/// The subject behind each code, so an archive names people the registry knows
/// rather than strings from a path.
fn subject_ids(store: &mut Store) -> Result<HashMap<String, i64>, Error> {
    let sql = format!("SELECT code, id FROM {}", store.qualified("subject"));
    let mut out = HashMap::new();
    for r in store.query(&sql, &[])? {
        out.insert(r.text(0)?.to_string(), r.int(1)?);
    }
    Ok(out)
}

fn digest_of(path: &Path) -> (String, i64) {
    use blake2::Digest;
    let Ok(file) = std::fs::File::open(path) else {
        return (String::new(), 0);
    };
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = blake2::Blake2s256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut bytes = 0i64;
    loop {
        use std::io::Read;
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                bytes += n as i64;
                hasher.update(&buffer[..n]);
            }
            Err(_) => return (String::new(), 0),
        }
    }
    (hex::encode(hasher.finalize()), bytes)
}

fn open_row(
    store: &mut Store,
    settings: &Settings,
    version: &Version,
    report: &Report,
) -> Result<i64, Error> {
    let written = store.insert(
        &Insert::new(
            table("handover"),
            &[
                "release_id",
                "root",
                "strategy",
                "chunk_bytes",
                "level",
                "key_name",
                "tool",
                "par2_percent",
                "actor",
                "started_at",
                "archives",
                "files",
                "bytes",
                "packed_bytes",
                "password",
                "password_sealed",
                "unencrypted_why",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::Int(version.id),
            Param::from(settings.out.display().to_string()),
            Param::from(settings.strategy.name()),
            Param::Int(settings.cap),
            Param::Int(settings.level as i64),
            Param::from(settings.protection.key_name()),
            Param::from(settings.archiver.describe()),
            match &settings.par2 {
                Some(p) => Param::Int(p.percent as i64),
                None => Param::Null,
            },
            Param::from(settings.actor),
            Param::from(now_iso()),
            Param::Int(0),
            Param::Int(report.files),
            Param::Int(report.bytes),
            Param::Int(0),
            Param::from(settings.protection.kind().word()),
            match &settings.protection {
                Protection::Typed { sealed, .. } => Param::Bytes(sealed.to_vec()),
                _ => Param::Null,
            },
            match &settings.protection {
                Protection::None { why } => Param::from(*why),
                _ => Param::Null,
            },
        ]],
    )?;
    Ok(written.first().map(|r| r.int(0)).transpose()?.unwrap_or(0))
}

fn close_row(store: &mut Store, report: &Report) -> Result<(), Error> {
    let d = store.dialect();
    let sql = format!(
        "UPDATE {} SET finished_at = {}, archives = {}, packed_bytes = {}, error = {} \
         WHERE id = {}",
        store.qualified("handover"),
        d.param(1, Type::Timestamp),
        d.param(2, Type::Int),
        d.param(3, Type::Int),
        d.param(4, Type::Text),
        d.param(5, Type::Int),
    );
    let failed = report.failed.join("; ");
    store.execute(
        &sql,
        &[
            Param::from(now_iso()),
            Param::Int(report.archives),
            Param::Int(report.packed_bytes),
            match failed.is_empty() {
                true => Param::Null,
                false => Param::from(failed.as_str()),
            },
            Param::Int(report.handover_id),
        ],
    )?;
    Ok(())
}

fn write_archives(store: &mut Store, rows: &[Vec<Param>]) -> Result<(), Error> {
    if rows.is_empty() {
        return Ok(());
    }
    store.begin()?;
    let result = store.insert(
        &Insert::new(
            table("handover_archive"),
            &[
                "handover_id",
                "ordinal",
                "name",
                "digest",
                "bytes",
                "files",
                "subjects",
                "verified_at",
                "error",
            ],
        ),
        rows,
    );
    match result {
        Ok(_) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

/// Which people are in which archive, keyed on the archive's own row.
fn write_subjects(
    store: &mut Store,
    handover: i64,
    people: &[(usize, Vec<Param>)],
) -> Result<(), Error> {
    if people.is_empty() {
        return Ok(());
    }
    let d = store.dialect();
    let sql = format!(
        "SELECT ordinal, id FROM {} WHERE handover_id = {}",
        store.qualified("handover_archive"),
        d.param(1, Type::Int),
    );
    let mut ids: HashMap<i64, i64> = HashMap::new();
    for r in store.query(&sql, &[Param::Int(handover)])? {
        ids.insert(r.int(0)?, r.int(1)?);
    }
    let rows: Vec<Vec<Param>> = people
        .iter()
        .filter_map(|(ordinal, row)| {
            let id = ids.get(&(*ordinal as i64))?;
            let mut out = vec![Param::Int(*id)];
            out.extend(row.iter().cloned());
            Some(out)
        })
        .collect();
    if rows.is_empty() {
        return Ok(());
    }
    store.begin()?;
    let result = store.insert(
        &Insert::new(
            table("handover_subject"),
            &["archive_id", "subject_id", "code", "files", "bytes"],
        ),
        &rows,
    );
    match result {
        Ok(_) => {
            store.commit()?;
            Ok(())
        }
        Err(e) => {
            store.rollback().ok();
            Err(Error::Store(e))
        }
    }
}

/// The manifest that travels with the archives.
///
/// A recipient has the archives and not the registry, so the set has to say
/// what it is: which archive holds whom, how big each is, and the checksum to
/// check it against before spending a day unpacking it.
fn write_manifest(
    out: &Path,
    report: &Report,
    chunks: &[plan::Chunk],
    dataset: &str,
    encrypted: bool,
) -> Result<(), std::io::Error> {
    use std::fmt::Write as _;
    let mut text = String::from("archive\tdigest\tbytes\tfiles\tsubjects\n");
    for c in chunks {
        let name = c.name(dataset);
        let _ = writeln!(
            text,
            "{name}\t{}\t{}\t{}\t{}",
            digest_of(&out.join(&name)).0,
            c.bytes,
            c.files(),
            c.members
                .iter()
                .filter(|m| !m.subject.is_empty())
                .map(|m| m.subject.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    std::fs::write(out.join("handover.tsv"), text)?;
    let readme = format!(
        "{dataset}, version {}\n\n\
         {} archive(s), {} file(s), {} bytes packed.\n\
         Written by nils {} with {}.\n\n\
         {}\n\
         Check a file against handover.tsv before unpacking it: the digest is BLAKE2s-256.\n\n\
             7z x <archive>\n",
        report.version,
        report.archives,
        report.files,
        report.packed_bytes,
        env!("CARGO_PKG_VERSION"),
        report.tool,
        if encrypted {
            "Every archive has encrypted headers, so listing one needs the password too."
        } else {
            "The archives are not encrypted: whoever holds them can open them."
        },
    );
    std::fs::write(out.join("HANDOVER"), readme)?;
    Ok(())
}
