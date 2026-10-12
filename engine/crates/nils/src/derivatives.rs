// SPDX-License-Identifier: AGPL-3.0-only

//! Derivatives at the doors and the keyboard (record 42 S4). The registry
//! keeps the rows (`nils_registry::derivative`); this module writes the
//! bytes into a working place, hashes them on the way in, and serves them
//! back.
//!
//! A file is written under `derivatives/<kind>/<first two of its digest>/`
//! in the working place, named by its sha256 and the extension it came
//! with, and never by a name a caller chose: a name may carry a person's
//! code, a digest cannot. The same content registered twice is one file and
//! two rows.
//!
//! Absence (D1): a deployment that binds no working place has no home for
//! a derivative, so the capability is off and the doors that write or read
//! bytes answer so, with the sentence that names the cure.
//!
//! The grants are the Pipelines page's (record 25 declares one grant per
//! page of the desk, and derivatives are what pipelines make): the
//! metadata and the listing need `pipelines:see`, the bytes `pipelines:see`
//! at detail quasi, since a mask or an embedding is drawn from the pixels,
//! and registering one `pipelines:work`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use nils_registry::Registry;
use nils_registry::derivative::{self, Belongs, Derivative, KINDS, TREE};
use nils_registry::place::{self, Place, Role};
use nils_registry::store::Store;
use serde_json::{Value, json};

use crate::serve::{Caller, Reply};

/// The largest body the door takes: 16 GiB.
pub(crate) const UPLOAD_MAX: u64 = 16 * 1024 * 1024 * 1024;

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 10] = [
    "GET /api/derivatives",
    "POST /api/derivatives",
    "GET /api/derivatives/{id}",
    "GET /api/derivatives/{id}/content",
    // record 55 H3, job type 3: a result's parts and its layer on a scan
    "GET /api/derivatives/{id}/parts",
    "GET /api/derivatives/{id}/layers/{stack}/{set}/manifest",
    "GET /api/derivatives/{id}/layers/{stack}/{set}/tiles/{level}/{z}",
    "GET /api/derivatives/{id}/layers/{stack}/{set}/slab/{level}/{z0}-{z1}",
    // record 55 H3, section 8.5: one result checked directly, and withdrawn
    "POST /api/derivatives/{id}/check",
    "POST /api/derivatives/{id}/check/withdraw",
];

/// The most parts one page of the parts door answers.
pub(crate) const PARTS_PAGE_MAX: usize = 500;

/// The working place a derivative is written into: the first active one,
/// or the sentence saying the capability is off.
pub(crate) fn working(store: &mut Store) -> Result<Place, String> {
    let places = place::active(store).map_err(|e| e.to_string())?;
    places
        .into_iter()
        .find(|p| p.role == Role::Working)
        .ok_or_else(|| {
            "derivatives are off: no working place is bound, and a derivative lives in one; an operator adds one under Settings or with nils place add --role working (Wave 5 section 10.2)".to_string()
        })
}

/// What `GET /api/capabilities` says of derivatives.
pub(crate) fn capability(store: &mut Store) -> Value {
    let grants = json!({"see": "pipelines:see", "content": "pipelines:see", "content_detail": "quasi", "register": "pipelines:work"});
    // record 55 H3, job type 3: a mask's parts and its layers on its scans,
    // read by pipelines:see or by a rater of an open campaign that holds it
    let parts = json!({
        "door": "/api/derivatives/{id}/parts", "grants": ["pipelines:see", "rater"],
        "detail": "plain", "page_max": PARTS_PAGE_MAX, "set_max": crate::mask::MAX_PARTS,
        "kinds": ["components", "labels", "whole"], "orders": ["position", "size"],
    });
    let layers = json!({
        "door": "/api/derivatives/{id}/layers/{stack}/{set}", "grants": ["pipelines:see", "rater"],
        "detail": "quasi", "codec": crate::pyramid::CODEC, "tile": crate::pyramid::TILE,
        "slab_max": crate::pyramid::SLAB_MAX, "dtype": "uint16",
        "links": derivative::GRID_LINKS, "checks": derivative::CHECKS,
    });
    // record 55 H3, section 8.5: a result checked directly by a person
    let check = json!({
        "door": "/api/derivatives/{id}/check", "withdraw": "/api/derivatives/{id}/check/withdraw",
        "grant": "review:work", "detail": "plain", "checks": derivative::CHECKS,
        "filter": ["accepted", "rejected", "flagged", "unchecked"],
    });
    match working(store) {
        Ok(p) => json!({
            "enabled": true, "place": p.name, "kinds": KINDS, "grants": grants,
            "upload_max_bytes": UPLOAD_MAX, "parts": parts, "layers": layers, "check": check,
        }),
        Err(reason) => json!({
            "enabled": false, "reason": reason, "kinds": KINDS, "grants": grants,
            "parts": parts, "layers": layers, "check": check,
        }),
    }
}

/// A file written into a place: its path under the place, its size and
/// its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stored {
    pub path: String,
    pub bytes: i64,
    pub sha256: String,
}

/// Why a file was not stored.
#[derive(Debug)]
pub(crate) enum StoreFail {
    /// The bytes are not the ones the caller named; nothing was kept.
    Mismatch {
        named: String,
        found: String,
    },
    TooLarge,
    Io(String),
}

impl std::fmt::Display for StoreFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreFail::Mismatch { named, found } => write!(
                f,
                "the bytes hash to sha256 {found}, not the {named} named; nothing was registered"
            ),
            StoreFail::TooLarge => write!(f, "a derivative is at most {UPLOAD_MAX} bytes"),
            StoreFail::Io(m) => write!(f, "{m}"),
        }
    }
}

/// The extension a file came with, when it is a plain one (`.nii.gz`,
/// `.npy`), and nothing else of its name.
fn extension(name: Option<&str>) -> String {
    let Some(name) = name else {
        return String::new();
    };
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let Some((_, ext)) = base.split_once('.') else {
        return String::new();
    };
    if ext.is_empty()
        || ext.len() > 16
        || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
        || ext.starts_with('.')
        || ext.ends_with('.')
    {
        return String::new();
    }
    format!(".{}", ext.to_ascii_lowercase())
}

/// Whether a text is a sha256 as hex.
pub(crate) fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// Write what `reader` gives into the place at `root`, hashing it on the
/// way, under its content address. With `expect`, bytes that hash to
/// anything else are removed and refused.
pub(crate) fn store_file(
    root: &Path,
    kind: &str,
    name: Option<&str>,
    reader: &mut dyn Read,
    expect: Option<&str>,
) -> Result<Stored, StoreFail> {
    let io = |what: &str, e: std::io::Error| StoreFail::Io(format!("{what}: {e}"));
    let incoming = root.join(TREE).join(".incoming");
    std::fs::create_dir_all(&incoming).map_err(|e| io("the working place", e))?;
    let mut nonce = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce)
        .map_err(|_| StoreFail::Io("no randomness for a temporary name".into()))?;
    let temp = incoming.join(hex::encode(nonce));
    let result = (|| -> Result<(u64, String), StoreFail> {
        let mut out = std::fs::File::create(&temp).map_err(|e| io("the working place", e))?;
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = vec![0u8; 1 << 20];
        let mut total: u64 = 0;
        loop {
            let n = reader.read(&mut buffer).map_err(|e| io("the upload", e))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > UPLOAD_MAX {
                return Err(StoreFail::TooLarge);
            }
            context.update(&buffer[..n]);
            out.write_all(&buffer[..n])
                .map_err(|e| io("the working place", e))?;
        }
        out.sync_all().map_err(|e| io("the working place", e))?;
        Ok((total, hex::encode(context.finish().as_ref())))
    })();
    let (bytes, sha256) = match result {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
    };
    if let Some(named) = expect
        && !named.eq_ignore_ascii_case(&sha256)
    {
        let _ = std::fs::remove_file(&temp);
        return Err(StoreFail::Mismatch {
            named: named.to_ascii_lowercase(),
            found: sha256,
        });
    }
    let path = format!("{TREE}/{kind}/{}/{sha256}{}", &sha256[..2], extension(name));
    let target = root.join(&path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io("the working place", e))?;
    }
    // The same content is the same file: a second registration leaves the
    // first file where it is.
    let same = std::fs::metadata(&target).is_ok_and(|m| m.len() == bytes);
    if same {
        let _ = std::fs::remove_file(&temp);
    } else {
        std::fs::rename(&temp, &target).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            io("the working place", e)
        })?;
    }
    Ok(Stored {
        path,
        bytes: bytes as i64,
        sha256,
    })
}

/// What a registration asks for, however it arrived.
pub(crate) struct Ask<'a> {
    pub kind: &'a str,
    pub stack: Option<i64>,
    pub series: Option<i64>,
    pub subject: Option<i64>,
    pub day: Option<&'a str>,
    pub media_type: &'a str,
    pub supersedes: Option<i64>,
    /// The registered model that made it: its id, its digest or
    /// `name@version` (record 42 S2).
    pub model: Option<&'a str>,
    pub name: Option<&'a str>,
    pub sha256: Option<&'a str>,
    pub principal: &'a str,
    pub actor: Option<&'a Value>,
    /// Record 55 H3: for a mask, the scan whose grid it is on.
    pub grid_stack: Option<i64>,
}

/// Why a registration was refused: a status and a sentence.
pub(crate) type Refused = (u16, String);

/// What an ask resolved to: what it belongs to, the place it goes into and
/// the registered model that made it.
pub(crate) struct Checked {
    pub belongs: Belongs,
    pub place: Place,
    pub model_id: Option<i64>,
}

/// Check an ask before any byte is read: the kind, what it belongs to, the
/// row it supersedes, the model that made it, and the place it goes into.
pub(crate) fn check(store: &mut Store, a: &Ask<'_>) -> Result<Checked, Refused> {
    let place = working(store).map_err(|m| (409, m))?;
    if !derivative::is_kind(a.kind) {
        return Err((
            400,
            format!(
                "{} is not a kind of derivative; the kinds are {}",
                a.kind,
                KINDS.join(", ")
            ),
        ));
    }
    if derivative::RUN_KINDS.contains(&a.kind) {
        return Err((
            400,
            format!(
                "a derivative of kind {} is written by a pipeline run, not registered at the door",
                a.kind
            ),
        ));
    }
    if let Some(s) = a.sha256
        && !is_sha256(s)
    {
        return Err((400, "sha256 is 64 hexadecimal digits".into()));
    }
    let belongs =
        derivative::belongs(store, a.stack, a.series, a.subject, a.day).map_err(|e| match e {
            Ok(m) => (400, m),
            Err(e) => (500, e.to_string()),
        })?;
    if let Some(old) = a.supersedes {
        let prior = derivative::get(store, old)
            .map_err(|e| (500, e.to_string()))?
            .ok_or_else(|| (400, format!("no derivative {old} to supersede")))?;
        if prior.kind != a.kind {
            return Err((
                400,
                format!(
                    "derivative {old} is {}; {} does not supersede it",
                    nils_registry::a_or_an(&prior.kind),
                    nils_registry::a_or_an(a.kind)
                ),
            ));
        }
        // a successor is of what its predecessor was of: the same subject,
        // and the same stack where either names one
        if prior.subject_id != belongs.subject_id || prior.stack_id != belongs.stack_id {
            return Err((
                400,
                format!(
                    "derivative {old} was made from {}; one made from {} does not supersede it",
                    made_from(prior.subject_id, prior.stack_id),
                    made_from(belongs.subject_id, belongs.stack_id)
                ),
            ));
        }
    }
    if let Some(s) = a.grid_stack {
        if a.kind != "mask" {
            return Err((
                400,
                format!(
                    "grid_stack names the scan a mask's grid is on; {} has no grid",
                    nils_registry::a_or_an(a.kind)
                ),
            ));
        }
        derivative::belongs(store, Some(s), None, None, None).map_err(|e| match e {
            Ok(_) => (400, format!("grid_stack: no stack {s}")),
            Err(e) => (500, e.to_string()),
        })?;
    }
    let model_id = match a.model {
        None => None,
        Some(reference) => Some(
            nils_registry::model::resolve(store, reference)
                .map_err(|e| (500, e.to_string()))?
                .ok_or_else(|| (400, format!("no registered model answers to {reference}")))?
                .id,
        ),
    };
    Ok(Checked {
        belongs,
        place,
        model_id,
    })
}

/// What a derivative was made from, as a refusal names it: ids only.
fn made_from(subject: Option<i64>, stack: Option<i64>) -> String {
    match (stack, subject) {
        (Some(k), _) => format!("stack {k}"),
        (None, Some(s)) => format!("subject {s}"),
        (None, None) => "nothing named".to_string(),
    }
}

/// Register a derivative: check, write the bytes, write the row, audit.
pub(crate) fn register(
    registry: &mut Registry,
    a: &Ask<'_>,
    reader: &mut dyn Read,
) -> Result<Derivative, Refused> {
    let Checked {
        belongs,
        place,
        model_id,
    } = check(registry.store(), a)?;
    let stored = store_file(Path::new(&place.path), a.kind, a.name, reader, a.sha256).map_err(
        |e| match e {
            StoreFail::Mismatch { .. } => (422, e.to_string()),
            StoreFail::TooLarge => (413, e.to_string()),
            StoreFail::Io(m) => (500, m),
        },
    )?;
    // record 55 H3 (the review of E1, item 15): a mask is read before it
    // is registered, its voxels no further than its header declares. One
    // whose header declares more than a mask holds is refused and nothing
    // of it registered, its bytes gone unless another derivative holds the
    // same file; one that is no NIfTI mask (a file an outside app answered
    // with, record 42) is registered, and read as one that cannot be drawn
    let read = (a.kind == "mask")
        .then(|| crate::mask::read_labels(&Path::new(&place.path).join(&stored.path)));
    if let Some(Err(why)) = &read
        && crate::mask::too_large(why)
    {
        let store = registry.store();
        let d = store.dialect();
        let held = store
            .query_opt(
                &format!(
                    "SELECT COUNT(*) FROM {} WHERE place_id = {} AND path = {}",
                    store.qualified("derivative"),
                    d.param(1, nils_registry::schema::Type::Int),
                    d.param(2, nils_registry::schema::Type::Text)
                ),
                &[
                    nils_registry::Param::Int(place.id),
                    nils_registry::Param::from(stored.path.as_str()),
                ],
            )
            .ok()
            .flatten()
            .and_then(|r| r.int(0).ok())
            .is_none_or(|n| n > 0);
        if !held {
            let _ = std::fs::remove_file(Path::new(&place.path).join(&stored.path));
        }
        return Err((413, format!("nothing is registered: {why}")));
    }
    let now = nils_registry::time::now_iso();
    let store = registry.store();
    let id = derivative::insert(
        store,
        &derivative::New {
            kind: a.kind,
            belongs: &belongs,
            place_id: place.id,
            path: &stored.path,
            bytes: stored.bytes,
            sha256: &stored.sha256,
            media_type: a.media_type,
            registered_by: a.principal,
            actor: a.actor,
            model_id,
            run_id: None,
            preprocess_version: None,
            supersedes_id: a.supersedes,
            created_at: &now,
        },
    )
    .map_err(|e| (500, e.to_string()))?;
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: a.principal,
            action: nils_registry::audit::Action::DerivativeRegister,
            scope: json!({
                "derivative": id, "kind": a.kind, "scope": belongs.scope,
                "stack": belongs.stack_id, "series": belongs.series_id,
                "subject": belongs.subject_id, "place": place.name, "model": model_id,
            }),
            policy: None,
            job_id: None,
            details: Some(json!({
                "bytes": stored.bytes, "sha256": stored.sha256, "supersedes": a.supersedes,
            })),
        },
    )
    .map_err(|e| (500, e.to_string()))?;
    // record 55 H3: a mask's grid and parts are read as it is registered
    if a.kind == "mask"
        && let Some(d) = derivative::get(registry.store(), id).map_err(|e| (500, e.to_string()))?
    {
        let at = match a.grid_stack {
            Some(s) => Where::Named(s),
            None => Where::Unknown,
        };
        read_mask_from(
            registry,
            &d,
            at,
            vec![crate::mask::SetSpec::whole()],
            read.unwrap_or_else(|| Err("not read".into())),
        )
        .map_err(|e| (500, e))?;
    }
    derivative::get(registry.store(), id)
        .map_err(|e| (500, e.to_string()))?
        .ok_or_else(|| (500, "the derivative was not written back".to_string()))
}

// ------------------------------------------------------------ a result's grid and parts

/// Where a mask's grid scan is looked for (record 55 H3, section 2.2).
#[derive(Debug, Clone)]
pub(crate) enum Where {
    /// The stacks a unit was given for the output's declared grid role,
    /// each with its input NIfTI where the run made one: the one whose
    /// grid the mask's matches, else the first.
    Role(Vec<(i64, Option<PathBuf>)>),
    /// A stack unit's own stack.
    Own(i64),
    /// The unit's inputs: the one whose NIfTI's grid the mask's matches,
    /// else none.
    Inputs(Vec<(i64, Option<PathBuf>)>),
    /// An upload's grid_stack, or the scan read before.
    Named(i64),
    Unknown,
}

/// The picture of a stack, where it is built.
fn scan_manifest(store: &mut Store, stack: i64) -> Option<crate::pyramid::Manifest> {
    let w = crate::pyramid::working_place_cached(store).ok()?;
    crate::pyramid::manifest_cached(&crate::pyramid::dir(Path::new(&w.path), stack))
        .ok()
        .flatten()
}

/// How a mask is tied to a candidate scan: by the scan's input NIfTI where
/// there is one, else by its picture where it is built, else `world`
/// (the layer's build ties it again once the picture is there).
fn tie_to(
    store: &mut Store,
    grid: &crate::mask::Grid,
    stack: i64,
    input: Option<&Path>,
) -> &'static str {
    if let Some(p) = input
        && let Ok(g) = crate::mask::read_grid(p)
    {
        return crate::mask::link_of(grid, Some(&g.lattice()), true);
    }
    let scan = scan_manifest(store, stack).and_then(|m| crate::mask::scan_lattice(&m).ok());
    crate::mask::link_of(grid, scan.as_ref(), true)
}

/// Read a mask whole: its grid from its header, the scan whose grid it is
/// on and how the two are tied, and the parts of each set, written to its
/// row and to `derivative_part`. Nothing of the file changes. A file that
/// is not a NIfTI mask is tied by `none` and read whole, with why.
pub(crate) fn read_mask(
    registry: &mut Registry,
    d: &Derivative,
    at: Where,
    sets: Vec<crate::mask::SetSpec>,
) -> Result<Value, String> {
    let store = registry.store();
    // a file that is not in its place, or not a NIfTI mask, is read as
    // one that cannot be drawn, with why, so a check counts it and leaves
    // it out instead of asking for it again (the review of E1, item 2)
    let read = file_of(store, d)
        .map_err(|(_, m)| m)
        .and_then(|file| crate::mask::read_labels(&file));
    read_mask_from(registry, d, at, sets, read)
}

/// [`read_mask`], from the mask's file read already.
pub(crate) fn read_mask_from(
    registry: &mut Registry,
    d: &Derivative,
    at: Where,
    sets: Vec<crate::mask::SetSpec>,
    read: Result<(crate::mask::Grid, Vec<u32>), String>,
) -> Result<Value, String> {
    let store = registry.store();
    let (grid, data) = match read {
        Ok(r) => r,
        Err(why) => {
            derivative::set_grid(store, d.id, &Value::Null, None, "none")
                .map_err(|e| e.to_string())?;
            let whole = crate::mask::SetSpec::whole().doc(0, Some(&why));
            derivative::set_parts(store, d.id, &json!([whole]), &[]).map_err(|e| e.to_string())?;
            return Ok(json!({"derivative": d.id, "grid_link": "none", "why": why}));
        }
    };
    let (stack, link) = match at {
        Where::Role(c) | Where::Inputs(c) if c.is_empty() => (None, "none"),
        Where::Role(c) => {
            let same = c.iter().find(|(s, p)| {
                p.as_deref()
                    .is_some_and(|p| tie_to(store, &grid, *s, Some(p)) == "same")
            });
            match same {
                Some((s, _)) => (Some(*s), "same"),
                None => {
                    let (s, p) = &c[0];
                    (Some(*s), tie_to(store, &grid, *s, p.as_deref()))
                }
            }
        }
        Where::Inputs(c) => c
            .iter()
            .find(|(s, p)| {
                p.as_deref()
                    .is_some_and(|p| tie_to(store, &grid, *s, Some(p)) == "same")
            })
            .map_or((None, "none"), |(s, _)| (Some(*s), "same")),
        Where::Own(s) | Where::Named(s) => (Some(s), tie_to(store, &grid, s, None)),
        Where::Unknown => (None, "none"),
    };
    let link = if grid.placed() { link } else { "none" };
    derivative::set_grid(store, d.id, &grid.doc(), stack, link).map_err(|e| e.to_string())?;

    let mut docs = Vec::new();
    let mut written = Vec::new();
    let mut summary = Vec::new();
    for set in &sets {
        match crate::mask::make_parts(&grid, &data, set) {
            crate::mask::Made::Parts(parts) => {
                docs.push(set.doc(parts.len(), None));
                summary.push(json!({"set": set.key, "parts": parts.len()}));
                written.push((set.key.clone(), parts));
            }
            crate::mask::Made::Whole(note) => {
                // read whole, it still draws only the labels it names
                let mut whole = crate::mask::SetSpec::whole();
                whole.key = set.key.clone();
                whole.words = set.words.clone();
                whole.labels = set.labels.clone();
                docs.push(whole.doc(0, Some(&note)));
                summary.push(json!({"set": set.key, "parts": 0, "note": note}));
                written.push((set.key.clone(), Vec::new()));
            }
        }
    }
    let withdrawn = derivative::set_parts(store, d.id, &Value::Array(docs), &written)
        .map_err(|e| e.to_string())?;
    // a part whose label moved onto other geometry lost its check
    if !withdrawn.is_empty() {
        nils_registry::audit::record(
            registry,
            &nils_registry::audit::Entry {
                principal: &crate::actor(),
                action: nils_registry::audit::Action::Decision,
                scope: json!({"withdrawn": withdrawn, "derivative": d.id, "axis": "check"}),
                policy: None,
                job_id: None,
                details: Some(json!({"why": "its parts were read again, and these parts are not the parts they were"})),
            },
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(json!({
        "derivative": d.id, "grid_link": link, "grid_stack": stack, "sets": summary,
        "withdrawn": withdrawn,
    }))
}

/// The declared output of a pipeline that a run's file is, by its path
/// under the run's folder: the descriptor and the output.
pub(crate) fn output_of(
    store: &mut Store,
    d: &Derivative,
) -> Option<(nils_pipeline::descriptor::Descriptor, String)> {
    let desc = descriptor_of_run(store, d.run_id?)?;
    let id = output_in(&desc, &d.path)?;
    Some((desc, id))
}

/// The descriptor of the pipeline a run ran.
pub(crate) fn descriptor_of_run(
    store: &mut Store,
    run: i64,
) -> Option<nils_pipeline::descriptor::Descriptor> {
    let run = nils_registry::pipeline::run(store, run).ok()??;
    let p = nils_registry::pipeline::get(store, run.pipeline_id).ok()??;
    nils_pipeline::descriptor::from_value(p.descriptor).ok()
}

/// Which declared output a path is: the first whose template, its words
/// read as any run of characters inside a segment, matches the path's end.
pub(crate) fn output_in(
    desc: &nils_pipeline::descriptor::Descriptor,
    path: &str,
) -> Option<String> {
    let segs: Vec<&str> = path.split('/').collect();
    for o in &desc.outputs {
        let mut t = String::new();
        let mut word = false;
        for c in o.template.chars() {
            match c {
                '{' => {
                    word = true;
                    t.push('*');
                }
                '}' => word = false,
                _ if word => {}
                _ => t.push(c),
            }
        }
        let pattern: Vec<&str> = t.split('/').collect();
        if segs.len() >= pattern.len()
            && pattern
                .iter()
                .zip(&segs[segs.len() - pattern.len()..])
                .all(|(p, s)| wildcard(p, s))
        {
            return Some(o.id.clone());
        }
    }
    None
}

/// Whether one path segment matches a pattern whose `*` is any run of
/// characters.
fn wildcard(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(at) => rest = &rest[at + mid.len()..],
            None => return false,
        }
    }
    true
}

/// The part sets a result is read as: its output's declared sets, or one
/// set, whole.
pub(crate) fn sets_of(store: &mut Store, d: &Derivative) -> Vec<crate::mask::SetSpec> {
    let declared: Vec<crate::mask::SetSpec> = output_of(store, d)
        .and_then(|(desc, id)| {
            desc.outputs
                .iter()
                .find(|o| o.id == id)
                .map(|o| o.parts.iter().map(crate::mask::SetSpec::of).collect())
        })
        .unwrap_or_default();
    if declared.is_empty() {
        vec![crate::mask::SetSpec::whole()]
    } else {
        declared
    }
}

/// The path a place declares its folder is reachable at by the clients
/// that share its volume (record 43 S3): `share` in its guarantees.
pub(crate) fn share_of(p: &Place) -> Option<String> {
    p.guarantees["share"]
        .as_str()
        .map(str::trim)
        .filter(|s| s.starts_with('/'))
        .map(|s| s.trim_end_matches('/').to_string())
}

/// A derivative as the doors and `--json` answer it: the row, the place
/// by name, the door its bytes are read from, and the transports it goes
/// by: the door always, and a shared path where its place declares one
/// (the path itself only at the content door, gated as the bytes are).
pub(crate) fn doc(store: &mut Store, d: &Derivative) -> Value {
    let place = place::show(store, d.place_id).ok().flatten();
    doc_in(
        d,
        place.as_ref().map(|p| p.name.clone()),
        place.as_ref().and_then(share_of).is_some(),
    )
}

/// [`doc`] with its place's name and share read already.
fn doc_in(d: &Derivative, place: Option<String>, shared: bool) -> Value {
    let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
    v["place"] = json!(place);
    v["content"] = json!(format!("/api/derivatives/{}/content", d.id));
    v["transports"] = if shared {
        json!(["door", "share"])
    } else {
        json!(["door"])
    };
    v
}

/// The file of a derivative on disk, or why it cannot be read.
pub(crate) fn file_of(store: &mut Store, d: &Derivative) -> Result<PathBuf, Refused> {
    let p = place::show(store, d.place_id)
        .map_err(|e| (500, e.to_string()))?
        .ok_or_else(|| (409, format!("the place of derivative {} is gone", d.id)))?;
    if p.retired_at.is_some() {
        return Err((
            409,
            format!(
                "derivative {} lives in {}, which is retired; its file is read after the place is brought back",
                d.id, p.name
            ),
        ));
    }
    let path = Path::new(&p.path).join(&d.path);
    match std::fs::metadata(&path) {
        Ok(m) if m.len() == d.bytes as u64 => Ok(path),
        Ok(m) => Err((
            409,
            format!(
                "derivative {} was registered at {} bytes and its file in {} is {}; it is not served",
                d.id,
                d.bytes,
                p.name,
                m.len()
            ),
        )),
        Err(_) => Err((
            404,
            format!("the file of derivative {} is not in {}", d.id, p.name),
        )),
    }
}

// ------------------------------------------------------------ sealed means sealed

/// The largest table the content door reads row by row to leave out the
/// rows of a sealed stack (record 48); a larger one is decided whole, by
/// what it was made from.
pub(crate) const SPLIT_MAX: i64 = 256 * 1024 * 1024;

/// Record 48, D1 of the move: what a derivative was made from, as the
/// registry tells it ([`made_of`]).
#[derive(Debug, Default)]
pub(crate) struct MadeFrom {
    /// The stacks it was made from.
    pub(crate) stacks: Vec<i64>,
    /// The subjects any of whose stacks it may have been made from, where
    /// the registry keeps no finer word.
    pub(crate) subjects: Vec<i64>,
    /// For a run's own file, each unit of the run by its name with the
    /// stacks it was given: a run's table names its rows by them.
    pub(crate) units: Vec<(String, Vec<i64>)>,
    /// False where what it was made from can no longer be told.
    pub(crate) known: bool,
}

/// The unit folder a run's file of a subject or a session lies under:
/// `sub-<s>`, and `ses-<t>` below it, as the bids units name their files.
fn unit_folder(path: &str) -> Option<(String, Option<String>)> {
    let segs: Vec<&str> = path.split('/').collect();
    let i = segs
        .iter()
        .position(|s| s.starts_with("sub-") && !s.contains('_'))?;
    let session = segs
        .get(i + 1)
        .filter(|s| s.starts_with("ses-"))
        .map(|s| s.to_string());
    Some((segs[i].to_string(), session))
}

/// Record 48, D1 of the move: what a derivative was made from, by its
/// manifest. A stack's file is of its stack, and a mask of the scan its
/// grid is on as well. A run's file of a subject or a session is of the
/// stacks the run gave that unit (a stack unit its stack, a subject's or a
/// session's the stacks the run's release placed in its folder), and,
/// where those are gone, of any stack of its subject; a run's own file is
/// of every unit's stacks, else of the list the run was frozen over. A file
/// registered at the door is of its stack, its series' stacks, or any
/// stack of its subject. Nothing is read of the run's folder, which its
/// container could write.
pub(crate) fn made_of(store: &mut Store, d: &Derivative) -> Result<MadeFrom, String> {
    let mut m = MadeFrom {
        known: true,
        ..MadeFrom::default()
    };
    m.stacks.extend(d.stack_id);
    m.stacks.extend(d.grid_stack_id);
    match (d.scope.as_str(), d.run_id) {
        ("stack" | "model", _) => {}
        ("series", _) => {
            if let Some(series) = d.series_id {
                let sql = format!(
                    "SELECT id FROM {} WHERE series_id = {}",
                    store.qualified("stack"),
                    store.dialect().param(1, nils_registry::schema::Type::Int)
                );
                for r in store
                    .query(&sql, &[nils_registry::Param::Int(series)])
                    .map_err(|e| e.to_string())?
                {
                    m.stacks.push(r.int(0).map_err(|e| e.to_string())?);
                }
            }
        }
        ("run", Some(run)) => {
            let units = crate::pipelines::unit_stacks(store, run)?;
            if units.is_empty() || units.iter().any(|(_, s)| s.is_empty()) {
                let frozen = crate::pipelines::frozen_stacks(store, run)?;
                m.known = !frozen.is_empty();
                m.stacks.extend(frozen);
            }
            for (_, s) in &units {
                m.stacks.extend(s.iter().copied());
            }
            m.units = units;
        }
        ("session" | "subject", Some(run)) => {
            let names: Vec<String> = match unit_folder(&d.path) {
                Some((subject, Some(session))) => vec![format!("{subject}_{session}"), subject],
                Some((subject, None)) => vec![subject],
                None => Vec::new(),
            };
            let units = if names.is_empty() {
                Vec::new()
            } else {
                crate::pipelines::unit_stacks(store, run)?
            };
            let given = names.iter().find_map(|n| {
                units
                    .iter()
                    .find(|(u, s)| u == n && !s.is_empty())
                    .map(|(_, s)| s.clone())
            });
            match given {
                Some(stacks) => m.stacks.extend(stacks),
                None => m.subjects.extend(d.subject_id),
            }
        }
        _ => m.subjects.extend(d.subject_id),
    }
    if m.stacks.is_empty() && m.subjects.is_empty() && d.scope != "model" {
        m.known = false;
    }
    m.stacks.sort_unstable();
    m.stacks.dedup();
    Ok(m)
}

/// What of a derivative's content is not shown to a caller who does not
/// read what a system said of a stack of a sample sealed now (record 48,
/// D1 of the move), as [`withheld`] finds it.
#[derive(Debug)]
pub(crate) enum Withheld {
    /// Nothing: the file as it was registered.
    Nothing,
    /// A table whose rows name their stacks, less the rows of a stack
    /// sealed now (and, where it was made from one, the rows that name no
    /// stack it can be told by): the bytes left, and how many rows went.
    Rows { bytes: Vec<u8>, rows: usize },
    /// The whole of it, and why.
    Whole(String),
}

/// The sentence that ends every refusal of a sealed stack's derivative.
const UNTIL_UNSEALED: &str = "what a system said of a stack of a sample sealed for certification is withheld until a certificate unseals it (record 48); a holder of sealed:see reads it, audited as sealed.read";

/// What of derivative `d` is withheld from a caller who does not read
/// sealed stacks. A stack's pictures (a pyramid) are the file, never an
/// answer, and a model's artifact is no stack's, so neither is withheld,
/// and nothing is while no sample is sealed. A stack's own file is all of
/// it said of its stack. A table whose rows name their stacks (a run's
/// table by its declared unit column, another by a column of stack ids)
/// loses the rows of the stacks sealed now; anything else made from one is
/// withheld whole, by what its manifest says it was made from.
pub(crate) fn withheld(store: &mut Store, d: &Derivative) -> Result<Withheld, Reply> {
    if matches!(d.kind.as_str(), "pyramid" | "model") || !crate::sealed::any(store)? {
        return Ok(Withheld::Nothing);
    }
    let made = made_of(store, d).map_err(|e| Reply::error(500, e))?;
    let (stacks, subjects) = crate::sealed::now_of(store, &made.stacks, &made.subjects)?;
    let sealed = !stacks.is_empty() || !subjects.is_empty() || !made.known;
    let own = d.scope == "stack" && d.stack_id.is_some_and(|s| stacks.contains(&s));
    if !own
        && d.kind == "table"
        && d.bytes <= SPLIT_MAX
        && let Some(split) = split_table(store, d, &made, sealed)?
    {
        return Ok(split);
    }
    if !sealed {
        return Ok(Withheld::Nothing);
    }
    let whole = match (d.kind.as_str(), own) {
        ("table", false) if d.bytes > SPLIT_MAX => {
            ", and it is too large to be read a row at a time, so it is withheld whole"
        }
        ("table", false) => {
            ", and its rows do not say which stack each is of, so it is withheld whole"
        }
        _ => "",
    };
    let why = if !made.known {
        format!(
            "what derivative {} was made from can no longer be told (its run keeps neither its units nor the list it was frozen over) while a sample is sealed, so it is withheld whole: {UNTIL_UNSEALED}",
            d.id
        )
    } else if stacks.is_empty() {
        format!(
            "derivative {} is of a subject with a stack of a sample sealed for certification, and the registry does not say which of their stacks it was made from{whole}: {UNTIL_UNSEALED}",
            d.id
        )
    } else {
        format!(
            "derivative {} was made from a stack of a sample sealed for certification{whole}: {UNTIL_UNSEALED}",
            d.id
        )
    };
    Ok(Withheld::Whole(why))
}

/// A table's format and the column that names each row's unit, as the run
/// that wrote it declared them (record 49 A3); else its format by its media
/// type or its name, with no unit column.
fn table_shape(store: &mut Store, d: &Derivative) -> Option<(String, Option<String>)> {
    if let Some((desc, id)) = output_of(store, d)
        && let Some(t) = desc
            .outputs
            .iter()
            .find(|o| o.id == id)
            .and_then(|o| o.table.as_ref())
    {
        return Some((t.format.clone(), t.unit_column.clone()));
    }
    let media = d.media_type.to_ascii_lowercase();
    let path = d.path.to_ascii_lowercase();
    let format = if media.contains("csv") || path.ends_with(".csv") {
        "csv"
    } else if media.contains("tab-separated") || path.ends_with(".tsv") {
        "tsv"
    } else if media.contains("json") || path.ends_with(".json") {
        "json"
    } else {
        return None;
    };
    Some((format.to_string(), None))
}

/// The column of `headers` a declared name is: the header written so, or
/// the one whose folded form is the name's, as a table is read.
fn column_named(headers: &[String], name: &str) -> Option<usize> {
    headers
        .iter()
        .position(|h| h.trim() == name.trim())
        .or_else(|| {
            let folded = nils_pipeline::descriptor::fold(name);
            headers
                .iter()
                .position(|h| nils_pipeline::descriptor::fold(h) == folded)
        })
}

/// A column of stack ids: `stack_id`, `stack` or `stackid`, folded.
fn stack_column(headers: &[String]) -> Option<usize> {
    headers.iter().position(|h| {
        matches!(
            nils_pipeline::descriptor::fold(h).as_str(),
            "stack_id" | "stack" | "stackid"
        )
    })
}

/// How a table's rows are written back, given which of them are kept.
type Rebuild = Box<dyn Fn(&[bool]) -> Vec<u8>>;

/// A table read for its rows: the text that names each row's stack (none
/// where its cell is empty), and how the rows kept are written back.
struct TableRows {
    keys: Vec<Option<String>>,
    /// One JSON object, which is one row: withholding it is withholding
    /// the whole.
    single: bool,
    rebuild: Rebuild,
}

/// The rows of a CSV or TSV table, each by its own bytes, so a row kept
/// is written back exactly as it was; none where it does not read, or no
/// column names its rows.
fn delimited_rows(bytes: &[u8], delimiter: u8, unit_column: Option<&str>) -> Option<TableRows> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(false)
        .from_reader(bytes);
    let mut record = csv::ByteRecord::new();
    let mut head_end: Option<usize> = None;
    let mut column = None;
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    let mut keys = Vec::new();
    // the reader stops between the CR and the LF of a CRLF: the LF is the
    // record's it ends, so a row left out takes its own line end with it
    let at = |p: u64| {
        let p = p as usize;
        if p > 0 && p < bytes.len() && bytes[p] == b'\n' && bytes[p - 1] == b'\r' {
            p + 1
        } else {
            p
        }
    };
    loop {
        let start = at(reader.position().byte());
        if !reader.read_byte_record(&mut record).ok()? {
            break;
        }
        let end = at(reader.position().byte());
        let Some(at) = column else {
            let names: Vec<String> = record
                .iter()
                .map(|f| String::from_utf8_lossy(f).trim().to_string())
                .collect();
            column = Some(match unit_column {
                Some(u) => column_named(&names, u)?,
                None => stack_column(&names)?,
            });
            head_end = Some(end);
            continue;
        };
        keys.push(
            record
                .get(at)
                .map(|f| String::from_utf8_lossy(f).trim().to_string())
                .filter(|t| !t.is_empty()),
        );
        spans.push(start..end);
    }
    let head_end = head_end?;
    let tail = spans.last().map_or(head_end, |s| s.end);
    let bytes = bytes.to_vec();
    Some(TableRows {
        keys,
        single: false,
        rebuild: Box::new(move |kept| {
            let mut out = bytes[..head_end].to_vec();
            for (span, keep) in spans.iter().zip(kept) {
                if *keep {
                    out.extend_from_slice(&bytes[span.clone()]);
                }
            }
            out.extend_from_slice(&bytes[tail..]);
            out
        }),
    })
}

/// The rows of a JSON table: a list of objects, each a row, written back
/// as a list, or one object, which is one row; none where it does not
/// read, or no row has a column that names it.
fn json_rows(bytes: &[u8], unit_column: Option<&str>) -> Option<TableRows> {
    let doc: Value = serde_json::from_slice(bytes).ok()?;
    let key_of = |o: &serde_json::Map<String, Value>| -> Option<Option<String>> {
        let names: Vec<String> = o.keys().cloned().collect();
        let at = match unit_column {
            Some(u) => column_named(&names, u),
            None => stack_column(&names),
        }?;
        Some(match &o[&names[at]] {
            Value::String(s) => Some(s.trim().to_string()).filter(|t| !t.is_empty()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };
    let pretty = bytes.contains(&b'\n');
    let newline = bytes.ends_with(b"\n");
    match doc {
        Value::Object(o) => {
            let key = key_of(&o)?;
            Some(TableRows {
                keys: vec![key],
                single: true,
                rebuild: Box::new(|_| Vec::new()),
            })
        }
        Value::Array(items) => {
            let mut keys = Vec::with_capacity(items.len());
            let mut named = items.is_empty();
            for item in &items {
                let key = item.as_object().and_then(key_of);
                named |= key.is_some();
                keys.push(key.flatten());
            }
            if !named {
                return None;
            }
            Some(TableRows {
                keys,
                single: false,
                rebuild: Box::new(move |kept| {
                    let left: Vec<&Value> = items
                        .iter()
                        .zip(kept)
                        .filter(|(_, k)| **k)
                        .map(|(v, _)| v)
                        .collect();
                    let mut out = if pretty {
                        serde_json::to_vec_pretty(&left)
                    } else {
                        serde_json::to_vec(&left)
                    }
                    .unwrap_or_default();
                    if newline {
                        out.push(b'\n');
                    }
                    out
                }),
            })
        }
        _ => None,
    }
}

/// The stack a cell of stack ids names: `12`, `12.0` or `stack-12`.
fn stack_named(text: &str) -> Option<i64> {
    let t = nils_pipeline::results::normalise(text);
    let id = t.strip_prefix("stack-").unwrap_or(&t);
    id.parse::<i64>().ok().or_else(|| {
        id.parse::<f64>()
            .ok()
            .filter(|f| f.is_finite() && f.fract() == 0.0)
            .map(|f| f as i64)
    })
}

/// A table split by its rows' stacks: the rows naming a stack sealed now
/// go, and, where the table was made from one (`sealed`), so do the rows
/// that name no stack it can be told by. None where its rows do not name
/// their stacks, which leaves it to be decided whole.
fn split_table(
    store: &mut Store,
    d: &Derivative,
    made: &MadeFrom,
    sealed: bool,
) -> Result<Option<Withheld>, Reply> {
    let Some((format, unit_column)) = table_shape(store, d) else {
        return Ok(None);
    };
    let file = file_of(store, d).map_err(|(status, m)| Reply::error(status, m))?;
    let bytes = std::fs::read(&file).map_err(|e| {
        Reply::error(
            500,
            format!("the file of derivative {} could not be read: {e}", d.id),
        )
    })?;
    let rows = match format.as_str() {
        "csv" => delimited_rows(&bytes, b',', unit_column.as_deref()),
        "tsv" => delimited_rows(&bytes, b'\t', unit_column.as_deref()),
        "json" => json_rows(&bytes, unit_column.as_deref()),
        _ => None,
    };
    let Some(rows) = rows else {
        return Ok(None);
    };
    // each row's stacks: its unit's, for a run's table, else its own
    let stacks_of: Vec<Option<Vec<i64>>> = rows
        .keys
        .iter()
        .map(|k| {
            let k = k.as_deref()?;
            if unit_column.is_some() {
                let name =
                    crate::measures::unit_name_of(k, made.units.iter().map(|(u, _)| u.as_str()))?;
                made.units
                    .iter()
                    .find(|(u, _)| u == name)
                    .map(|(_, s)| s.clone())
                    .filter(|s| !s.is_empty())
            } else {
                stack_named(k).map(|s| vec![s])
            }
        })
        .collect();
    let named: Vec<i64> = stacks_of.iter().flatten().flatten().copied().collect();
    let now = crate::sealed::now(store, &named)?;
    let kept: Vec<bool> = stacks_of
        .iter()
        .map(|s| match s {
            Some(stacks) => !stacks.iter().any(|s| now.contains(s)),
            None => !sealed,
        })
        .collect();
    let gone = kept.iter().filter(|k| !**k).count();
    if gone == 0 {
        return Ok(Some(Withheld::Nothing));
    }
    if rows.single {
        return Ok(Some(Withheld::Whole(format!(
            "derivative {} is one row, of a stack of a sample sealed for certification: {UNTIL_UNSEALED}",
            d.id
        ))));
    }
    Ok(Some(Withheld::Rows {
        bytes: (rows.rebuild)(&kept),
        rows: gone,
    }))
}

/// How long a result's parts and layer doors keep what [`withheld`] said of
/// it: a viewer asks for many tiles of one layer, and each would read the
/// run's units again.
const WITHHELD_KEPT: std::time::Duration = std::time::Duration::from_secs(5);

/// What [`withheld`] said of a result lately, by its id: when, and the
/// refusal where it is withheld whole.
type Kept = std::collections::HashMap<i64, (std::time::Instant, Option<String>)>;

static WITHHELD_LATELY: std::sync::LazyLock<std::sync::Mutex<Kept>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Kept::new()));

/// Why a result is withheld whole from a caller without sealed:see, or
/// none, as [`withheld`] says and keeps for [`WITHHELD_KEPT`]; a result is
/// no table, so it is withheld whole or not at all.
fn withheld_whole(store: &mut Store, d: &Derivative) -> Result<Option<String>, Reply> {
    {
        let kept = WITHHELD_LATELY.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, why)) = kept.get(&d.id)
            && at.elapsed() < WITHHELD_KEPT
        {
            return Ok(why.clone());
        }
    }
    let why = match withheld(store, d)? {
        Withheld::Whole(why) => Some(why),
        Withheld::Nothing | Withheld::Rows { .. } => None,
    };
    let mut kept = WITHHELD_LATELY.lock().unwrap_or_else(|e| e.into_inner());
    if kept.len() >= 4096 {
        kept.retain(|_, (at, _)| at.elapsed() < WITHHELD_KEPT);
    }
    kept.insert(d.id, (std::time::Instant::now(), why.clone()));
    Ok(why)
}

/// Record 48, D1 of the move: a derivative's row as a caller who does not
/// read sealed stacks reads it. Where its content is withheld whole, what a
/// system found in it and what a person checked of it go: each part set's
/// `count` and `note` (its key, words, kind and labels are its pipeline's,
/// declared before anything ran) and the check (`check`, `check_reasons`,
/// `check_decision_id`, `checked_at`), and `sealed` says so. The file's own
/// facts stay: what it is, of what, where, its bytes, digest and grid.
/// Answers whether the row was shaped.
pub(crate) fn shape_sealed(
    store: &mut Store,
    d: &Derivative,
    v: &mut Value,
) -> Result<bool, Reply> {
    if d.part_sets.is_null() && d.check_status.is_none() && d.check_decision_id.is_none() {
        return Ok(false);
    }
    if withheld_whole(store, d)?.is_none() {
        return Ok(false);
    }
    if let Some(sets) = v["part_sets"].as_array_mut() {
        for set in sets.iter_mut().filter_map(Value::as_object_mut) {
            set.remove("count");
            set.remove("note");
        }
    }
    for key in ["check", "check_reasons", "check_decision_id", "checked_at"] {
        v[key] = Value::Null;
    }
    v["sealed"] = json!(true);
    Ok(true)
}

/// [`shape_sealed`] at the keyboard, which withholds the same unless
/// `--unsealed-access` says why (or the caller that queued the job held
/// sealed:see).
fn keyboard_shape(store: &mut Store, d: &Derivative, v: &mut Value) -> Result<bool, crate::Exit> {
    if crate::sealed::keyboard_reads() {
        return Ok(false);
    }
    shape_sealed(store, d, v).map_err(|r| {
        crate::fail(
            r.body["error"]
                .as_str()
                .unwrap_or("the seal could not be read")
                .to_string(),
        )
    })
}

/// What a command at the keyboard says in place of what a result made from
/// a sealed stack holds.
const KEYBOARD_WITHHELD: &str = "what it holds is withheld, since it was made from a stack of a sample sealed for certification (record 48); pass --unsealed-access REASON to read it";

/// Whether the keyboard withholds what result `id` holds (its parts' counts,
/// its layer's voxels): one made from a stack of a sample sealed now, unless
/// `--unsealed-access` says why. The work is done all the same.
fn keyboard_withholds(store: &mut Store, id: i64) -> Result<bool, crate::Exit> {
    if crate::sealed::keyboard_reads() {
        return Ok(false);
    }
    let Some(d) = derivative::get(store, id).map_err(|e| crate::fail(e.to_string()))? else {
        return Ok(false);
    };
    withheld_whole(store, &d).map(|w| w.is_some()).map_err(|r| {
        crate::fail(
            r.body["error"]
                .as_str()
                .unwrap_or("the seal could not be read")
                .to_string(),
        )
    })
}

/// Record 48, D1 of the move: the `sealed.read` row of a caller holding
/// sealed:see whom a door showed what it withholds from everyone else.
fn audit_sealed(
    registry: &mut Registry,
    caller: &Caller,
    door: &str,
    d: &Derivative,
    held: &Withheld,
) -> Result<(), Reply> {
    let (what, rows) = match held {
        Withheld::Nothing => return Ok(()),
        Withheld::Rows { rows, .. } => ("rows", Some(*rows)),
        Withheld::Whole(_) => ("whole", None),
    };
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &caller.principal,
            action: nils_registry::audit::Action::SealedRead,
            scope: json!({
                "door": door, "derivative": d.id, "kind": d.kind, "withheld": what,
                "rows": rows,
            }),
            policy: None,
            job_id: None,
            details: Some(json!({"grant": crate::grants::UNSEALED})),
        },
    )
    .map(|_| ())
    .map_err(|e| Reply::error(500, e.to_string()))
}

fn int_of(
    query: &std::collections::HashMap<String, String>,
    key: &str,
) -> Result<Option<i64>, Reply> {
    query
        .get(key)
        .map(|v| {
            v.parse::<i64>()
                .map_err(|_| Reply::error(400, format!("{key} is a number")))
        })
        .transpose()
}

/// `POST /api/derivatives`: the body is the file, and the query says what
/// it is: `kind`, one of `stack`, `series` or `subject` (with `day` for one
/// occasion), `sha256`, and optionally `supersedes`, `model` and `name`, whose
/// extension alone is kept. The media type is the request's content type.
/// The bytes are read only once the caller, the place and the ask pass.
pub(crate) fn upload(
    registry: &mut Registry,
    caller: &Caller,
    query: &std::collections::HashMap<String, String>,
    request: &mut tiny_http::Request,
) -> Result<Reply, Reply> {
    let (need, detail) = crate::serve::door("POST", &["api", "derivatives"]);
    caller.allowed("/api/derivatives", need, detail)?;
    // the router decoded every query value (record 45)
    let q = query.clone();
    let header = |name: &str| -> Option<String> {
        request
            .headers()
            .iter()
            .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str().to_string())
    };
    let media_type = header("Content-Type")
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    if let Some(n) = header("Content-Length").and_then(|n| n.trim().parse::<u64>().ok())
        && n > UPLOAD_MAX
    {
        return Err(Reply::error(
            413,
            format!("a derivative is at most {UPLOAD_MAX} bytes"),
        ));
    }
    let kind = q
        .get("kind")
        .cloned()
        .ok_or_else(|| Reply::error(400, "kind names what the file is"))?;
    let sha256 = q.get("sha256").cloned().ok_or_else(|| {
        Reply::error(
            400,
            "sha256 names the digest of the body, which the engine checks before it registers anything",
        )
    })?;
    let ask = Ask {
        kind: &kind,
        stack: int_of(&q, "stack")?,
        series: int_of(&q, "series")?,
        subject: int_of(&q, "subject")?,
        day: q.get("day").map(String::as_str),
        media_type: &media_type,
        supersedes: int_of(&q, "supersedes")?,
        model: q.get("model").map(String::as_str),
        name: q.get("name").map(String::as_str),
        sha256: Some(&sha256),
        principal: &caller.principal,
        actor: Some(&caller.actor),
        grid_stack: int_of(&q, "grid_stack")?,
    };
    let d = register(registry, &ask, request.as_reader())
        .map_err(|(status, m)| Reply::error(status, m))?;
    let mut v = doc(registry.store(), &d);
    if !crate::sealed::reads(&caller.access) {
        shape_sealed(registry.store(), &d, &mut v)?;
    }
    Ok(Reply::created(v))
}

/// The reading doors: the listing, one derivative, and its bytes.
pub(crate) fn route(
    registry: &mut Registry,
    caller: &Caller,
    get: bool,
    segs: &[&str],
    query: &std::collections::HashMap<String, String>,
) -> Option<Result<Reply, Reply>> {
    if !get {
        return None;
    }
    let id_of = |s: &str| -> Result<i64, Reply> {
        s.parse::<i64>()
            .map_err(|_| Reply::error(404, "a derivative is named by its id"))
    };
    let one = |registry: &mut Registry, id: i64| -> Result<Derivative, Reply> {
        derivative::get(registry.store(), id)
            .map_err(|e| Reply::error(500, e.to_string()))?
            .ok_or_else(|| Reply::error(404, format!("no derivative {id}")))
    };
    Some(match segs {
        ["api", "derivatives"] => (|| {
            let kind = query.get("kind").cloned();
            let check = query.get("check").cloned();
            if let Some(c) = &check
                && c != "unchecked"
                && !derivative::CHECKS.contains(&c.as_str())
            {
                return Err(Reply::error(
                    400,
                    format!("check is accepted, rejected, flagged or unchecked, not {c}"),
                ));
            }
            let run_id = int_of(query, "run")?;
            let output = query.get("output").cloned();
            if output.is_some() && run_id.is_none() {
                return Err(Reply::error(
                    400,
                    "output names a declared output of a run's pipeline; give the run beside it",
                ));
            }
            let limit = query
                .get("limit")
                .and_then(|l| l.parse().ok())
                .unwrap_or(50);
            let mut rows = derivative::list(
                registry.store(),
                &derivative::Filter {
                    kind: kind.as_deref(),
                    stack_id: int_of(query, "stack")?,
                    subject_id: int_of(query, "subject")?,
                    run_id,
                    check: check.as_deref(),
                    // a run's files are read whole to find one output's
                    limit: if output.is_some() { 1_000_000 } else { limit },
                },
            )
            .map_err(|e| Reply::error(500, e.to_string()))?;
            if let Some(want) = &output {
                let desc = run_id.and_then(|r| descriptor_of_run(registry.store(), r));
                rows.retain(|d| {
                    desc.as_ref()
                        .and_then(|desc| output_in(desc, &d.path))
                        .is_some_and(|o| &o == want)
                });
                rows.truncate(limit.max(1));
            }
            let store = registry.store();
            // the places the rows live in, each read once
            let mut places: std::collections::BTreeMap<i64, Option<(String, bool)>> =
                std::collections::BTreeMap::new();
            for d in &rows {
                if let std::collections::btree_map::Entry::Vacant(e) = places.entry(d.place_id) {
                    e.insert(
                        place::show(store, d.place_id)
                            .ok()
                            .flatten()
                            .map(|p| (p.name.clone(), share_of(&p).is_some())),
                    );
                }
            }
            let mut docs: Vec<Value> = rows
                .iter()
                .map(|d| {
                    let (name, shared) = places
                        .get(&d.place_id)
                        .cloned()
                        .flatten()
                        .map_or((None, false), |(n, s)| (Some(n), s));
                    doc_in(d, name, shared)
                })
                .collect();
            // record 48, D1 of the move: a result made from a sealed stack
            // keeps its file's facts and loses its parts' counts and its
            // check; nor is it listed by a check's status, which is a
            // person's word on what a system said of the stack
            if !crate::sealed::reads(&caller.access) {
                let mut shaped = Vec::with_capacity(docs.len());
                for (d, v) in rows.iter().zip(docs.iter_mut()) {
                    shaped.push(shape_sealed(store, d, v)?);
                }
                if check.is_some() {
                    let mut i = 0;
                    docs.retain(|_| {
                        i += 1;
                        !shaped[i - 1]
                    });
                }
            }
            Ok(Reply::ok(json!({
                "derivatives": docs,
                "capability": capability(store),
            })))
        })(),
        ["api", "derivatives", id] => (|| {
            let d = one(registry, id_of(id)?)?;
            let mut v = doc(registry.store(), &d);
            if !crate::sealed::reads(&caller.access) {
                shape_sealed(registry.store(), &d, &mut v)?;
            }
            Ok(Reply::ok(v))
        })(),
        ["api", "derivatives", id, "content"] => (|| {
            let d = one(registry, id_of(id)?)?;
            let path =
                file_of(registry.store(), &d).map_err(|(status, m)| Reply::error(status, m))?;
            // record 48, D1 of the move: what a system said of a stack of a
            // sample sealed now is withheld from every caller but a holder
            // of sealed:see, whose read of it is audited as sealed.read
            let reads = crate::sealed::reads(&caller.access);
            let held = withheld(registry.store(), &d)?;
            if !reads && let Withheld::Whole(why) = &held {
                let mut r = Reply::error(403, why.clone());
                r.body["sealed"] = json!(true);
                return Err(r);
            }
            let door = "GET /api/derivatives/{id}/content";
            // record 43 S3: the second transport, a path on a volume the
            // caller shares with the engine, gated and audited as the bytes
            match query.get("transport").map(String::as_str) {
                None | Some("door") => {}
                Some("share") => {
                    // a shared path reads the whole file, so a table whose
                    // rows of a sealed stack are left out comes by the door
                    if !reads && let Withheld::Rows { rows, .. } = &held {
                        let mut r = Reply::error(
                            403,
                            format!(
                                "{rows} row(s) of derivative {} name a stack of a sample sealed for certification and are withheld, and a shared path reads the whole file; the rest of it comes by the door (transport=door): {UNTIL_UNSEALED}",
                                d.id
                            ),
                        );
                        r.body["sealed"] = json!(true);
                        r.body["withheld_sealed"] = json!(rows);
                        return Err(r);
                    }
                    let p = place::show(registry.store(), d.place_id)
                        .map_err(|e| Reply::error(500, e.to_string()))?
                        .ok_or_else(|| {
                            Reply::error(409, format!("the place of derivative {} is gone", d.id))
                        })?;
                    let Some(share) = share_of(&p) else {
                        return Err(Reply::error(
                            409,
                            format!(
                                "the place {} declares no share path, so derivative {} comes through this door alone; an operator declares one with nils place set --share <path>",
                                p.name, d.id
                            ),
                        ));
                    };
                    let shared = format!("{share}/{}", d.path);
                    nils_registry::audit::record(
                        registry,
                        &nils_registry::audit::Entry {
                            principal: &caller.principal,
                            action: nils_registry::audit::Action::DerivativeRead,
                            scope: json!({
                                "derivative": d.id, "kind": d.kind, "stack": d.stack_id,
                                "subject": d.subject_id, "bytes": d.bytes,
                            }),
                            policy: None,
                            job_id: None,
                            details: Some(json!({"transport": "share", "place": p.name})),
                        },
                    )
                    .map_err(|e| Reply::error(500, e.to_string()))?;
                    if reads {
                        audit_sealed(registry, caller, door, &d, &held)?;
                    }
                    return Ok(Reply::ok(json!({
                        "derivative": d.id, "transport": "share", "path": shared,
                        "place": p.name, "bytes": d.bytes, "sha256": d.sha256,
                        "media_type": d.media_type,
                    })));
                }
                Some(other) => {
                    return Err(Reply::error(
                        400,
                        format!("transport is door or share, not {other}"),
                    ));
                }
            }
            let left_out = match &held {
                Withheld::Rows { rows, .. } if !reads => Some(*rows),
                _ => None,
            };
            nils_registry::audit::record(
                registry,
                &nils_registry::audit::Entry {
                    principal: &caller.principal,
                    action: nils_registry::audit::Action::DerivativeRead,
                    scope: json!({
                        "derivative": d.id, "kind": d.kind, "stack": d.stack_id,
                        "subject": d.subject_id, "bytes": d.bytes,
                    }),
                    policy: None,
                    job_id: None,
                    details: left_out.map(|n| json!({"withheld_sealed": n})),
                },
            )
            .map_err(|e| Reply::error(500, e.to_string()))?;
            if reads {
                audit_sealed(registry, caller, door, &d, &held)?;
            }
            match held {
                // a table less its rows of a sealed stack: the bytes served,
                // their own digest, and how many rows went, never which
                Withheld::Rows { bytes, rows } if !reads => {
                    let sha =
                        hex::encode(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
                    Ok(Reply::raw(
                        &d.media_type,
                        bytes,
                        vec![
                            ("X-Nils-Sha256".to_string(), sha),
                            ("X-Nils-Derivative".to_string(), d.id.to_string()),
                            ("X-Nils-Withheld-Sealed".to_string(), rows.to_string()),
                            ("Cache-Control".to_string(), "private, no-store".to_string()),
                        ],
                    ))
                }
                _ => Ok(Reply::file(
                    &d.media_type,
                    path,
                    vec![
                        ("X-Nils-Sha256".to_string(), d.sha256.clone()),
                        ("X-Nils-Derivative".to_string(), d.id.to_string()),
                        ("Cache-Control".to_string(), "private, no-store".to_string()),
                    ],
                )),
            }
        })(),
        // record 55 H3, job type 3
        ["api", "derivatives", id, "parts"] => (|| {
            let (d, _) = reader_of(registry, caller, id_of(id)?, None)?;
            parts_door(registry, caller, &d, query)
        })(),
        ["api", "derivatives", id, "layers", stack, set, rest @ ..] => (|| {
            let stack: i64 = stack
                .parse()
                .map_err(|_| Reply::error(404, "a stack is named by its id"))?;
            let (d, through) = reader_of(registry, caller, id_of(id)?, Some(stack))?;
            layer_door(registry, caller, &d, through, stack, set, rest, query)
        })(),
        _ => return None,
    })
}

// ------------------------------------------------------------ parts and layers at the door

/// Who reads a result's parts or layer: `pipelines:see`, or a rater of an
/// open campaign that holds it (and, for a layer, shows the scan). Answers
/// the result and the campaign a rater reads through. The grant is checked
/// first: to a caller without pipelines:see a result that is not there,
/// one that is not a mask and one they do not rate are one answer, 404, so
/// the door says nothing of what it holds (the review of E1, item 16).
/// Record 48, D1 of the move: a result made from a stack of a sample sealed
/// now is what a system said of it, and pipelines:see alone does not read
/// it (403, saying why); sealed:see does, and so does a rater of a campaign
/// that holds it, which reads its results as a check is made to.
fn reader_of(
    registry: &mut Registry,
    caller: &Caller,
    id: i64,
    stack: Option<i64>,
) -> Result<(Derivative, Option<i64>), Reply> {
    let found =
        derivative::get(registry.store(), id).map_err(|e| Reply::error(500, e.to_string()))?;
    if caller.access.holds("pipelines:see") {
        let d = found.ok_or_else(|| Reply::error(404, format!("no derivative {id}")))?;
        if d.kind != "mask" {
            return Err(Reply::error(
                404,
                format!(
                    "derivative {} is {}; only a mask has parts and layers",
                    d.id,
                    nils_registry::a_or_an(&d.kind)
                ),
            ));
        }
        if !crate::sealed::reads(&caller.access)
            && let Some(why) = withheld_whole(registry.store(), &d)?
        {
            return match crate::campaigns::results_through(registry.store(), caller, d.id, stack)? {
                Some(c) => Ok((d, Some(c))),
                None => {
                    let mut r = Reply::error(403, why);
                    r.body["sealed"] = json!(true);
                    Err(r)
                }
            };
        }
        return Ok((d, None));
    }
    let not_yours = || {
        Reply::error(
            404,
            format!(
                "no result {id} that {} reads: a result opens with pipelines:see, or to a rater of an open campaign that holds it",
                caller.principal
            ),
        )
    };
    let Some(d) = found.filter(|d| d.kind == "mask") else {
        return Err(not_yours());
    };
    match crate::campaigns::results_through(registry.store(), caller, d.id, stack)? {
        Some(c) => Ok((d, Some(c))),
        None => Err(not_yours()),
    }
}

/// The job a door queued for a result, while it is not over, and when the
/// door first saw it: many requests at once queue one job.
static QUEUED: std::sync::LazyLock<std::sync::Mutex<Queued>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(Queued::new()));

/// A queued command line, by its words: its job and when it was first seen.
type Queued = std::collections::BTreeMap<Vec<String>, (i64, std::time::Instant)>;

/// Answer 202 with the job that makes what a door was asked for, queued
/// now or queued or running already; 422 once that job failed, which the
/// door does not queue again.
pub(crate) fn made_by_job(
    store: &mut Store,
    caller: &Caller,
    command: Vec<String>,
    name: &str,
    what: &str,
) -> Result<Reply, Reply> {
    use nils_registry::job::{self, State};
    let jerr = |e: job::Error| Reply::error(500, e.to_string());
    let mut queued = QUEUED.lock().unwrap_or_else(|e| e.into_inner());
    let since = queued
        .get(&command)
        .map_or_else(std::time::Instant::now, |(_, at)| *at);
    // a job of the command still open answers, whoever queued it; else the
    // one this door queued last, which, failed, stays failed until a person
    // queues it again at the jobs door, so a door never alternates between
    // queueing it and saying it failed (the review of E1, item 2)
    let open = job::list(store, false, 100)
        .map_err(jerr)?
        .into_iter()
        .find(|j| j.queued().as_ref() == Some(&command));
    let found = match open {
        Some(j) => Some(j),
        None => match queued.get(&command) {
            Some(&(id, _)) => job::show(store, id).map_err(jerr)?,
            None => None,
        },
    };
    let answer = |j: &job::Job| {
        let ms = crate::pyramid::retry_after_ms(since.elapsed());
        let mut r = Reply::accepted(json!({
            "building": true, "job": j.id, "state": j.state.name(), "progress": j.progress,
            "retry_after": crate::pyramid::RETRY_AFTER_SECS, "retry_after_ms": ms,
            "message": format!("{what} is being made; ask again in {ms} milliseconds"),
        }));
        r.headers.push((
            "Retry-After".to_string(),
            crate::pyramid::RETRY_AFTER_SECS.to_string(),
        ));
        r
    };
    match &found {
        Some(j) if matches!(j.state, State::Queued | State::Running | State::Cancelling) => {
            queued.insert(command, (j.id, since));
            return Ok(answer(j));
        }
        Some(j) if j.state == State::Failed => {
            let mut r = Reply::error(
                422,
                format!(
                    "{what} could not be made; job {} says why, and {} at the jobs door tries again",
                    j.id,
                    command.join(" ")
                ),
            );
            r.body["building"] = json!(false);
            r.body["job"] = json!(j.id);
            return Err(r);
        }
        _ => {}
    }
    let id = job::enqueue_with(
        store,
        &command,
        Some(name),
        Some(caller.principal.as_str()),
        crate::serve::queued_by(caller),
    )
    .map_err(jerr)?;
    crate::worker::wake();
    let j = job::show(store, id)
        .map_err(jerr)?
        .ok_or_else(|| Reply::error(500, format!("job {id} was queued and is not there")))?;
    queued.insert(command, (id, std::time::Instant::now()));
    Ok(answer(&j))
}

/// The part sets a result is read as, or the job that reads them.
fn sets_or_job(
    store: &mut Store,
    caller: &Caller,
    d: &Derivative,
) -> Result<Result<Vec<Value>, Reply>, Reply> {
    match d.part_sets.as_array() {
        Some(sets) if d.grid_link.is_some() => Ok(Ok(sets.clone())),
        _ => Ok(Err(made_by_job(
            store,
            caller,
            vec!["derivative".into(), "parts".into(), d.id.to_string()],
            &format!("result {}", d.id),
            &format!("the parts of result {}", d.id),
        )?)),
    }
}

/// `GET /api/derivatives/{id}/parts`: one page of a set's parts.
fn parts_door(
    registry: &mut Registry,
    caller: &Caller,
    d: &Derivative,
    query: &std::collections::HashMap<String, String>,
) -> Result<Reply, Reply> {
    let sets = match sets_or_job(registry.store(), caller, d)? {
        Ok(s) => s,
        Err(queued) => return Ok(queued),
    };
    let set = match query.get("set") {
        Some(k) => sets
            .iter()
            .find(|s| s["key"] == k.as_str())
            .ok_or_else(|| {
                Reply::error(
                    404,
                    format!(
                        "result {} is read as {}; it has no set {k}",
                        d.id,
                        sets.iter()
                            .filter_map(|s| s["key"].as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?,
        None => sets
            .first()
            .ok_or_else(|| Reply::error(409, format!("result {} has no part set", d.id)))?,
    };
    let key = set["key"].as_str().unwrap_or_default().to_string();
    let order = match query.get("order").map(String::as_str) {
        None | Some("position") => derivative::PartOrder::Position,
        Some("size") => derivative::PartOrder::Size,
        Some(other) => {
            return Err(Reply::error(
                400,
                format!("order is position or size, not {other}"),
            ));
        }
    };
    let number = |k: &str, default: usize| -> Result<usize, Reply> {
        query.get(k).map_or(Ok(default), |v| {
            v.parse::<usize>()
                .map_err(|_| Reply::error(400, format!("{k} is a whole number")))
        })
    };
    let page = number("page", 0)?;
    let per_page = number("per_page", PARTS_PAGE_MAX)?.clamp(1, PARTS_PAGE_MAX);
    let (count, parts) = derivative::parts(registry.store(), d.id, &key, order, page, per_page)
        .map_err(|e| Reply::error(500, e.to_string()))?;
    let mut doc = json!({
        "derivative": d.id, "set": key, "words": set["words"], "kind": set["kind"],
        "count": count, "page": page, "per_page": per_page,
        "order": if order == derivative::PartOrder::Size { "size" } else { "position" },
        "parts": parts,
    });
    if let Some(note) = set["note"].as_str() {
        doc["note"] = json!(note);
    }
    Ok(Reply::ok(doc))
}

/// Whether two stacks are of one scanner frame: their series name the same
/// frame of reference.
pub(crate) fn same_frame(store: &mut Store, a: i64, b: i64) -> Result<bool, Reply> {
    if a == b {
        return Ok(true);
    }
    let d = store.dialect();
    let sql = format!(
        "SELECT se.frame_of_reference_uid FROM {} st JOIN {} se ON se.id = st.series_id WHERE st.id = {}",
        store.qualified("stack"),
        store.qualified("series"),
        d.param(1, nils_registry::schema::Type::Int)
    );
    let mut frame = |s: i64| -> Result<Option<String>, Reply> {
        Ok(store
            .query_opt(&sql, &[nils_registry::Param::Int(s)])?
            .and_then(|r| r.opt_text(0).ok().flatten().map(str::to_string))
            .filter(|f| !f.trim().is_empty()))
    };
    let (fa, fb) = (frame(a)?, frame(b)?);
    Ok(fa.is_some() && fa == fb)
}

/// `GET /api/derivatives/{id}/layers/{stack}/{set}/...`: a result's layer
/// on a scan, as the instance doors answer a picture: `manifest`,
/// `tiles/{level}/{z}` and `slab/{level}/{z0}-{z1}`. A layer not built yet
/// answers 202 with the job that builds it (and the scan's picture first,
/// where it is not built), and the manifest is held while it builds.
#[allow(clippy::too_many_arguments)]
fn layer_door(
    registry: &mut Registry,
    caller: &Caller,
    d: &Derivative,
    through: Option<i64>,
    stack: i64,
    set: &str,
    rest: &[&str],
    query: &std::collections::HashMap<String, String>,
) -> Result<Reply, Reply> {
    let _ = query;
    if !matches!(rest.first().copied(), Some("manifest" | "tiles" | "slab")) {
        return Err(Reply::error(
            404,
            format!(
                "GET /api/derivatives/{}/layers/{stack}/{set}/{} is not a door; manifest, tiles/{{level}}/{{z}} and slab/{{level}}/{{z0}}-{{z1}} are",
                d.id,
                rest.join("/")
            ),
        ));
    }
    let store = registry.store();
    let sets = match sets_or_job(store, caller, d)? {
        Ok(s) => s,
        Err(queued) => return Ok(queued),
    };
    if !sets.iter().any(|s| s["key"] == set) {
        return Err(Reply::error(
            404,
            format!(
                "result {} is read as {}; it has no set {set}",
                d.id,
                sets.iter()
                    .filter_map(|s| s["key"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    let Some(grid_stack) = d
        .grid_stack_id
        .filter(|_| d.grid_link.as_deref() != Some("none"))
    else {
        return Err(Reply::error(
            409,
            format!(
                "result {} cannot be drawn: no scan is known whose grid it is on",
                d.id
            ),
        ));
    };
    if !same_frame(store, grid_stack, stack)? {
        return Err(Reply::error(
            409,
            format!(
                "result {} is on the grid of stack {grid_stack}, and stack {stack} is not in its scanner's frame, so it is not drawn there",
                d.id
            ),
        ));
    }
    let working = crate::pyramid::working_place_cached(store).map_err(|m| Reply::error(409, m))?;
    let pictures = crate::pyramid::dir(Path::new(&working.path), stack);
    let Some(scan) =
        crate::pyramid::manifest_cached(&pictures).map_err(|e| Reply::error(500, e))?
    else {
        // the picture first, as the instance doors build it
        return crate::pyramid::on_demand(store, caller, stack, &working);
    };
    crate::mask::scan_lattice(&scan).map_err(|m| Reply::error(409, m))?;
    let root = crate::mask::layer_dir(Path::new(&working.path), stack, &d.sha256, set);
    let layer = match crate::mask::layer_manifest(&root, &scan) {
        Some(m) => m,
        None => {
            let queued = made_by_job(
                store,
                caller,
                vec![
                    "derivative".into(),
                    "layer".into(),
                    d.id.to_string(),
                    "--stack".into(),
                    stack.to_string(),
                ],
                &format!("result {} on stack {stack}", d.id),
                &format!("the layer of result {} on stack {stack}", d.id),
            )?;
            if rest != ["manifest"] || queued.status != 202 {
                return Ok(queued);
            }
            let Some(_slot) = crate::preview::hold() else {
                return Ok(queued);
            };
            let until = std::time::Instant::now() + crate::pyramid::MANIFEST_HOLD;
            let mut built = None;
            while built.is_none() && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(40));
                built = crate::mask::layer_manifest(&root, &scan);
            }
            match built {
                Some(m) => m,
                None => return Ok(queued),
            }
        }
    };
    let level_of = |s: &str| -> Result<u32, Reply> {
        let l: u32 = s
            .parse()
            .map_err(|_| Reply::error(400, "a level is a number from 0"))?;
        if l >= layer.levels {
            return Err(Reply::error(
                404,
                format!("level {l} is not in the layer; it has {}", layer.levels),
            ));
        }
        Ok(l)
    };
    let headers = |planes: u32| {
        vec![
            ("X-Nils-Codec".to_string(), layer.codec.clone()),
            ("X-Nils-Stack".to_string(), stack.to_string()),
            ("X-Nils-Derivative".to_string(), d.id.to_string()),
            ("X-Nils-Planes".to_string(), planes.to_string()),
            (
                "Cache-Control".to_string(),
                "private, max-age=3600".to_string(),
            ),
        ]
    };
    let open = |registry: &mut Registry, level: u32, purpose: &str| {
        crate::pyramid::note_open_of(registry, caller, stack, through, level, purpose, Some(d.id))
            .map_err(|e| Reply::error(500, e))
    };
    match rest {
        ["manifest"] => {
            open(registry, 0, "layer")?;
            let mut doc =
                serde_json::to_value(&layer).map_err(|e| Reply::error(500, e.to_string()))?;
            doc["place"] = json!(working.name);
            doc["grid_stack"] = json!(grid_stack);
            let mut r = Reply::ok(doc);
            r.headers.push((
                "Cache-Control".to_string(),
                "private, max-age=300".to_string(),
            ));
            Ok(r)
        }
        ["tiles", level, z] => {
            let level = level_of(level)?;
            let z: u32 = z
                .parse()
                .map_err(|_| Reply::error(400, "a plane is a number from 0"))?;
            open(registry, level, "layer")?;
            let bytes = crate::pyramid::plane_container_in(&root, &layer.level_shapes, level, z)
                .map_err(crate::pyramid::unread_reply)?;
            Ok(Reply::raw(crate::pyramid::CONTENT_TYPE, bytes, headers(1)))
        }
        ["slab", level, range] => {
            let level = level_of(level)?;
            let (z0, z1) = range
                .split_once('-')
                .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
                .ok_or_else(|| Reply::error(400, "a slab is z0-z1, the end exclusive"))?;
            if z1 <= z0 {
                return Err(Reply::error(400, "a slab is z0-z1 with z1 past z0"));
            }
            if z1 - z0 > crate::pyramid::SLAB_MAX {
                return Err(Reply::error(
                    416,
                    format!(
                        "a slab is at most {} planes; {} asked",
                        crate::pyramid::SLAB_MAX,
                        z1 - z0
                    ),
                ));
            }
            let nz = layer.level_shapes[level as usize].shape[0];
            if z1 > nz {
                return Err(Reply::error(
                    416,
                    format!("the stack has {nz} planes at level {level}"),
                ));
            }
            open(registry, level, "layer")?;
            let bytes = crate::pyramid::slab_cached_in(&root, &layer.level_shapes, level, z0, z1)?;
            Ok(Reply::raw(
                crate::pyramid::CONTENT_TYPE,
                bytes.as_ref().clone(),
                headers(z1 - z0),
            ))
        }
        _ => Err(Reply::error(
            404,
            format!(
                "GET /api/derivatives/{}/layers/{stack}/{set}/{} is not a door",
                d.id,
                rest.join("/")
            ),
        )),
    }
}

// ------------------------------------------------------------ command line

/// `nils derivative`: register, list and show derivatives.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum DerivativeCommand {
    /// Copy a file into the working place under its digest and register it
    Add {
        /// The file
        file: PathBuf,
        /// What it is: mask, embedding, pyramid or output
        #[arg(long, value_name = "KIND")]
        kind: String,
        /// The stack it belongs to
        #[arg(long, value_name = "ID")]
        stack: Option<i64>,
        /// The series it belongs to
        #[arg(long, value_name = "ID")]
        series: Option<i64>,
        /// The subject it belongs to, by id
        #[arg(long, value_name = "ID")]
        subject: Option<i64>,
        /// With --subject: the day of the occasion it belongs to
        #[arg(long, value_name = "YYYY-MM-DD", requires = "subject")]
        day: Option<String>,
        /// Its media type
        #[arg(
            long = "media-type",
            value_name = "TYPE",
            default_value = "application/octet-stream"
        )]
        media_type: String,
        /// The derivative this one replaces; both stay
        #[arg(long, value_name = "ID")]
        supersedes: Option<i64>,
        /// The registered model that made it: its id, digest or name@version
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
        /// The digest the file must have, checked before anything is registered
        #[arg(long, value_name = "HEX")]
        sha256: Option<String>,
        /// For a mask: the scan whose grid it is on (record 55 H3)
        #[arg(long = "grid-stack", value_name = "ID")]
        grid_stack: Option<i64>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The derivatives, newest first
    List {
        /// Only this kind
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
        /// Only this stack's
        #[arg(long, value_name = "ID")]
        stack: Option<i64>,
        /// Only this subject's, by id
        #[arg(long, value_name = "ID")]
        subject: Option<i64>,
        /// Only the files this pipeline run made
        #[arg(long, value_name = "RUN")]
        run: Option<i64>,
        /// Only the results a check left so: accepted, rejected, flagged or unchecked (record 55 H3)
        #[arg(long, value_name = "STATUS")]
        check: Option<String>,
        /// At most this many
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// One derivative: what it is, what it belongs to, where it lives and who made it
    Show {
        /// The derivative's id
        id: i64,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Read a mask's grid from its header, tie it to the scan whose grid it is on, and make its parts (lesions, structures) in its own grid (record 55 H3)
    Parts {
        /// The mask's id
        #[arg(required_unless_present = "run", conflicts_with = "run")]
        id: Option<i64>,
        /// Every mask this pipeline run made
        #[arg(long, value_name = "RUN")]
        run: Option<i64>,
        /// Read again a mask whose parts are made
        #[arg(long)]
        force: bool,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Build a mask's layer on a scan's picture: a label pyramid of the picture's shape and levels, the mask resampled onto the scan by world position (record 55 H3); with --campaign, every result's layer on every panel of a check, the pictures first
    Layer {
        /// The mask's id
        #[arg(required_unless_present = "campaign", conflicts_with = "campaign")]
        id: Option<i64>,
        /// The scan it is drawn on
        #[arg(
            long,
            value_name = "ID",
            required_unless_present = "campaign",
            conflicts_with = "campaign"
        )]
        stack: Option<i64>,
        /// Every result's layer on every panel of this check campaign, as making the check queues it
        #[arg(long, value_name = "CAMPAIGN")]
        campaign: Option<String>,
        /// Build it again where it is built
        #[arg(long)]
        force: bool,
        /// Threads that build its planes
        #[arg(long, default_value_t = 4)]
        workers: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn command(
    home: &nils_registry::home::Home,
    command: DerivativeCommand,
) -> Result<(), crate::Exit> {
    let mut registry = crate::open(home)?;
    let print = |v: &Value| -> Result<(), crate::Exit> {
        println!(
            "{}",
            serde_json::to_string_pretty(v)
                .map_err(|e| crate::fail(format!("will not serialize: {e}")))?
        );
        Ok(())
    };
    match command {
        DerivativeCommand::Add {
            file,
            kind,
            stack,
            series,
            subject,
            day,
            media_type,
            supersedes,
            model,
            sha256,
            grid_stack,
            json,
        } => {
            let who = crate::actor();
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned());
            let ask = Ask {
                kind: &kind,
                stack,
                series,
                subject,
                day: day.as_deref(),
                media_type: &media_type,
                supersedes,
                model: model.as_deref(),
                name: name.as_deref(),
                sha256: sha256.as_deref(),
                principal: &who,
                actor: None,
                grid_stack,
            };
            // everything but the bytes first, so a refusal reads nothing
            check(registry.store(), &ask).map_err(|(_, m)| crate::usage(m))?;
            let mut f = std::fs::File::open(&file)
                .map_err(|e| crate::usage(format!("{}: {e}", file.display())))?;
            let d = register(&mut registry, &ask, &mut f).map_err(|(status, m)| {
                if status >= 500 {
                    crate::fail(m)
                } else {
                    crate::usage(m)
                }
            })?;
            if json {
                let mut v = doc(registry.store(), &d);
                keyboard_shape(registry.store(), &d, &mut v)?;
                return print(&v);
            }
            println!(
                "derivative {}: {} of {} {}, {} bytes, sha256 {}",
                d.id,
                nils_registry::a_or_an(&d.kind),
                d.scope,
                d.stack_id
                    .or(d.series_id)
                    .or(d.subject_id)
                    .or(d.run_id)
                    .or(d.model_id)
                    .unwrap_or_default(),
                d.bytes,
                d.sha256
            );
            Ok(())
        }
        DerivativeCommand::List {
            kind,
            stack,
            subject,
            run,
            check,
            limit,
            json,
        } => {
            let rows = derivative::list(
                registry.store(),
                &derivative::Filter {
                    kind: kind.as_deref(),
                    stack_id: stack,
                    subject_id: subject,
                    run_id: run,
                    check: check.as_deref(),
                    limit,
                },
            )
            .map_err(|e| crate::usage(e.to_string()))?;
            let store = registry.store();
            // record 48, D1 of the move: as the listing door, a result made
            // from a sealed stack loses its parts' counts and its check, and
            // is not listed by a check's status
            let mut rows_left = Vec::with_capacity(rows.len());
            let mut docs: Vec<Value> = Vec::with_capacity(rows.len());
            for d in rows {
                let mut v = doc(store, &d);
                if keyboard_shape(store, &d, &mut v)? && check.is_some() {
                    continue;
                }
                docs.push(v);
                rows_left.push(d);
            }
            let rows = rows_left;
            if json {
                return print(&json!(docs));
            }
            println!(
                "{:>6}  {:<9} {:<8} {:>8} {:>12}  sha256",
                "id", "kind", "scope", "of", "bytes"
            );
            for d in &rows {
                println!(
                    "{:>6}  {:<9} {:<8} {:>8} {:>12}  {}",
                    d.id,
                    d.kind,
                    d.scope,
                    d.stack_id
                        .or(d.series_id)
                        .or(d.subject_id)
                        .or(d.run_id)
                        .or(d.model_id)
                        .unwrap_or_default(),
                    d.bytes,
                    &d.sha256[..d.sha256.len().min(16)]
                );
            }
            match working(store) {
                Ok(p) => println!("{} derivatives; new ones go into {}", rows.len(), p.name),
                Err(m) => println!("{} derivatives; {m}", rows.len()),
            }
            Ok(())
        }
        DerivativeCommand::Show { id, json } => {
            let d = derivative::get(registry.store(), id)
                .map_err(|e| crate::fail(e.to_string()))?
                .ok_or_else(|| crate::usage(format!("no derivative {id}")))?;
            let mut v = doc(registry.store(), &d);
            let sealed = keyboard_shape(registry.store(), &d, &mut v)?;
            if json {
                return print(&v);
            }
            println!("derivative {id}: {}", nils_registry::a_or_an(&d.kind));
            println!(
                "  belongs to       {} {}{}",
                d.scope,
                d.stack_id
                    .or(d.series_id)
                    .or(d.subject_id)
                    .or(d.run_id)
                    .or(d.model_id)
                    .unwrap_or_default(),
                d.session_day
                    .as_deref()
                    .map(|s| format!(" on {s}"))
                    .unwrap_or_default()
            );
            println!(
                "  lives in         {} at {}",
                v["place"].as_str().unwrap_or("a place that is gone"),
                d.path
            );
            println!("  bytes            {}", d.bytes);
            println!("  sha256           {}", d.sha256);
            println!("  media type       {}", d.media_type);
            println!(
                "  registered by    {} at {}",
                d.registered_by.as_deref().unwrap_or("-"),
                d.created_at
            );
            if let Some(old) = d.supersedes_id {
                println!("  supersedes       {old}");
            }
            if let Some(run) = d.run_id {
                println!("  made by run      {run}");
            }
            println!(
                "  transports       {}",
                v["transports"]
                    .as_array()
                    .map(|a| a
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            );
            if let Some(link) = &d.grid_link {
                println!(
                    "  grid             {link}{}",
                    d.grid_stack_id
                        .map(|s| format!(" to stack {s}"))
                        .unwrap_or_default()
                );
            }
            if sealed {
                println!(
                    "  sealed           made from a stack of a sample sealed for certification: its parts' counts and its check are withheld (record 48); pass --unsealed-access REASON to read them"
                );
            } else if let Some(c) = &d.check_status {
                println!("  check            {c}");
            }
            Ok(())
        }
        DerivativeCommand::Parts {
            id,
            run,
            force,
            json,
        } => {
            let ids: Vec<i64> = match (id, run) {
                (Some(id), _) => vec![id],
                (None, Some(run)) => derivative::list(
                    registry.store(),
                    &derivative::Filter {
                        kind: Some("mask"),
                        run_id: Some(run),
                        limit: 1_000_000,
                        ..derivative::Filter::default()
                    },
                )
                .map_err(|e| crate::fail(e.to_string()))?
                .into_iter()
                .rev()
                // a withdrawn mask is checked by nobody
                .filter(|d| d.withdrawn_at.is_none())
                .map(|d| d.id)
                .collect(),
                (None, None) => return Err(crate::usage("name a mask, or --run")),
            };
            let mut done = Vec::new();
            let mut failed = 0usize;
            for id in ids {
                let d = derivative::get(registry.store(), id)
                    .map_err(|e| crate::fail(e.to_string()))?
                    .ok_or_else(|| crate::usage(format!("no derivative {id}")))?;
                if d.kind != "mask" {
                    return Err(crate::usage(format!(
                        "derivative {id} is {}; only a mask has parts",
                        nils_registry::a_or_an(&d.kind)
                    )));
                }
                if !force && !d.part_sets.is_null() && d.grid_link.is_some() {
                    done.push(json!({"derivative": id, "skipped": "made already"}));
                    continue;
                }
                let at = where_of(registry.store(), &d);
                let sets = sets_of(registry.store(), &d);
                // one mask that cannot be read stops no other: it is said,
                // and the run's other masks are read
                let read = match read_mask(&mut registry, &d, at, sets) {
                    Ok(r) => r,
                    Err(why) => {
                        failed += 1;
                        if !json {
                            println!("result {id}: not read ({why})");
                        }
                        done.push(json!({"derivative": id, "failed": why}));
                        continue;
                    }
                };
                if keyboard_withholds(registry.store(), id)? {
                    if !json {
                        println!("result {id}: read; {KEYBOARD_WITHHELD}");
                    }
                    done.push(json!({"derivative": id, "withheld_sealed": true}));
                    continue;
                }
                if !json {
                    println!(
                        "result {id}: grid {}{}, {}",
                        read["grid_link"].as_str().unwrap_or("none"),
                        read["grid_stack"]
                            .as_i64()
                            .map(|s| format!(" to stack {s}"))
                            .unwrap_or_default(),
                        read["sets"]
                            .as_array()
                            .map(|a| a
                                .iter()
                                .map(|s| format!(
                                    "{} {}{}",
                                    s["set"].as_str().unwrap_or(""),
                                    s["parts"],
                                    s["note"]
                                        .as_str()
                                        .map(|n| format!(" ({n})"))
                                        .unwrap_or_default()
                                ))
                                .collect::<Vec<_>>()
                                .join(", "))
                            .unwrap_or_else(|| read["why"].as_str().unwrap_or("").to_string())
                    );
                }
                done.push(read);
            }
            if json {
                return print(&json!({"results": done, "failed": failed}));
            }
            Ok(())
        }
        DerivativeCommand::Layer {
            id: None,
            campaign: Some(which),
            force,
            workers,
            json,
            ..
        } => {
            // record 55 H3: a check's pictures and layers, warmed behind its
            // first items; a pair that cannot be drawn is said and passed
            let c = nils_registry::campaign::find(registry.store(), &which)
                .map_err(|e| crate::usage(e.to_string()))?;
            let pairs = crate::checks::layer_pairs(registry.store(), c.id)
                .map_err(|e| crate::fail(e.to_string()))?;
            let mut done = Vec::new();
            let (mut built, mut failed) = (0usize, 0usize);
            for (d, stack) in pairs {
                match build_layers(&mut registry, d, stack, force, workers) {
                    Ok(_) if keyboard_withholds(registry.store(), d)? => {
                        built += 1;
                        done.push(
                            json!({"derivative": d, "stack": stack, "withheld_sealed": true}),
                        );
                    }
                    Ok(v) => {
                        built += 1;
                        done.push(v);
                    }
                    Err((_, why)) => {
                        failed += 1;
                        done.push(json!({"derivative": d, "stack": stack, "why": why}));
                    }
                }
            }
            if json {
                return print(
                    &json!({"campaign": c.id, "built": built, "failed": failed, "layers": done}),
                );
            }
            println!(
                "check {} ({}): {built} result-scan pair(s) drawn, {failed} not",
                c.id, c.name
            );
            for v in done.iter().filter(|v| v["why"].is_string()) {
                println!(
                    "  result {} on stack {}: {}",
                    v["derivative"],
                    v["stack"],
                    v["why"].as_str().unwrap_or_default()
                );
            }
            Ok(())
        }
        DerivativeCommand::Layer {
            id,
            stack,
            force,
            workers,
            json,
            ..
        } => {
            let (Some(id), Some(stack)) = (id, stack) else {
                return Err(crate::usage("name a mask and --stack, or --campaign"));
            };
            let built =
                build_layers(&mut registry, id, stack, force, workers).map_err(|(c, m)| {
                    if c >= 500 {
                        crate::fail(m)
                    } else {
                        crate::usage(m)
                    }
                })?;
            if keyboard_withholds(registry.store(), id)? {
                if json {
                    return print(
                        &json!({"derivative": id, "stack": stack, "withheld_sealed": true}),
                    );
                }
                println!("result {id} on stack {stack}: built; {KEYBOARD_WITHHELD}");
                return Ok(());
            }
            if json {
                return print(&built);
            }
            for l in built["layers"].as_array().into_iter().flatten() {
                println!(
                    "result {id} on stack {stack}: set {} {}, {} voxels drawn, {:.2} s",
                    l["set"].as_str().unwrap_or(""),
                    if l["skipped"].is_string() {
                        "built already"
                    } else {
                        "built"
                    },
                    l["voxels"],
                    l["wall_seconds"].as_f64().unwrap_or(0.0)
                );
            }
            Ok(())
        }
    }
}

/// Where a mask registered before is looked for its grid scan: the scan
/// read before, a stack's own, else the run's unit's inputs.
fn where_of(store: &mut Store, d: &Derivative) -> Where {
    if let Some(s) = d.grid_stack_id {
        return Where::Named(s);
    }
    if d.scope == "stack"
        && let Some(s) = d.stack_id
    {
        return Where::Own(s);
    }
    let role = output_of(store, d).and_then(|(desc, id)| {
        desc.outputs
            .iter()
            .find(|o| o.id == id)
            .and_then(|o| o.grid.clone())
    });
    match d.run_id {
        Some(run) => crate::pipelines::grid_candidates(store, run, &d.path, role.as_deref()),
        None => Where::Unknown,
    }
}

/// Build a mask's layers on a scan, one per set it is read as, and tie it
/// again where the scan's picture shows its grid is the scan's own.
pub(crate) fn build_layers(
    registry: &mut Registry,
    id: i64,
    stack: i64,
    force: bool,
    workers: usize,
) -> Result<Value, Refused> {
    let store = registry.store();
    let mut d = derivative::get(store, id)
        .map_err(|e| (500, e.to_string()))?
        .ok_or_else(|| (400, format!("no derivative {id}")))?;
    if d.kind != "mask" {
        return Err((
            400,
            format!(
                "derivative {id} is {}; only a mask has layers",
                nils_registry::a_or_an(&d.kind)
            ),
        ));
    }
    if d.part_sets.is_null() || d.grid_link.is_none() {
        let at = where_of(store, &d);
        let sets = sets_of(store, &d);
        read_mask(registry, &d, at, sets).map_err(|e| (500, e))?;
        d = derivative::get(registry.store(), id)
            .map_err(|e| (500, e.to_string()))?
            .ok_or_else(|| (500, format!("derivative {id} went")))?;
    }
    let store = registry.store();
    let Some(grid_stack) = d
        .grid_stack_id
        .filter(|_| d.grid_link.as_deref() != Some("none"))
    else {
        return Err((
            409,
            format!("result {id} cannot be drawn: no scan is known whose grid it is on"),
        ));
    };
    if !same_frame(store, grid_stack, stack).map_err(|r| (r.status, r.body["error"].to_string()))? {
        return Err((
            409,
            format!(
                "result {id} is on the grid of stack {grid_stack}, and stack {stack} is not in its scanner's frame"
            ),
        ));
    }
    let working = crate::pyramid::working_place(store, None).map_err(|m| (409, m))?;
    let pictures = crate::pyramid::dir(Path::new(&working.path), stack);
    // the scan's picture first, where it is not built
    let scan = match crate::pyramid::manifest(&pictures).map_err(|e| (500, e))? {
        Some(m) => m,
        None => {
            // a reading's words can hold a file's path, which is the
            // archive's: what a door or a job serves is its class, as the
            // picture worker says it (the review of E1, item 3)
            let vol = crate::pyramid::read_volume(store, stack).map_err(|e| {
                (
                    409,
                    format!(
                        "stack {stack}: its picture could not be built ({})",
                        crate::pyramid::reason_of(&e, true)
                    ),
                )
            })?;
            crate::pyramid::build(&vol, stack, &pictures, workers, None).map_err(|e| {
                (
                    500,
                    format!(
                        "stack {stack}: its picture could not be built ({})",
                        crate::pyramid::reason_of(&e, false)
                    ),
                )
            })?
        }
    };
    let lattice = crate::mask::scan_lattice(&scan).map_err(|m| (409, m))?;
    let file = file_of(store, &d)?;
    let (grid, data) = crate::mask::read_labels(&file).map_err(|e| (409, e))?;
    // the picture says it is the scan's own grid: tied voxel for voxel
    let mut link = d.grid_link.clone().unwrap_or_else(|| "world".into());
    if stack == grid_stack {
        let now = crate::mask::link_of(&grid, Some(&lattice), true);
        if now != link {
            derivative::set_grid(store, id, &d.grid, Some(grid_stack), now)
                .map_err(|e| (500, e.to_string()))?;
            link = now.to_string();
        }
    } else {
        link = "world".to_string();
    }
    let mut layers = Vec::new();
    for set in d.part_sets.as_array().into_iter().flatten() {
        let Some(spec) = crate::mask::SetSpec::from_doc(set) else {
            continue;
        };
        let root = crate::mask::layer_dir(Path::new(&working.path), stack, &d.sha256, &spec.key);
        if !force && let Some(m) = crate::mask::layer_manifest(&root, &scan) {
            layers.push(json!({
                "set": spec.key, "skipped": "built already", "voxels": m.voxels,
                "wall_seconds": m.wall_seconds,
            }));
            continue;
        }
        let (_, parts) = derivative::parts(
            store,
            id,
            &spec.key,
            derivative::PartOrder::Position,
            0,
            crate::mask::MAX_PARTS,
        )
        .map_err(|e| (500, e.to_string()))?;
        let m = crate::mask::build_layer(
            &root,
            &scan,
            &grid,
            &data,
            &crate::mask::LayerOf {
                derivative: id,
                sha256: &d.sha256,
                link: &link,
                set: &spec,
                parts: &parts,
            },
            workers,
        )
        .map_err(|e| (500, e))?;
        layers.push(json!({
            "set": spec.key, "voxels": m.voxels, "levels": m.levels,
            "wall_seconds": m.wall_seconds,
        }));
    }
    Ok(json!({"derivative": id, "stack": stack, "link": link, "layers": layers}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Record 48: a table's rows are found with what names their stacks,
    /// and a row kept is written back byte for byte as it was, a quoted
    /// line break, a blank line and CRLF endings among them.
    #[test]
    fn a_table_s_rows_are_written_back_as_they_were_less_the_ones_left_out() {
        let csv = b"\xef\xbb\xbfunit,note\r\nstack-3,\"two\r\nlines\"\r\n\r\nstack-4,plain\r\nstack-5,last";
        let rows = delimited_rows(csv, b',', Some("unit")).unwrap();
        assert_eq!(
            rows.keys,
            [
                Some("stack-3".into()),
                Some("stack-4".into()),
                Some("stack-5".into())
            ]
        );
        assert_eq!((rows.rebuild)(&[true, true, true]), csv.to_vec());
        assert_eq!(
            (rows.rebuild)(&[true, false, true]),
            b"\xef\xbb\xbfunit,note\r\nstack-3,\"two\r\nlines\"\r\nstack-5,last".to_vec()
        );
        assert_eq!(
            (rows.rebuild)(&[false, true, false]),
            b"\xef\xbb\xbfunit,note\r\n\r\nstack-4,plain\r\n".to_vec()
        );
        // a column of stack ids, folded; none is no way to split
        let tsv = b"Stack ID\tvolume\n12\t1.5\n\t2\n13\t3\n";
        let rows = delimited_rows(tsv, b'\t', None).unwrap();
        assert_eq!(rows.keys, [Some("12".into()), None, Some("13".into())]);
        assert_eq!(
            (rows.rebuild)(&[true, true, false]),
            b"Stack ID\tvolume\n12\t1.5\n\t2\n".to_vec()
        );
        assert!(delimited_rows(b"subject\tvolume\n1\t2\n", b'\t', None).is_none());
        assert!(delimited_rows(b"stack,volume\n1,2\n", b',', Some("unit")).is_none());
        // JSON: a list of objects is rows, one object one row
        let json = br#"[{"stack_id": 7, "v": 1.25}, {"stack_id": "stack-8", "v": 2}, {"v": 3}]"#;
        let rows = json_rows(json, None).unwrap();
        assert!(!rows.single);
        assert_eq!(rows.keys, [Some("7".into()), Some("stack-8".into()), None]);
        let kept: Value = serde_json::from_slice(&(rows.rebuild)(&[false, true, true])).unwrap();
        assert_eq!(kept, json!([{"stack_id": "stack-8", "v": 2}, {"v": 3}]));
        let one = json_rows(br#"{"stack": 9, "v": 1}"#, None).unwrap();
        assert!(one.single);
        assert_eq!(one.keys, [Some("9".into())]);
        assert!(json_rows(br#"{"score": 1}"#, None).is_none());
        assert!(json_rows(b"not json", None).is_none());
        for (text, id) in [
            ("12", Some(12)),
            ("12.0", Some(12)),
            ("stack-12", Some(12)),
            ("sub-1", None),
            ("1.5", None),
        ] {
            assert_eq!(stack_named(text), id, "{text}");
        }
        assert_eq!(
            unit_folder("derivatives/p/3/sub-01_ses-a/sub-01/ses-a/anat/x.json"),
            Some(("sub-01".into(), Some("ses-a".into())))
        );
        assert_eq!(
            unit_folder("derivatives/p/3/sub-01/x.json"),
            Some(("sub-01".into(), None))
        );
        assert_eq!(unit_folder("derivatives/p/3/scores.csv"), None);
    }

    #[test]
    fn only_a_plain_extension_of_a_name_is_kept() {
        assert_eq!(extension(Some("mask.nii.gz")), ".nii.gz");
        assert_eq!(extension(Some("/a/b/SUBJ01_mask.NPY")), ".npy");
        assert_eq!(extension(Some("noext")), "");
        assert_eq!(extension(Some("a.b c")), "");
        assert_eq!(extension(Some("a..b")), "");
        assert_eq!(extension(None), "");
    }

    #[test]
    fn a_file_is_kept_under_its_digest_and_a_wrong_digest_keeps_nothing() {
        let root = nils_dicom::synth::TempDir::new("derivative-store");
        let body = b"a mask, as bytes".to_vec();
        let sha = hex::encode(ring::digest::digest(&ring::digest::SHA256, &body).as_ref());
        let stored = store_file(
            root.path(),
            "mask",
            Some("m.nii.gz"),
            &mut body.as_slice(),
            Some(&sha),
        )
        .unwrap();
        assert_eq!(stored.sha256, sha);
        assert_eq!(stored.bytes, body.len() as i64);
        assert_eq!(
            stored.path,
            format!("derivatives/mask/{}/{sha}.nii.gz", &sha[..2])
        );
        assert_eq!(std::fs::read(root.path().join(&stored.path)).unwrap(), body);
        // the same bytes again are the same file
        let again = store_file(
            root.path(),
            "mask",
            Some("m.nii.gz"),
            &mut body.as_slice(),
            None,
        )
        .unwrap();
        assert_eq!(again, stored);

        let wrong = "0".repeat(64);
        let e =
            store_file(root.path(), "mask", None, &mut &b"other"[..], Some(&wrong)).unwrap_err();
        assert!(matches!(e, StoreFail::Mismatch { .. }), "{e}");
        let incoming: Vec<_> = std::fs::read_dir(root.path().join("derivatives/.incoming"))
            .unwrap()
            .collect();
        assert!(incoming.is_empty(), "nothing is left half written");
        let masks: Vec<_> = std::fs::read_dir(root.path().join("derivatives/mask"))
            .unwrap()
            .collect();
        assert_eq!(masks.len(), 1, "only the first file is kept");
    }
}
