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
    /// The key the password is derived from, by name.
    pub key_name: &'a str,
    pub password: &'a str,
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
struct Version {
    id: i64,
    version: String,
    root: String,
}

/// Pack a release into archives, and record what was sent.
pub fn run(registry: &mut Registry, settings: &Settings) -> Result<Report, Error> {
    let started = std::time::Instant::now();
    let found = newest(registry.store(), settings.release)?;
    let Some(found) = found else {
        return Err(Error::Refused(format!(
            "no finished release named {}",
            settings.release
        )));
    };
    let files = files_of(registry.store(), found.id)?;
    if files.is_empty() {
        return Err(Error::Refused(format!(
            "version {} of {} wrote no files",
            found.version, settings.release
        )));
    }

    // The tree as the release recorded it, and the tree as it is now. A file
    // the record names and the disk does not is reported rather than skipped:
    // a handover of a tree somebody edited is not a handover of the release.
    let root = PathBuf::from(&found.root);
    let mut present: Vec<(String, i64)> = Vec::new();
    let mut missing = 0i64;
    for (path, bytes) in &files {
        match root.join(path).is_file() {
            true => present.push((path.clone(), *bytes)),
            false => missing += 1,
        }
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
        ..Report::default()
    };
    std::fs::create_dir_all(settings.out).map_err(Error::Io)?;
    report.handover_id = open_row(registry.store(), settings, &found, &report)?;

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
                settings.password,
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
            (Ok(()), true) => settings.archiver.verify(&path, settings.password).err(),
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
    write_manifest(settings.out, &report, &chunks, settings.release).map_err(Error::Io)?;
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
    password: &str,
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

/// The newest finished version of a release, by id: a version string sorts by
/// component and `2026.09.05.10` sorts before `.9`.
fn newest(store: &mut Store, name: &str) -> Result<Option<Version>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id, version, root FROM {} WHERE name = {} AND finished_at IS NOT NULL \
         ORDER BY id DESC LIMIT 1",
        store.qualified("release"),
        d.param(1, Type::Text),
    );
    let Some(r) = store.query_opt(&sql, &[Param::from(name)])? else {
        return Ok(None);
    };
    Ok(Some(Version {
        id: r.int(0)?,
        version: r.text(1)?.to_string(),
        root: r.text(2)?.to_string(),
    }))
}

/// Every file the version wrote, from the manifest rather than from a scan.
fn files_of(store: &mut Store, release: i64) -> Result<Vec<(String, i64)>, Error> {
    let d = store.dialect();
    let sql = format!(
        "SELECT path, bytes FROM {} WHERE release_id = {} ORDER BY path",
        store.qualified("release_file"),
        d.param(1, Type::Int),
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[Param::Int(release)])? {
        out.push((r.text(0)?.to_string(), r.int(1)?));
    }
    Ok(out)
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
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::Int(version.id),
            Param::from(settings.out.display().to_string()),
            Param::from(settings.strategy.name()),
            Param::Int(settings.cap),
            Param::Int(settings.level as i64),
            Param::from(settings.key_name),
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
         Every archive has encrypted headers, so listing one needs the password too.\n\
         Check a file against handover.tsv before unpacking it: the digest is BLAKE2s-256.\n\n\
             7z x <archive>\n",
        report.version,
        report.archives,
        report.files,
        report.packed_bytes,
        env!("CARGO_PKG_VERSION"),
        report.tool,
    );
    std::fs::write(out.join("HANDOVER"), readme)?;
    Ok(())
}
