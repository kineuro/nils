// SPDX-License-Identifier: AGPL-3.0-only

//! The software place (record 57 P5; Wave 7b, section 6): where the engine
//! installs the tools it runs, by digest from each tool's own source, and
//! registers the installs a site holds already, one copy for NILS and for
//! people. The layout, the manifests, the wrappers and the fingerprints
//! are [`nils_pipeline::software`]; this module owns the place's path (a
//! place of role `software` in the setup record, else `software/` under the
//! working place apptainer's image cache used), the installs themselves,
//! their verification, the recorded acceptances of a tool's terms, the
//! dependency checks at install, the doors and `nils software`.
//!
//! Two ways in: `install` pulls the image by its digest and fails unless
//! the digest got is the one asked for; `register` records an existing
//! install as Spack records externals, `managed: false`, and the engine
//! never updates or removes it. Both are verified the same way (section
//! 6.4): the pack's version probe, the fingerprint (the sha256 of a SIF, or
//! of the pack's `key_files` in a sandbox or a home), and the pack's test
//! run. A run records which install made its results, installed or
//! registered, with its version and fingerprint.
//!
//! Licences are recorded acceptances (section 6.5): who, when, which terms
//! and, for a site file, the file's sha256; never its content. A site file
//! lives under `licences/<app>/`, readable by the engine's account alone,
//! and reaches its pipeline as a secret input (record 49 R3), mounted
//! read-only with the variable the pack names. A run of a tool whose terms
//! need an acceptance is refused before it starts (409, naming the terms'
//! address) until an administrator accepts them. Nothing here pushes an
//! image anywhere (G9).

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nils_pipeline::descriptor::{self, Descriptor};
use nils_pipeline::runtime;
use nils_pipeline::software::{
    self as layout, Checked, External, How, Kind, Layout, Licence, Manifest, PackSoftware,
    Verification,
};
use nils_registry::Registry;
use nils_registry::pipeline::{self as rows, Pipeline};
use nils_registry::place::Role;
use nils_registry::schema::{Type, table};
use nils_registry::store::{Insert, Param, Store};
use serde_json::{Value, json};

use crate::Exit;
use crate::requirements::Check;

/// A refusal: its status and its body.
pub(crate) type Refused = (u16, Value);

/// The folder under the working place that is the software place where
/// none is declared: the one apptainer's image cache used.
pub(crate) const DEFAULT_DIR: &str = "software";

/// How long a version probe may take, and the pack's test run.
const PROBE_CAP: Duration = Duration::from_secs(300);
const TEST_CAP: Duration = Duration::from_secs(1800);
/// How long an inspect or a pull may take before it is given up on.
const INSPECT_CAP: Duration = Duration::from_secs(60);
const PULL_CAP: Duration = Duration::from_secs(4 * 3600);
/// The most a test input fetched over the network may hold.
const INPUT_CAP: u64 = 512 << 20;
/// How much of a probe's or a test's output is searched for its expected text.
const SEARCH_CAP: u64 = 1 << 20;
/// The grant every write here needs.
const GRANT: &str = nils_pipeline::failure::INSTALL_GRANT;

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 7] = [
    "GET /api/software",
    "POST /api/software/install",
    "POST /api/software/register",
    "POST /api/software/{app}/{version}/verify",
    "POST /api/software/{app}/{version}/accept",
    "POST /api/software/{app}/{version}/forget",
    "DELETE /api/software/{app}/{version}",
];

// ------------------------------------------------------------- the place

/// Where the software place is: the place of role `software`, or the
/// default under the working place.
#[derive(Debug, Clone)]
pub(crate) struct Place {
    pub(crate) path: PathBuf,
    /// The declared place's name, where one is declared.
    pub(crate) declared: Option<String>,
}

impl Place {
    pub(crate) fn doc(&self) -> Value {
        // read_only: the engine cannot write the place (a folder a site shares
        // read-only still lists, verifies and runs what it holds, but installs
        // and registrations into it fail). The place has no notion of a
        // shared folder yet, so no `shared` is named.
        json!({
            "path": self.path.display().to_string(),
            "declared": self.declared,
            "read_only": !crate::places::may_write(&self.path),
        })
    }

    pub(crate) fn layout(&self) -> Layout {
        Layout::new(&self.path)
    }
}

/// The software place, which every install, registration and acceptance
/// writes: the setup record's place of role `software`, else `software/`
/// under the pipelines' scratch working place (where the image cache was).
pub(crate) fn place(registry: &mut Registry) -> Result<Place, String> {
    let active = nils_registry::place::active(registry.store()).map_err(|e| e.to_string())?;
    if let Some(p) = active.iter().find(|p| p.role == Role::Software) {
        return Ok(Place {
            path: PathBuf::from(&p.path),
            declared: Some(p.name.clone()),
        });
    }
    let working = match crate::pipelines::run_places(registry.store()) {
        Ok(places) => places.scratch,
        Err(_) => crate::derivatives::working(registry.store()).map_err(|_| {
            "no software place: declare one with nils place add software PATH --role software \
             (or nils setup --place software=PATH,role=software), or a working place, under which \
             software/ is used"
                .to_string()
        })?,
    };
    Ok(Place {
        path: Path::new(&working.path).join(DEFAULT_DIR),
        declared: None,
    })
}

/// The place, made where it is not yet.
fn place_made(registry: &mut Registry) -> Result<Place, String> {
    let p = place(registry)?;
    std::fs::create_dir_all(&p.path).map_err(|e| format!("{}: {e}", p.path.display()))?;
    Ok(p)
}

// ------------------------------------------------------------ identities

/// The app and the version a pipeline's tool is kept under: what the pack's
/// `source` names, else the pipeline's name and its tool version.
pub(crate) fn identity(p: &Pipeline, pack: Option<&PackSoftware>) -> (String, String) {
    let app = pack
        .and_then(|k| k.app.clone())
        .unwrap_or_else(|| p.name.clone());
    let version = pack
        .and_then(|k| k.version.clone())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| p.tool_version.clone());
    (app, version)
}

/// The pack an install was made with, kept beside its manifest.
fn pack_copy(l: &Layout, app: &str, version: &str) -> Option<(Value, PackSoftware, String)> {
    let text = std::fs::read_to_string(l.pack(app, version)).ok()?;
    layout::parse_pack(&text).ok()
}

/// The pipeline a pack names, `pipeline.name`.
fn pack_pipeline(doc: &Value) -> Option<&str> {
    doc["pipeline"]["name"].as_str()
}

/// Every descriptor in the catalog with its row, read once.
fn catalog(registry: &mut Registry) -> Result<Vec<(Pipeline, Descriptor)>, String> {
    let mut out = Vec::new();
    for p in rows::list(registry.store()).map_err(|e| e.to_string())? {
        if let Ok(d) = descriptor::from_value(p.descriptor.clone()) {
            out.push((p, d));
        }
    }
    Ok(out)
}

/// The entry point of a descriptor's command line: its first word.
fn entry_point(d: &Descriptor) -> Option<String> {
    d.command_line
        .as_deref()
        .and_then(|c| nils_pipeline::words::split(c).ok())
        .and_then(|w| w.into_iter().next())
        .map(|w| {
            Path::new(&w)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or(w)
        })
        .filter(|w| layout::name_ok(w))
}

// ------------------------------------------------- what a run uses

/// The install a run uses, and what it records of it.
#[derive(Debug, Clone)]
pub(crate) struct Used {
    pub(crate) manifest: Manifest,
    /// What the runtime is given: the image's path, or its reference.
    pub(crate) runnable: String,
    pub(crate) pack: Option<PackSoftware>,
}

impl Used {
    /// `{spec, app, version, how, kind, fingerprint, digest, verified_at,
    /// image}`: "installed (or registered) install, version X, fingerprint
    /// Y", as a run records it.
    pub(crate) fn doc(&self) -> Value {
        let m = &self.manifest;
        json!({
            "spec": m.spec(), "app": m.app, "version": m.version,
            "how": m.how.name(), "kind": m.kind.name(), "managed": m.managed,
            "fingerprint": m.fingerprint, "digest": m.digest,
            "verified_at": m.verified_at, "image": self.runnable,
        })
    }

    /// In one line, for a log or a listing.
    pub(crate) fn words(&self) -> String {
        let m = &self.manifest;
        format!(
            "{} install {} {}, fingerprint {}",
            m.how.name(),
            m.app,
            m.version,
            m.fingerprint.as_deref().unwrap_or("not yet taken")
        )
    }
}

/// Whether the runtime of this machine can run an install: apptainer runs a
/// SIF or a sandbox kept on disk, docker or podman an image in their own
/// store. Any, where the runtime is not known.
fn runs_under(m: &Manifest, runtime: Option<runtime::Kind>) -> bool {
    match runtime {
        None => true,
        Some(runtime::Kind::Apptainer) => matches!(m.kind, Kind::Sif | Kind::Sandbox),
        Some(k) => m.kind == Kind::Image && m.runtime.as_deref().is_none_or(|r| r == k.name()),
    }
}

/// Why a run must not use an install whose last verification failed, with
/// the fix; None where it passed (or none ran yet).
pub(crate) fn failed_verification(u: &Used) -> Option<String> {
    let m = &u.manifest;
    if !m.verification.failed() {
        return None;
    }
    let what: Vec<String> = [
        ("probe", &m.verification.probe),
        ("fingerprint", &m.verification.fingerprint),
        ("test", &m.verification.test),
    ]
    .into_iter()
    .filter_map(|(n, c)| {
        c.as_ref()
            .filter(|c| c.failed())
            .map(|c| format!("the {n}: {}", c.said))
    })
    .collect();
    Some(format!(
        "the {} failed its verification ({}); fix the install, then nils software verify {} \
         (or nils software remove {} and install it again)",
        u.words(),
        what.join("; "),
        m.spec(),
        m.spec()
    ))
}

/// The install a run of `p` uses, where the place holds one: an install
/// whose digest is the pipeline's pinned image, else a registered install
/// that stands in for it (its pack pins that image, its pack names the
/// pipeline, or it is kept under the pipeline's own name and tool version).
/// None where the run pulls or builds the image itself.
pub(crate) fn for_run(registry: &mut Registry, p: &Pipeline) -> Result<Option<Used>, String> {
    let place = match place(registry) {
        Ok(place) => place,
        Err(_) => return Ok(None),
    };
    let l = place.layout();
    // only an install this machine's runtime can run is the run's: a copy
    // in the docker store is no image for apptainer, nor a SIF for docker
    let here = crate::pipelines::detect_cached(registry)
        .runtime
        .map(|r| r.kind);
    let installs: Vec<Manifest> = l
        .installs()
        .into_iter()
        .filter(|m| runs_under(m, here))
        .collect();
    let used = |m: &Manifest| -> Used {
        let pack = pack_copy(&l, &m.app, &m.version).map(|(_, s, _)| s);
        Used {
            runnable: m.runnable(&l),
            manifest: m.clone(),
            pack,
        }
    };
    if let Some(m) = installs
        .iter()
        .filter(|m| m.digest.as_deref() == Some(p.image_digest.as_str()))
        .max_by_key(|m| (m.verified_ok(), m.how == How::Installed))
    {
        return Ok(Some(used(m)));
    }
    for m in installs.iter().filter(|m| m.how == How::Registered) {
        let by_pin = m.digest_asked.as_deref() == Some(p.image_digest.as_str());
        let by_name = m.app == p.name && m.version == p.tool_version;
        let by_pack = pack_copy(&l, &m.app, &m.version)
            .is_some_and(|(doc, _, _)| pack_pipeline(&doc) == Some(p.name.as_str()));
        if by_pin || by_name || by_pack {
            return Ok(Some(used(m)));
        }
    }
    Ok(None)
}

/// The tool's part of a result's comparability key (section 5.6, 57-Q4),
/// for the pack slice to read: the tool, its version, the image digest the
/// pipeline pins, and the install's fingerprint. A registered install whose
/// key-file fingerprint equals the pinned image's (an installed copy of it
/// here) is the same tool, `same_as_pinned: true`, and the key keeps the
/// pinned digest; one that differs, or that cannot be compared, has its
/// own key, its fingerprint. An installed copy is the pinned image.
pub(crate) fn key_part(registry: &mut Registry, p: &Pipeline) -> Value {
    let used = for_run(registry, p).ok().flatten();
    let Some(u) = used else {
        return json!({
            "tool": p.name, "version": p.tool_version, "digest": p.image_digest,
            "fingerprint": null, "how": null, "same_as_pinned": true,
            "key": p.image_digest,
        });
    };
    let m = &u.manifest;
    let same = match m.how {
        How::Installed => true,
        How::Registered => {
            let pinned = place(registry)
                .ok()
                .map(|pl| pl.layout().installs())
                .unwrap_or_default()
                .into_iter()
                .find(|c| {
                    c.how == How::Installed && c.digest.as_deref() == Some(p.image_digest.as_str())
                });
            matches!((pinned.and_then(|c| c.fingerprint), &m.fingerprint), (Some(a), Some(b)) if a == *b)
        }
    };
    json!({
        "tool": m.app, "version": m.version, "digest": p.image_digest,
        "fingerprint": m.fingerprint, "how": m.how.name(), "same_as_pinned": same,
        "key": if same { p.image_digest.clone() } else { m.fingerprint.clone().unwrap_or_else(|| m.image.clone()) },
    })
}

// ---------------------------------------------------------- acceptances

/// One recorded acceptance of a tool's terms: who, when, which terms, and
/// the sha256 of the site file where there is one. Never its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Acceptance {
    pub(crate) id: i64,
    pub(crate) app: String,
    pub(crate) version: String,
    pub(crate) principal: String,
    pub(crate) at: String,
    pub(crate) terms_address: String,
    pub(crate) terms_version: Option<String>,
    pub(crate) file_sha256: Option<String>,
    pub(crate) file_name: Option<String>,
    pub(crate) note: Option<String>,
    pub(crate) spdx: Option<String>,
}

impl Acceptance {
    pub(crate) fn doc(&self) -> Value {
        json!({
            "id": self.id, "app": self.app, "version": self.version,
            "by": self.principal, "at": self.at,
            "terms": {"address": self.terms_address, "version": self.terms_version},
            "file": self.file_sha256.as_ref().map(|s| json!({"sha256": s, "name": self.file_name})),
            "note": self.note, "spdx": self.spdx,
        })
    }
}

/// Every acceptance recorded for an app, newest first.
pub(crate) fn acceptances(store: &mut Store, app: &str) -> Result<Vec<Acceptance>, String> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id, app, version, principal, accepted_at, terms_address, terms_version, \
         file_sha256, file_name, note, spdx FROM {} WHERE app = {} ORDER BY id DESC",
        store.qualified("software_acceptance"),
        d.param(1, Type::Text)
    );
    let rows = store
        .query(&sql, &[Param::from(app)])
        .map_err(|e| e.to_string())?;
    rows.iter()
        .map(|r| {
            Ok(Acceptance {
                id: r.int(0)?,
                app: r.text(1)?.to_string(),
                version: r.text(2)?.to_string(),
                principal: r.text(3)?.to_string(),
                at: r.text(4)?.to_string(),
                terms_address: r.text(5)?.to_string(),
                terms_version: r.opt_text(6)?.map(str::to_string),
                file_sha256: r.opt_text(7)?.map(str::to_string),
                file_name: r.opt_text(8)?.map(str::to_string),
                note: r.opt_text(9)?.map(str::to_string),
                spdx: r.opt_text(10)?.map(str::to_string),
            })
        })
        .collect::<Result<Vec<_>, nils_registry::store::Error>>()
        .map_err(|e| e.to_string())
}

/// The acceptance that covers a licence's terms: the same address and,
/// where the licence names a version, that version.
pub(crate) fn accepted(store: &mut Store, app: &str, licence: &Licence) -> Option<Acceptance> {
    let address = licence.terms_address.as_deref()?;
    acceptances(store, app).ok()?.into_iter().find(|a| {
        a.terms_address == address
            && licence
                .terms_version
                .as_deref()
                .is_none_or(|v| a.terms_version.as_deref() == Some(v))
    })
}

struct NewAcceptance<'a> {
    app: &'a str,
    version: &'a str,
    principal: &'a str,
    terms_address: &'a str,
    terms_version: Option<&'a str>,
    file_sha256: Option<&'a str>,
    file_name: Option<&'a str>,
    note: Option<&'a str>,
    spdx: Option<&'a str>,
}

fn record_acceptance(store: &mut Store, a: &NewAcceptance<'_>) -> Result<Acceptance, String> {
    let now = nils_registry::time::now_iso();
    let opt = |v: Option<&str>| v.map_or(Param::Null, Param::from);
    let rows = store
        .insert(
            &Insert::new(
                table("software_acceptance"),
                &[
                    "app",
                    "version",
                    "principal",
                    "accepted_at",
                    "terms_address",
                    "terms_version",
                    "file_sha256",
                    "file_name",
                    "note",
                    "spdx",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(a.app),
                Param::from(a.version),
                Param::from(a.principal),
                Param::from(now.as_str()),
                Param::from(a.terms_address),
                opt(a.terms_version),
                opt(a.file_sha256),
                opt(a.file_name),
                opt(a.note),
                opt(a.spdx),
            ]],
        )
        .map_err(|e| e.to_string())?;
    let id = rows
        .first()
        .and_then(|r| r.int(0).ok())
        .ok_or("the acceptance was not recorded")?;
    Ok(Acceptance {
        id,
        app: a.app.to_string(),
        version: a.version.to_string(),
        principal: a.principal.to_string(),
        at: now,
        terms_address: a.terms_address.to_string(),
        terms_version: a.terms_version.map(str::to_string),
        file_sha256: a.file_sha256.map(str::to_string),
        file_name: a.file_name.map(str::to_string),
        note: a.note.map(str::to_string),
        spdx: a.spdx.map(str::to_string),
    })
}

/// The licence file of an app under `licences/<app>/`: the one the pack
/// names, else the one regular file there. None where there is none.
fn licence_file(l: &Layout, app: &str, licence: Option<&Licence>) -> Option<PathBuf> {
    let dir = l.licences(app);
    if let Some(name) = licence
        .and_then(|k| k.site_file.as_ref())
        .and_then(|s| s.name.as_deref())
    {
        let p = dir.join(name);
        return std::fs::symlink_metadata(&p)
            .ok()
            .filter(|m| m.is_file())
            .map(|_| p);
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
        .collect();
    files.sort();
    (files.len() == 1).then(|| files.remove(0))
}

/// The site file a secret input is read from where the site set none with
/// `nils pipeline secret set` (record 49 R3): the licence file under
/// `licences/<app>/` of the install the run uses, or of the pipeline's own
/// name, when the secret's variable is the one the pack names or the
/// pipeline reads one secret alone.
pub(crate) fn licence_file_for(
    registry: &mut Registry,
    p: &Pipeline,
    d: &Descriptor,
    s: &descriptor::Secret,
) -> Option<PathBuf> {
    let place = place(registry).ok()?;
    let l = place.layout();
    let used = for_run(registry, p).ok().flatten();
    let (app, licence) = match &used {
        Some(u) => (
            u.manifest.app.clone(),
            u.pack.as_ref().and_then(|k| k.licence.clone()),
        ),
        None => (p.name.clone(), None),
    };
    let fits = match licence.as_ref().and_then(|k| k.site_file.as_ref()) {
        Some(site) => s.env.as_deref() == Some(site.env.as_str()) || s.mount == site.mount,
        None => d.secrets.len() == 1,
    };
    if !fits {
        return None;
    }
    licence_file(&l, &app, licence.as_ref()).or_else(|| licence_file(&l, &p.name, None))
}

/// The variables a tool's terms want set beside the licence file
/// (`FS_ALLOW_DEEP`), from the pack of the install a run uses.
pub(crate) fn licence_env(registry: &mut Registry, p: &Pipeline) -> Vec<(String, String)> {
    for_run(registry, p)
        .ok()
        .flatten()
        .and_then(|u| u.pack)
        .and_then(|k| k.licence)
        .map(|k| k.env)
        .unwrap_or_default()
}

/// The `licence` check of a run (section 6.5, 6.6; G9, G10): where the
/// tool's terms need an acceptance none is recorded, or the site file the
/// pack or the descriptor needs is neither set as a secret nor under
/// `licences/<app>/`, the run is refused before it starts, naming the
/// terms' address or the file and the way: `nils software accept`. None
/// where the tool has no terms and reads no secret.
pub(crate) fn licence_check(
    registry: &mut Registry,
    p: &Pipeline,
    d: &Descriptor,
) -> Option<Check> {
    use nils_pipeline::failure::Cause;
    let used = for_run(registry, p).ok().flatten();
    let (app, version, licence) = match &used {
        Some(u) => (
            u.manifest.app.clone(),
            u.manifest.version.clone(),
            u.pack.as_ref().and_then(|k| k.licence.clone()),
        ),
        None => (p.name.clone(), p.tool_version.clone(), None),
    };
    let spec = format!("{app}@{version}");
    let fix = json!({
        "door": format!("POST /api/software/{app}/{version}/accept"),
        "command": format!("nils software accept {spec}"),
        "grant": GRANT,
    });
    let mut extra = json!({"app": app, "version": version, "fix": fix});
    if let Some(k) = &licence {
        extra["terms"] = json!({"address": k.terms_address, "version": k.terms_version});
        extra["spdx"] = json!(k.spdx);
        if k.accept {
            match accepted(registry.store(), &app, k) {
                Some(a) => {
                    extra["accepted"] = a.doc();
                }
                None => {
                    return Some(Check::failed(
                        "licence",
                        Cause::Licence,
                        format!(
                            "the terms of {app} {version} at {} need an administrator's acceptance, \
                             and none is recorded: nils software accept {spec}",
                            k.terms_address.as_deref().unwrap_or("their address")
                        ),
                        extra,
                    ));
                }
            }
        }
    }
    // the site file: a secret the site set, or the file in the place
    let needs_file = licence.as_ref().and_then(|k| k.site_file.as_ref());
    let mut unset: Vec<&descriptor::Secret> = Vec::new();
    for s in d.secrets.iter().filter(|s| !s.optional) {
        if crate::pipelines::secret_path(registry, &s.id).is_none()
            && licence_file_for(registry, p, d, s).is_none()
        {
            unset.push(s);
        }
    }
    if let Some(site) = needs_file {
        let set_as_secret = d.secrets.iter().any(|s| {
            (s.env.as_deref() == Some(site.env.as_str()) || s.mount == site.mount)
                && crate::pipelines::secret_path(registry, &s.id).is_some()
        });
        let in_place = place(registry)
            .ok()
            .and_then(|pl| licence_file(&pl.layout(), &app, licence.as_ref()))
            .is_some();
        extra["file"] = json!({"env": site.env, "present": set_as_secret || in_place});
        if !set_as_secret && !in_place {
            return Some(Check::failed(
                "licence",
                Cause::Licence,
                format!(
                    "{app} {version} reads a site licence file ({}), and none is in the software \
                     place under licences/{app}/ or set as a secret: nils software accept {spec} \
                     --file PATH places it",
                    site.env
                ),
                extra,
            ));
        }
    } else if let Some(s) = unset.first() {
        return Some(Check::failed(
            "licence",
            Cause::Licence,
            format!(
                "{} reads the secret {}, which this site has not set: nils software accept {spec} \
                 --file PATH places it under licences/{app}/, or nils pipeline secret set {} --file PATH",
                p.label(),
                s.id,
                s.id
            ),
            extra,
        ));
    }
    if licence.is_none() && d.secrets.is_empty() {
        return None;
    }
    let said = match &licence {
        Some(k) if k.accept => format!(
            "the terms of {app} {version} are accepted{}",
            extra["accepted"]["by"]
                .as_str()
                .map(|b| format!(" by {b}"))
                .unwrap_or_default()
        ),
        Some(k) => format!(
            "{app} {version} is {}; no acceptance is needed",
            k.spdx.as_deref().unwrap_or("licensed")
        ),
        None => "the secrets it reads are set".to_string(),
    };
    Some(Check::ok("licence", said, extra))
}

// --------------------------------------------------------------- runtimes

/// The runtime an install is pulled with or runs under: the engine's own,
/// as detected, or for an install that names one, that program where it is
/// on the search path.
fn runtime_for(registry: &mut Registry, named: Option<&str>) -> Result<runtime::Cli, String> {
    let detected = crate::pipelines::detect_cached(registry);
    if let Some(name) = named
        && detected
            .runtime
            .as_ref()
            .is_none_or(|r| r.kind.name() != name)
    {
        let kind = match name {
            "podman" => runtime::Kind::Podman,
            "apptainer" => runtime::Kind::Apptainer,
            "docker" => runtime::Kind::Docker,
            other => {
                return Err(format!(
                    "{other} is not a container runtime the engine runs"
                ));
            }
        };
        if let Some(program) = runtime::which(name, std::env::var_os("PATH").as_deref()) {
            return Ok(runtime::Cli {
                kind,
                program,
                version: String::new(),
                gpu: None,
            });
        }
        return Err(format!(
            "the install runs under {name}, which is not on the engine's search path"
        ));
    }
    detected.runtime.ok_or_else(|| {
        detected
            .reason
            .unwrap_or_else(|| "no container runtime here".to_string())
    })
}

/// A command run to its end within `cap`, its output kept in `log` and
/// its tail returned; one that runs past the cap is killed (its own pid
/// alone) and said so.
fn run_logged(
    program: &Path,
    args: &[String],
    cap: Duration,
    log: &Path,
    cwd: Option<&Path>,
) -> Result<(bool, String), String> {
    let file = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = file
        .try_clone()
        .map_err(|e| format!("{}: {e}", log.display()))?;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(file))
        .stderr(std::process::Stdio::from(err));
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{} could not start: {e}", program.display()))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() > cap => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("{}: {e}", program.display())),
        }
    };
    let tail = log_tail(log);
    match status {
        Some(s) => Ok((s.success(), tail)),
        None => Ok((
            false,
            format!(
                "did not end within {} s and was stopped; its output ended: {tail}",
                cap.as_secs()
            ),
        )),
    }
}

/// The last lines of a log, at most 4 KiB.
fn log_tail(log: &Path) -> String {
    let bytes = std::fs::read(log).unwrap_or_default();
    let start = bytes.len().saturating_sub(4096);
    String::from_utf8_lossy(&bytes[start..]).trim().to_string()
}

/// The output a command left in `log`, up to 1 MiB from its start, for an
/// expected text to be searched in; the record keeps the tail.
fn log_search(log: &Path) -> String {
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Ok(f) = std::fs::File::open(log) {
        let _ = f.take(SEARCH_CAP).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// What docker or podman says of an image in its store.
fn inspect(rt: &runtime::Cli, reference: &str, log: &Path) -> Result<layout::Inspected, String> {
    let (ok, _) = run_logged(
        &rt.program,
        &layout::inspect_argv(reference),
        INSPECT_CAP,
        log,
        None,
    )?;
    if !ok {
        return Err(format!(
            "{} does not hold {reference}: {}",
            rt.kind.name(),
            log_tail(log)
        ));
    }
    let text = std::fs::read_to_string(log).unwrap_or_default();
    Ok(layout::parse_inspect(&text))
}

// ---------------------------------------------------------- verification

/// The three results of a verification, taken now (section 6.4): the pack's
/// version probe, the fingerprint (compared with the one recorded, where
/// one is), and the pack's test run on its small public input. The
/// manifest is written with `verified_at` and the results.
fn verify_install(
    registry: &mut Registry,
    l: &Layout,
    m: &mut Manifest,
    pack: Option<&PackSoftware>,
) -> Result<(), String> {
    let dir = l.install_dir(&m.app, &m.version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut v = Verification::default();
    // the fingerprint
    let key_files: Vec<String> = pack.map(|k| k.key_files.clone()).unwrap_or_default();
    let found = match m.kind {
        Kind::Image => {
            let rt = runtime_for(registry, m.runtime.as_deref())?;
            let said = inspect(&rt, &m.image, &dir.join("inspect.log"))?;
            let (repo, _) = layout::split_reference(&m.image)
                .ok_or_else(|| format!("{} is not pinned by a digest", m.image))?;
            m.size_bytes = said.size.or(m.size_bytes);
            said.digest_for(repo).or(said.id)
        }
        _ => {
            let path = m
                .image_path(l)
                .ok_or_else(|| "the install has no image on disk".to_string())?;
            match layout::fingerprint(m.kind, &path, &key_files) {
                Ok((f, size)) => {
                    m.size_bytes = Some(size);
                    Some(f)
                }
                Err(e) => {
                    v.fingerprint = Some(Checked::result(
                        false,
                        format!("{} could not be read: {e}", path.display()),
                    ));
                    None
                }
            }
        }
    };
    if let Some(f) = found {
        v.fingerprint = Some(match &m.fingerprint {
            Some(recorded) if recorded != &f => Checked::result(
                false,
                format!(
                    "the fingerprint is {f}, where {recorded} was recorded: the install changed"
                ),
            ),
            Some(_) => Checked::result(true, format!("{f}, as recorded")),
            None => {
                m.fingerprint = Some(f.clone());
                Checked::result(
                    true,
                    format!(
                        "{f}{}",
                        if key_files.is_empty() {
                            String::new()
                        } else {
                            format!(" over {}", key_files.join(", "))
                        }
                    ),
                )
            }
        });
    }
    // the version probe, in the tool, with no network
    let rt = (pack.is_some_and(|k| k.probe.is_some() || k.test.is_some()))
        .then(|| runtime_for(registry, m.runtime.as_deref()))
        .transpose()?;
    let runnable = m.runnable(l);
    match pack.and_then(|k| k.probe.as_ref()) {
        None => v.probe = Some(Checked::not_run("the pack declares no version probe")),
        Some(probe) => {
            let rt = rt.as_ref().ok_or("no runtime")?;
            let work = dir.join("probe");
            std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
            let argv = layout::exec_argv(rt.kind, &runnable, &probe.command, None, &work);
            let (ok, said) =
                run_logged(&rt.program, &argv, PROBE_CAP, &dir.join("probe.log"), None)?;
            let expected = probe.expect.trim();
            let whole = log_search(&dir.join("probe.log"));
            let passed = (expected.is_empty() && ok)
                || (!expected.is_empty() && (whole.contains(expected) || said.contains(expected)));
            v.probe = Some(Checked::result(
                passed,
                if passed {
                    format!(
                        "{} said {}",
                        probe.command.join(" "),
                        one_line(
                            if said.contains(expected) {
                                &said
                            } else {
                                &whole
                            },
                            expected
                        )
                    )
                } else {
                    format!(
                        "{} did not say {expected:?}; it said: {}",
                        probe.command.join(" "),
                        one_line(&said, "")
                    )
                },
            ));
            let _ = std::fs::remove_dir_all(&work);
        }
    }
    // the pack's test run
    match pack.and_then(|k| k.test.as_ref()) {
        None => v.test = Some(Checked::not_run("the pack declares no test run")),
        Some(test) => {
            let rt = rt.as_ref().ok_or("no runtime")?;
            let work = dir.join("test");
            let _ = std::fs::remove_dir_all(&work);
            std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
            let input = match &test.input {
                None => Ok(None),
                Some(i) => test_input(i, &dir.join("test-input")).map(Some),
            };
            v.test = Some(match input {
                Err(e) => Checked::result(false, format!("the test input could not be had: {e}")),
                Ok(input) => {
                    let argv = layout::exec_argv(
                        rt.kind,
                        &runnable,
                        &test.command,
                        input.as_deref(),
                        &work,
                    );
                    let (ok, said) =
                        run_logged(&rt.program, &argv, TEST_CAP, &dir.join("test.log"), None)?;
                    let expected = test.expect.trim();
                    let whole = log_search(&dir.join("test.log"));
                    let passed = ok
                        && (expected.is_empty()
                            || whole.contains(expected)
                            || said.contains(expected));
                    Checked::result(
                        passed,
                        if passed {
                            format!(
                                "{} ran{}",
                                test.command.join(" "),
                                if expected.is_empty() {
                                    String::new()
                                } else {
                                    format!(" and said {expected:?}")
                                }
                            )
                        } else if !ok {
                            format!("{} failed: {}", test.command.join(" "), one_line(&said, ""))
                        } else {
                            format!(
                                "{} did not say {expected:?}: {}",
                                test.command.join(" "),
                                one_line(&said, "")
                            )
                        },
                    )
                }
            });
            let _ = std::fs::remove_dir_all(&work);
        }
    }
    m.verified_at = Some(nils_registry::time::now_iso());
    m.verification = v;
    m.write(&l.manifest(&m.app, &m.version))
        .map_err(|e| format!("the manifest: {e}"))
}

/// The line of an output that holds `wanted`, else its last line, cut to
/// two hundred characters.
fn one_line(said: &str, wanted: &str) -> String {
    let line = said
        .lines()
        .rev()
        .find(|l| !wanted.is_empty() && l.contains(wanted))
        .or_else(|| said.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("nothing");
    line.chars().take(200).collect()
}

/// The pack's test input, ready to mount: a folder on this machine as it
/// is, or an address fetched into `into` once (an archive `.tar.gz`, `.tgz`
/// or `.tar` unpacked, any other file kept by its name).
fn test_input(input: &str, into: &Path) -> Result<PathBuf, String> {
    if !input.starts_with("https://") && !input.starts_with("http://") {
        let p = Path::new(input);
        if !p.is_absolute() || !p.is_dir() {
            return Err(format!(
                "{input} is neither a folder of this machine nor an https address"
            ));
        }
        return Ok(p.to_path_buf());
    }
    let done = into.join(".fetched");
    // kept only for the address it was fetched from: a pack that names
    // another address is tested on that one
    if std::fs::read_to_string(&done).is_ok_and(|had| had.trim() == input) {
        return Ok(into.to_path_buf());
    }
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let mut response = ureq::get(input)
        .call()
        .map_err(|e| format!("{input}: {e}"))?;
    let mut bytes = Vec::new();
    std::io::Read::take(response.body_mut().as_reader(), INPUT_CAP + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{input}: {e}"))?;
    if bytes.len() as u64 > INPUT_CAP {
        return Err(format!(
            "{input} is over {} MiB, more than a small test input",
            INPUT_CAP >> 20
        ));
    }
    let name = input
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("input");
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()))
            .unpack(into)
            .map_err(|e| format!("{input} did not unpack: {e}"))?;
    } else if name.ends_with(".tar") {
        tar::Archive::new(bytes.as_slice())
            .unpack(into)
            .map_err(|e| format!("{input} did not unpack: {e}"))?;
    } else {
        std::fs::write(into.join(name), &bytes).map_err(|e| format!("{}: {e}", into.display()))?;
    }
    std::fs::write(&done, input.as_bytes()).map_err(|e| format!("{}: {e}", done.display()))?;
    Ok(into.to_path_buf())
}

// ------------------------------------------------------------ the needs

/// The dependency checks of a package (section 6.6, G10), as the pre-flight
/// makes them for its pipeline: the runtime and its version, the image,
/// the memory, the GPU where one is needed, the disk, the converter for a
/// bids pipeline, and the licence. Each answers ok, warn or missing with
/// its fix. None where no pipeline of the catalog uses the package.
fn needs(
    registry: &mut Registry,
    home: &Path,
    pipelines: &[(Pipeline, Descriptor)],
    detected: &runtime::Detected,
) -> Vec<Value> {
    let Some((p, d)) = pipelines.first() else {
        return Vec::new();
    };
    let mut checks = crate::requirements::checks(
        registry,
        home,
        &crate::requirements::Asked {
            pipeline: p,
            descriptor: d,
            dcm2niix: Path::new(crate::tools::DCM2NIIX),
            units: 1,
            detected: Some(detected),
            releases: d.layout == descriptor::Layout::Bids,
        },
    );
    if !checks.iter().any(|c| c.check == "licence")
        && let Some(c) = licence_check(registry, p, d)
    {
        checks.push(c);
    }
    checks.iter().map(Check::doc).collect()
}

// -------------------------------------------------------------- listing

/// How a package stands: installed, registered, missing, failed
/// verification, or update available (the catalog pins another image).
fn status(m: Option<&Manifest>, pinned: &[String]) -> &'static str {
    match m {
        None => "missing",
        Some(m) if m.verification.failed() => "failed verification",
        Some(m)
            if m.how == How::Installed
                && m.digest.is_some()
                && !pinned.is_empty()
                && pinned.iter().all(|d| Some(d) != m.digest.as_ref()) =>
        {
            "update available"
        }
        Some(m) => m.how.name(),
    }
}

/// One package's document, as `GET /api/software` lists it.
fn package_doc(
    registry: &mut Registry,
    place: &Place,
    app: &str,
    version: &str,
    m: Option<&Manifest>,
    pipelines: &[(Pipeline, Descriptor)],
    checked: Vec<Value>,
) -> Value {
    let l = place.layout();
    let pack = pack_copy(&l, app, version).map(|(_, s, _)| s);
    let licence = pack.as_ref().and_then(|k| k.licence.as_ref());
    let pinned: Vec<String> = pipelines
        .iter()
        .map(|(p, _)| p.image_digest.clone())
        .collect();
    let accepted = licence.and_then(|k| accepted(registry.store(), app, k));
    let file = licence_file(&l, app, licence);
    let licence_doc = json!({
        "spdx": licence.and_then(|k| k.spdx.clone()),
        "redistributable": licence.and_then(|k| k.redistributable),
        "terms": licence.map(|k| json!({"address": k.terms_address, "version": k.terms_version})),
        "accept": licence.is_some_and(|k| k.accept),
        "accepted": accepted.as_ref().map(Acceptance::doc),
        "site_file": licence.and_then(|k| k.site_file.as_ref()).map(|s| json!({"env": s.env, "mount": s.mount, "name": s.name})),
        "file": {"present": file.is_some(), "name": file.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned())},
    });
    let spec = format!("{app}@{version}");
    json!({
        "app": app, "version": version, "spec": spec,
        "kind": m.map(|m| m.kind.name()),
        "how": m.map(|m| m.how.name()),
        "managed": m.map(|m| m.managed),
        "status": status(m, &pinned),
        "place": place.doc(),
        "path": m.and_then(|m| m.image_path(&l)).map(|p| p.display().to_string()),
        "image": m.map(|m| m.image.clone()),
        "source": m.and_then(|m| m.source.clone()).or_else(|| pipelines.first().map(|(p, _)| p.image.clone())),
        "digest": m.and_then(|m| m.digest.clone()),
        "digest_asked": m.and_then(|m| m.digest_asked.clone()),
        "pinned": pinned,
        "size_bytes": m.and_then(|m| m.size_bytes),
        "runtime": m.and_then(|m| m.runtime.clone()),
        "by": m.map(|m| m.by.clone()),
        "at": m.map(|m| m.at.clone()),
        "fingerprint": m.and_then(|m| m.fingerprint.clone()),
        "verified_at": m.and_then(|m| m.verified_at.clone()),
        "verification": m.map(|m| serde_json::to_value(&m.verification).unwrap_or(Value::Null)),
        "commands": m.map(|m| m.commands.clone()).unwrap_or_default(),
        "pack_sha256": m.and_then(|m| m.pack_sha256.clone()),
        "licence": licence_doc,
        "pipelines": pipelines.iter().map(|(p, _)| p.label()).collect::<Vec<_>>(),
        "needs": checked,
        "install": {
            "door": "POST /api/software/install",
            "command": format!("nils software install {spec}"),
            "grant": GRANT,
        },
    })
}

/// Every package: the installs in the place, the tools the catalog's
/// pipelines pin that are not installed, and the converter.
pub(crate) fn list(registry: &mut Registry, home: &Path) -> Result<Value, String> {
    let place = place(registry).ok();
    let catalog = catalog(registry)?;
    let detected = crate::pipelines::detect_cached(registry);
    let mut packages: Vec<Value> = Vec::new();
    if let Some(place) = &place {
        let l = place.layout();
        let installs = l.installs();
        let mut seen: Vec<(String, String)> = Vec::new();
        for m in &installs {
            let pack = pack_copy(&l, &m.app, &m.version).map(|(_, s, _)| s);
            let mine: Vec<(Pipeline, Descriptor)> = catalog
                .iter()
                .filter(|(p, _)| {
                    identity(p, pack.as_ref()) == (m.app.clone(), m.version.clone())
                        || m.digest.as_deref() == Some(p.image_digest.as_str())
                        || m.digest_asked.as_deref() == Some(p.image_digest.as_str())
                })
                .cloned()
                .collect();
            let checked = needs(registry, home, &mine, &detected);
            packages.push(package_doc(
                registry,
                place,
                &m.app,
                &m.version,
                Some(m),
                &mine,
                checked,
            ));
            seen.push((m.app.clone(), m.version.clone()));
            for (p, _) in &mine {
                seen.push((p.name.clone(), p.tool_version.clone()));
            }
        }
        // the tools the catalog pins that are not installed
        let mut missing: Vec<(String, String)> = Vec::new();
        for (p, _) in &catalog {
            let id = identity(p, None);
            if !seen.contains(&id) && !missing.contains(&id) {
                missing.push(id);
            }
        }
        for (app, version) in missing {
            let mine: Vec<(Pipeline, Descriptor)> = catalog
                .iter()
                .filter(|(p, _)| identity(p, None) == (app.clone(), version.clone()))
                .cloned()
                .collect();
            let checked = needs(registry, home, &mine, &detected);
            packages.push(package_doc(
                registry, place, &app, &version, None, &mine, checked,
            ));
        }
    }
    packages.push(converter_package(home, place.as_ref()));
    Ok(json!({
        "place": place.as_ref().map(Place::doc),
        "packages": packages,
        "runtime": detected.runtime.as_ref().map(|r| json!({"name": r.kind.name(), "version": r.version})),
    }))
}

/// The converter as one package among the others (section 6.7): the
/// engine's own install of the pinned dcm2niix, or where it is found.
fn converter_package(home: &Path, place: Option<&Place>) -> Value {
    let t = crate::tools::dcm2niix_status(Some(home));
    let present = t["present"] == true;
    let own = t["found_by"] == "tools";
    json!({
        "app": crate::tools::DCM2NIIX, "version": crate::tools::DCM2NIIX_VERSION,
        "spec": format!("{}@{}", crate::tools::DCM2NIIX, crate::tools::DCM2NIIX_VERSION),
        "kind": "binary",
        "how": present.then_some(if own { "installed" } else { "registered" }),
        "managed": present.then_some(own),
        "status": if !present { "missing" } else if t["current"] == true { "installed" } else { "update available" },
        "place": place.map(Place::doc),
        "path": t["path"],
        "image": null, "source": t["install"]["source"], "digest": null,
        "digest_asked": t["install"]["sha256"], "pinned": [],
        "size_bytes": null, "runtime": null, "by": null, "at": null,
        "fingerprint": null, "verified_at": null,
        "verification": {"probe": {"ran": present, "passed": present.then_some(true), "said": t["version"]}},
        "commands": [crate::tools::DCM2NIIX],
        "pack_sha256": null,
        "licence": {"spdx": "BSD-2-Clause", "accept": false, "accepted": null, "file": {"present": false}},
        "pipelines": [],
        "needs": [],
        "tool": t,
        "install": {
            "door": "POST /api/software/install",
            "command": format!("nils software install {}", crate::tools::DCM2NIIX),
            "grant": GRANT,
            "alias": nils_pipeline::failure::DCM2NIIX_INSTALL_ALIAS,
        },
    })
}

// -------------------------------------------------------------- install

/// What an install is asked.
#[derive(Debug, Clone, Default)]
pub(crate) struct InstallRequest {
    pub(crate) app: String,
    /// The version; for the converter, the pinned build when none is given.
    pub(crate) version: String,
    /// The image's reference, `repository@sha256:<hex>`, where no pack
    /// and no pipeline of the catalog gives it.
    pub(crate) source: Option<String>,
    /// The pack's text, where one is given.
    pub(crate) pack: Option<String>,
    pub(crate) principal: String,
}

fn refused(status: u16, error: String) -> Refused {
    (
        status,
        json!({"error": error, "disclosure": "safe", "failure": nils_pipeline::failure::Cause::Image.doc()}),
    )
}

fn missing_dependency(
    error: String,
    needs: Vec<Value>,
    cause: nils_pipeline::failure::Cause,
) -> Refused {
    (
        409,
        json!({"error": error, "disclosure": "safe", "failure": cause.refusal_doc(), "needs": needs}),
    )
}

/// An install's folder let go after a refused install, and its app's folder
/// where that is empty now.
fn discard(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
    if let Some(app) = dir.parent() {
        let _ = std::fs::remove_dir(app);
    }
}

/// One install or registration at a time in this process.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The pipelines of the catalog whose tool is `app@version` under a pack
/// (its own, or none).
fn pipelines_of(
    catalog: &[(Pipeline, Descriptor)],
    app: &str,
    version: &str,
    pack: Option<&(Value, PackSoftware, String)>,
) -> Vec<(Pipeline, Descriptor)> {
    catalog
        .iter()
        .filter(|(p, _)| {
            identity(p, pack.map(|k| &k.1)) == (app.to_string(), version.to_string())
                || pack.is_some_and(|(doc, _, _)| pack_pipeline(doc) == Some(p.name.as_str()))
        })
        .cloned()
        .collect()
}

/// `nils software install <app>@<version>` (section 6.2): the image pulled
/// by its digest from the pack's `source` (else the reference given, else
/// the one the catalog's pipeline of that tool pins), unpacked where the
/// host needs a sandbox, the manifest, wrappers and module written, and
/// the install verified. The digest asked for and the digest got are equal
/// or the install fails and nothing is recorded. On docker or podman the
/// image stays in the runtime's store and the manifest records
/// `image@sha256`. The converter is installed as its own package. The
/// dependencies are checked first (G10) and answered with the install; a
/// missing runtime refuses it.
pub(crate) fn install(
    registry: &mut Registry,
    home: &Path,
    req: InstallRequest,
) -> Result<Value, Refused> {
    use nils_pipeline::failure::Cause;
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if req.app == crate::tools::DCM2NIIX {
        if !req.version.is_empty() && req.version != crate::tools::DCM2NIIX_VERSION {
            return Err(refused(
                422,
                format!(
                    "this version installs dcm2niix {} alone, not {}",
                    crate::tools::DCM2NIIX_VERSION,
                    req.version
                ),
            ));
        }
        let done = crate::tools::install(
            registry,
            home,
            &req.principal,
            crate::tools::Archive::Download,
        )?;
        let place = place(registry).ok();
        return Ok(json!({
            "installed": done["installed"],
            "why": done["why"],
            "package": converter_package(home, place.as_ref()),
            "needs": [],
        }));
    }
    let place = place_made(registry).map_err(|e| refused(409, e))?;
    let l = place.layout();
    // the pack, where one is given
    let pack = match &req.pack {
        Some(text) => Some(layout::parse_pack(text).map_err(|e| refused(422, e))?),
        None => None,
    };
    let (app, version) = match pack.as_ref().map(|k| &k.1) {
        Some(k) if k.app.is_some() || k.version.is_some() => {
            let app = k.app.clone().unwrap_or_else(|| req.app.clone());
            let version = k.version.clone().unwrap_or_else(|| req.version.clone());
            if (app.as_str(), version.as_str()) != (req.app.as_str(), req.version.as_str())
                && !req.app.is_empty()
            {
                return Err(refused(
                    422,
                    format!(
                        "the pack names {app}@{version}, and the install asks {}@{}",
                        req.app, req.version
                    ),
                ));
            }
            (app, version)
        }
        _ => (req.app.clone(), req.version.clone()),
    };
    if !layout::name_ok(&app) || !layout::name_ok(&version) {
        return Err(refused(
            422,
            format!("{app}@{version} is not a package name"),
        ));
    }
    if let Some(m) = Manifest::read(&l.manifest(&app, &version)) {
        if m.how == How::Registered {
            return Err(refused(
                409,
                format!(
                    "{app}@{version} is registered as the site's install at {}; the engine does not \
                     install over what it did not install",
                    m.image
                ),
            ));
        }
        let catalog = catalog(registry).map_err(|e| refused(500, e))?;
        let mine = pipelines_of(&catalog, &app, &version, pack.as_ref());
        let detected = crate::pipelines::detect_cached(registry);
        let checked = needs(registry, home, &mine, &detected);
        return Ok(json!({
            "installed": false,
            "why": format!("{app}@{version} is installed already ({}); nils software verify {app}@{version} checks it again", m.at),
            "package": package_doc(registry, &place, &app, &version, Some(&m), &mine, checked.clone()),
            "needs": checked,
        }));
    }
    let catalog = catalog(registry).map_err(|e| refused(500, e))?;
    let mine = pipelines_of(&catalog, &app, &version, pack.as_ref());
    // the source: the pack's, the one given, or the catalog's
    let source = pack
        .as_ref()
        .and_then(|k| k.1.image.clone())
        .or_else(|| req.source.clone())
        .or_else(|| {
            let mut images: Vec<String> = mine.iter().map(|(p, _)| p.image.clone()).collect();
            images.sort();
            images.dedup();
            (images.len() == 1).then(|| images.remove(0))
        });
    let Some(source) = source else {
        let images: Vec<String> = mine.iter().map(|(p, _)| p.image.clone()).collect();
        return Err(refused(
            422,
            if images.len() > 1 {
                format!(
                    "the pipelines of {app}@{version} pin different images ({}); give the pack, or \
                     --source repository@sha256:<hex>",
                    images.join(", ")
                )
            } else {
                format!(
                    "no source for {app}@{version}: no pipeline of the catalog pins its image; give \
                     the pack with --pack, or --source repository@sha256:<hex>"
                )
            },
        ));
    };
    let Some((repository, digest_asked)) = layout::split_reference(&source) else {
        return Err(refused(
            422,
            format!("{source} is not pinned by its digest: an image is repository@sha256:<hex>"),
        ));
    };
    // the dependencies, checked before anything is pulled (G10)
    let detected = crate::pipelines::detect_cached(registry);
    let checked = needs(registry, home, &mine, &detected);
    let Some(rt) = detected.runtime.clone() else {
        return Err(missing_dependency(
            detected
                .reason
                .clone()
                .unwrap_or_else(|| "no container runtime here".to_string()),
            checked,
            Cause::Runtime,
        ));
    };
    // the commands people run it by
    let mut commands: Vec<String> = mine.iter().filter_map(|(_, d)| entry_point(d)).collect();
    if let Some(first) = pack
        .as_ref()
        .and_then(|k| k.1.probe.as_ref())
        .and_then(|p| p.command.first())
        .filter(|w| layout::name_ok(w))
    {
        commands.push(first.clone());
    }
    commands.sort();
    commands.dedup();
    let _lock = lock_install(&l, &app, &version).map_err(|e| refused(500, e))?;
    if Manifest::read(&l.manifest(&app, &version)).is_some() {
        return Err(refused(
            409,
            format!("{app}@{version} was put in the software place by another process meanwhile"),
        ));
    }
    let dir = l.install_dir(&app, &version);
    let now = nils_registry::time::now_iso();
    let mut m = Manifest {
        app: app.clone(),
        version: version.clone(),
        kind: Kind::Image,
        how: How::Installed,
        managed: true,
        source: Some(source.clone()),
        digest_asked: Some(digest_asked.to_string()),
        digest: None,
        digest_how: None,
        pack_sha256: pack.as_ref().map(|k| k.2.clone()),
        image: source.clone(),
        size_bytes: None,
        runtime: Some(rt.kind.name().to_string()),
        by: req.principal.clone(),
        at: now,
        commands,
        fingerprint: None,
        verified_at: None,
        verification: Verification::default(),
        licence: pack
            .as_ref()
            .and_then(|k| k.1.licence.as_ref())
            .map(Licence::doc)
            .unwrap_or(Value::Null),
    };
    // the pull, by digest
    let log = dir.join("install.log");
    match rt.kind {
        runtime::Kind::Apptainer => {
            let form = crate::pipelines::image_form(registry);
            let target = l.image(&app, &version);
            let part = dir.join(format!(".image.part-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&part);
            let _ = std::fs::remove_file(&part);
            let (ok, said) = run_logged(
                &rt.program,
                &runtime::build_argv(&source, &part, form),
                PULL_CAP,
                &log,
                None,
            )
            .map_err(|e| refused(502, e))?;
            if !ok || std::fs::symlink_metadata(&part).is_err() {
                let _ = std::fs::remove_dir_all(&part);
                let _ = std::fs::remove_file(&part);
                discard(&dir);
                return Err(refused(
                    502,
                    format!("apptainer could not build {source}; its log ends: {said}"),
                ));
            }
            let _ = std::fs::remove_dir_all(&target);
            let _ = std::fs::remove_file(&target);
            std::fs::rename(&part, &target)
                .map_err(|e| refused(500, format!("{}: {e}", target.display())))?;
            m.kind = Kind::of_form(form);
            m.image = layout::IMAGE.to_string();
            // apptainer fetches docker://<reference> through its OCI client,
            // which refuses a manifest whose digest is not the one named:
            // what came is the digest asked, and the SIF's own sha256 is
            // the fingerprint taken below
            m.digest = Some(digest_asked.to_string());
            m.digest_how = Some(
                "apptainer: the reference's digest, which its OCI client verifies on the pull"
                    .to_string(),
            );
        }
        runtime::Kind::Podman | runtime::Kind::Docker => {
            let (ok, said) = run_logged(
                &rt.program,
                &layout::pull_argv(&source),
                PULL_CAP,
                &log,
                None,
            )
            .map_err(|e| refused(502, e))?;
            if !ok {
                discard(&dir);
                return Err(refused(
                    502,
                    format!(
                        "{} could not pull {source}; it said: {said}",
                        rt.kind.name()
                    ),
                ));
            }
            let said = inspect(&rt, &source, &dir.join("inspect.log")).map_err(|e| {
                discard(&dir);
                refused(502, e)
            })?;
            let got = said.digest_for(repository);
            if got.as_deref() != Some(digest_asked) {
                // not what was asked: taken out of the store again, and
                // nothing recorded
                let _ = run_logged(
                    &rt.program,
                    &layout::remove_image_argv(&source),
                    INSPECT_CAP,
                    &dir.join("remove.log"),
                    None,
                );
                discard(&dir);
                return Err(refused(
                    409,
                    format!(
                        "the pull of {source} asked for {digest_asked} and got {}: not installed",
                        got.as_deref().unwrap_or("no digest the store reports")
                    ),
                ));
            }
            m.digest = got;
            m.digest_how = Some(format!(
                "{}: the repository digest its store reports after the pull",
                rt.kind.name()
            ));
            m.size_bytes = said.size;
            m.fingerprint = m.digest.clone();
        }
    }
    if let Some(text) = &req.pack {
        std::fs::write(l.pack(&app, &version), text)
            .map_err(|e| refused(500, format!("the pack's copy: {e}")))?;
    }
    m.write(&l.manifest(&app, &version))
        .map_err(|e| refused(500, format!("the manifest: {e}")))?;
    layout::write_wrappers(&l, &m).map_err(|e| refused(500, format!("the wrappers: {e}")))?;
    // verified: the probe, the fingerprint and the test run
    verify_install(registry, &l, &mut m, pack.as_ref().map(|k| &k.1))
        .map_err(|e| refused(500, e))?;
    let _ = nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &req.principal,
            action: nils_registry::audit::Action::SoftwareInstall,
            scope: json!({"app": app, "version": version}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "source": source, "digest": m.digest, "kind": m.kind.name(), "runtime": rt.kind.name(),
                "place": place.path.display().to_string(), "pack_sha256": m.pack_sha256,
                "verified": !m.verification.failed(),
            })),
        },
    );
    let checked = needs(registry, home, &mine, &detected);
    Ok(json!({
        "installed": true,
        "package": package_doc(registry, &place, &app, &version, Some(&m), &mine, checked.clone()),
        "needs": checked,
        "verified": !m.verification.failed(),
    }))
}

/// A run's apptainer image, built into the place as an install (section
/// 6.1: the image cache becomes the place): what a run does when the tool
/// is not installed, so the next run and people find it there. The
/// runner's own build with its job beats hands the built file here, which
/// moves it into `<app>/<version>/image` and writes the manifest, the
/// wrappers and the module; a verified copy of the old cache is adopted
/// the same way.
pub(crate) fn adopt_built(
    registry: &mut Registry,
    p: &Pipeline,
    built: &Path,
    form: runtime::ImageForm,
    by: &str,
) -> Result<Used, String> {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let place = place_made(registry)?;
    let l = place.layout();
    let kind = Kind::of_form(form);
    let drop_built = || {
        let _ = std::fs::remove_dir_all(built);
        let _ = std::fs::remove_file(built);
    };
    let used_of = |m: Manifest| Used {
        runnable: m.runnable(&l),
        manifest: m,
        pack: None,
    };
    // another builder may have installed it already
    if let Some(m) = existing_entry(&l, p)? {
        drop_built();
        return Ok(used_of(m));
    }
    // the hash of a multi-GB file is taken outside the lock; the lock is
    // held for the check and the move alone
    let (fingerprint, size) = layout::fingerprint(kind, built, &[]).map_err(|e| e.to_string())?;
    let (base_app, base_version) = identity(p, None);
    let _lock = lock_install(&l, &base_app, &base_version)?;
    // re-checked under the lock, after the hash: a builder that finished
    // meanwhile has its manifest and file in place and keeps them
    if let Some(m) = existing_entry(&l, p)? {
        drop_built();
        return Ok(used_of(m));
    }
    let (app, version) = entry_of(&l, p)?;
    let dir = l.install_dir(&app, &version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let target = l.image(&app, &version);
    // no manifest here, and every writer holds the lock: what lies at the
    // target is a leftover of a build that did not finish
    let _ = std::fs::remove_dir_all(&target);
    let _ = std::fs::remove_file(&target);
    std::fs::rename(built, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    // the file in place is the file hashed
    let now_is = layout::fingerprint(kind, &target, &[]).map_err(|e| e.to_string());
    if now_is.as_ref().map(|(f, _)| f) != Ok(&fingerprint) {
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&target);
        return Err(format!(
            "the image built for {app}@{version} is not the image hashed after the move; it was \
             removed, and the next run builds it again"
        ));
    }
    let d = descriptor::from_value(p.descriptor.clone()).ok();
    let m = Manifest {
        app: app.clone(),
        version: version.clone(),
        kind,
        how: How::Installed,
        managed: true,
        source: Some(p.image.clone()),
        digest_asked: Some(p.image_digest.clone()),
        digest: Some(p.image_digest.clone()),
        digest_how: Some(
            "apptainer: the reference's digest, which its OCI client verifies on the pull"
                .to_string(),
        ),
        pack_sha256: None,
        image: layout::IMAGE.to_string(),
        size_bytes: Some(size),
        runtime: Some("apptainer".to_string()),
        by: by.to_string(),
        at: nils_registry::time::now_iso(),
        commands: d.as_ref().and_then(entry_point).into_iter().collect(),
        fingerprint: Some(fingerprint),
        verified_at: Some(nils_registry::time::now_iso()),
        verification: Verification {
            probe: Some(Checked::not_run(
                "built by a run, with no pack: no version probe",
            )),
            fingerprint: Some(Checked::result(true, "taken as built".to_string())),
            test: Some(Checked::not_run(
                "built by a run, with no pack: no test run",
            )),
        },
        licence: Value::Null,
    };
    m.write(&l.manifest(&app, &version))
        .map_err(|e| format!("the manifest: {e}"))?;
    layout::write_wrappers(&l, &m).map_err(|e| format!("the wrappers: {e}"))?;
    let _ = nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: by,
            action: nils_registry::audit::Action::SoftwareInstall,
            scope: json!({"app": app, "version": version}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "source": p.image, "digest": p.image_digest, "kind": m.kind.name(),
                "runtime": "apptainer", "by_run": true, "place": place.path.display().to_string(),
            })),
        },
    );
    Ok(used_of(m))
}

/// The entry in the place a pipeline's apptainer image is kept under: the
/// pipeline's own name and tool version, else that version with
/// `-apptainer` where the first entry holds the same image in the docker
/// or podman store (an image there is no file for apptainer). Refused
/// where the entry holds another image.
fn entry_of(l: &Layout, p: &Pipeline) -> Result<(String, String), String> {
    let (app, version) = identity(p, None);
    let other = |m: &Manifest, v: &str| {
        format!(
            "{app}@{v} is {} in the software place with another image ({}); nils software \
             remove {app}@{v} or a pack naming this version first",
            m.how.name(),
            m.digest.as_deref().unwrap_or("none")
        )
    };
    match Manifest::read(&l.manifest(&app, &version)) {
        None => Ok((app, version)),
        Some(m) if m.digest.as_deref() == Some(p.image_digest.as_str()) && m.kind.on_disk() => {
            Ok((app, version))
        }
        Some(m) if m.digest.as_deref() == Some(p.image_digest.as_str()) => {
            let alt = format!("{version}-apptainer");
            match Manifest::read(&l.manifest(&app, &alt)) {
                Some(a) if a.digest.as_deref() != Some(p.image_digest.as_str()) => {
                    Err(other(&a, &alt))
                }
                _ => Ok((app, alt)),
            }
        }
        Some(m) => Err(other(&m, &version)),
    }
}

/// The install of the pipeline's image kept on disk already in its entry,
/// with its file in place.
fn existing_entry(l: &Layout, p: &Pipeline) -> Result<Option<Manifest>, String> {
    let (app, version) = entry_of(l, p)?;
    Ok(Manifest::read(&l.manifest(&app, &version)).filter(|m| {
        m.digest.as_deref() == Some(p.image_digest.as_str())
            && m.kind.on_disk()
            && m.image_path(l)
                .is_some_and(|f| std::fs::symlink_metadata(f).is_ok())
    }))
}

/// An exclusive lock on an install's folder, `<app>/<version>/.lock`, held
/// until the returned file is dropped: the engine's processes (two runs,
/// the server and the CLI) take it before they write an install.
fn lock_install(l: &Layout, app: &str, version: &str) -> Result<std::fs::File, String> {
    let dir = l.install_dir(app, version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(".lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.lock().map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(f)
}

/// Where a run stages the apptainer image it builds: the install's own
/// folder in the place, so the built file is moved into place on one
/// filesystem. Refused where the place holds another image under the
/// pipeline's name and tool version already.
pub(crate) fn staging_dir(registry: &mut Registry, p: &Pipeline) -> Result<PathBuf, String> {
    let place = place_made(registry)?;
    let l = place.layout();
    let (app, version) = entry_of(&l, p)?;
    let dir = l.install_dir(&app, &version);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

/// Whether an install kept on disk is what was verified: its fingerprint
/// now is the one recorded (a SIF by its bytes; a sandbox by its key files,
/// or its tree). What a run asks before it runs the file.
pub(crate) fn still_verified(u: &Used) -> Result<bool, String> {
    let m = &u.manifest;
    if m.verification.failed() {
        return Ok(false);
    }
    if !m.kind.on_disk() {
        return Ok(true);
    }
    let Some(recorded) = &m.fingerprint else {
        return Ok(false);
    };
    let keys = u
        .pack
        .as_ref()
        .map(|k| k.key_files.clone())
        .unwrap_or_default();
    let (now, _) =
        layout::fingerprint(m.kind, Path::new(&u.runnable), &keys).map_err(|e| e.to_string())?;
    Ok(&now == recorded)
}

// ------------------------------------------------------------- register

/// What a registration is asked.
#[derive(Debug, Clone, Default)]
pub(crate) struct RegisterRequest {
    pub(crate) app: String,
    pub(crate) version: String,
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) pack: Option<String>,
    pub(crate) principal: String,
}

/// `nils software register <app>@<version> --kind sif|sandbox|image --path
/// <path>` (section 6.3): an existing install recorded as Spack records
/// externals, the spec, the kind, the path and `managed: false`, in
/// `externals.yaml` and a manifest, and verified. Nothing under the path
/// is written; the wrappers and the module are written in the place. An
/// image is a reference in the runtime's store, `repository@sha256:<hex>`.
pub(crate) fn register(
    registry: &mut Registry,
    home: &Path,
    req: RegisterRequest,
) -> Result<Value, Refused> {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (app, version) = (req.app.clone(), req.version.clone());
    if !layout::name_ok(&app) || !layout::name_ok(&version) {
        return Err(refused(
            422,
            format!("{app}@{version} is not a package name"),
        ));
    }
    let Some(kind) = Kind::parse(&req.kind) else {
        return Err(refused(
            422,
            format!(
                "--kind is one of sif, sandbox or image (a binary is registered as the converter is), not {}",
                req.kind
            ),
        ));
    };
    if kind == Kind::Binary {
        return Err(refused(
            422,
            "a binary is not registered here: the engine finds the converter where the setup \
             record or the search path names it"
                .to_string(),
        ));
    }
    let pack = match &req.pack {
        Some(text) => Some(layout::parse_pack(text).map_err(|e| refused(422, e))?),
        None => None,
    };
    let place = place_made(registry).map_err(|e| refused(409, e))?;
    let l = place.layout();
    if let Some(m) = Manifest::read(&l.manifest(&app, &version)) {
        return Err(refused(
            409,
            format!(
                "{app}@{version} is {} already ({}); nils software remove {app}@{version} first \
                 where the engine installed it",
                m.how.name(),
                m.image
            ),
        ));
    }
    let detected = crate::pipelines::detect_cached(registry);
    let (image, runtime_name, size) = match kind {
        Kind::Image => {
            let Some((_, _)) = layout::split_reference(&req.path) else {
                return Err(refused(
                    422,
                    format!(
                        "{} is not an image pinned by its digest: --path repository@sha256:<hex>",
                        req.path
                    ),
                ));
            };
            let Some(rt) = detected
                .runtime
                .clone()
                .filter(|r| r.kind != runtime::Kind::Apptainer)
            else {
                return Err(refused(
                    409,
                    "an image is registered from a docker or podman store, and the engine's runtime \
                     here is neither"
                        .to_string(),
                ));
            };
            let dir = l.install_dir(&app, &version);
            std::fs::create_dir_all(&dir)
                .map_err(|e| refused(500, format!("{}: {e}", dir.display())))?;
            let said = inspect(&rt, &req.path, &dir.join("inspect.log")).map_err(|e| {
                discard(&dir);
                refused(409, e)
            })?;
            (
                req.path.clone(),
                Some(rt.kind.name().to_string()),
                said.size,
            )
        }
        _ => {
            let path = Path::new(&req.path);
            if !path.is_absolute() {
                return Err(refused(
                    422,
                    format!(
                        "{} is not a path of this machine: a registered install is named by its whole path",
                        req.path
                    ),
                ));
            }
            let meta = std::fs::metadata(path)
                .map_err(|e| refused(404, format!("{}: {e}", path.display())))?;
            match kind {
                Kind::Sif if !meta.is_file() => {
                    return Err(refused(
                        422,
                        format!("{} is not a file, which a SIF is", path.display()),
                    ));
                }
                Kind::Sandbox if !meta.is_dir() => {
                    return Err(refused(
                        422,
                        format!("{} is not a folder, which a sandbox is", path.display()),
                    ));
                }
                _ => {}
            }
            (req.path.clone(), Some("apptainer".to_string()), None)
        }
    };
    let mut commands: Vec<String> = Vec::new();
    let catalog = catalog(registry).map_err(|e| refused(500, e))?;
    let mine = pipelines_of(&catalog, &app, &version, pack.as_ref());
    commands.extend(mine.iter().filter_map(|(_, d)| entry_point(d)));
    if let Some(first) = pack
        .as_ref()
        .and_then(|k| k.1.probe.as_ref())
        .and_then(|p| p.command.first())
        .filter(|w| layout::name_ok(w))
    {
        commands.push(first.clone());
    }
    commands.sort();
    commands.dedup();
    let now = nils_registry::time::now_iso();
    let mut m = Manifest {
        app: app.clone(),
        version: version.clone(),
        kind,
        how: How::Registered,
        managed: false,
        source: None,
        // the pinned image this install stands in for, where the pack says
        digest_asked: pack
            .as_ref()
            .and_then(|k| k.1.image.as_deref())
            .and_then(layout::split_reference)
            .map(|(_, d)| d.to_string()),
        digest: None,
        digest_how: None,
        pack_sha256: pack.as_ref().map(|k| k.2.clone()),
        image,
        size_bytes: size,
        runtime: runtime_name,
        by: req.principal.clone(),
        at: now.clone(),
        commands,
        fingerprint: None,
        verified_at: None,
        verification: Verification::default(),
        licence: pack
            .as_ref()
            .and_then(|k| k.1.licence.as_ref())
            .map(Licence::doc)
            .unwrap_or(Value::Null),
    };
    let _lock = lock_install(&l, &app, &version).map_err(|e| refused(500, e))?;
    if Manifest::read(&l.manifest(&app, &version)).is_some() {
        return Err(refused(
            409,
            format!("{app}@{version} was put in the software place by another process meanwhile"),
        ));
    }
    if let Some(text) = &req.pack {
        std::fs::write(l.pack(&app, &version), text)
            .map_err(|e| refused(500, format!("the pack's copy: {e}")))?;
    }
    m.write(&l.manifest(&app, &version))
        .map_err(|e| refused(500, format!("the manifest: {e}")))?;
    let mut externals = layout::read_externals(&l.externals()).map_err(|e| refused(500, e))?;
    externals.retain(|e| e.spec != m.spec());
    externals.push(External {
        spec: m.spec(),
        kind,
        path: req.path.clone(),
        by: req.principal.clone(),
        at: now,
    });
    layout::write_externals(&l.externals(), &externals)
        .map_err(|e| refused(500, format!("externals.yaml: {e}")))?;
    layout::write_wrappers(&l, &m).map_err(|e| refused(500, format!("the wrappers: {e}")))?;
    verify_install(registry, &l, &mut m, pack.as_ref().map(|k| &k.1))
        .map_err(|e| refused(500, e))?;
    let _ = nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &req.principal,
            action: nils_registry::audit::Action::SoftwareRegister,
            scope: json!({"app": app, "version": version}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "kind": kind.name(), "path": req.path, "managed": false,
                "fingerprint": m.fingerprint, "verified": !m.verification.failed(),
                "place": place.path.display().to_string(),
            })),
        },
    );
    let checked = needs(registry, home, &mine, &detected);
    Ok(json!({
        "registered": true,
        "package": package_doc(registry, &place, &app, &version, Some(&m), &mine, checked.clone()),
        "needs": checked,
        "verified": !m.verification.failed(),
    }))
}

// --------------------------------------------------------------- verify

/// `nils software verify <app>@<version>` (section 6.4): the probe, the
/// fingerprint against the one recorded, and the test run, taken now and
/// recorded in the manifest. A pack given now replaces the copy kept.
pub(crate) fn verify(
    registry: &mut Registry,
    home: &Path,
    app: &str,
    version: &str,
    pack: Option<&str>,
    principal: &str,
) -> Result<Value, Refused> {
    if !layout::name_ok(app) || !layout::name_ok(version) {
        return Err(refused(
            400,
            format!("{app}@{version} is not a package name"),
        ));
    }
    let place = place(registry).map_err(|e| refused(409, e))?;
    let l = place.layout();
    let Some(mut m) = Manifest::read(&l.manifest(app, version)) else {
        return Err(refused(
            404,
            format!("{app}@{version} is not in the software place"),
        ));
    };
    let parsed = match pack {
        Some(text) => {
            let parsed = layout::parse_pack(text).map_err(|e| refused(422, e))?;
            std::fs::write(l.pack(app, version), text)
                .map_err(|e| refused(500, format!("the pack's copy: {e}")))?;
            m.pack_sha256 = Some(parsed.2.clone());
            if let Some(k) = &parsed.1.licence {
                m.licence = k.doc();
            }
            Some(parsed)
        }
        None => pack_copy(&l, app, version),
    };
    verify_install(registry, &l, &mut m, parsed.as_ref().map(|k| &k.1))
        .map_err(|e| refused(500, e))?;
    let _ = nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal,
            action: nils_registry::audit::Action::SoftwareVerify,
            scope: json!({"app": app, "version": version}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "fingerprint": m.fingerprint, "verified": !m.verification.failed(),
                "verification": serde_json::to_value(&m.verification).unwrap_or(Value::Null),
            })),
        },
    );
    let catalog = catalog(registry).map_err(|e| refused(500, e))?;
    let mine = pipelines_of(&catalog, app, version, parsed.as_ref());
    let detected = crate::pipelines::detect_cached(registry);
    let checked = needs(registry, home, &mine, &detected);
    Ok(json!({
        "verified": !m.verification.failed(),
        "verification": serde_json::to_value(&m.verification).unwrap_or(Value::Null),
        "package": package_doc(registry, &place, app, version, Some(&m), &mine, checked.clone()),
        "needs": checked,
    }))
}

// --------------------------------------------------------------- accept

/// What an acceptance is asked.
#[derive(Debug, Clone, Default)]
pub(crate) struct AcceptRequest {
    pub(crate) app: String,
    pub(crate) version: String,
    /// The terms accepted: their address and version, where the pack does
    /// not name them.
    pub(crate) terms_address: Option<String>,
    pub(crate) terms_version: Option<String>,
    /// The site file's content, to place under `licences/<app>/`, and its
    /// name; the content is written and hashed, never recorded.
    pub(crate) file: Option<Vec<u8>>,
    pub(crate) file_name: Option<String>,
    pub(crate) note: Option<String>,
    pub(crate) principal: String,
}

/// `nils software accept <app>@<version>` (section 6.5): an administrator
/// accepts a tool's terms in their own name, once per site. The acceptance
/// is an event: who, when, which terms (address and version) and, for a
/// site file given with it, the file's sha256; its content is written under
/// `licences/<app>/`, readable by the engine's account alone, and never
/// recorded. Audited as `software.accept`.
///
/// Wave 7b (agenda 3.8, from the desk): a call that brings only the file (and
/// its name) to a package whose terms are accepted already places the file
/// and records no new acceptance; the acceptance that stands is answered,
/// with `recorded: false`. A call with terms or a note records as before.
pub(crate) fn accept(registry: &mut Registry, req: AcceptRequest) -> Result<Value, Refused> {
    let (app, version) = (req.app.clone(), req.version.clone());
    if !layout::name_ok(&app) || !layout::name_ok(&version) {
        return Err(refused(
            422,
            format!("{app}@{version} is not a package name"),
        ));
    }
    let place = place_made(registry).map_err(|e| refused(409, e))?;
    let l = place.layout();
    let licence = pack_copy(&l, &app, &version).and_then(|(_, s, _)| s.licence);
    // the file alone, to a package accepted already: the acceptance that
    // stands is kept and answered, and only the file is placed
    let file_only = req.file.is_some()
        && req.terms_address.is_none()
        && req.terms_version.is_none()
        && req.note.is_none();
    let existing = match file_only {
        true => acceptances(registry.store(), &app)
            .map_err(|e| refused(500, e))?
            .into_iter()
            .find(|a| a.version == version),
        false => None,
    };
    let address = match &existing {
        Some(a) => Some(a.terms_address.clone()),
        None => req
            .terms_address
            .clone()
            .or_else(|| licence.as_ref().and_then(|k| k.terms_address.clone())),
    };
    let Some(address) = address.filter(|a| !a.trim().is_empty()) else {
        return Err(refused(
            422,
            format!(
                "the terms of {app}@{version} have no address: the pack names none, so give \
                 --terms URL (and --terms-version where the terms have one)"
            ),
        ));
    };
    let terms_version = match &existing {
        Some(a) => a.terms_version.clone(),
        None => req
            .terms_version
            .clone()
            .or_else(|| licence.as_ref().and_then(|k| k.terms_version.clone())),
    };
    // the site file, placed
    let mut file_sha256 = None;
    let mut file_name = None;
    if let Some(bytes) = &req.file {
        if bytes.is_empty() || bytes.len() as u64 > nils_pipeline::secrets::MAX_BYTES {
            return Err(refused(
                422,
                format!(
                    "a licence file is a small text, under {} KiB, and not empty",
                    nils_pipeline::secrets::MAX_BYTES / 1024
                ),
            ));
        }
        let name = req
            .file_name
            .clone()
            .or_else(|| {
                licence
                    .as_ref()
                    .and_then(|k| k.site_file.as_ref())
                    .and_then(|s| s.name.clone())
            })
            .unwrap_or_else(|| "license.txt".to_string());
        if !layout::name_ok(&name) {
            return Err(refused(422, format!("{name} is not a file name")));
        }
        let dir = l.licences(&app);
        std::fs::create_dir_all(&dir)
            .map_err(|e| refused(500, format!("{}: {e}", dir.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                l.place.join(layout::LICENCES),
                std::fs::Permissions::from_mode(0o700),
            );
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        let path = dir.join(&name);
        let part = dir.join(format!(".{name}.part"));
        let _ = std::fs::remove_file(&part);
        nils_pipeline::files::write_new(&part, bytes)
            .map_err(|e| refused(500, format!("{}: {e}", path.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&part, &path)
            .map_err(|e| refused(500, format!("{}: {e}", path.display())))?;
        file_sha256 = Some(nils_pipeline::sha256(bytes));
        file_name = Some(name);
    } else if let Some(existing) = licence_file(&l, &app, licence.as_ref()) {
        // the file is there already: its digest is recorded with the acceptance
        if let Ok((_, hex)) = nils_pipeline::files::sha256_file(&existing) {
            file_sha256 = Some(format!("sha256:{hex}"));
            file_name = existing
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
        }
    }
    if let Some(a) = existing {
        let _ = nils_registry::audit::record(
            registry,
            &nils_registry::audit::Entry {
                principal: &req.principal,
                action: nils_registry::audit::Action::SoftwareAccept,
                scope: json!({"app": app, "version": version}),
                policy: None,
                job_id: None,
                details: Some(json!({
                    "recorded": false, "acceptance": a.id,
                    "file_sha256": file_sha256, "file_name": file_name,
                })),
            },
        );
        return Ok(json!({
            "accepted": true,
            "recorded": false,
            "acceptance": a.doc(),
            "file": file_sha256.as_ref().map(|s| json!({"sha256": s, "name": file_name, "under": l.licences(&app).display().to_string()})),
        }));
    }
    let a = record_acceptance(
        registry.store(),
        &NewAcceptance {
            app: &app,
            version: &version,
            principal: &req.principal,
            terms_address: &address,
            terms_version: terms_version.as_deref(),
            file_sha256: file_sha256.as_deref(),
            file_name: file_name.as_deref(),
            note: req.note.as_deref(),
            spdx: licence.as_ref().and_then(|k| k.spdx.as_deref()),
        },
    )
    .map_err(|e| refused(500, e))?;
    let _ = nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &req.principal,
            action: nils_registry::audit::Action::SoftwareAccept,
            scope: json!({"app": app, "version": version}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "terms": {"address": address, "version": terms_version},
                "file_sha256": file_sha256, "file_name": file_name, "note": req.note,
            })),
        },
    );
    Ok(json!({
        "accepted": true,
        "recorded": true,
        "acceptance": a.doc(),
        "file": file_sha256.as_ref().map(|s| json!({"sha256": s, "name": file_name, "under": l.licences(&app).display().to_string()})),
    }))
}

// --------------------------------------------------------------- remove

/// `nils software remove <app>@<version>`: an install the engine made is
/// taken away whole, its image (out of a docker or podman store too), its
/// manifest, its wrappers and its module. A registered install is refused
/// (409): the engine never removes what it did not install; it stays the
/// site's, recorded in `externals.yaml`.
pub(crate) fn remove(
    registry: &mut Registry,
    home: &Path,
    app: &str,
    version: &str,
    principal: &str,
) -> Result<Value, Refused> {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if app == crate::tools::DCM2NIIX {
        let into = crate::tools::installed_at(home);
        if !into.is_file() {
            return Err(refused(
                404,
                format!("the engine has no install of {app} of its own to remove"),
            ));
        }
        std::fs::remove_file(&into)
            .map_err(|e| refused(500, format!("{}: {e}", into.display())))?;
        let _ = std::fs::remove_dir(into.parent().unwrap_or(&into));
        let _ = nils_registry::audit::record(
            registry,
            &nils_registry::audit::Entry {
                principal,
                action: nils_registry::audit::Action::SoftwareRemove,
                scope: json!({"app": app, "version": version}),
                policy: None,
                job_id: None,
                details: Some(json!({"path": into.display().to_string()})),
            },
        );
        return Ok(json!({"removed": true, "app": app, "version": version}));
    }
    if !layout::name_ok(app) || !layout::name_ok(version) {
        return Err(refused(
            400,
            format!("{app}@{version} is not a package name"),
        ));
    }
    let place = place(registry).map_err(|e| refused(409, e))?;
    let l = place.layout();
    let Some(m) = Manifest::read(&l.manifest(app, version)) else {
        return Err(refused(
            404,
            format!("{app}@{version} is not in the software place"),
        ));
    };
    if m.how == How::Registered || !m.managed {
        return Err(refused(
            409,
            format!(
                "{app}@{version} is registered as the site's install at {}, not installed by the \
                 engine, which never updates or removes what it did not install; it stays recorded \
                 in externals.yaml (nils software forget {app}@{version} takes the registration \
                 back and leaves the install where it is)",
                m.image
            ),
        ));
    }
    if m.kind == Kind::Image
        && let Ok(rt) = runtime_for(registry, m.runtime.as_deref())
    {
        let _ = run_logged(
            &rt.program,
            &layout::remove_image_argv(&m.image),
            INSPECT_CAP,
            &l.install_dir(app, version).join("remove.log"),
            None,
        );
    }
    let _lock = lock_install(&l, app, version).map_err(|e| refused(500, e))?;
    layout::remove_wrappers(&l, &m).map_err(|e| refused(500, format!("the wrappers: {e}")))?;
    let dir = l.install_dir(app, version);
    std::fs::remove_dir_all(&dir).map_err(|e| refused(500, format!("{}: {e}", dir.display())))?;
    let _ = std::fs::remove_dir(l.place.join(app));
    let _ = nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal,
            action: nils_registry::audit::Action::SoftwareRemove,
            scope: json!({"app": app, "version": version}),
            policy: None,
            job_id: None,
            details: Some(json!({"kind": m.kind.name(), "image": m.image, "digest": m.digest})),
        },
    );
    Ok(json!({"removed": true, "app": app, "version": version, "kind": m.kind.name()}))
}

// --------------------------------------------------------------- forget

/// `nils software forget <app>@<version>`: a registration taken back. The
/// engine's own writings about it go (the manifest and the folder around
/// it, the wrappers, the links, the module, the entry in `externals.yaml`);
/// the registered path, the SIF, the folder or the image in the store, is
/// never touched. An install the engine made is refused (409): that is
/// `remove`.
pub(crate) fn forget(
    registry: &mut Registry,
    app: &str,
    version: &str,
    principal: &str,
) -> Result<Value, Refused> {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if !layout::name_ok(app) || !layout::name_ok(version) {
        return Err(refused(
            400,
            format!("{app}@{version} is not a package name"),
        ));
    }
    let place = place(registry).map_err(|e| refused(409, e))?;
    let l = place.layout();
    let Some(m) = Manifest::read(&l.manifest(app, version)) else {
        return Err(refused(
            404,
            format!("{app}@{version} is not in the software place"),
        ));
    };
    if m.how != How::Registered || m.managed {
        return Err(refused(
            409,
            format!(
                "{app}@{version} was installed by the engine, which forgets nothing it installed; \
                 nils software remove {app}@{version} takes it away"
            ),
        ));
    }
    let dir = l.install_dir(app, version);
    // the registered path is the site's: nothing is removed that lies on it
    if let Some(path) = m.image_path(&l)
        && (path.starts_with(&dir) || dir.starts_with(&path))
    {
        return Err(refused(
            409,
            format!(
                "{app}@{version} is registered at {}, which holds the engine's own folder for it; \
                 nothing is removed",
                path.display()
            ),
        ));
    }
    layout::remove_wrappers(&l, &m).map_err(|e| refused(500, format!("the wrappers: {e}")))?;
    let _lock = lock_install(&l, app, version).map_err(|e| refused(500, e))?;
    let mut externals = layout::read_externals(&l.externals()).map_err(|e| refused(500, e))?;
    let before = externals.len();
    externals.retain(|e| e.spec != m.spec());
    if externals.len() != before {
        layout::write_externals(&l.externals(), &externals)
            .map_err(|e| refused(500, format!("externals.yaml: {e}")))?;
    }
    std::fs::remove_dir_all(&dir).map_err(|e| refused(500, format!("{}: {e}", dir.display())))?;
    let _ = std::fs::remove_dir(l.place.join(app));
    let _ = nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal,
            action: nils_registry::audit::Action::SoftwareRemove,
            scope: json!({"app": app, "version": version}),
            policy: None,
            job_id: None,
            details: Some(json!({
                "forgotten": true, "kind": m.kind.name(), "path": m.image, "managed": false,
            })),
        },
    );
    Ok(json!({
        "forgotten": true, "app": app, "version": version, "kind": m.kind.name(),
        "path": m.image,
    }))
}

// ---------------------------------------------------------------- doors

fn replied(done: Result<Value, Refused>) -> Result<crate::serve::Reply, crate::serve::Reply> {
    use crate::serve::Reply;
    match done {
        Ok(done) => Ok(Reply::ok(done)),
        Err((status, body)) => {
            let mut r = Reply::error(status, body["error"].as_str().unwrap_or_default());
            r.body = body;
            Err(r)
        }
    }
}

fn spec_of(body: &Value) -> Result<(String, String), crate::serve::Reply> {
    use crate::serve::Reply;
    if let Some(spec) = body["spec"].as_str() {
        return layout::parse_spec(spec).map_err(|e| Reply::error(422, e));
    }
    let app = body["app"].as_str().unwrap_or_default().trim();
    let version = body["version"].as_str().unwrap_or_default().trim();
    if app.is_empty() {
        return Err(Reply::error(
            422,
            "the body names the package: {spec: \"<app>@<version>\"} or {app, version}",
        ));
    }
    if version.is_empty() {
        if app == crate::tools::DCM2NIIX {
            return Ok((app.to_string(), crate::tools::DCM2NIIX_VERSION.to_string()));
        }
        return Err(Reply::error(422, "the body names the version"));
    }
    layout::parse_spec(&format!("{app}@{version}")).map_err(|e| Reply::error(422, e))
}

/// The doors (section 6.7). The grants are the door table's: install:see
/// or pipelines:see to read, install:work for every act.
pub(crate) fn route(
    home: &Path,
    registry: &mut Registry,
    principal: &str,
    method: &str,
    segs: &[&str],
    body: &str,
) -> Option<Result<crate::serve::Reply, crate::serve::Reply>> {
    use crate::serve::Reply;
    let json = || crate::serve::json_body(if body.trim().is_empty() { "{}" } else { body });
    match (method, segs) {
        ("GET", ["api", "software"]) => Some(
            list(registry, home)
                .map(Reply::ok)
                .map_err(|e| Reply::error(500, e)),
        ),
        ("POST", ["api", "software", "install"]) => Some((|| {
            let b = json()?;
            let (app, version) = spec_of(&b)?;
            replied(install(
                registry,
                home,
                InstallRequest {
                    app,
                    version,
                    source: b["source"].as_str().map(str::to_string),
                    pack: b["pack"].as_str().map(str::to_string),
                    principal: principal.to_string(),
                },
            ))
        })()),
        ("POST", ["api", "software", "register"]) => Some((|| {
            let b = json()?;
            let (app, version) = spec_of(&b)?;
            replied(register(
                registry,
                home,
                RegisterRequest {
                    app,
                    version,
                    kind: b["kind"].as_str().unwrap_or_default().to_string(),
                    path: b["path"].as_str().unwrap_or_default().to_string(),
                    pack: b["pack"].as_str().map(str::to_string),
                    principal: principal.to_string(),
                },
            ))
        })()),
        ("POST", ["api", "software", app, version, "verify"]) => Some((|| {
            let b = json()?;
            replied(verify(
                registry,
                home,
                app,
                version,
                b["pack"].as_str(),
                principal,
            ))
        })()),
        ("POST", ["api", "software", app, version, "accept"]) => Some((|| {
            let b = json()?;
            replied(accept(
                registry,
                AcceptRequest {
                    app: app.to_string(),
                    version: version.to_string(),
                    terms_address: b["terms"]["address"]
                        .as_str()
                        .or_else(|| b["terms"].as_str())
                        .map(str::to_string),
                    terms_version: b["terms"]["version"].as_str().map(str::to_string),
                    file: b["file"].as_str().map(|f| f.as_bytes().to_vec()),
                    file_name: b["file_name"].as_str().map(str::to_string),
                    note: b["note"].as_str().map(str::to_string),
                    principal: principal.to_string(),
                },
            ))
        })()),
        ("POST", ["api", "software", app, version, "forget"]) => {
            Some(replied(forget(registry, app, version, principal)))
        }
        ("DELETE", ["api", "software", app, version]) => {
            Some(replied(remove(registry, home, app, version, principal)))
        }
        _ => None,
    }
}

// --------------------------------------------------------------- the CLI

/// `nils software`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum SoftwareCommand {
    /// Every package: its version, licence, size, place, how it came
    /// (installed or registered), status and its needs, checked
    List {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Install <app>@<version> into the software place: the image pulled by
    /// its digest from the pack's source (or --source), the manifest,
    /// wrappers and module written, and the install verified; dcm2niix is
    /// installed as its own package
    Install {
        /// <app>@<version>, as freesurfer@8.2.0, or dcm2niix
        spec: String,
        /// The pipeline pack (nils-pack.yaml), whose source, probe, key
        /// files, licence and test the install reads
        #[arg(long, value_name = "FILE")]
        pack: Option<PathBuf>,
        /// The image, repository@sha256:<hex>, where no pack and no pipeline
        /// of the catalog pins it
        #[arg(long, value_name = "IMAGE")]
        source: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Register an existing install as the site's: the engine never updates
    /// or removes it
    Register {
        /// <app>@<version>
        spec: String,
        /// sif, sandbox or image
        #[arg(long, value_name = "KIND")]
        kind: String,
        /// The SIF file or sandbox folder, or an image reference
        /// (repository@sha256:<hex>) in the docker or podman store
        #[arg(long, value_name = "PATH")]
        path: String,
        /// The pipeline pack, whose probe, key files, licence and test the
        /// verification reads
        #[arg(long, value_name = "FILE")]
        pack: Option<PathBuf>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Verify an install again: the version probe, the fingerprint against
    /// the one recorded, and the pack's test run
    Verify {
        /// <app>@<version>
        spec: String,
        /// The pipeline pack, replacing the copy kept with the install
        #[arg(long, value_name = "FILE")]
        pack: Option<PathBuf>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Accept a tool's terms in your own name, once for this site, and
    /// place its licence file where the engine mounts it
    Accept {
        /// <app>@<version>
        spec: String,
        /// The site's licence file, placed under licences/<app>/ and hashed;
        /// its content is never recorded. Alone, on a package whose terms
        /// are accepted already, it places the file and records nothing new
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// The terms' address, where the pack names none
        #[arg(long, value_name = "URL")]
        terms: Option<String>,
        /// The terms' version
        #[arg(long, value_name = "VERSION")]
        terms_version: Option<String>,
        /// The name the file is placed under, where the pack names none or
        /// another (the default is the pack's, never the source file's)
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// A note kept with the acceptance
        #[arg(long)]
        note: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Take a registration back: the engine's own writings about it go, the
    /// registered path is never touched; an install the engine made is
    /// refused (remove)
    Forget {
        /// <app>@<version>
        spec: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Remove an install the engine made; a registered install is refused
    Remove {
        /// <app>@<version>
        spec: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

fn print(v: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
    );
}

fn said(status: u16, body: Value) -> Exit {
    let message = body["error"].as_str().unwrap_or_default().to_string();
    if status == 404 || status == 422 {
        crate::usage(message)
    } else {
        crate::fail(message)
    }
}

fn read_pack(path: Option<&PathBuf>) -> Result<Option<String>, Exit> {
    match path {
        None => Ok(None),
        Some(p) => std::fs::read_to_string(p)
            .map(Some)
            .map_err(|e| crate::usage(format!("{}: {e}", p.display()))),
    }
}

/// The checks of a package, each on a line: its state, its name, and what
/// was found with its fix where it does not hold.
fn say_needs(needs: &[Value]) {
    for c in needs {
        let state = c["state"]
            .as_str()
            .unwrap_or(if c["ok"] == true { "ok" } else { "missing" });
        println!(
            "    {state:<8} {:<10} {}",
            c["check"].as_str().unwrap_or_default(),
            c["said"].as_str().unwrap_or_default()
        );
        if state != "ok"
            && let Some(fix) = c["failure"]["advice"].as_str()
        {
            println!("             fix: {fix}");
        }
    }
}

fn say_package(p: &Value) {
    println!(
        "{:<28} {:<20} {}{}",
        p["spec"].as_str().unwrap_or_default(),
        p["status"].as_str().unwrap_or_default(),
        p["kind"].as_str().unwrap_or("-"),
        p["path"]
            .as_str()
            .or(p["image"].as_str())
            .map(|x| format!(" at {x}"))
            .unwrap_or_default()
    );
    if let Some(f) = p["fingerprint"].as_str() {
        println!(
            "    fingerprint {f}{}",
            p["verified_at"]
                .as_str()
                .map(|v| format!(", verified {v}"))
                .unwrap_or_default()
        );
    }
    let licence = &p["licence"];
    if licence["accept"] == true {
        match licence["accepted"]["by"].as_str() {
            Some(by) => println!(
                "    terms accepted by {by} on {}",
                licence["accepted"]["at"].as_str().unwrap_or_default()
            ),
            None => println!(
                "    terms at {} not accepted yet: nils software accept {}",
                licence["terms"]["address"]
                    .as_str()
                    .unwrap_or("their address"),
                p["spec"].as_str().unwrap_or_default()
            ),
        }
    }
    if let Some(needs) = p["needs"].as_array()
        && !needs.is_empty()
    {
        say_needs(needs);
    }
}

/// `nils software list | install | register | verify | accept | remove`.
pub(crate) fn command(
    home: &nils_registry::home::Home,
    command: Option<SoftwareCommand>,
) -> Result<(), Exit> {
    let mut registry = crate::open(home)?;
    let principal = nils_registry::principal::Principal::current().to_string();
    match command.unwrap_or(SoftwareCommand::List { json: false }) {
        SoftwareCommand::List { json } => {
            let listed = list(&mut registry, home.dir()).map_err(crate::fail)?;
            if json {
                print(&listed);
            } else {
                match listed["place"]["path"].as_str() {
                    Some(path) => println!(
                        "software place: {path}{}",
                        if listed["place"]["declared"].is_null() {
                            " (the default under the working place; declare one with --place NAME=DIR,role=software)"
                        } else {
                            ""
                        }
                    ),
                    None => println!(
                        "software place: none (no working place either); declare one with nils place add software PATH --role software"
                    ),
                }
                for p in listed["packages"].as_array().into_iter().flatten() {
                    say_package(p);
                }
            }
            Ok(())
        }
        SoftwareCommand::Install {
            spec,
            pack,
            source,
            json,
        } => {
            let (app, version) = if spec == crate::tools::DCM2NIIX {
                (spec.clone(), crate::tools::DCM2NIIX_VERSION.to_string())
            } else {
                layout::parse_spec(&spec).map_err(crate::usage)?
            };
            let done = install(
                &mut registry,
                home.dir(),
                InstallRequest {
                    app,
                    version,
                    source,
                    pack: read_pack(pack.as_ref())?,
                    principal,
                },
            )
            .map_err(|(s, b)| said(s, b))?;
            if json {
                print(&done);
            } else if done["installed"] == true {
                println!(
                    "installed {}{}",
                    done["package"]["spec"].as_str().unwrap_or_default(),
                    if done["verified"] == true {
                        ", verified"
                    } else {
                        ", FAILED VERIFICATION"
                    }
                );
                say_package(&done["package"]);
            } else {
                println!("{}", done["why"].as_str().unwrap_or_default());
            }
            Ok(())
        }
        SoftwareCommand::Register {
            spec,
            kind,
            path,
            pack,
            json,
        } => {
            let (app, version) = layout::parse_spec(&spec).map_err(crate::usage)?;
            let path = if kind.trim() == "image" {
                path
            } else {
                std::fs::canonicalize(&path)
                    .map(|p| p.display().to_string())
                    .unwrap_or(path)
            };
            let done = register(
                &mut registry,
                home.dir(),
                RegisterRequest {
                    app,
                    version,
                    kind,
                    path,
                    pack: read_pack(pack.as_ref())?,
                    principal,
                },
            )
            .map_err(|(s, b)| said(s, b))?;
            if json {
                print(&done);
            } else {
                println!(
                    "registered {}{}",
                    done["package"]["spec"].as_str().unwrap_or_default(),
                    if done["verified"] == true {
                        ", verified"
                    } else {
                        ", FAILED VERIFICATION"
                    }
                );
                say_package(&done["package"]);
            }
            Ok(())
        }
        SoftwareCommand::Verify { spec, pack, json } => {
            let (app, version) = layout::parse_spec(&spec).map_err(crate::usage)?;
            let text = read_pack(pack.as_ref())?;
            let done = verify(
                &mut registry,
                home.dir(),
                &app,
                &version,
                text.as_deref(),
                &principal,
            )
            .map_err(|(s, b)| said(s, b))?;
            if json {
                print(&done);
            } else {
                println!(
                    "{app}@{version}: {}",
                    if done["verified"] == true {
                        "verified"
                    } else {
                        "FAILED VERIFICATION"
                    }
                );
                for (name, c) in done["verification"].as_object().into_iter().flatten() {
                    println!(
                        "    {name:<12} {} {}",
                        match (c["ran"].as_bool(), c["passed"].as_bool()) {
                            (Some(false), _) => "not run",
                            (_, Some(true)) => "passed ",
                            _ => "FAILED ",
                        },
                        c["said"].as_str().unwrap_or_default()
                    );
                }
            }
            Ok(())
        }
        SoftwareCommand::Accept {
            spec,
            file,
            terms,
            terms_version,
            name,
            note,
            json,
        } => {
            let (app, version) = layout::parse_spec(&spec).map_err(crate::usage)?;
            let bytes = match &file {
                Some(p) => Some(
                    std::fs::read(p).map_err(|e| crate::usage(format!("{}: {e}", p.display())))?,
                ),
                None => None,
            };
            let done = accept(
                &mut registry,
                AcceptRequest {
                    app,
                    version,
                    terms_address: terms,
                    terms_version,
                    file: bytes,
                    // the placed file takes the pack's name; the path given is
                    // only where the content comes from
                    file_name: name,
                    note,
                    principal,
                },
            )
            .map_err(|(s, b)| said(s, b))?;
            if json {
                print(&done);
            } else {
                let a = &done["acceptance"];
                if done["recorded"] == false {
                    println!(
                        "the terms of {} at {} were accepted by {} on {} already; nothing new recorded",
                        a["app"].as_str().unwrap_or_default(),
                        a["terms"]["address"].as_str().unwrap_or_default(),
                        a["by"].as_str().unwrap_or_default(),
                        a["at"].as_str().unwrap_or_default()
                    );
                } else {
                    println!(
                        "accepted the terms of {} at {} by {} on {}",
                        a["app"].as_str().unwrap_or_default(),
                        a["terms"]["address"].as_str().unwrap_or_default(),
                        a["by"].as_str().unwrap_or_default(),
                        a["at"].as_str().unwrap_or_default()
                    );
                }
                if let Some(f) = done["file"].as_object() {
                    println!(
                        "licence file {} placed under {}, sha256 {}",
                        f["name"].as_str().unwrap_or_default(),
                        f["under"].as_str().unwrap_or_default(),
                        f["sha256"].as_str().unwrap_or_default()
                    );
                }
            }
            Ok(())
        }
        SoftwareCommand::Forget { spec, json } => {
            let (app, version) = layout::parse_spec(&spec).map_err(crate::usage)?;
            let done =
                forget(&mut registry, &app, &version, &principal).map_err(|(s, b)| said(s, b))?;
            if json {
                print(&done);
            } else {
                println!(
                    "forgot {app}@{version}; {} is left as it was",
                    done["path"].as_str().unwrap_or_default()
                );
            }
            Ok(())
        }
        SoftwareCommand::Remove { spec, json } => {
            let (app, version) = if spec == crate::tools::DCM2NIIX {
                (spec.clone(), crate::tools::DCM2NIIX_VERSION.to_string())
            } else {
                layout::parse_spec(&spec).map_err(crate::usage)?
            };
            let done = remove(&mut registry, home.dir(), &app, &version, &principal)
                .map_err(|(s, b)| said(s, b))?;
            if json {
                print(&done);
            } else {
                println!("removed {app}@{version}");
            }
            Ok(())
        }
    }
}
