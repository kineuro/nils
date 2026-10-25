// SPDX-License-Identifier: AGPL-3.0-only

//! Pipelines at the keyboard, the doors and the queue (record 43 S1 to S3).
//!
//! The catalog: `nils pipeline add <nils.job.yml>` checks a descriptor
//! (`contracts/job/v1`, through `nils-pipeline`) and keeps it as the name's
//! next version, refusing an image not pinned by its registry manifest
//! digest; `list` and `show` read it back.
//!
//! The runner: `nils run <pipeline> --select selection:<name>@<v>` is one run
//! over a frozen selection (or, record 55, over a dataset's or a cohort's
//! main scans of a role, `--select main:<role>@dataset:<name>`, or of each
//! role the pipeline needs, `main@dataset:<name>`, frozen the same way:
//! `crate::main_input`), a job of kind `pipeline`, queued at `POST
//! /api/jobs` under `pipelines:work` at detail quasi, since a pipeline reads
//! pixels. Runs whose stacks do not meet go on side by side within the
//! lane's one budget; a run that would take a stack a running run holds is
//! refused (R1, as widened on 2026-09-30), and the steps that write what
//! every run shares are taken one run at a time. In order:
//!
//! 1. The descriptor, its parameters with the defaults filled, the models
//!    and the label set it reads, the working place and a runtime (rootless
//!    podman, apptainer, docker only by an operator's choice; none is the
//!    capability off, D1), and the GPU it asks for.
//! 2. The selection frozen to a handle, which the run pins.
//! 3. The input materialised under `<working>/runs/<run>/`: a release in
//!    the BIDS layout with the picks applied (R4), or `stacks.json` naming
//!    each stack's files under the source places, mounted read-only.
//! 4. The container: no network, the input and the typed inputs read-only,
//!    one output folder, `<working>/derivatives/<pipeline>/<run>/`, and a
//!    process that is not root on the host.
//! 5. After it exits: `results.json` (or, without one, each unit's files
//!    found by the declared templates), every file hashed by the engine and
//!    registered as a derivative naming the run, the proposals passed to
//!    the review spine's hook, a `pipeline:qc` review item for each unit
//!    that failed or went unreported, and the run closed with the digest of
//!    what it said.
//!
//! The doors read the catalog and the runs; the derivatives a run made are
//! served by the derivative doors, by bytes or by a shared path (S3).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nils_pipeline::descriptor::{self, Descriptor, Gpu, Layout, Level, Units};
use nils_pipeline::lane;
use nils_pipeline::runtime::{self, Choice, Detected, ImageForm, Invocation, Mount, Runtime};
use nils_pipeline::secrets;
use nils_registry::derivative::{self, Belongs};
use nils_registry::home::Home;
use nils_registry::job;
use nils_registry::pipeline::{self as rows, Pipeline, Run};
use nils_registry::schema::Type;
use nils_registry::store::Store;
use nils_registry::{Param, Registry};
use serde_json::{Value, json};

use crate::serve::{Caller, Reply};
use crate::{Exit, fail, usage};

/// The doors, as the capabilities list them.
pub(crate) const DOORS: [&str; 10] = [
    "GET /api/pipelines",
    "GET /api/pipelines/{id}",
    // Wave 7b section 7: a family, the one row that opens to its members
    "GET /api/pipelines/family/{name}",
    // Wave 7b: the pack as loaded, its replay, a run's numbers per key; the desk gates its features on these being listed
    "GET /api/pipelines/{id}/pack",
    "GET /api/pipelines/{id}/replay",
    "GET /api/pipeline-runs/{id}/numbers",
    "GET /api/pipeline-runs",
    "GET /api/pipeline-runs/{id}",
    "GET /api/pipeline-runs/{id}/log",
    "GET /api/pipeline-runs/{id}/units/{unit}/log",
];

/// Where the registry keeps the operator's choice of runtime.
pub(crate) const RUNTIME_KEY: &str = "pipeline_runtime";

/// Where a run's input, typed inputs and log go under the working place.
pub(crate) const RUNS: &str = "runs";

/// Where apptainer's copies of images are kept under the working place, by
/// digest (record 49 A2).
pub(crate) const IMAGES: &str = "images";

/// The lane's settings in the registry (record 49 A1, A2): its cores, its
/// GB of memory, the card a GPU unit leases (`none` for no card), and how
/// apptainer keeps an image.
pub(crate) const LANE_CORES_KEY: &str = "pipeline_lane_cores";
pub(crate) const LANE_MEMORY_KEY: &str = "pipeline_lane_memory_gb";
pub(crate) const GPU_CARD_KEY: &str = "pipeline_gpu_card";
pub(crate) const APPTAINER_IMAGE_KEY: &str = "pipeline_apptainer_image";

/// The lane's places (record 49 R7, as Nima corrected it: "result is for
/// analysis pipeline output"): the working place, by name, a run's outputs
/// go to, and the one its scratch goes to. Unset, each is the first active
/// working place, as before.
pub(crate) const OUTPUT_PLACE_KEY: &str = "pipeline_output_place";
pub(crate) const SCRATCH_PLACE_KEY: &str = "pipeline_scratch_place";

/// A secret's file, as the site set it, under this prefix and its id
/// (record 49 R3): the path, never the bytes.
pub(crate) const SECRET_PREFIX: &str = "pipeline_secret:";

/// How apptainer keeps an image here: `sif` until set.
pub(crate) fn image_form(registry: &mut Registry) -> ImageForm {
    registry
        .meta_value(APPTAINER_IMAGE_KEY)
        .ok()
        .flatten()
        .and_then(|v| ImageForm::parse(&v))
        .unwrap_or(ImageForm::Sif)
}

/// The operator's choice of runtime: `auto` until one is set.
pub(crate) fn choice(registry: &mut Registry) -> Choice {
    registry
        .meta_value(RUNTIME_KEY)
        .ok()
        .flatten()
        .and_then(|v| Choice::parse(&v))
        .unwrap_or(Choice::Auto)
}

/// How long a runtime found is kept before it is looked for again. Asking
/// podman is not free: `podman info` took 5 s of a busy laptop's time.
const LOOK_KEPT: Duration = Duration::from_secs(120);

/// How long a door waits for the first look at the choice in force, where
/// nothing is known of it yet: a stand-in answers well within it, a real
/// podman on a busy host does not, and the door answers without it.
const FIRST_LOOK_WAIT: Duration = Duration::from_secs(1);

/// What the looks for a runtime found, and the look in flight.
struct Looks {
    found: Option<(Instant, Choice, Detected)>,
    looking: Option<Choice>,
}

static LOOKS: Mutex<Looks> = Mutex::new(Looks {
    found: None,
    looking: None,
});
static LOOKED: std::sync::Condvar = std::sync::Condvar::new();
/// Whether this process serves doors, so a look runs behind them.
static LOOK_BEHIND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `nils serve` (2026-10-11): from here on the look for a runtime runs on a
/// thread of its own and never on a door's, and the first starts now. The
/// capabilities door ran `podman info` itself whenever its finding was half
/// a minute old; that took 5 s on a busy laptop, up to the probe's 30 s cap
/// on a busy host, and the desk, which gives the engine 5 s, said every
/// half minute that the engine did not answer.
pub(crate) fn look_behind(registry: &mut Registry) {
    LOOK_BEHIND.store(true, std::sync::atomic::Ordering::SeqCst);
    let chosen = choice(registry);
    let mut looks = LOOKS.lock().unwrap_or_else(|e| e.into_inner());
    if looks.looking.is_none() {
        start_look(&mut looks, chosen);
    }
}

/// A look at `chosen` on a thread of its own; what it finds is kept and
/// whoever waits for it is woken.
fn start_look(looks: &mut Looks, chosen: Choice) {
    looks.looking = Some(chosen);
    let path = std::env::var_os("PATH");
    let started = std::thread::Builder::new()
        .name("nils-runtime-look".to_string())
        .spawn(move || {
            let d = runtime::detect(chosen, path.as_deref());
            let mut looks = LOOKS.lock().unwrap_or_else(|e| e.into_inner());
            looks.found = Some((Instant::now(), chosen, d));
            looks.looking = None;
            drop(looks);
            LOOKED.notify_all();
        });
    if started.is_err() {
        looks.looking = None;
    }
}

/// What a door says while the first look has not come back.
fn still_looking(chosen: Choice) -> Detected {
    Detected {
        choice: chosen,
        runtime: None,
        reason: Some(
            "pipelines are off for a moment: the engine is still looking for a container runtime; ask again shortly"
                .to_string(),
        ),
        looked: Vec::new(),
        unknown: true,
    }
}

/// What the look for a runtime found. In `nils serve` this never waits on
/// a runtime: the last finding is answered and an old or unknown one is
/// looked at again behind the door, so a capabilities read costs no
/// subprocess (see [`look_behind`]). At the command line it looks, once.
pub(crate) fn detect_cached(registry: &mut Registry) -> Detected {
    let chosen = choice(registry);
    if !LOOK_BEHIND.load(std::sync::atomic::Ordering::SeqCst) {
        {
            let looks = LOOKS.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, c, d)) = looks.found.as_ref()
                && *c == chosen
                && !d.unknown
                && at.elapsed() < LOOK_KEPT
            {
                return d.clone();
            }
        }
        let d = runtime::detect(chosen, std::env::var_os("PATH").as_deref());
        // a runtime that did not answer in time is not a finding to keep:
        // the next read looks again (wave 43's proof)
        if !d.unknown {
            LOOKS.lock().unwrap_or_else(|e| e.into_inner()).found =
                Some((Instant::now(), chosen, d.clone()));
        }
        return d;
    }
    let until = Instant::now() + FIRST_LOOK_WAIT;
    let mut looks = LOOKS.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some((at, c, d)) = looks.found.as_ref()
            && *c == chosen
        {
            let d = d.clone();
            // an unknown finding, or an old one, is looked at again behind the door
            if (d.unknown || at.elapsed() >= LOOK_KEPT) && looks.looking.is_none() {
                start_look(&mut looks, chosen);
            }
            return d;
        }
        if looks.looking.is_none() {
            start_look(&mut looks, chosen);
        }
        let now = Instant::now();
        if now >= until {
            return still_looking(chosen);
        }
        looks = LOOKED
            .wait_timeout(looks, until - now)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

/// What `GET /api/capabilities` says of pipelines: on when a runtime and a
/// working place are both there, off with the sentence that names the cure
/// when either is not (D1).
pub(crate) fn capability(registry: &mut Registry) -> Value {
    let d = detect_cached(registry);
    let extra = lane_doc(registry);
    capability_with(registry.store(), &d, Some(extra))
}

/// The capability with the lane (record 49): its budget, its card, how
/// apptainer keeps images, and the secrets the site set, by id alone.
fn capability_with(store: &mut Store, d: &Detected, extra: Option<Value>) -> Value {
    let mut v = capability_base(store, d);
    if let Some(Value::Object(more)) = extra {
        for (k, x) in more {
            v[k] = x;
        }
    }
    v
}

/// The secrets the site set, by id.
fn secret_ids(store: &mut Store) -> Vec<String> {
    let d = store.dialect();
    let sql = format!(
        "SELECT key, value FROM {} WHERE key LIKE {} ORDER BY key",
        store.qualified("registry_meta"),
        d.param(1, Type::Text)
    );
    store
        .query(&sql, &[Param::from(format!("{SECRET_PREFIX}%"))])
        .unwrap_or_default()
        .iter()
        .filter(|r| r.text(1).is_ok_and(|v| !v.trim().is_empty()))
        .filter_map(|r| {
            r.text(0)
                .ok()
                .and_then(|k| k.strip_prefix(SECRET_PREFIX))
                .map(str::to_string)
        })
        .collect()
}

/// What the lane adds to the capability.
fn lane_doc(registry: &mut Registry) -> Value {
    let lane = lane_of(registry);
    let form = image_form(registry);
    let places = run_places(registry.store()).ok();
    json!({
        "lane": lane.doc(),
        "places": places.map(|p| p.doc()),
        "apptainer_image": form.name(),
        "secrets": secret_ids(registry.store()),
    })
}

fn capability_base(store: &mut Store, d: &Detected) -> Value {
    let places = run_places(store);
    let catalog = rows::list(store)
        .map(|l| l.iter().filter(|p| p.state == "active").count())
        .unwrap_or(0);
    let reason = d.reason.clone().or_else(|| places.as_ref().err().cloned());
    json!({
        "enabled": d.runtime.is_some() && places.is_ok(),
        "reason": reason,
        "runtime": d.runtime.as_ref().map(|r| json!({
            "name": r.kind.name(), "version": r.version, "gpu": r.gpu,
        })),
        "choice": d.choice.name(),
        "looked": d.looked.iter().map(|(k, s)| json!({"runtime": k, "found": s})).collect::<Vec<_>>(),
        "place": places.as_ref().ok().map(|p| p.output.name.clone()),
        "scratch": places.as_ref().ok().map(|p| p.scratch.name.clone()),
        "pipelines": catalog,
        "contract": nils_pipeline::CONTRACT,
        "grants": {"see": "pipelines:see", "run": "pipelines:work", "run_detail": "quasi"},
        // 2026-10-12: the pre-flight takes a run taken up again ({resume:
        // <run>}), and the jobs door checks one as it checks a new run
        "preflight": {"door": crate::preflight::DOOR, "resume": true},
    })
}

/// Add a descriptor's text to the catalog, as `nils pipeline add --json`
/// and `POST /api/pipelines` do (record 55 H2): parsed and its image's pin
/// checked, kept as its name's next version unless the catalog holds the
/// same descriptor, and audited when it is new. The answer is the catalog
/// entry with `added`, `replaces` (the earlier versions a new run no longer
/// takes) and `replaces_why`. A descriptor that does not parse, or names an
/// image not pinned by digest, is a usage error in plain words.
pub(crate) fn add_descriptor(
    registry: &mut Registry,
    text: &str,
    pack_text: Option<&str>,
) -> Result<Value, Exit> {
    let d = descriptor::parse(text).map_err(|e| usage(e.to_string()))?;
    let digest = d.digest();
    // record 57 P1 (Wave 7b A1): the pack beside the descriptor, checked
    // against it with the loader's words; a version is the pair
    let pack = match pack_text {
        Some(t) => Some(
            crate::pipeline_pack::load(registry.store(), &d, t)
                .map_err(|e| usage(format!("{}: {e}", nils_pipeline::pack::FILE)))?,
        ),
        None => None,
    };
    let pack_digest = pack.as_ref().map(nils_pipeline::pack::PipelinePack::digest);
    let version_digest = nils_pipeline::pack::version_digest(&digest, pack_digest.as_deref());
    let who = crate::actor();
    let now = nils_registry::time::now_iso();
    let (p, fresh) = rows::add(
        registry.store(),
        &rows::New {
            name: &d.name,
            tool_version: &d.tool_version,
            descriptor: &d.document,
            descriptor_digest: &digest,
            image: &d.image.reference,
            image_digest: &d.image.digest,
            layout: d.layout.name(),
            level: d.level.name(),
            added_by: &who,
            added_at: &now,
            pack: pack.as_ref().map(|p| &p.document),
            pack_digest: pack_digest.as_deref(),
            version_digest: &version_digest,
        },
    )?;
    if fresh {
        crate::audit(
            registry,
            nils_registry::audit::Action::PipelineAdd,
            json!({"pipeline": p.id, "name": p.name, "version": p.version}),
            Some(
                json!({"descriptor": p.descriptor_digest, "image": p.image_digest, "pack": p.pack_digest, "version_digest": p.version_digest}),
            ),
        )?;
    }
    // 2026-10-12: the earlier versions it replaces, which a new run no
    // longer takes
    let replaced: Vec<String> = rows::versions(registry.store(), &p.name)?
        .iter()
        .filter(|q| q.version < p.version && q.state == "active")
        .filter(|q| d.replaces.as_ref().is_some_and(|r| r.covers(q.version)))
        .map(Pipeline::label)
        .collect();
    let mut v = pipeline_doc_of(registry.store(), &p);
    v["added"] = json!(fresh);
    v["replaces"] = json!(replaced);
    v["replaces_why"] = json!(d.replaces.as_ref().map(|r| r.why.clone()));
    Ok(v)
}

/// The file an engine release ships its model pipelines' descriptors in
/// (record 55 A5, 2026-10-14): `pipelines/<package>/<name>/nils.job.yml`
/// for each descriptor whose image the release built, naming that image by
/// the digest the release pushed, so a tag's descriptors never name an
/// older image than its own.
pub(crate) const RELEASE_DESCRIPTORS: &str = "pipelines.tar.gz";

/// The descriptors a release's `pipelines.tar.gz` holds, by the folder each
/// is in (the pipeline's name), read without unpacking it.
pub(crate) fn descriptors_in(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    use std::io::Read;
    let e = |e: std::io::Error| format!("reading the release's descriptors: {e}");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut out = BTreeMap::new();
    for entry in archive.entries().map_err(e)? {
        let mut entry = entry.map_err(e)?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(e)?
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .filter(|c| c != ".")
            .collect::<Vec<_>>();
        // pipelines/<package>/<name>/nils.job.yml, and nothing else
        let [top, _package, name, file] = path.as_slice() else {
            continue;
        };
        if top != "pipelines" || file != "nils.job.yml" || name.starts_with('.') {
            continue;
        }
        let mut text = String::new();
        entry.read_to_string(&mut text).map_err(e)?;
        out.insert(name.clone(), text);
    }
    if out.is_empty() {
        return Err(format!(
            "the release's {RELEASE_DESCRIPTORS} holds no descriptor"
        ));
    }
    Ok(out)
}

/// The descriptor of the pipeline `name` that the engine release `version`
/// ships, from `base`, checked against the release's sums, and named as
/// its folder says.
pub(crate) fn release_descriptor(base: &str, version: &str, name: &str) -> Result<String, Exit> {
    let bytes = crate::update::fetch_checked_typed(base, &format!("v{version}"), RELEASE_DESCRIPTORS)
        .map_err(|e| match e {
            // an integrity failure, never a reason to take another
            // descriptor: nothing is added
            crate::update::Fetched::Mismatch { file, got, want } => fail(format!(
                "the release {version}'s descriptors failed their integrity check ({file} does not match the release's checksum: {got} against {want}): the file is not the one the release published, so nothing was added"
            )),
            crate::update::Fetched::Failed(e) => usage(format!(
                "the release {version} ships no descriptors that can be read here ({}); a build that is not a release adds a descriptor from its checkout: nils pipeline add <nils.job.yml>",
                e.message
            )),
        })?;
    let mut all = descriptors_in(&bytes).map_err(usage)?;
    let Some(text) = all.remove(name) else {
        return Err(usage(format!(
            "the release {version} ships no descriptor of {name}; it ships {}",
            all.into_keys().collect::<Vec<_>>().join(", ")
        )));
    };
    let d = descriptor::parse(&text).map_err(|e| {
        usage(format!(
            "the release {version}'s descriptor of {name} does not check: {e}"
        ))
    })?;
    if d.name != name {
        return Err(usage(format!(
            "the release {version}'s descriptor in {name}/ names the pipeline {}",
            d.name
        )));
    }
    Ok(text)
}

/// A change to the lane, as `nils pipeline lane` and `PUT
/// /api/pipelines/lane` take it: each field left out is kept.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LaneChange {
    pub(crate) cores: Option<u32>,
    pub(crate) memory_gb: Option<u64>,
    pub(crate) gpu_card: Option<String>,
    pub(crate) output_place: Option<String>,
    pub(crate) scratch_place: Option<String>,
}

/// Set the lane (record 49): its cores and memory, the card a GPU unit
/// leases, and its output and scratch places, each checked as the command
/// line checks it, and the change audited. A place is an active working
/// place by name, or `default` for the first working place.
pub(crate) fn set_lane(registry: &mut Registry, change: &LaneChange) -> Result<(), Exit> {
    let mut changed = serde_json::Map::new();
    if let Some(c) = change.cores {
        if c == 0 {
            return Err(usage("the lane's cores are one or more"));
        }
        registry.set_meta(LANE_CORES_KEY, &c.to_string())?;
        changed.insert("cores".into(), json!(c));
    }
    if let Some(m) = change.memory_gb {
        if m == 0 {
            return Err(usage("the lane's memory is one GB or more"));
        }
        registry.set_meta(LANE_MEMORY_KEY, &m.to_string())?;
        changed.insert("memory_gb".into(), json!(m));
    }
    if let Some(card) = &change.gpu_card {
        let word = card.trim();
        if word != "none" && word.parse::<u32>().is_err() {
            return Err(usage(format!(
                "the GPU card is a card's index, or none, not {word}"
            )));
        }
        registry.set_meta(GPU_CARD_KEY, word)?;
        changed.insert("gpu_card".into(), json!(word));
    }
    for (given, key, flag) in [
        (&change.output_place, OUTPUT_PLACE_KEY, "output"),
        (&change.scratch_place, SCRATCH_PLACE_KEY, "scratch"),
    ] {
        let Some(name) = given.as_ref().map(|n| n.trim().to_string()) else {
            continue;
        };
        let value = if name == "default" {
            String::new()
        } else {
            let working = nils_registry::place::active(registry.store())
                .map_err(|e| fail(e.to_string()))?
                .into_iter()
                .any(|p| p.name == name && p.role == nils_registry::place::Role::Working);
            if !working {
                return Err(usage(format!(
                    "the {flag} place {name}: no active working place is named so; add one with the working role under Places"
                )));
            }
            name
        };
        registry.set_meta(key, &value)?;
        changed.insert(
            format!("{flag}_place"),
            json!(if value.is_empty() {
                "default"
            } else {
                value.as_str()
            }),
        );
    }
    if !changed.is_empty() {
        crate::audit(
            registry,
            nils_registry::audit::Action::PipelineRuntime,
            json!({"lane": Value::Object(changed)}),
            None,
        )?;
    }
    Ok(())
}

/// The lane as `nils pipeline lane --json` prints it and `GET
/// /api/pipelines/lane` answers it: cores, memory_gb, set, why, gpu_card,
/// and its places (or the error that turns pipelines off).
pub(crate) fn lane_answer(registry: &mut Registry) -> Value {
    let lane = lane_of(registry);
    let places = run_places(registry.store());
    let mut doc = lane.doc();
    doc["places"] = match &places {
        Ok(p) => p.doc(),
        Err(e) => json!({"error": e}),
    };
    doc
}

/// Whether a word is a secret's id: lowercase letters, digits and `_`.
fn secret_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Set the file a secret is read from (record 49 R3), as `nils pipeline
/// secret set` does: the path is kept, never the bytes, and the file is read
/// now so a file the engine cannot read is said now.
pub(crate) fn set_secret_file(registry: &mut Registry, id: &str, file: &Path) -> Result<(), Exit> {
    if !secret_id_ok(id) {
        return Err(usage(format!(
            "a secret's id is lowercase letters, digits and _, not {id}"
        )));
    }
    if !file.is_absolute() {
        return Err(usage(format!(
            "{} is not an absolute path; the engine reads the file where it runs",
            file.display()
        )));
    }
    secrets::read(id, file).map_err(usage)?;
    let path = file.display().to_string();
    if path.contains(':') || path.contains(',') {
        return Err(usage(format!(
            "{path} holds a ':' or a ',', which a runtime's mount syntax splits on"
        )));
    }
    registry.set_meta(&format!("{SECRET_PREFIX}{id}"), &path)?;
    crate::audit(
        registry,
        nils_registry::audit::Action::PipelineRuntime,
        json!({"secret": id, "set": true}),
        None,
    )?;
    Ok(())
}

/// Forget a secret's file, as `nils pipeline secret unset` does.
pub(crate) fn unset_secret(registry: &mut Registry, id: &str) -> Result<(), Exit> {
    if !secret_id_ok(id) {
        return Err(usage(format!(
            "a secret's id is lowercase letters, digits and _, not {id}"
        )));
    }
    registry.set_meta(&format!("{SECRET_PREFIX}{id}"), "")?;
    crate::audit(
        registry,
        nils_registry::audit::Action::PipelineRuntime,
        json!({"secret": id, "set": false}),
        None,
    )?;
    Ok(())
}

/// Where a secret given at `PUT /api/pipelines/secrets/{id}` is kept: under
/// the engine's home, in `secrets/`, readable by the engine alone.
pub(crate) const SECRETS_DIR: &str = "secrets";

/// Keep a secret's text, given at the door, as a file of the engine's own
/// (record 55 H2): written to `secrets/<id>` under the engine's home with
/// mode 0600 in a folder of mode 0700, then set as `nils pipeline secret
/// set` sets a file. The bytes never enter the registry, the audit or the
/// answer.
pub(crate) fn keep_secret_text(
    registry: &mut Registry,
    home: &Path,
    id: &str,
    content: &str,
) -> Result<(), Exit> {
    if !secret_id_ok(id) {
        return Err(usage(format!(
            "a secret's id is lowercase letters, digits and _, not {id}"
        )));
    }
    if content.trim().is_empty() {
        return Err(usage(format!("the secret {id} is empty")));
    }
    if content.len() as u64 > secrets::MAX_BYTES {
        return Err(usage(format!(
            "the secret {id} is {} bytes, more than a secret file's {}",
            content.len(),
            secrets::MAX_BYTES
        )));
    }
    let dir = home.join(SECRETS_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|e| fail(format!("the secrets folder cannot be made: {e}")))?;
    let file = dir.join(id);
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            let tmp = dir.join(format!(".{id}.new"));
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
            std::fs::rename(&tmp, &file)
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&file, content.as_bytes())
        }
    };
    write().map_err(|e| fail(format!("the secret {id} cannot be written: {e}")))?;
    set_secret_file(registry, id, &file)
}

/// The secrets, as `GET /api/pipelines/secrets` answers them: each one an
/// active pipeline declares or the site has set, with the pipelines that
/// declare it, whether every declaration is optional, whether it is set and
/// whether its file can be read now. Never its path or its bytes.
pub(crate) fn secrets_answer(registry: &mut Registry) -> Result<Value, Exit> {
    let mut declared: std::collections::BTreeMap<String, (Vec<String>, bool)> =
        std::collections::BTreeMap::new();
    for p in rows::list(registry.store())?
        .iter()
        .filter(|p| p.state == "active")
    {
        for s in p.descriptor["x-nils"]["secrets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let Some(id) = s["id"].as_str() else { continue };
            let e = declared
                .entry(id.to_string())
                .or_insert_with(|| (Vec::new(), true));
            e.0.push(p.label());
            e.1 &= s["optional"] == true;
        }
    }
    for id in secret_ids(registry.store()) {
        declared.entry(id).or_insert_with(|| (Vec::new(), true));
    }
    let mut out = Vec::new();
    for (id, (by, optional)) in declared {
        let path = secret_path(registry, &id).filter(|p| !p.trim().is_empty());
        let readable = path
            .as_deref()
            .is_some_and(|p| secrets::read(&id, Path::new(p)).is_ok());
        out.push(json!({
            "id": id,
            "declared_by": by,
            "optional": optional,
            "set": path.is_some(),
            "readable": readable,
        }));
    }
    Ok(json!({ "secrets": out }))
}

/// The doors that change the catalog and the lane (record 55 H2: no
/// command line inside a desk flow), as the capabilities list them.
pub(crate) const SETUP_DOORS: [&str; 6] = [
    "POST /api/pipelines",
    "GET /api/pipelines/lane",
    "PUT /api/pipelines/lane",
    "GET /api/pipelines/secrets",
    "PUT /api/pipelines/secrets/{id}",
    "DELETE /api/pipelines/secrets/{id}",
];

/// `POST /api/pipelines` (a descriptor's text added to the catalog), `GET`
/// and `PUT /api/pipelines/lane`, `GET /api/pipelines/secrets`, and `PUT`
/// and `DELETE /api/pipelines/secrets/{id}`. The grants are the door
/// table's: pipelines:see to read, install:work to change, since each
/// brings a container image or a site's file into what runs.
pub(crate) fn setup_route(
    home: &Path,
    registry: &mut Registry,
    method: &str,
    segs: &[&str],
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let parsed = |body: &str| -> Result<Value, Reply> {
        serde_json::from_str(body)
            .map_err(|e| Reply::error(400, format!("the body is not JSON: {e}")))
    };
    Some(match (method, segs) {
        ("POST", ["api", "pipelines"]) => (|| {
            let v = parsed(body)?;
            let Some(text) = v["descriptor"].as_str().filter(|t| !t.trim().is_empty()) else {
                return Err(Reply::error(
                    400,
                    "descriptor is the text of a nils.job.yml",
                ));
            };
            let pack_text = v["pack"].as_str().filter(|t| !t.trim().is_empty());
            let doc = add_descriptor(registry, text, pack_text)?;
            Ok(if doc["added"] == true {
                Reply::created(doc)
            } else {
                Reply::ok(doc)
            })
        })(),
        ("GET", ["api", "pipelines", "lane"]) => Ok(Reply::ok(lane_answer(registry))),
        ("PUT", ["api", "pipelines", "lane"]) => (|| {
            let change: LaneChange = serde_json::from_str(body).map_err(|e| {
                Reply::error(
                    400,
                    format!(
                        "the lane takes cores, memory_gb, gpu_card, output_place and scratch_place: {e}"
                    ),
                )
            })?;
            set_lane(registry, &change)?;
            Ok(Reply::ok(lane_answer(registry)))
        })(),
        ("GET", ["api", "pipelines", "secrets"]) => {
            secrets_answer(registry).map(Reply::ok).map_err(Reply::from)
        }
        ("PUT", ["api", "pipelines", "secrets", id]) => (|| {
            let v = parsed(body)?;
            let Some(content) = v["content"].as_str() else {
                return Err(Reply::error(400, "content is the secret file's text"));
            };
            keep_secret_text(registry, home, id, content)?;
            let all = secrets_answer(registry)?;
            Ok(Reply::ok(one_secret(&all, id)))
        })(),
        ("DELETE", ["api", "pipelines", "secrets", id]) => (|| {
            unset_secret(registry, id)?;
            // the file the door kept goes with it; a file the command line
            // named is the site's own and stays
            let _ = std::fs::remove_file(home.join(SECRETS_DIR).join(id));
            let all = secrets_answer(registry)?;
            Ok(Reply::ok(one_secret(&all, id)))
        })(),
        _ => return None,
    })
}

/// Gate T31, step 15.8: the doors of the runtime pipelines run under, the
/// starter catalog and a run's seeds, as the capabilities list them.
pub(crate) const RUNTIME_DOORS: [&str; 6] = [
    "GET /api/pipelines/runtime",
    "PUT /api/pipelines/runtime",
    "GET /api/pipelines/starters",
    "PUT /api/pipelines/starters",
    "POST /api/pipelines/starters/seed",
    "GET /api/pipeline-runs/{id}/seeds",
];

/// The policy rows of [`RUNTIME_DOORS`]: door, writes, idempotent, cost,
/// cap, and what the assistant says while and after.
pub(crate) const RUNTIME_POLICY: &[(&str, bool, bool, &str, &str, &str, &str)] = &[
    (
        "GET /api/pipelines/runtime",
        false,
        false,
        "free",
        "one document",
        "Reading the pipeline runtime",
        "Read the pipeline runtime",
    ),
    (
        "PUT /api/pipelines/runtime",
        true,
        true,
        "free",
        "one document",
        "Choosing the pipeline runtime",
        "Chose the pipeline runtime",
    ),
    (
        "GET /api/pipelines/starters",
        false,
        false,
        "free",
        "the starters",
        "Reading the starter catalog",
        "Read the starter catalog",
    ),
    (
        "PUT /api/pipelines/starters",
        true,
        true,
        "free",
        "one setting",
        "Turning the starter seeding on or off",
        "Turned the starter seeding on or off",
    ),
    (
        "POST /api/pipelines/starters/seed",
        true,
        true,
        "free",
        "the starters",
        "Seeding the starter catalog",
        "Seeded the starter catalog",
    ),
    (
        "GET /api/pipeline-runs/{id}/seeds",
        false,
        false,
        "bounded",
        "one run's seeds",
        "Reading a run's seeds",
        "Read a run's seeds",
    ),
];

/// The runtime and the starters as their doors answer them.
fn runtime_answer(registry: &mut Registry) -> Value {
    json!({
        "choice": choice(registry).name(),
        "choices": Choice::WORDS,
        "apptainer_image": image_form(registry).name(),
        "apptainer_images": ImageForm::WORDS,
        "capability": capability(registry),
    })
}

fn starters_answer(registry: &mut Registry) -> Result<Value, Reply> {
    let list = crate::starter::list(registry).map_err(|e| Reply::error(500, e))?;
    Ok(json!({
        "seeding": if crate::starter::enabled(registry) { "on" } else { "off" },
        "starters": list,
    }))
}

/// One act of these doors on the audit log, under the person who asked, as
/// `nils pipeline runtime` and `nils pipeline starter` record theirs.
fn runtime_audit(registry: &mut Registry, principal: &str, scope: Value) -> Result<(), Reply> {
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal,
            action: nils_registry::audit::Action::PipelineRuntime,
            scope,
            policy: None,
            job_id: None,
            details: None,
        },
    )
    .map(|_| ())
    .map_err(|e| Reply::error(500, e.to_string()))
}

/// Gate T31, step 15.8: `GET` and `PUT /api/pipelines/runtime` (the
/// runtime pipelines run under and how apptainer keeps an image, as `nils
/// pipeline runtime --set` and `--apptainer-image`), `GET` and `PUT
/// /api/pipelines/starters` (whether the engine seeds its starters at its
/// start, as `nils pipeline starter --on` and `--off`) and `POST
/// /api/pipelines/starters/seed` (seed what the catalog lacks now, as
/// `--seed`). The grants are the door table's: pipelines:see to read,
/// install:work to change, since each changes what runs and where. Each
/// change is audited under the caller.
pub(crate) fn runtime_route(
    registry: &mut Registry,
    principal: &str,
    method: &str,
    segs: &[&str],
    body: &str,
) -> Option<Result<Reply, Reply>> {
    let parsed = |body: &str| -> Result<Value, Reply> {
        serde_json::from_str(body)
            .map_err(|e| Reply::error(400, format!("the body is not JSON: {e}")))
    };
    Some(match (method, segs) {
        ("GET", ["api", "pipelines", "runtime"]) => Ok(Reply::ok(runtime_answer(registry))),
        ("PUT", ["api", "pipelines", "runtime"]) => (|| {
            let v = parsed(body)?;
            let word = |key: &str| -> Result<Option<String>, Reply> {
                match &v[key] {
                    Value::Null => Ok(None),
                    Value::String(s) => Ok(Some(s.trim().to_string())),
                    _ => Err(Reply::error(400, format!("{key} is a word"))),
                }
            };
            let set = word("choice")?;
            let form = word("apptainer_image")?;
            if set.is_none() && form.is_none() {
                return Err(Reply::error(
                    400,
                    "the runtime takes choice, apptainer_image or both",
                ));
            }
            // both are checked before either is written
            let set = set
                .map(|w| {
                    Choice::parse(&w).ok_or_else(|| {
                        Reply::error(
                            400,
                            format!("choice is one of {}, not {w}", Choice::WORDS.join(", ")),
                        )
                    })
                })
                .transpose()?;
            let form = form
                .map(|w| {
                    ImageForm::parse(&w).ok_or_else(|| {
                        Reply::error(
                            400,
                            format!(
                                "apptainer_image is one of {}, not {w}",
                                ImageForm::WORDS.join(", ")
                            ),
                        )
                    })
                })
                .transpose()?;
            let err = |e: nils_registry::home::HomeError| Reply::error(500, e.to_string());
            if let Some(form) = form {
                registry
                    .set_meta(APPTAINER_IMAGE_KEY, form.name())
                    .map_err(err)?;
                runtime_audit(registry, principal, json!({"apptainer_image": form.name()}))?;
            }
            if let Some(c) = set {
                registry.set_meta(RUNTIME_KEY, c.name()).map_err(err)?;
                runtime_audit(registry, principal, json!({"choice": c.name()}))?;
            }
            Ok(Reply::ok(runtime_answer(registry)))
        })(),
        ("GET", ["api", "pipelines", "starters"]) => starters_answer(registry).map(Reply::ok),
        ("PUT", ["api", "pipelines", "starters"]) => (|| {
            let v = parsed(body)?;
            let word = match v["seeding"].as_str() {
                Some(w @ ("on" | "off")) => w,
                _ => return Err(Reply::error(400, "seeding is on or off")),
            };
            registry
                .set_meta(crate::starter::SETTING, word)
                .map_err(|e| Reply::error(500, e.to_string()))?;
            runtime_audit(registry, principal, json!({"starter": word}))?;
            starters_answer(registry).map(Reply::ok)
        })(),
        ("POST", ["api", "pipelines", "starters", "seed"]) => (|| {
            let added = crate::starter::seed(registry).map_err(|e| Reply::error(500, e))?;
            let mut doc = starters_answer(registry)?;
            doc["added"] = json!(added);
            Ok(Reply::ok(doc))
        })(),
        _ => return None,
    })
}

/// `GET /api/pipeline-runs/{id}/seeds` (gate T31, step 15.8): what `nils
/// pipeline seeds` reads of a run, the values it suggests a person curate,
/// counted by axis and value, and the stacks it suggests curating, which a
/// caller saves as a selection at the ask's selection door. A seed or a
/// suggested stack of a sample sealed now is withheld from a caller who
/// does not read sealed samples, and how many were is said.
fn seeds_answer(registry: &mut Registry, caller: &Caller, run: i64) -> Result<Reply, Reply> {
    let doc = seeds_of(registry.store(), run).map_err(|e| {
        if e.starts_with("no pipeline run") || e.contains("suggested no seeds") {
            Reply::error(404, e)
        } else {
            Reply::error(500, e)
        }
    })?;
    let mut seeds: Vec<Value> = doc["seeds"].as_array().cloned().unwrap_or_default();
    let mut stacks: Vec<i64> = doc["selection"]["stacks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .collect();
    let mut withheld = 0usize;
    if !crate::sealed::reads(&caller.access) {
        let mut named: Vec<i64> = stacks.clone();
        named.extend(seeds.iter().filter_map(|s| s["stack_id"].as_i64()));
        let sealed = crate::sealed::now(registry.store(), &named)
            .map_err(|e| Reply::error(500, e.to_string()))?;
        if !sealed.is_empty() {
            let before = seeds.len() + stacks.len();
            seeds.retain(|s| {
                !s["stack_id"]
                    .as_i64()
                    .is_some_and(|id| sealed.contains(&id))
            });
            stacks.retain(|id| !sealed.contains(id));
            withheld = before - seeds.len() - stacks.len();
        }
    }
    let mut per: BTreeMap<String, usize> = BTreeMap::new();
    for s in &seeds {
        let key = format!(
            "{}={}",
            s["axis"].as_str().unwrap_or_default(),
            s["value"].as_str().unwrap_or_default()
        );
        *per.entry(key).or_default() += 1;
    }
    Ok(Reply::ok(json!({
        "run": run,
        "seeds": seeds,
        "per_value": per,
        "selection": {"stacks": stacks},
        "withheld": withheld,
    })))
}

/// One secret's row of the list, or a row that says it is not set.
fn one_secret(all: &Value, id: &str) -> Value {
    all["secrets"]
        .as_array()
        .and_then(|a| a.iter().find(|s| s["id"] == id).cloned())
        .unwrap_or_else(|| json!({"id": id, "declared_by": [], "optional": true, "set": false, "readable": false}))
}

/// A catalog entry as the doors and `--json` answer it.
pub(crate) fn pipeline_doc(p: &Pipeline) -> Value {
    let x = &p.descriptor["x-nils"];
    json!({
        "id": p.id, "name": p.name, "version": p.version, "label": p.label(),
        "tool_version": p.tool_version, "description": p.descriptor["description"],
        "image": p.image, "image_digest": p.image_digest,
        "descriptor_digest": p.descriptor_digest, "layout": p.layout, "level": p.level,
        "state": p.state, "added_by": p.added_by, "added_at": p.added_at,
        "origin": p.origin, "starter": p.origin.as_deref() == Some(rows::STARTER),
        "checks": x["qc"], "roles": x["input"]["roles"],
        "parameters": p.descriptor["inputs"], "inputs": x["inputs"], "outputs": x["outputs"],
        "needs": x["needs"], "proposals": x["proposals"],
        "descriptor": p.descriptor,
        // record 57 P1: the pack beside the descriptor, `none` for a version
        // with no pack, and the digests of the pack and the pair
        "pack": match crate::pipeline_pack::pack_error(p) {
            Some(e) => json!({"error": e}),
            None if p.pack.is_null() => json!("none"),
            None => p.pack.clone(),
        },
        "pack_error": crate::pipeline_pack::pack_error(p),
        "pack_digest": p.pack_digest,
        "version_digest": p.version_digest,
    })
}

/// Catalog entries as the doors and `--json` answer them, each with how it
/// stands for a new run (Nima's ruling of 2026-10-12, [`crate::versions`]):
/// current, selectable, replaced (by which version, and why) or retired.
pub(crate) fn pipeline_docs(list: &[Pipeline]) -> (Vec<Value>, Value) {
    let standing = crate::versions::of_catalog(list);
    let docs = list
        .iter()
        .map(|p| {
            let mut doc = pipeline_doc(p);
            if let Some(s) = standing.get(&p.id) {
                s.fields(&mut doc);
            }
            doc
        })
        .collect();
    (docs, crate::versions::names(list, &standing))
}

/// One catalog entry as the doors and `--json` answer it, with its standing
/// read beside the other versions of its name.
pub(crate) fn pipeline_doc_of(store: &mut Store, p: &Pipeline) -> Value {
    let mut doc = pipeline_doc(p);
    if let Ok(s) = crate::versions::of_one(store, p) {
        s.fields(&mut doc);
    }
    doc
}

/// A run as `--json` answers it at the keyboard: the row, the pipeline by
/// its label, the derivatives it made, and each unit with its curation
/// state.
pub(crate) fn run_doc(store: &mut Store, r: &Run) -> Result<Value, String> {
    run_doc_at(store, r, true)
}

/// [`run_doc`] as a door answers it: the units' curation state (the rules
/// now against those that ran, record 58 P5) is read, once for every unit
/// of the run, at detail quasi or above only.
pub(crate) fn run_doc_at(store: &mut Store, r: &Run, quasi: bool) -> Result<Value, String> {
    let mut doc = runs_docs(store, std::slice::from_ref(r))
        .pop()
        .unwrap_or(Value::Null);
    // record 49 A1: each unit as the lane scheduled it
    let units = rows::units(store, r.id).unwrap_or_default();
    let mut by_state: BTreeMap<String, usize> = BTreeMap::new();
    for u in &units {
        *by_state.entry(u.state.clone()).or_default() += 1;
    }
    doc["unit_states"] = json!(by_state);
    // record 55 (rulings of 2026-10-12, item 6): a run over a dataset's or a
    // cohort's main scans says which, and under which rules they were read
    doc["main"] = r
        .handle_id
        .and_then(|h| crate::main_input::entry_of(store, h))
        .map_or(Value::Null, |e| crate::main_input::shown(&e));
    // Wave 7b A7 (record 58 P5): each unit's stacks, for its curation
    // state now against the one it recorded, the state now read once for
    // every stack of the run (after review, finding 6)
    let stacks_of: BTreeMap<String, Vec<i64>> = if quasi {
        unit_stacks(store, r.id)
            .unwrap_or_default()
            .into_iter()
            .collect()
    } else {
        BTreeMap::new()
    };
    let every_stack: Vec<i64> = stacks_of
        .values()
        .flatten()
        .copied()
        .collect::<std::collections::BTreeSet<i64>>()
        .into_iter()
        .collect();
    // the registry's error is the door's (after review, finding 4 of the
    // second review), never every unit `current`
    let states = quasi
        .then(|| crate::provenance::States::read(store, &every_stack))
        .transpose()?;
    doc["units_run"] = json!(
        units
            .iter()
            .map(|u| json!({
                "unit": u.unit, "state": u.state, "status": u.outcome["status"],
                // Wave 7b A7: the run whose kept result the unit reused (section
                // 8.5), and the curation state of its inputs (record 58 P5)
                "reused_from": u.reused_from_run,
                "words_reused": u.reused_from_run.map(|_| u.outcome["words"].clone()),
                "curation": states.as_ref().map(|st| st.of(
                    &u.curation,
                    stacks_of.get(&u.unit).map_or(&[][..], Vec::as_slice),
                )),
                "attempts": u.attempts, "device": u.device, "gpu_card": u.gpu_card,
                "cores": u.cores, "memory_mb": u.memory_mb, "exit_code": u.exit_code,
                "started_at": u.started_at, "finished_at": u.finished_at,
                // 2026-10-12: why a unit failed, beside what it said
                "error": u.outcome["error"],
                "failure": crate::failures::of_unit(&u.outcome),
                // 2026-10-12: what a skipped unit lacked, in a few words
                "lacks": crate::made::lacks(&u.outcome),
                // record 57 P2 to P4: what the pack decided, who chose each
                // knob, the caveats, the flags and the two keys
                "pack_status": u.pack["status"], "need": u.pack["need"], "words": u.pack["words"],
                "variant": u.pack["variant"], "knobs": u.pack["knobs"], "caveats": u.pack["caveats"],
                // Wave 7b section 7: the family a member's run ran under
                "family": u.pack["family"],
                "flags": u.pack["flags"], "cohort_checks": u.pack["cohort_checks"],
                "key_members": u.pack["key_members"],
                "keys": {"comparability": u.comparability_key, "result": u.result_key},
                // after review (finding 10): when the pack judged the unit
                // (`run`, or `start` for a chained unit judged again as it
                // started), and the judgement at the run's start it replaced
                "judged_at": u.pack["judged_at"], "forecast": u.pack["forecast"],
                // after review (finding 8 of the second review): why no kept
                // result was reused, where one was looked for
                "reuse": u.pack["reuse"],
                // after the third review (finding 9): the optional inputs the
                // chain fed nothing for, each with why
                "fed": u.pack["fed"],
            }))
            .collect::<Vec<_>>()
    );
    // 2026-10-12: what it made, from its units as they stand, a running
    // run's too; a run that kept no units is read from its summary
    if !units.is_empty() {
        doc["made"] = crate::made::of_units(&units);
    }
    Ok(doc)
}

/// The most runs one page of `GET /api/pipeline-runs` answers.
pub(crate) const RUNS_PAGE_MAX: usize = 500;

/// Many runs as the doors answer them, the catalog and the derivatives
/// they made read once for all of them, not once per run.
pub(crate) fn runs_docs(store: &mut Store, runs: &[Run]) -> Vec<Value> {
    let labels: BTreeMap<i64, String> = rows::list(store)
        .unwrap_or_default()
        .into_iter()
        .map(|p| (p.id, p.label()))
        .collect();
    let mut made: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for chunk in runs.chunks(500) {
        let list = chunk
            .iter()
            .map(|r| r.id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT run_id, id FROM {} WHERE run_id IN ({list}) ORDER BY id",
            store.qualified("derivative")
        );
        for r in store.query(&sql, &[]).unwrap_or_default() {
            if let (Ok(run), Ok(id)) = (r.int(0), r.int(1)) {
                made.entry(run).or_default().push(id);
            }
        }
    }
    runs.iter()
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
            v["pipeline"] = json!(labels.get(&r.pipeline_id));
            v["derivatives"] = json!(made.get(&r.id).cloned().unwrap_or_default());
            // 2026-10-12: a run that failed before its failure was recorded
            // is read from its error
            v["failure"] = crate::failures::of_run_doc(&v);
            // 2026-10-12: what it made, unit by unit, its skipped units'
            // reasons read from its units where its summary did not sum them
            let unsummed = !r.summary["skipped_by_why"].is_array()
                && r.summary["units"]["skipped"]
                    .as_u64()
                    .is_some_and(|n| n > 0);
            let skipped = unsummed.then(|| crate::made::skipped_of_run(store, r.id));
            v["made"] = crate::made::of_summary(&r.summary, skipped.as_ref());
            v
        })
        .collect()
}

/// A run read at detail plain (record 43 review): a unit named by its
/// subject or session, `sub-<s>` or `sub-<s>_ses-<t>`, is quasi
/// identifying, so every such text in the document is left out.
pub(crate) fn plain(v: &mut Value) {
    match v {
        Value::String(t) if t.starts_with("sub-") => *v = Value::Null,
        Value::Array(items) => items.iter_mut().for_each(plain),
        Value::Object(map) => map.values_mut().for_each(plain),
        _ => {}
    }
}

/// The fewest scans a count below detail quasi stands for (record 49 R4b,
/// the ask's own k): a count of 1 to 4 is withheld, none is still none.
const SCANS_K: usize = nils_ask::validate::MEASURE_K as usize;

/// What says that something was left out below detail quasi.
const WITHHELD: &str = "withheld below detail quasi";

/// A count of scans below detail quasi, under `key`: the count, or null
/// and marked withheld when it stands for 1 to 4 scans.
fn held(key: &str, n: usize) -> Value {
    if n == 0 || n >= SCANS_K {
        json!({ key: n })
    } else {
        json!({ key: null, "withheld": true })
    }
}

/// Counts under their names, each held to [`SCANS_K`], as a list.
fn held_list(name: &str, key: &str, counts: BTreeMap<String, usize>) -> Vec<Value> {
    counts
        .into_iter()
        .map(|(what, n)| {
            let mut v = held(key, n);
            v[name] = json!(what);
            v
        })
        .collect()
}

/// A run's summary read below detail quasi (record 49 R4 and R4b, after
/// the assistant's review): its checks and its failures as counts by check
/// and by reason, each count held to 5 scans, and no unit, no scan's value
/// and no error text a tool wrote. What was held is named in `withheld`.
pub(crate) fn summary_totals_only(s: &mut Value) {
    if !s.is_object() {
        return;
    }
    let mut withheld: Vec<&str> = Vec::new();
    // the breaches: a unit and its values become counts by check
    let mut by_check: BTreeMap<String, usize> = BTreeMap::new();
    let mut metric_of: BTreeMap<String, Value> = BTreeMap::new();
    for u in s["breaches"].as_array().into_iter().flatten() {
        for b in u["breaches"].as_array().into_iter().flatten() {
            let check = b["check"]
                .as_str()
                .or_else(|| b["metric"].as_str())
                .unwrap_or("a check")
                .to_string();
            *by_check.entry(check.clone()).or_default() += 1;
            metric_of
                .entry(check)
                .or_insert_with(|| b["metric"].clone());
        }
    }
    let mut checks = held_list("check", "units", by_check);
    for c in &mut checks {
        c["metric"] = metric_of
            .get(c["check"].as_str().unwrap_or_default())
            .cloned()
            .unwrap_or(Value::Null);
    }
    s["breaches"] = json!([]);
    s["breaches_by_check"] = json!(checks);
    if let Some(n) = s["numbers"]["checks"]["breaches"].as_u64()
        && held("n", n as usize)["withheld"] == true
    {
        s["numbers"]["checks"]["breaches"] = Value::Null;
        withheld.push("numbers.checks.breaches");
    }
    // record 48: how many units' breaches a seal left out, a count of scans
    // held as any is
    if let Some(n) = s["breaches_withheld_sealed"].as_u64()
        && held("n", n as usize)["withheld"] == true
    {
        s["breaches_withheld_sealed"] = Value::Null;
        withheld.push("breaches_withheld_sealed");
    }
    // a cell a table could not read is said as its value: counted by column
    if let Some(list) = s["numbers"]["tables"]["refused_values"].as_array() {
        let mut by_column: BTreeMap<String, usize> = BTreeMap::new();
        for v in list {
            let t = v.as_str().unwrap_or_default();
            // `output: column: value`, the value dropped
            let column = t.splitn(3, ": ").take(2).collect::<Vec<_>>().join(": ");
            *by_column.entry(column).or_default() += 1;
        }
        s["numbers"]["tables"]["refused_values"] = json!(held_list("column", "values", by_column));
    }
    // the failures: counted by reason, never a unit or a tool's words
    let mut by_reason: BTreeMap<String, usize> = BTreeMap::new();
    for reason in ["failed", "unreported"] {
        let n = s["units"][reason].as_u64().unwrap_or(0) as usize;
        if n > 0 {
            by_reason.insert(reason.to_string(), n);
        }
    }
    let mut refused_units = std::collections::BTreeSet::new();
    if let Some(list) = s["refused_files"].as_array_mut() {
        // a refusal of a unit's file names the unit and its file; one of
        // the run's own, a model's output, names neither and stays
        list.retain(|r| match r["unit"].as_str() {
            Some(u) => {
                refused_units.insert(u.to_string());
                false
            }
            None => true,
        });
    }
    if !refused_units.is_empty() {
        by_reason.insert("refused".into(), refused_units.len());
    }
    // a count of failed units held, and the count of those that succeeded
    // with it, since the total less it says the same number
    let mut held_units = false;
    for reason in ["failed", "unreported"] {
        if let Some(n) = s["units"][reason].as_u64()
            && held("n", n as usize)["withheld"] == true
        {
            s["units"][reason] = Value::Null;
            held_units = true;
        }
    }
    if held_units && s["units"]["succeeded"].is_u64() {
        s["units"]["succeeded"] = Value::Null;
    }
    if held_units {
        withheld.push("units");
    }
    // the review items a run raised are one a unit: their ids are a count
    if let Some(ids) = s["review_items"].as_array() {
        let n = ids.len();
        s["review_items"] = json!(n);
        if held("n", n)["withheld"] == true {
            s["review_items"] = Value::Null;
            withheld.push("review_items");
        }
    }
    s["failures_by_reason"] = json!(held_list("reason", "units", by_reason));
    // 2026-10-12: the failures by their cause, each count held as a count
    // of units is
    if let Some(list) = s["failures_by_cause"].as_array_mut() {
        for f in list.iter_mut() {
            let n = f["units"].as_u64().unwrap_or(0) as usize;
            let kept = held("units", n);
            f["units"] = kept["units"].clone();
            if kept["withheld"] == true {
                f["withheld"] = json!(true);
            }
        }
    }
    s["detail"] = json!("totals");
    s["withheld"] = json!(withheld);
}

/// A run's document read below detail quasi: its summary as
/// [`summary_totals_only`] says, no unit's own row, and no error text.
pub(crate) fn run_totals_only(doc: &mut Value) {
    summary_totals_only(&mut doc["summary"]);
    // what it made, its counts held as the summary holds them
    if doc.get("made").is_some() {
        doc["made"] = crate::made::held(&doc["made"], &doc["summary"]);
    }
    if doc.get("units_run").is_some() {
        doc["units_run"] = json!([]);
    }
    if doc["error"].is_string() {
        doc["error"] = json!(WITHHELD);
    }
    plain(doc);
}

/// A job read below detail quasi: a pipeline run's result and error as its
/// run's (record 49 R4, after review), and a dataset probe's merge reading
/// as its counts, never a subject's code (record 55 K9; below quasi a code
/// is a shape everywhere, K7, and a pair says more than its two codes). A
/// job with no result, or none of candidates, is left as it is.
pub(crate) fn job_totals_only(job: &mut Value) {
    if let Some(candidates) = job
        .get_mut("result")
        .and_then(|r| r.get_mut("candidates"))
        .and_then(Value::as_array_mut)
    {
        for c in candidates {
            if let Some(n) = c["alike"]["pairs"].as_array().map(Vec::len) {
                c["alike"]["pairs"] = json!([]);
                c["alike"]["withheld"] = json!(n);
            }
        }
    }
    let a_run = job["kind"] == "pipeline" || job["result"]["run"].is_i64();
    if !a_run {
        return;
    }
    if job["result"]["summary"].is_object() {
        summary_totals_only(&mut job["result"]["summary"]);
        if job.get("made").is_some() {
            job["made"] = crate::made::held(&job["made"], &job["result"]["summary"]);
        }
    }
    if job["error"].is_string() {
        job["error"] = json!(WITHHELD);
    }
    plain(&mut job["result"]);
}

/// The `pipeline:qc` items of a review list read below detail quasi
/// (record 49 R4b, after review): never one a unit, since counting them
/// would say a small number, but one entry a run, an item status and a
/// check (a breach) or a reason (a failure), with its count of units held
/// to [`SCANS_K`]. Other kinds are left as they are, in their order; a
/// group stands where its first item stood.
pub(crate) fn qc_items_grouped(rows: Vec<Value>) -> Vec<Value> {
    type Key = (Option<i64>, String, String, String, Option<String>);
    let mut out: Vec<Result<Value, Key>> = Vec::new();
    let mut groups: BTreeMap<Key, (Value, usize)> = BTreeMap::new();
    for r in rows {
        if r["kind"] != nils_registry::review::PIPELINE_QC_KIND {
            out.push(Ok(r));
            continue;
        }
        let run = r["ref"]["run_id"].as_i64();
        let pipeline = r["ref"]["pipeline"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let status = r["status"].as_str().unwrap_or_default().to_string();
        let reason = r["evidence"]["status"]
            .as_str()
            .unwrap_or("failed")
            .to_string();
        let mut checks: Vec<Option<String>> = r["evidence"]["metrics"]["breaches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| {
                b["check"]
                    .as_str()
                    .or_else(|| b["metric"].as_str())
                    .map(|c| Some(c.to_string()))
            })
            .collect();
        checks.sort();
        checks.dedup();
        if reason != "breach" || checks.is_empty() {
            checks = vec![None];
        }
        for check in checks {
            let key: Key = (run, pipeline.clone(), status.clone(), reason.clone(), check);
            let g = groups.entry(key.clone()).or_insert_with(|| {
                out.push(Err(key.clone()));
                (r.clone(), 0)
            });
            g.1 += 1;
        }
    }
    out.into_iter()
        .map(|o| match o {
            Ok(v) => v,
            Err(key) => {
                let (first, n) = groups.get(&key).cloned().unwrap_or((Value::Null, 0));
                let mut v = held("units", n);
                v["kind"] = json!(nils_registry::review::PIPELINE_QC_KIND);
                v["scope"] = first["scope"].clone();
                v["status"] = json!(key.2);
                v["grouped"] = json!(true);
                v["ref"] = json!({"run_id": key.0, "pipeline": key.1});
                v["evidence"] = json!({"status": key.3, "check": key.4, "withheld": WITHHELD});
                v["group_key"] = key.0.map_or(Value::Null, |r| json!(format!("run:{r}")));
                v
            }
        })
        .collect()
}

/// A count of units under `key` in `map`, held to [`SCANS_K`]: null when
/// it stands for 1 to 4, and named under `withheld` beside it.
pub(crate) fn hold_count(map: &mut Value, key: &str) {
    if let Some(n) = map[key].as_u64()
        && held("n", n as usize)["withheld"] == true
    {
        map[key] = Value::Null;
    }
}

// ------------------------------------------------------- sealed means sealed

/// Record 48, D1 of the move: of the units of a run named, those holding a
/// stack of a sample sealed now, by the stacks the run gave each
/// ([`unit_stacks`], read from the registry, never from a file the run's
/// container could reach). A unit whose stacks can no longer be told holds
/// one while a stack of the list the run was frozen over is sealed now, or
/// while that list is gone as well, as a run's own file does at the
/// derivative doors. None while no sample is sealed.
pub(crate) fn sealed_units(
    store: &mut Store,
    run_id: i64,
    names: &[String],
) -> Result<std::collections::BTreeSet<String>, String> {
    let mut out = std::collections::BTreeSet::new();
    if names.is_empty() || !crate::sealed::any(store).map_err(|e| e.to_string())? {
        return Ok(out);
    }
    let units: BTreeMap<String, Vec<i64>> = unit_stacks(store, run_id)?
        .into_iter()
        .filter(|(_, stacks)| !stacks.is_empty())
        .collect();
    let named: Vec<i64> = names
        .iter()
        .filter_map(|n| units.get(n))
        .flatten()
        .copied()
        .collect();
    let sealed = crate::sealed::now(store, &named).map_err(|e| e.to_string())?;
    let mut untold = Vec::new();
    for n in names {
        match units.get(n) {
            Some(stacks) if stacks.iter().any(|s| sealed.contains(s)) => {
                out.insert(n.clone());
            }
            Some(_) => {}
            None => untold.push(n.clone()),
        }
    }
    if !untold.is_empty() {
        let frozen = frozen_stacks(store, run_id)?;
        if frozen.is_empty()
            || !crate::sealed::now(store, &frozen)
                .map_err(|e| e.to_string())?
                .is_empty()
        {
            out.extend(untold);
        }
    }
    Ok(out)
}

/// The units a run's summary names in its breaches, each once.
fn breach_units(s: &Value) -> Vec<String> {
    let mut out: Vec<String> = s["breaches"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["unit"].as_str().map(str::to_string))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Record 48, D1 of the move: a run's summary as a reader who does not read
/// what a system said of a stack of a sample sealed now reads it. A unit's
/// breaches are its checks' values on its stacks, so those of each unit
/// holding such a stack are left out, and `breaches_withheld_sealed` says
/// how many units went, never which; the run's counts stay as they are.
/// Answers how many units went.
pub(crate) fn summary_unsealed(
    store: &mut Store,
    run_id: i64,
    s: &mut Value,
) -> Result<usize, String> {
    let sealed = sealed_units(store, run_id, &breach_units(s))?;
    if sealed.is_empty() {
        return Ok(0);
    }
    if let Some(list) = s["breaches"].as_array_mut() {
        list.retain(|b| !b["unit"].as_str().is_some_and(|u| sealed.contains(u)));
    }
    s["breaches_withheld_sealed"] = json!(sealed.len());
    Ok(sealed.len())
}

/// 2026-10-12, with record 48: the error a unit's pipeline wrote of a unit
/// holding a stack of a sample sealed now is what a system said of that
/// stack, so a reader who does not read such stacks reads the unit's
/// failure by its kind and cause, never its words: on those units of
/// `units_run`, `error` is null and `sealed` true. Answers how many.
pub(crate) fn units_run_unsealed(
    store: &mut Store,
    run_id: i64,
    doc: &mut Value,
) -> Result<usize, String> {
    let named: Vec<String> = doc["units_run"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|u| u["error"].is_string())
        .filter_map(|u| u["unit"].as_str().map(str::to_string))
        .collect();
    let sealed = sealed_units(store, run_id, &named)?;
    if sealed.is_empty() {
        return Ok(0);
    }
    for u in doc["units_run"].as_array_mut().into_iter().flatten() {
        if u["unit"].as_str().is_some_and(|n| sealed.contains(n)) {
            u["error"] = Value::Null;
            u["sealed"] = json!(true);
        }
    }
    Ok(sealed.len())
}

/// A job's document as [`summary_unsealed`] leaves a run's summary: the
/// result of a pipeline run. Answers how many units went.
pub(crate) fn job_unsealed(store: &mut Store, job: &mut Value) -> Result<usize, String> {
    match job["result"]["run"].as_i64() {
        Some(run) if job["result"]["summary"].is_object() => {
            summary_unsealed(store, run, &mut job["result"]["summary"])
        }
        _ => Ok(0),
    }
}

/// What a run's QC summary shows a door's caller of the units holding a
/// sealed stack: nothing, unless it holds sealed:see, whose read of them
/// is audited as `sealed.read`, naming the door, the run and how many
/// units, never which.
fn shown_unsealed(
    registry: &mut Registry,
    caller: &Caller,
    door: &str,
    run_id: i64,
    s: &mut Value,
) -> Result<(), Reply> {
    let gone = |e: String| Reply::error(500, e);
    if !crate::sealed::reads(&caller.access) {
        summary_unsealed(registry.store(), run_id, s).map_err(gone)?;
        return Ok(());
    }
    let read = sealed_units(registry.store(), run_id, &breach_units(s)).map_err(gone)?;
    if read.is_empty() {
        return Ok(());
    }
    audit_sealed_run(registry, &caller.principal, door, run_id, read.len())
}

/// The `sealed.read` row of a reader holding sealed:see shown the QC of
/// units holding a sealed stack.
fn audit_sealed_run(
    registry: &mut Registry,
    principal: &str,
    door: &str,
    run_id: i64,
    units: usize,
) -> Result<(), Reply> {
    nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal,
            action: nils_registry::audit::Action::SealedRead,
            scope: json!({"door": door, "run": run_id, "breaches_of_units": units}),
            policy: None,
            job_id: None,
            details: Some(json!({"grant": crate::grants::UNSEALED})),
        },
    )
    .map(|_| ())
    .map_err(|e| Reply::error(500, e.to_string()))
}

/// A job's document as the jobs doors show it to `caller`: a pipeline
/// run's result as [`shown_unsealed`] leaves its summary.
pub(crate) fn job_shown_unsealed(
    registry: &mut Registry,
    caller: &Caller,
    door: &str,
    job: &mut Value,
) -> Result<(), Reply> {
    let Some(run) = job["result"]["run"].as_i64() else {
        return Ok(());
    };
    if !job["result"]["summary"].is_object() {
        return Ok(());
    }
    shown_unsealed(registry, caller, door, run, &mut job["result"]["summary"])
}

// ---------------------------------------------------------------- the doors

/// The reading doors: the catalog and the runs.
pub(crate) fn route(
    registry: &mut Registry,
    caller: &Caller,
    quasi: bool,
    get: bool,
    segs: &[&str],
    query: &std::collections::HashMap<String, String>,
) -> Option<Result<Reply, Reply>> {
    if !get {
        return None;
    }
    let id_of = |s: &str, what: &str| -> Result<i64, Reply> {
        s.parse::<i64>()
            .map_err(|_| Reply::error(404, format!("a {what} is named by its id")))
    };
    let err = |e: nils_registry::Error| Reply::error(500, e.to_string());
    Some(match segs {
        ["api", "pipelines"] => (|| {
            let list = rows::list(registry.store()).map_err(err)?;
            let (mut docs, names) = pipeline_docs(&list);
            // Wave 7b section 7: each family one row that opens to its
            // members, and a member's row names its family
            let families = crate::pipeline_family::decorate(registry.store(), &mut docs)
                .map_err(|e| Reply::error(500, e))?;
            Ok(Reply::ok(json!({
                "pipelines": docs,
                "names": names,
                "families": families,
                "capability": capability(registry),
            })))
        })(),
        ["api", "pipelines", "family", name] => (|| {
            let s = crate::pipeline_family::get(registry.store(), name)
                .map_err(|e| Reply::error(500, e))?
                .ok_or_else(|| Reply::error(404, format!("no family {name}")))?;
            Ok(Reply::ok(crate::pipeline_family::doc(registry.store(), &s)))
        })(),
        ["api", "pipelines", id] => (|| {
            let id = id_of(id, "pipeline")?;
            let p = rows::get(registry.store(), id)
                .map_err(err)?
                .ok_or_else(|| Reply::error(404, format!("no pipeline {id}")))?;
            Ok(Reply::ok(pipeline_doc_of(registry.store(), &p)))
        })(),
        // record 57 P10 (Wave 7b, section 10): a catalog version's pack
        // replayed over the registry against another version's, read-only
        ["api", "pipelines", id, "replay"] => (|| {
            let id = id_of(id, "pipeline")?;
            let p = rows::get(registry.store(), id)
                .map_err(err)?
                .ok_or_else(|| Reply::error(404, format!("no pipeline {id}")))?;
            if let Some(e) = crate::pipeline_pack::pack_error(&p) {
                return Err(Reply::error(409, e));
            }
            let Some(pack) = crate::pipeline_pack::pack_of(&p) else {
                return Err(Reply::error(
                    409,
                    format!("{} has no pack to replay", p.label()),
                ));
            };
            let d =
                descriptor::from_value(p.descriptor.clone()).map_err(|e| Reply::error(500, e))?;
            let against = match query.get("against") {
                Some(reference) => Some(
                    rows::resolve(registry.store(), reference)
                        .map_err(err)?
                        .ok_or_else(|| Reply::error(404, format!("no pipeline {reference}")))?,
                ),
                // the name's newest active version before this one, where there
                // is one (the CLI's default too)
                None => rows::versions(registry.store(), &p.name)
                    .map_err(err)?
                    .into_iter()
                    .filter(|o| o.id != p.id && o.version < p.version && o.state == "active")
                    .max_by_key(|o| o.version),
            };
            if let Some(o) = &against
                && let Some(e) = crate::pipeline_pack::pack_error(o)
            {
                return Err(Reply::error(409, e));
            }
            let ask = crate::pipeline_pack::PersonAsk::of_body(&json!({
                "goal": query.get("goal"),
                "switches": query.iter().filter_map(|(k, v)| k.strip_prefix("switch.").map(|s| (s.to_string(), json!(v)))).collect::<serde_json::Map<_, _>>(),
            }))
            .map_err(|e| Reply::error(400, e))?;
            let mut doc =
                crate::pipeline_replay::replay_folder(registry, &d, &pack, against.as_ref(), &ask)
                    .map_err(|e| Reply::error(500, e))?;
            doc["pipeline_id"] = json!(p.id);
            Ok(Reply::ok(doc))
        })(),
        // record 57 P1 (Wave 7b A1): the pack as loaded, with its digest and
        // the facts it reads
        ["api", "pipelines", id, "pack"] => (|| {
            let id = id_of(id, "pipeline")?;
            let p = rows::get(registry.store(), id)
                .map_err(err)?
                .ok_or_else(|| Reply::error(404, format!("no pipeline {id}")))?;
            if let Some(e) = crate::pipeline_pack::pack_error(&p) {
                return Ok(Reply::ok(json!({
                    "pipeline": p.label(), "pipeline_id": p.id, "pack": {"error": e},
                    "pack_digest": p.pack_digest, "version_digest": p.version_digest,
                    "descriptor_digest": p.descriptor_digest,
                    "contract": nils_pipeline::pack::CONTRACT,
                })));
            }
            let Some(pack) = crate::pipeline_pack::pack_of(&p) else {
                return Ok(Reply::ok(json!({
                    "pipeline": p.label(), "pipeline_id": p.id, "pack": "none",
                    "pack_digest": Value::Null, "version_digest": p.version_digest,
                    "contract": nils_pipeline::pack::CONTRACT,
                })));
            };
            let d = descriptor::from_value(p.descriptor.clone()).ok();
            let vocab = d.as_ref().map(|d| pack.vocabulary(d));
            let mut reads: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for (role, rn) in &pack.needs {
                let scope = (role != nils_pipeline::pack::LINK
                    && role != nils_pipeline::pack::HOST)
                    .then_some(role.as_str());
                for e in rn
                    .hard_if
                    .iter()
                    .chain(rn.hard.iter().map(|n| &n.when))
                    .chain(rn.soft.iter().map(|n| &n.when))
                {
                    let e = match &vocab {
                        Some(v) => e.scoped(scope, v),
                        None => e.clone(),
                    };
                    reads.extend(e.reads());
                }
            }
            for r in &pack.rules {
                let e = match &vocab {
                    Some(v) => r.when.scoped(None, v),
                    None => r.when.clone(),
                };
                reads.extend(e.reads());
            }
            Ok(Reply::ok(json!({
                "pipeline": p.label(), "pipeline_id": p.id,
                "pack": pack.document, "pack_digest": p.pack_digest, "version_digest": p.version_digest,
                "descriptor_digest": p.descriptor_digest,
                "contract": nils_pipeline::pack::CONTRACT,
                "facts": reads,
                "breaking_knobs": pack.breaking_knobs(),
                "goals": {"goal": pack.goals.goal, "switches": pack.goals.switches, "defaults": pack.default_choices()},
                "rules": pack.rules.iter().map(|r| json!({"id": r.id, "when": r.text, "set": r.set, "route": r.route, "why": r.why, "source": r.source})).collect::<Vec<_>>(),
                "checks": pack.checks.iter().map(|c| json!({"metric": c.metric, "check": c.text(), "action": c.action.name(), "cohort": c.cohort, "stratum": c.stratum, "min_sessions": c.min_sessions, "source": c.source})).collect::<Vec<_>>(),
            })))
        })(),
        // record 57 P3 (Wave 7b A1): a run's numbers per comparability key,
        // never pooled across keys; ?format=csv is the export
        ["api", "pipeline-runs", id, "numbers"] => (|| {
            let id = id_of(id, "run")?;
            let r = rows::run(registry.store(), id)
                .map_err(err)?
                .ok_or_else(|| Reply::error(404, format!("no pipeline run {id}")))?;
            if query.get("format").map(String::as_str) == Some("csv") {
                let text = crate::pipeline_pack::numbers_csv(registry.store(), r.id)
                    .map_err(|e| Reply::error(500, e))?;
                return Ok(Reply::raw(
                    "text/csv; charset=utf-8",
                    text.into_bytes(),
                    Vec::new(),
                ));
            }
            let mut doc = crate::pipeline_pack::numbers_by_key(registry.store(), r.id)
                .map_err(|e| Reply::error(500, e))?;
            doc["pipeline_id"] = json!(r.pipeline_id);
            Ok(Reply::ok(doc))
        })(),
        ["api", "pipeline-runs"] => (|| {
            let pipeline = query
                .get("pipeline")
                .map(|p| p.parse::<i64>())
                .transpose()
                .map_err(|_| Reply::error(400, "pipeline is a pipeline's id"))?;
            let limit = query
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(50)
                .clamp(1, RUNS_PAGE_MAX);
            let list = rows::runs(registry.store(), pipeline, limit).map_err(err)?;
            let mut docs = runs_docs(registry.store(), &list);
            // record 48, D1 of the move: no unit's QC of a sealed stack
            for (doc, r) in docs.iter_mut().zip(&list) {
                shown_unsealed(
                    registry,
                    caller,
                    "GET /api/pipeline-runs",
                    r.id,
                    &mut doc["summary"],
                )?;
            }
            if !quasi {
                docs.iter_mut().for_each(run_totals_only);
            }
            Ok(Reply::ok(json!({ "runs": docs, "limit": limit })))
        })(),
        ["api", "pipeline-runs", id] => (|| {
            let id = id_of(id, "run")?;
            let r = rows::run(registry.store(), id)
                .map_err(err)?
                .ok_or_else(|| Reply::error(404, format!("no pipeline run {id}")))?;
            let mut doc =
                run_doc_at(registry.store(), &r, quasi).map_err(|e| Reply::error(500, e))?;
            shown_unsealed(
                registry,
                caller,
                "GET /api/pipeline-runs/{id}",
                r.id,
                &mut doc["summary"],
            )?;
            // 2026-10-12: nor a unit's own words of a stack of such a sample
            if !crate::sealed::reads(&caller.access) {
                units_run_unsealed(registry.store(), r.id, &mut doc)
                    .map_err(|e| Reply::error(500, e))?;
            }
            if !quasi {
                run_totals_only(&mut doc);
            }
            Ok(Reply::ok(doc))
        })(),
        // record 55 H2, round 5 (A23): what a run's containers printed
        ["api", "pipeline-runs", id, "log"] => (|| {
            let id = id_of(id, "run")?;
            run_log(registry, caller, id, None, query)
        })(),
        ["api", "pipeline-runs", id, "units", unit, "log"] => (|| {
            let id = id_of(id, "run")?;
            run_log(registry, caller, id, Some(unit), query)
        })(),
        // gate T31, step 15.8: what a run suggests curating
        ["api", "pipeline-runs", id, "seeds"] => (|| {
            let id = id_of(id, "run")?;
            seeds_answer(registry, caller, id)
        })(),
        _ => return None,
    })
}

/// A run's log, or one unit's, as `GET /api/pipeline-runs/{id}/log` and
/// `GET /api/pipeline-runs/{id}/units/{unit}/log` read it (record 55 H2,
/// round 5): a slice of the `log.txt` its containers wrote in its scratch
/// place. A unit of a run whose units ran together reads the run's one log,
/// `shared`; a run whose units ran apart keeps one a unit and none of its
/// own. A log that holds the words of a unit with a stack of a sample sealed
/// now is read only with sealed:see, and that read is audited as the run's
/// QC of such units is.
fn run_log(
    registry: &mut Registry,
    caller: &Caller,
    run_id: i64,
    unit: Option<&str>,
    query: &std::collections::HashMap<String, String>,
) -> Result<Reply, Reply> {
    let gone = |e: String| Reply::error(500, e);
    let r = rows::run(registry.store(), run_id)
        .map_err(|e| gone(e.to_string()))?
        .ok_or_else(|| Reply::error(404, format!("no pipeline run {run_id}")))?;
    let units = rows::units(registry.store(), run_id).map_err(|e| gone(e.to_string()))?;
    let names: Vec<String> = units.iter().map(|u| u.unit.clone()).collect();
    let apart = r.units.as_deref() == Some("apart");
    let (rel, shared, read): (PathBuf, bool, Vec<String>) = match unit {
        None if apart => {
            return Err(Reply::error(
                404,
                format!(
                    "run {run_id}'s units ran apart, each with a log of its own: read a unit's at /api/pipeline-runs/{run_id}/units/<unit>/log"
                ),
            ));
        }
        None => (PathBuf::from("log.txt"), false, names),
        Some(u) => {
            if !names.iter().any(|n| n == u) {
                return Err(Reply::error(404, format!("run {run_id} has no unit {u}")));
            }
            folder_word(u).map_err(|e| Reply::error(404, e))?;
            if apart {
                (
                    Path::new("units").join(u).join("log.txt"),
                    false,
                    vec![u.to_string()],
                )
            } else {
                (PathBuf::from("log.txt"), true, names)
            }
        }
    };
    let door = match unit {
        None => "GET /api/pipeline-runs/{id}/log",
        Some(_) => "GET /api/pipeline-runs/{id}/units/{unit}/log",
    };
    // record 48, D1 of the move: no unit's words of a sealed stack
    let sealed = sealed_units(registry.store(), run_id, &read).map_err(gone)?;
    if !sealed.is_empty() {
        if !crate::sealed::reads(&caller.access) {
            return Err(Reply::error(
                403,
                format!(
                    "this log holds the words of a unit with a stack of a sample sealed for certification; it is read only with {}",
                    crate::grants::UNSEALED
                ),
            ));
        }
        audit_sealed_run(registry, &caller.principal, door, run_id, sealed.len())?;
    }
    // its scratch where it kept it, as a run taken up again finds it
    let place = r
        .scratch_place_id
        .or(r.place_id)
        .and_then(|id| {
            nils_registry::place::show(registry.store(), id)
                .ok()
                .flatten()
        })
        .ok_or_else(|| {
            Reply::error(
                404,
                format!("run {run_id}'s working place is gone, and its log with it"),
            )
        })?;
    let file = Path::new(&place.path)
        .join(RUNS)
        .join(run_id.to_string())
        .join(&rel);
    let (from, limit) = crate::logs::asked(query).map_err(|e| Reply::error(400, e))?;
    match crate::logs::read(&file, from, limit) {
        Ok(mut doc) => {
            doc["run"] = json!(run_id);
            doc["unit"] = json!(unit);
            doc["shared"] = json!(shared);
            doc["over"] = json!(r.status != "running");
            Ok(Reply::ok(doc))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Reply::error(
            404,
            match unit {
                Some(u) if r.status == "running" => {
                    format!("unit {u} of run {run_id} has written no log yet: it has not started")
                }
                Some(u) => format!(
                    "unit {u} of run {run_id} kept no log: it never ran, or its log was cleared with the run's scratch"
                ),
                None => format!(
                    "run {run_id} kept no log: it never started its container, or its log was cleared with its scratch"
                ),
            },
        )),
        Err(e) => Err(gone(format!("run {run_id}'s log could not be read: {e}"))),
    }
}

/// A `run` a door queues: the pipeline and the flags a caller may give,
/// and the deployment's pack directory; never a path a caller composes.
pub(crate) fn located(pack_dir: Option<&Path>, command: Vec<String>) -> Result<Vec<String>, Reply> {
    let mut out = vec!["run".to_string()];
    let mut named = 0;
    let mut resume = false;
    let mut it = command.into_iter().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--select" | "--handle" | "--param" | "--model" | "--labels" | "--pack"
            | "--threshold" | "--resume" | "--goal" | "--switch" => {
                resume |= arg == "--resume";
                let value = it
                    .next()
                    .ok_or_else(|| Reply::error(400, format!("run {arg} takes a value")))?;
                out.push(arg);
                out.push(value);
            }
            // record 57 P2: the person lets a unit run whose hard need the
            // registry cannot tell
            "--let-run" => out.push(arg),
            // Wave 7b: the parts a run over a handle is cut into, a whole
            // number of stacks, as `nils run --chunk` takes it
            "--chunk" => {
                let value = it
                    .next()
                    .filter(|v| v.parse::<u64>().is_ok_and(|n| n > 0))
                    .ok_or_else(|| {
                        Reply::error(400, "run --chunk takes a whole number of stacks, above 0")
                    })?;
                out.push(arg);
                out.push(value);
            }
            a if a.starts_with('-') => {
                return Err(Reply::error(
                    400,
                    format!(
                        "run takes --select, --handle, --param, --model, --labels, --threshold, --pack, --resume, --goal, --switch, --chunk and --let-run, not {a}"
                    ),
                ));
            }
            _ => {
                named += 1;
                if named > 1 {
                    return Err(Reply::error(400, "run names one pipeline"));
                }
                out.push(arg);
            }
        }
    }
    if named == 0 && !resume {
        return Err(Reply::error(400, "run <pipeline>: the pipeline to run"));
    }
    if let Some(d) = pack_dir {
        out.push("--pack-dir".into());
        out.push(d.display().to_string());
    }
    Ok(out)
}

/// The stacks a part of a run holds for each GB of memory a unit of its
/// pipeline declares, where the arrival score, a step's door or System 1's
/// archive door queues the run over more (2026-10-17). A header model's units run together, in one
/// container that reads every stack's rows at once: System 1's scored
/// 5,220 stacks in a container of 4 GB (1,305 for each GB) and was killed
/// for memory over 456,110, so a part holds no more for each GB than a run
/// was seen to fit.
pub(crate) const PART_STACKS_PER_GB: f64 = 1_250.0;

/// A part's size the tests set, so a small batch is split; nothing else
/// sets it.
const PART_STACKS_VAR: &str = "NILS_TEST_PART_STACKS";

/// The most stacks one run of a pipeline is given when the arrival score
/// or a door queues it over a frozen handle ([`PART_STACKS_PER_GB`] for
/// each GB a unit declares, at least one GB's worth), or none where its
/// units run apart, a container each, which no number of stacks overfills.
pub(crate) fn part_stacks(d: &Descriptor) -> Option<u64> {
    let set = std::env::var(PART_STACKS_VAR)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0);
    part_stacks_of(d, set)
}

/// [`part_stacks`], with the size the tests set.
fn part_stacks_of(d: &Descriptor, set: Option<u64>) -> Option<u64> {
    if d.units == Units::Apart {
        return None;
    }
    // a whole number of stacks, never below one GB's worth
    Some(set.unwrap_or((d.needs.memory_gb.max(1.0) * PART_STACKS_PER_GB).floor() as u64))
}

/// A `run` over a handle of `stacks` stacks as the arrival score, a step's
/// door and System 1's archive door queue it (2026-10-17): with `--chunk`
/// where they are more than a part of its pipeline holds
/// ([`part_stacks`]), so each part is a run of its own, one after another,
/// in one job.
pub(crate) fn in_parts(store: &mut Store, command: Vec<String>, stacks: usize) -> Vec<String> {
    let part = command
        .get(1)
        .and_then(|name| rows::resolve(store, name).ok().flatten())
        .and_then(|p| descriptor::from_value(p.descriptor).ok())
        .and_then(|d| part_stacks(&d));
    with_chunk(command, stacks, part)
}

/// `command` with `--chunk <part>` where `stacks` are more than `part`.
fn with_chunk(mut command: Vec<String>, stacks: usize, part: Option<u64>) -> Vec<String> {
    if let Some(n) = part
        && u64::try_from(stacks).is_ok_and(|s| s > n)
    {
        command.extend(["--chunk".to_string(), n.to_string()]);
    }
    command
}

// --------------------------------------------------------- the command line

/// `nils pipeline`: the catalog and the runtime.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum PipelineCommand {
    /// Add a descriptor (nils.job.yml, contracts/job/v1) to the catalog, as
    /// its name's next version; an image not pinned by its registry
    /// manifest digest is refused
    Add {
        /// The descriptor; with --release, the name of a pipeline the
        /// release ships
        #[arg(value_name = "FILE|NAME")]
        file: PathBuf,
        /// Take the descriptor from an engine release's pipelines.tar.gz,
        /// checked against the release's SHA256SUMS: this engine's own
        /// release, or --release=<version>. A release's descriptors name
        /// the images it built, by the digests it pushed
        #[arg(long, value_name = "VERSION", num_args = 0..=1, require_equals = true, default_missing_value = "")]
        release: Option<String>,
        /// Where releases come from, with --release; NILS_RELEASES sets
        /// the same thing
        #[arg(long, value_name = "URL", requires = "release")]
        channel: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Record 57 P1: check a pipeline's folder offline, its nils.job.yml and
    /// the nils-pack.yaml beside it, with the loader's words; prints the
    /// descriptor's digest the pack must pin
    Validate {
        /// The folder, or its nils.job.yml
        dir: PathBuf,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Record 57 P10 (Wave 7b, section 10): replay a pipeline folder's pack
    /// over this registry, read-only, before it ships: over every session
    /// the registry's main-scan picks make, how many change from run to
    /// skipped and why, which variant they are routed to, which knob values
    /// change and from what to what, against the pack the catalog runs
    /// today; and which kept results the new pack would make stale
    Replay {
        /// The folder, or its nils.job.yml, with the nils-pack.yaml beside it
        dir: PathBuf,
        /// The catalog version replayed against: a name (its newest version),
        /// name@version or an id; the folder's name when not given
        #[arg(long, value_name = "PIPELINE")]
        against: Option<String>,
        /// The goal the sessions are judged under (the pack's default when
        /// not given)
        #[arg(long, value_name = "GOAL")]
        goal: Option<String>,
        /// A switch of the pack, name=value; may repeat
        #[arg(long = "switch", value_name = "NAME=VALUE")]
        switches: Vec<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The catalog, by name and version, and whether pipelines can run here
    List {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// One pipeline: its image, parameters, inputs, outputs and needs
    Show {
        /// Its id, name@version, or a name for its newest version
        pipeline: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The container runtime pipelines run under: what was found, or the
    /// operator's choice. Docker is taken only when chosen here (D18)
    Runtime {
        /// auto (rootless podman, then apptainer), apptainer-first (apptainer,
        /// then podman), podman, apptainer, docker or off
        #[arg(long, value_name = "CHOICE")]
        set: Option<String>,
        /// How apptainer keeps an image it builds from the pinned digest:
        /// sif, or sandbox where there is no /dev/fuse (record 49 A2)
        #[arg(long, value_name = "FORM")]
        apptainer_image: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The pipeline lane (record 49): the cores and memory its units run
    /// within together, and the card a GPU unit leases
    Lane {
        /// The cores the lane's units hold together (48 until set)
        #[arg(long)]
        cores: Option<u32>,
        /// The GB of memory they hold together (512 until set); never more
        /// than this machine or its container offers
        #[arg(long)]
        memory_gb: Option<u64>,
        /// The card a GPU unit leases, by index, or none for no card (0
        /// until set)
        #[arg(long, value_name = "CARD")]
        gpu_card: Option<String>,
        /// The working place a run's outputs go to, the derivatives and
        /// tables it registers, by name; default for the first working
        /// place (record 49 R7)
        #[arg(long, value_name = "PLACE")]
        output_place: Option<String>,
        /// The working place a run's scratch goes to, its input, each
        /// unit's folder, its log and apptainer's images, by name; default
        /// for the first working place
        #[arg(long, value_name = "PLACE")]
        scratch_place: Option<String>,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Secret inputs (record 49 R3): a file the site keeps, such as a
    /// licence, that a pipeline declaring it reads at run time
    Secret {
        #[command(subcommand)]
        command: SecretCommand,
    },
    /// A run's seeds (record 43): the values it suggests a person curate,
    /// and the stacks it suggests curating, which --save keeps as a
    /// selection a campaign starts from (nils campaign create --select)
    Seeds {
        /// The run, by id
        run: i64,
        /// Save the suggested stacks as the next version of this selection
        #[arg(long, value_name = "NAME")]
        save: Option<String>,
        #[arg(long, value_name = "DIR")]
        pack_dir: Option<PathBuf>,
        #[arg(long, default_value = "mri")]
        pack: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The starter catalog (record 49 A4): the analyses the engine seeds at
    /// its start, and what the catalog holds of each
    Starter {
        /// Seed what the catalog lacks now, as the engine does at its start
        #[arg(long)]
        seed: bool,
        /// Turn the seeding at the engine's start off
        #[arg(long, conflicts_with = "on")]
        off: bool,
        /// Turn it back on
        #[arg(long)]
        on: bool,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Take in a finished run's proposals (record 43 S6) as its intake
    /// does: read again from the results.json files it left, grouped into
    /// review items and staged at each model's threshold. For a run whose
    /// intake took none, as one with a failed unit did before
    /// 1.0.0-alpha.83. Taking in twice takes nothing the second time, and
    /// says so. Through a door this is pipelines:work, as a run is
    Ingest {
        /// The run, by id
        run: i64,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The runs, newest first, or one run with its derivatives
    Runs {
        /// One run, by id
        id: Option<i64>,
        /// Only this pipeline's runs: its id, name@version or name
        #[arg(long, value_name = "PIPELINE")]
        pipeline: Option<String>,
        /// At most this many
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// `nils pipeline secret`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum SecretCommand {
    /// Set the file a secret is read from; the engine keeps its path and
    /// never its bytes
    Set {
        /// The secret's id, as a descriptor declares it
        id: String,
        /// The file, an absolute path readable by the engine
        #[arg(long)]
        file: PathBuf,
    },
    /// Forget a secret's file
    Unset { id: String },
    /// The secrets set, by id, and whether each file can be read now
    List {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// `nils run`: one pipeline over a frozen selection.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct RunArgs {
    /// Wave 7b section 7: set by a family's run on the member's run it
    /// starts, never from the command line; the runner records the family,
    /// the member and the rule that chose on every unit.
    #[arg(skip)]
    pub(crate) family: Option<crate::pipeline_family::Route>,
    /// The pipeline: its id, name@version, or a name for its newest version
    #[arg(required_unless_present = "resume")]
    pub(crate) pipeline: Option<String>,
    /// Take up again a run that stopped, was cancelled or whose engine went
    /// away (record 49 A1): the units it finished are kept, and those in
    /// flight run again, with everything the run recorded; a part of a run
    /// split with --chunk then goes on with the parts after it that no run
    /// holds yet
    #[arg(
        long,
        value_name = "RUN",
        conflicts_with_all = ["pipeline", "select", "handle", "params", "models", "labels", "threshold"]
    )]
    pub(crate) resume: Option<i64>,
    /// The selection to run over, frozen now into its stacks and pinned; or
    /// a dataset's or a cohort's main scans of a role, resolved now and
    /// frozen the same way: main:ROLE@dataset:NAME or main:ROLE@cohort:NAME;
    /// or, without a role, the main scans of each role the pipeline needs,
    /// a visit taken where it has every one: main@dataset:NAME
    #[arg(
        long,
        value_name = "selection:NAME@V|main:ROLE@dataset:NAME|main@dataset:NAME",
        conflicts_with = "handle"
    )]
    pub(crate) select: Option<String>,
    /// A frozen list of stacks to run over, by handle id, pinned
    #[arg(long, value_name = "ID")]
    pub(crate) handle: Option<i64>,
    /// A parameter, as id=value (repeatable); the rest take their defaults
    #[arg(long = "param", value_name = "ID=VALUE")]
    pub(crate) params: Vec<String>,
    /// A registered model the pipeline reads: its id, digest or name@version (repeatable, in the order the descriptor declares its model inputs)
    #[arg(long = "model", value_name = "MODEL")]
    pub(crate) models: Vec<String>,
    /// A label set the pipeline reads, by id
    #[arg(long = "labels", value_name = "ID")]
    pub(crate) labels: Option<i64>,
    /// Stage the run's proposals at this probability or above: a model's
    /// card sets the threshold, and a run may raise it, never lower it
    #[arg(long, value_name = "P")]
    pub(crate) threshold: Option<f64>,
    /// The pack a selection is frozen and a bids input released under
    #[arg(long, default_value = "mri")]
    pub(crate) pack: String,
    #[arg(long, value_name = "DIR")]
    pub(crate) pack_dir: Option<PathBuf>,
    /// The DICOM to NIfTI converter a bids input is released with
    #[arg(long, default_value = "dcm2niix", value_name = "PATH")]
    pub(crate) dcm2niix: PathBuf,
    /// Split the selection, or the handle, into parts of at most N stacks,
    /// in stack order, and run each part as a run of its own over a handle
    /// of its own, one after another: for a selection too large for one
    /// container's input. A part that fails or is stopped stops the parts
    /// after it; its run taken up again (--resume) goes on with them
    #[arg(long, value_name = "N", conflicts_with = "resume")]
    pub(crate) chunk: Option<u64>,
    /// At most N parts of a split run at once (wave 7b): by default, where
    /// a part's units run together in one container on the CPU, as many as
    /// the lane's budget holds of the cores and memory a part declares; 1
    /// runs them one after another
    #[arg(long, value_name = "N")]
    pub(crate) parts_at_once: Option<usize>,
    /// Run nothing: say what the run would do (record 49 A3), its units,
    /// the units that cannot run and what they lack, the time, the GPU, and
    /// what a unit needs against the lane's budget; with --resume, what a
    /// run taken up again would do, of the units it still has to run
    #[arg(long)]
    pub(crate) preflight: bool,
    /// Run a version a later one replaces because it fixes it, named by
    /// name@version or its id (2026-10-12): a new run is otherwise refused
    /// and the doors never take it
    #[arg(long, conflicts_with = "resume")]
    pub(crate) replaced: bool,
    /// Record 57 P2: the goal of the run, where the pipeline has a pack
    /// (fast, standard or research; standard when not given)
    #[arg(long, value_name = "GOAL")]
    pub(crate) goal: Option<String>,
    /// Record 57 P2: a switch of the pipeline's pack, name=value
    #[arg(long = "switch", value_name = "NAME=VALUE")]
    pub(crate) switches: Vec<String>,
    /// Record 57 P2: run a unit whose hard need the registry cannot tell,
    /// recorded as the person's choice
    #[arg(long)]
    pub(crate) let_run: bool,
    /// Machine-readable output
    #[arg(long)]
    pub(crate) json: bool,
    /// The parts of a split run still to run after one of them was taken
    /// up again; never on the command line
    #[arg(skip)]
    pub(crate) go_on: Option<GoOn>,
    /// Wave 7b A7 (section 8.4): the step of a workflow run this run is,
    /// and the runs of the steps it waits for per unit; never on the
    /// command line
    #[arg(skip)]
    pub(crate) chain: Option<Chain>,
}

impl RunArgs {
    /// The pack directory the run reads, as given or the deployment's.
    pub(crate) fn pack_dir_or_default(&self, home: &Home) -> Option<PathBuf> {
        self.pack_dir
            .clone()
            .or_else(|| crate::pack_dir(home, None).ok())
            .map(|dir| dir.join(&self.pack))
    }
}

/// Wave 7b A7 (section 8.4): what makes a run a step of a workflow run. A
/// unit of this run starts when the same unit of every run in `after` is
/// over and made a result; a unit skipped or failed there is skipped here
/// with the same reason, and a unit the run before never had is skipped
/// too.
#[derive(Debug, Clone, Default)]
pub(crate) struct Chain {
    pub(crate) workflow_run: i64,
    pub(crate) step: usize,
    pub(crate) after: Vec<i64>,
    /// After review (finding 3 of the second review): what each link into
    /// this step binds, carried from the plan into the run's resolution of
    /// its inputs.
    pub(crate) links: Vec<ChainLink>,
    /// After the third review (finding 9): the optional typed inputs whose
    /// link comes from a step that ended without a run: each takes nothing,
    /// and every unit records why under `fed`.
    pub(crate) unfed: Vec<Unfed>,
}

/// An optional typed input a link feeds from a step that fed nothing.
#[derive(Debug, Clone)]
pub(crate) struct Unfed {
    pub(crate) input: String,
    pub(crate) words: String,
}

/// What the chain says fed nothing for a unit, by input (after the third
/// review, finding 9): the step-level words of every unfed link, and the
/// gate's per-unit notes where the unit before failed or was skipped at a
/// step whose link feeds an optional input.
fn fed_notes(chain: Option<&Chain>, notes: Option<&Vec<(String, String)>>) -> Value {
    let mut fed = serde_json::Map::new();
    for u in chain.into_iter().flat_map(|c| c.unfed.iter()) {
        fed.insert(u.input.clone(), json!(u.words));
    }
    for (input, words) in notes.into_iter().flatten() {
        fed.insert(input.clone(), json!(words));
    }
    if fed.is_empty() {
        Value::Null
    } else {
        Value::Object(fed)
    }
}

/// One link of the workflow into this step, bound to the run of the step
/// it comes from: a typed derivative input takes that run's derivatives of
/// the unit's stacks and no other run's; a pick role takes that run's
/// image as the role's file for the unit. A unit the binding cannot be
/// made for is skipped with the link's words.
#[derive(Debug, Clone)]
pub(crate) struct ChainLink {
    /// The step the link comes from, by its place in the document.
    pub(crate) from_step: usize,
    /// The step's id in the document.
    pub(crate) from_id: String,
    /// That step's pipeline run in this workflow run.
    pub(crate) run: i64,
    pub(crate) output: String,
    /// The input of this step: a typed derivative input's id, or a role.
    pub(crate) input: String,
    /// Whether `input` is a pick role (else a typed derivative input).
    pub(crate) role: bool,
}

impl ChainLink {
    /// `<from>.<output> -> <to>.<input>`, as the plan words it.
    pub(crate) fn words(&self, to: &str) -> String {
        format!("{}.{} -> {to}.{}", self.from_id, self.output, self.input)
    }

    fn doc(&self) -> Value {
        json!({
            "from_step": self.from_step, "from": self.from_id, "run": self.run,
            "output": self.output, "input": self.input,
            "binds": if self.role { "role" } else { "typed" },
        })
    }
}

/// A link bound to what the fed step's run made: its derivatives, and,
/// where a unit of that run reused a kept result, the run it reused from,
/// by the unit's stacks (a reused unit made no file of its own; the file
/// it stands for is its source run's). No other run's file is taken.
#[derive(Debug, Clone)]
pub(crate) struct Fed {
    pub(crate) link: ChainLink,
    /// The fed run and every run its units reused from.
    runs: BTreeSet<i64>,
    /// By stack, the run that made the fed step's result for it: the fed
    /// run, or the run its unit reused from.
    by_stack: BTreeMap<i64, i64>,
    /// After the third review (finding 4): a binding the plan knows before
    /// any run exists, the fed step's declared output; the pre-flight reads
    /// such an input as fed by the step, never as missing.
    planned: bool,
}

impl Fed {
    /// The plan's binding of a typed input to the step that will feed it:
    /// no run yet.
    pub(crate) fn planned(from_step: usize, from_id: &str, output: &str, input: &str) -> Fed {
        Fed {
            link: ChainLink {
                from_step,
                from_id: from_id.to_string(),
                run: 0,
                output: output.to_string(),
                input: input.to_string(),
                role: false,
            },
            runs: BTreeSet::new(),
            by_stack: BTreeMap::new(),
            planned: true,
        }
    }

    /// Whether this is the plan's binding, with no run behind it.
    pub(crate) fn is_planned(&self) -> bool {
        self.planned
    }

    /// `fed by step <X>`: how the pre-flight words a planned binding.
    pub(crate) fn planned_words(&self) -> String {
        format!("fed by step {}", self.link.from_id)
    }

    fn of(store: &mut Store, link: &ChainLink) -> Result<Fed, String> {
        let mut runs: BTreeSet<i64> = BTreeSet::from([link.run]);
        let mut by_stack: BTreeMap<i64, i64> = BTreeMap::new();
        let stacks: BTreeMap<String, Vec<i64>> =
            unit_stacks(store, link.run)?.into_iter().collect();
        for u in rows::units(store, link.run).map_err(|e| e.to_string())? {
            let source = u.reused_from_run.unwrap_or(link.run);
            runs.insert(source);
            for s in stacks.get(&u.unit).into_iter().flatten() {
                by_stack.insert(*s, source);
            }
        }
        Ok(Fed {
            link: link.clone(),
            runs,
            by_stack,
            planned: false,
        })
    }

    /// Whether a registered derivative is one the link feeds: of a stack,
    /// the run that made the fed step's result for that stack; of a
    /// session or a subject, any run the fed step's units stand for. A
    /// planned binding has no run, and takes nothing.
    fn takes(&self, r: &derivative::Derivative) -> bool {
        if self.planned {
            return false;
        }
        let Some(run) = r.run_id else {
            return false;
        };
        match r.stack_id {
            Some(s) => self.by_stack.get(&s).copied().unwrap_or(self.link.run) == run,
            None => self.runs.contains(&run),
        }
    }

    /// The runs this link feeds from, in words.
    fn words(&self) -> String {
        let reused: Vec<String> = self
            .runs
            .iter()
            .filter(|r| **r != self.link.run)
            .map(i64::to_string)
            .collect();
        if reused.is_empty() {
            format!("run {} of {}", self.link.run, self.link.from_id)
        } else {
            format!(
                "run {} of {} (reusing run {})",
                self.link.run,
                self.link.from_id,
                reused.join(", ")
            )
        }
    }
}

/// The links of a run's chain by the typed input each feeds (after review,
/// finding 3): what the run's derivative inputs are bound to.
pub(crate) type Bound = BTreeMap<String, Fed>;

/// The typed inputs a chain binds, and the roles it binds, of this
/// descriptor, each with what the fed run made.
fn bound_of(
    store: &mut Store,
    chain: Option<&Chain>,
    d: &Descriptor,
) -> Result<(Bound, Vec<Fed>), String> {
    let mut typed = Bound::new();
    let mut roles = Vec::new();
    for l in chain.into_iter().flat_map(|c| c.links.iter()) {
        if !l.role && d.inputs.iter().any(|t| t.id == l.input) {
            typed.insert(l.input.clone(), Fed::of(store, l)?);
        } else if l.role && d.roles.contains(&l.input) {
            roles.push(Fed::of(store, l)?);
        }
    }
    Ok((typed, roles))
}

/// The parts a run split with `--chunk` was made of, as each part's job
/// records them (`part`), and those still to run when one of them is taken
/// up again (2026-10-17): every part's handle, in order, and the size.
#[derive(Debug, Clone, Default)]
pub(crate) struct GoOn {
    /// Every part's handle, in order.
    handles: Vec<i64>,
    /// The parts to run, by their place in `handles`.
    next: Vec<usize>,
    /// The `--chunk` the parts were made with.
    chunk: Option<u64>,
    /// The parameters every part runs with, as the part taken up recorded
    /// them.
    params: serde_json::Map<String, Value>,
    /// The selection the parts were frozen from, where they were.
    selection: Option<String>,
    /// The worker's job the part taken up again kept running for them.
    kept: Option<i64>,
}

impl GoOn {
    /// The parts a part's job recorded, read back: none for a run that is
    /// no part of a split one.
    fn recorded(part: &Value) -> Option<GoOn> {
        let handles: Vec<i64> = part["handles"]
            .as_array()?
            .iter()
            .filter_map(Value::as_i64)
            .collect();
        (handles.len() > 1).then(|| GoOn {
            handles,
            next: Vec::new(),
            chunk: part["chunk"].as_u64(),
            params: serde_json::Map::new(),
            selection: None,
            kept: None,
        })
    }

    /// What a part's job records of the parts: which it is, of how many,
    /// the size and every part's handle.
    fn part(&self, i: usize) -> Value {
        json!({
            "part": i + 1, "of": self.handles.len(), "chunk": self.chunk,
            "handles": self.handles,
        })
    }

    /// The handles of these parts, as words.
    fn words(&self, parts: &[usize]) -> String {
        parts
            .iter()
            .map(|i| self.handles[*i].to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(crate) fn print_json(v: &Value) -> Result<(), Exit> {
    print(v)
}

fn print(v: &Value) -> Result<(), Exit> {
    println!(
        "{}",
        serde_json::to_string_pretty(v).map_err(|e| fail(format!("will not serialize: {e}")))?
    );
    Ok(())
}

/// `nils pipelines validate <dir>`: offline, it opens no registry and needs
/// no install on this machine.
fn validate(dir: PathBuf, json: bool) -> Result<(), Exit> {
    let dir = if dir.is_dir() {
        dir
    } else {
        dir.parent().map(Path::to_path_buf).unwrap_or(dir)
    };
    let (d, pack) = nils_pipeline::pack::load_dir(&dir, None).map_err(usage)?;
    let pack_digest = pack.as_ref().map(nils_pipeline::pack::PipelinePack::digest);
    let doc = json!({
        "folder": dir.display().to_string(),
        "pipeline": d.name,
        "descriptor_digest": d.digest(),
        "pack": pack.as_ref().map_or(json!("none"), |p| json!({
            "digest": pack_digest, "version": p.version, "rules": p.rules.len(),
            "needs": p.needs.keys().cloned().collect::<Vec<_>>(), "checks": p.checks.len(),
            "breaking_knobs": p.breaking_knobs(),
        })),
        "version_digest": nils_pipeline::pack::version_digest(&d.digest(), pack_digest.as_deref()),
        "ok": true,
    });
    if json {
        print(&doc)?;
    } else {
        println!("{}: descriptor {}", d.name, d.digest());
        match &pack {
            Some(p) => println!(
                "pack {}: {} rules, {} checks, needs on {}; version {}",
                pack_digest.clone().unwrap_or_default(),
                p.rules.len(),
                p.checks.len(),
                if p.needs.is_empty() {
                    "nothing".to_string()
                } else {
                    p.needs.keys().cloned().collect::<Vec<_>>().join(", ")
                },
                doc["version_digest"].as_str().unwrap_or_default()
            ),
            None => println!(
                "pack: none ({} is not beside it); version {}",
                nils_pipeline::pack::FILE,
                doc["version_digest"].as_str().unwrap_or_default()
            ),
        }
    }
    Ok(())
}

pub(crate) fn command(home: &Home, command: PipelineCommand) -> Result<(), Exit> {
    // validate is offline: it matches before the registry is opened
    if let PipelineCommand::Validate { dir, json } = &command
        && nils_pipeline::pack::family::is_family_dir(dir)
    {
        let doc = crate::pipeline_family::validate(dir)?;
        if *json {
            return print(&doc);
        }
        println!(
            "family {}: digest {}, {} rules, tasks {}",
            doc["family"].as_str().unwrap_or_default(),
            doc["digest"].as_str().unwrap_or_default(),
            doc["rules"].as_u64().unwrap_or_default(),
            doc["tasks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for m in doc["members"].as_array().into_iter().flatten() {
            println!(
                "  {} ({}, {}): descriptor {}, pack {}",
                m["member"].as_str().unwrap_or_default(),
                m["pipeline"].as_str().unwrap_or_default(),
                m["level"].as_str().unwrap_or_default(),
                m["descriptor_digest"].as_str().unwrap_or_default(),
                m["pack"]["digest"].as_str().unwrap_or("none")
            );
        }
        return Ok(());
    }
    if let PipelineCommand::Validate { dir, json } = command {
        return validate(dir, json);
    }
    let mut registry = crate::open(home)?;
    match command {
        PipelineCommand::Validate { .. } => unreachable!("matched before the registry opens"),
        PipelineCommand::Add {
            file,
            release,
            channel,
            json,
        } => {
            // Wave 7b section 7: a folder with a family document adds every
            // member's pair, then the family
            if release.is_none() && nils_pipeline::pack::family::is_family_dir(&file) {
                let v = crate::pipeline_family::add_folder(&mut registry, &file)?;
                if json {
                    return print(&v);
                }
                println!(
                    "family {} ({}): {}",
                    v["name"].as_str().unwrap_or_default(),
                    v["digest"]
                        .as_str()
                        .map(|d| &d[..12.min(d.len())])
                        .unwrap_or_default(),
                    if v["added"] == json!(true) {
                        "added"
                    } else {
                        "already in the catalog"
                    }
                );
                for m in v["members_added"].as_array().into_iter().flatten() {
                    println!(
                        "  {} -> {} ({})",
                        m["member"].as_str().unwrap_or_default(),
                        m["label"].as_str().unwrap_or_default(),
                        if m["added"] == json!(true) {
                            "added"
                        } else {
                            "the version it is"
                        }
                    );
                }
                return Ok(());
            }
            let (text, from) = match release.as_deref().map(str::trim) {
                None => {
                    // a folder names its nils.job.yml (record 57 P1)
                    let path = if file.is_dir() {
                        file.join(nils_pipeline::pack::DESCRIPTOR_FILE)
                    } else {
                        file.clone()
                    };
                    (
                        std::fs::read_to_string(&path)
                            .map_err(|e| usage(format!("{}: {e}", path.display())))?,
                        path.display().to_string(),
                    )
                }
                // record 55 A5: the descriptor a release ships, which names
                // the image that release built
                Some(asked) => {
                    let version = match asked.trim_start_matches('v') {
                        "" => crate::update::VERSION,
                        v => v,
                    };
                    let name = file.to_string_lossy().into_owned();
                    let base = crate::update::engine_base(channel.as_deref());
                    (
                        release_descriptor(&base, version, &name)?,
                        format!("{name} of the release {version}"),
                    )
                }
            };
            // record 57 P1: a folder, or a nils.job.yml with nils-pack.yaml
            // beside it, adds the pair
            let pack_text = match release {
                Some(_) => None,
                None => {
                    let beside = if file.is_dir() {
                        file.join(nils_pipeline::pack::FILE)
                    } else {
                        file.with_file_name(nils_pipeline::pack::FILE)
                    };
                    if beside.is_file() {
                        Some(
                            std::fs::read_to_string(&beside)
                                .map_err(|e| usage(format!("{}: {e}", beside.display())))?,
                        )
                    } else {
                        None
                    }
                }
            };
            let v = add_descriptor(&mut registry, &text, pack_text.as_deref())
                .map_err(|e| usage(format!("{from}: {}", e.message)))?;
            if json {
                return print(&v);
            }
            println!(
                "pipeline {}: {} ({}), image {}{}",
                v["id"],
                v["label"].as_str().unwrap_or_default(),
                v["descriptor_digest"].as_str().unwrap_or_default(),
                v["image"].as_str().unwrap_or_default(),
                if v["added"] == true {
                    ""
                } else {
                    "; the catalog already held this descriptor"
                }
            );
            let replaced: Vec<&str> = v["replaces"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if !replaced.is_empty() {
                println!(
                    "it replaces {}, which a new run no longer takes: {}",
                    replaced.join(", "),
                    v["replaces_why"].as_str().unwrap_or_default()
                );
            }
            Ok(())
        }
        PipelineCommand::List { json } => {
            let list = rows::list(registry.store())?;
            let cap = {
                let d = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
                let extra = lane_doc(&mut registry);
                capability_with(registry.store(), &d, Some(extra))
            };
            let (mut docs, names) = pipeline_docs(&list);
            // Wave 7b section 7: the families, each one row over its members
            let families =
                crate::pipeline_family::decorate(registry.store(), &mut docs).map_err(fail)?;
            if json {
                return print(&json!({
                    "pipelines": docs,
                    "names": names,
                    "families": families,
                    "capability": cap,
                }));
            }
            for f in &families {
                println!(
                    "family {}: {} (members {})",
                    f["name"].as_str().unwrap_or_default(),
                    f["words"].as_str().unwrap_or_default(),
                    f["members"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|m| m["label"].as_str().unwrap_or("?").to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            // 2026-10-12: each version as a new run takes it: current,
            // selectable, replaced or retired
            println!(
                "{:>5}  {:<32} {:<8} {:<11} {:<10} image",
                "id", "pipeline", "layout", "level", "state"
            );
            for (p, doc) in list.iter().zip(&docs) {
                println!(
                    "{:>5}  {:<32} {:<8} {:<11} {:<10} {}",
                    p.id,
                    p.label(),
                    p.layout,
                    p.level,
                    doc["standing"].as_str().unwrap_or(p.state.as_str()),
                    p.image
                );
            }
            for (p, doc) in list.iter().zip(&docs) {
                if doc["standing"] == "replaced" {
                    println!(
                        "{} is replaced by {}: {}",
                        p.label(),
                        doc["replaced_by"].as_str().unwrap_or_default(),
                        doc["replaced_why"].as_str().unwrap_or_default()
                    );
                }
            }
            if cap["enabled"] == true {
                println!(
                    "{} pipelines; they run under {} {} into {}",
                    list.len(),
                    cap["runtime"]["name"].as_str().unwrap_or_default(),
                    cap["runtime"]["version"].as_str().unwrap_or_default(),
                    cap["place"].as_str().unwrap_or_default()
                );
            } else {
                println!(
                    "{} pipelines; {}",
                    list.len(),
                    cap["reason"].as_str().unwrap_or("pipelines are off")
                );
            }
            Ok(())
        }
        PipelineCommand::Replay {
            dir,
            against,
            goal,
            switches,
            json,
        } => {
            let dir = if dir.is_dir() {
                dir
            } else {
                dir.parent().map(Path::to_path_buf).unwrap_or(dir)
            };
            // Wave 7b section 7: a family replayed counts per member
            if nils_pipeline::pack::family::is_family_dir(&dir) {
                if against.is_some() {
                    return Err(usage(
                        "a family is replayed against the family the registry holds of its name; --against names no other",
                    ));
                }
                let ask =
                    crate::pipeline_pack::PersonAsk::of_args(goal.as_deref(), &switches, false)
                        .map_err(usage)?;
                let doc = crate::pipeline_family::replay(&mut registry, &dir, &ask)?;
                if json {
                    return print(&doc);
                }
                println!("{}", crate::pipeline_family::replay_words(&doc));
                return Ok(());
            }
            let d_path = dir.join(nils_pipeline::pack::DESCRIPTOR_FILE);
            let d_text = std::fs::read_to_string(&d_path)
                .map_err(|e| usage(format!("{}: {e}", d_path.display())))?;
            let d = descriptor::parse(&d_text)
                .map_err(|e| usage(format!("{}: {e}", d_path.display())))?;
            let p_path = dir.join(nils_pipeline::pack::FILE);
            let p_text = std::fs::read_to_string(&p_path).map_err(|e| {
                usage(format!(
                    "{}: {e}; a replay needs the pack beside the descriptor",
                    p_path.display()
                ))
            })?;
            let pack = crate::pipeline_pack::load(registry.store(), &d, &p_text)
                .map_err(|e| usage(format!("{}: {e}", p_path.display())))?;
            let reference = against.clone().unwrap_or_else(|| d.name.clone());
            let row = rows::resolve(registry.store(), &reference)?;
            if against.is_some() && row.is_none() {
                return Err(usage(format!("no pipeline {reference} in the catalog")));
            }
            let ask = crate::pipeline_pack::PersonAsk::of_args(goal.as_deref(), &switches, false)
                .map_err(usage)?;
            let doc =
                crate::pipeline_replay::replay_folder(&mut registry, &d, &pack, row.as_ref(), &ask)
                    .map_err(fail)?;
            if json {
                return print(&doc);
            }
            println!("{}", crate::pipeline_replay::words(&doc));
            Ok(())
        }
        PipelineCommand::Show { pipeline, json } => {
            let found = rows::resolve(registry.store(), &pipeline)?;
            if found.is_none()
                && let Some(s) =
                    crate::pipeline_family::get(registry.store(), &pipeline).map_err(fail)?
            {
                let doc = crate::pipeline_family::doc(registry.store(), &s);
                if json {
                    return print(&doc);
                }
                println!(
                    "family {}: {}",
                    s.name,
                    doc["words"].as_str().unwrap_or_default()
                );
                for m in doc["members"].as_array().into_iter().flatten() {
                    println!(
                        "  {:<12} {} ({}), tasks {}",
                        m["member"].as_str().unwrap_or_default(),
                        m["label"].as_str().unwrap_or("not in the catalog"),
                        m["method"].as_str().unwrap_or_default(),
                        m["tasks"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                return Ok(());
            }
            let p = found.ok_or_else(|| usage(format!("no pipeline {pipeline} in the catalog")))?;
            let doc = pipeline_doc_of(registry.store(), &p);
            if json {
                return print(&doc);
            }
            println!("pipeline {}: {}", p.id, p.label());
            match doc["standing"].as_str() {
                Some("replaced") => println!(
                    "  standing         replaced by {}: {}",
                    doc["replaced_by"].as_str().unwrap_or_default(),
                    doc["replaced_why"].as_str().unwrap_or_default()
                ),
                Some(state) => println!("  standing         {state}"),
                None => {}
            }
            println!("  tool version     {}", p.tool_version);
            println!("  image            {}", p.image);
            println!("  descriptor       {}", p.descriptor_digest);
            println!("  input            {} layout, {} units", p.layout, p.level);
            println!(
                "  needs a GPU      {}",
                doc["needs"]["gpu"].as_str().unwrap_or("none")
            );
            for param in doc["parameters"].as_array().into_iter().flatten() {
                println!(
                    "  parameter        {} ({}), default {}",
                    param["id"].as_str().unwrap_or_default(),
                    param["type"].as_str().unwrap_or_default(),
                    param["default-value"]
                );
            }
            for o in doc["outputs"].as_array().into_iter().flatten() {
                println!(
                    "  output           {}: {} at {}",
                    o["id"].as_str().unwrap_or_default(),
                    nils_registry::a_or_an(o["kind"].as_str().unwrap_or_default()),
                    o["path-template"].as_str().unwrap_or_default()
                );
            }
            println!("  added by         {} at {}", p.added_by, p.added_at);
            Ok(())
        }
        PipelineCommand::Runtime {
            set,
            apptainer_image,
            json,
        } => {
            if let Some(word) = apptainer_image {
                let form = ImageForm::parse(&word).ok_or_else(|| {
                    usage(format!(
                        "--apptainer-image is one of {}, not {word}",
                        ImageForm::WORDS.join(", ")
                    ))
                })?;
                registry.set_meta(APPTAINER_IMAGE_KEY, form.name())?;
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineRuntime,
                    json!({"apptainer_image": form.name()}),
                    None,
                )?;
            }
            if let Some(word) = set {
                let c = Choice::parse(&word).ok_or_else(|| {
                    usage(format!(
                        "--set is one of {}, not {word}",
                        Choice::WORDS.join(", ")
                    ))
                })?;
                registry.set_meta(RUNTIME_KEY, c.name())?;
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineRuntime,
                    json!({"choice": c.name()}),
                    None,
                )?;
            }
            let d = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
            let extra = lane_doc(&mut registry);
            let cap = capability_with(registry.store(), &d, Some(extra));
            if json {
                return print(&cap);
            }
            println!("choice   {}", d.choice.name());
            for (k, s) in &d.looked {
                println!("  {k:<10} {s}");
            }
            match &d.runtime {
                Some(r) => println!(
                    "runtime  {} {}{}",
                    r.kind.name(),
                    r.version,
                    r.gpu
                        .as_deref()
                        .map(|g| format!(", GPU {g}"))
                        .unwrap_or_else(|| ", no GPU".into())
                ),
                None => println!("runtime  none: {}", d.reason.as_deref().unwrap_or("")),
            }
            if cap["enabled"] != true
                && let Some(r) = cap["reason"].as_str()
                && d.runtime.is_some()
            {
                println!("{r}");
            }
            Ok(())
        }
        PipelineCommand::Lane {
            cores,
            memory_gb,
            gpu_card,
            output_place,
            scratch_place,
            json,
        } => {
            set_lane(
                &mut registry,
                &LaneChange {
                    cores,
                    memory_gb,
                    gpu_card,
                    output_place,
                    scratch_place,
                },
            )?;
            let lane = lane_of(&mut registry);
            let places = run_places(registry.store());
            if json {
                let mut doc = lane.doc();
                doc["places"] = match &places {
                    Ok(p) => p.doc(),
                    Err(e) => json!({"error": e}),
                };
                return print(&doc);
            }
            println!(
                "lane     {} cores, {} GB of memory{}",
                lane.ledger.cores,
                lane.ledger.memory_mib / 1024,
                lane.why
                    .as_deref()
                    .map(|w| format!(" ({w})"))
                    .unwrap_or_default()
            );
            println!(
                "card     {}",
                lane.card
                    .map_or("none: GPU units do not run".to_string(), |c| format!(
                        "{c}, leased by free memory"
                    ))
            );
            match &places {
                Ok(p) => {
                    let how = |set: bool| if set { "" } else { ", the first working place" };
                    println!("output   {}{}", p.output.name, how(p.named.0));
                    println!("scratch  {}{}", p.scratch.name, how(p.named.1));
                }
                Err(e) => println!("places   {e}"),
            }
            Ok(())
        }
        PipelineCommand::Secret { command } => match command {
            SecretCommand::Set { id, file } => {
                set_secret_file(&mut registry, &id, &file)?;
                println!("secret {id} is read from its file at each run that declares it");
                Ok(())
            }
            SecretCommand::Unset { id } => {
                unset_secret(&mut registry, &id)?;
                println!("secret {id} is no longer set");
                Ok(())
            }
            SecretCommand::List { json } => {
                let ids = secret_ids(registry.store());
                let mut docs = Vec::new();
                for id in ids {
                    let readable = secret_path(&mut registry, &id)
                        .is_some_and(|p| secrets::read(&id, Path::new(&p)).is_ok());
                    docs.push(json!({"id": id, "readable": readable}));
                }
                if json {
                    return print(&json!(docs));
                }
                for d in &docs {
                    println!(
                        "{:<32} {}",
                        d["id"].as_str().unwrap_or_default(),
                        if d["readable"] == true {
                            "set, readable"
                        } else {
                            "set, NOT readable now"
                        }
                    );
                }
                Ok(())
            }
        },
        PipelineCommand::Seeds {
            run,
            save,
            pack_dir,
            pack,
            json,
        } => {
            let doc = seeds_of(registry.store(), run).map_err(usage)?;
            let mut out = json!({
                "run": run, "seeds": doc["seeds"], "selection": doc["selection"],
            });
            let mut per: BTreeMap<String, usize> = BTreeMap::new();
            for s in doc["seeds"].as_array().into_iter().flatten() {
                let key = format!(
                    "{}={}",
                    s["axis"].as_str().unwrap_or_default(),
                    s["value"].as_str().unwrap_or_default()
                );
                *per.entry(key).or_default() += 1;
            }
            out["per_value"] = json!(per);
            if let Some(name) = save {
                let stacks: Vec<i64> = doc["selection"]["stacks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                    .collect();
                if stacks.is_empty() {
                    return Err(usage(format!("run {run} suggests no stacks to curate")));
                }
                drop(registry);
                let ask = json!({
                    "ast_version": 1,
                    "sets": {"seeded": {"grain": "stack", "where": [
                        ["in", {}, ["field", {}, "id"], stacks],
                    ]}},
                    "out": {"set": "seeded", "level": "record"},
                });
                let saved = crate::ask_cli::save_selection_doc(
                    home,
                    &name,
                    &ask,
                    pack_dir,
                    &pack,
                    Some(&format!("the stacks pipeline run {run} suggests curating")),
                )?;
                out["saved"] = saved;
            }
            if json {
                return print(&out);
            }
            println!(
                "run {run}: {} seed(s), {} stack(s) suggested for curation",
                doc["seeds"].as_array().map_or(0, Vec::len),
                doc["selection"]["stacks"].as_array().map_or(0, Vec::len)
            );
            for (k, n) in &per {
                println!("  {k:<32} {n}");
            }
            if let Some(s) = out.get("saved") {
                println!(
                    "saved as selection:{}@{}; nils campaign create --select selection:{}@{} makes a campaign of it",
                    s["name"].as_str().unwrap_or_default(),
                    s["version"],
                    s["name"].as_str().unwrap_or_default(),
                    s["version"]
                );
            }
            Ok(())
        }
        PipelineCommand::Starter {
            seed,
            off,
            on,
            json,
        } => {
            if off || on {
                let word = if off { "off" } else { "on" };
                registry.set_meta(crate::starter::SETTING, word)?;
                crate::audit(
                    &mut registry,
                    nils_registry::audit::Action::PipelineRuntime,
                    json!({"starter": word}),
                    None,
                )?;
            }
            let added = if seed {
                crate::starter::seed(&mut registry).map_err(fail)?
            } else {
                Vec::new()
            };
            let enabled = crate::starter::enabled(&mut registry);
            let list = crate::starter::list(&mut registry).map_err(fail)?;
            if json {
                return print(&json!({
                    "seeding": if enabled { "on" } else { "off" },
                    "starters": list, "added": added,
                }));
            }
            println!(
                "seeding at the engine's start: {}",
                if enabled { "on" } else { "off" }
            );
            for s in &list {
                println!(
                    "  {:<24} {}",
                    s["name"].as_str().unwrap_or_default(),
                    s["state"].as_str().unwrap_or_default()
                );
            }
            for a in &added {
                println!("seeded {}", a["label"].as_str().unwrap_or_default());
            }
            Ok(())
        }
        PipelineCommand::Ingest { run, json } => {
            let doc = ingest_run(&mut registry, run)?;
            if json {
                return print(&doc);
            }
            println!("{}", doc["line"].as_str().unwrap_or_default());
            Ok(())
        }
        PipelineCommand::Runs {
            id,
            pipeline,
            limit,
            json,
        } => {
            // record 48, D1 of the move: no unit's QC of a sealed stack,
            // unless --unsealed-access says why
            let unsealed = crate::sealed::keyboard_reads();
            if let Some(id) = id {
                let r = rows::run(registry.store(), id)?
                    .ok_or_else(|| usage(format!("no pipeline run {id}")))?;
                let mut doc = run_doc(registry.store(), &r).map_err(fail)?;
                if !unsealed {
                    summary_unsealed(registry.store(), r.id, &mut doc["summary"]).map_err(fail)?;
                    units_run_unsealed(registry.store(), r.id, &mut doc).map_err(fail)?;
                }
                if json {
                    return print(&doc);
                }
                print_run(&doc);
                return Ok(());
            }
            let pid = match pipeline {
                None => None,
                Some(name) => Some(
                    rows::resolve(registry.store(), &name)?
                        .ok_or_else(|| usage(format!("no pipeline {name} in the catalog")))?
                        .id,
                ),
            };
            let list = rows::runs(registry.store(), pid, limit)?;
            let mut docs = runs_docs(registry.store(), &list);
            if !unsealed {
                for (doc, r) in docs.iter_mut().zip(&list) {
                    summary_unsealed(registry.store(), r.id, &mut doc["summary"]).map_err(fail)?;
                }
            }
            if json {
                return print(&json!(docs));
            }
            println!(
                "{:>5}  {:<28} {:<9} {:<10} {:>6}  {:<24}  why",
                "run", "pipeline", "status", "runtime", "files", "started"
            );
            for d in &docs {
                // 2026-10-12: a failed run's kind and cause beside it, and a
                // run that did not make all it was for says what it made
                let why = match (
                    d["failure"]["kind"].as_str(),
                    d["failure"]["cause"].as_str(),
                ) {
                    (Some(kind), Some(cause)) => format!("{kind} ({cause})"),
                    _ if d["made"]["made"] != d["made"]["units"] => {
                        d["made"]["line"].as_str().unwrap_or_default().to_string()
                    }
                    _ => String::new(),
                };
                println!(
                    "{:>5}  {:<28} {:<9} {:<10} {:>6}  {:<24}  {why}",
                    d["id"],
                    d["pipeline"].as_str().unwrap_or("-"),
                    d["status"].as_str().unwrap_or_default(),
                    d["runtime"].as_str().unwrap_or_default(),
                    d["derivatives"].as_array().map_or(0, Vec::len),
                    d["started_at"].as_str().unwrap_or_default()
                );
            }
            Ok(())
        }
    }
}

/// A run's words, printed in one call: parts that end at the same moment
/// on their own threads each print a whole block, never lines among
/// another's.
fn print_run(doc: &Value) {
    print!("{}", run_text(doc));
}

fn run_text(doc: &Value) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    // 2026-10-12: its units summed beside its status, never a bare done
    let _ = writeln!(
        text,
        "run {} of {}: {}{}",
        doc["id"],
        doc["pipeline"].as_str().unwrap_or("-"),
        doc["status"].as_str().unwrap_or_default(),
        doc["made"]["line"]
            .as_str()
            .map(|l| format!(" · {l}"))
            .unwrap_or_default()
    );
    let _ = writeln!(
        text,
        "  over             {} (handle {})",
        doc["selection"].as_str().unwrap_or("a handle"),
        doc["handle_id"]
    );
    // record 55: a dataset's or a cohort's main scans, and what was read
    if doc["main"].is_object() {
        let _ = writeln!(
            text,
            "  main scans       {}",
            crate::main_input::line(&doc["main"])
        );
    }
    let _ = writeln!(text, "  parameters       {}", doc["params"]);
    let _ = writeln!(
        text,
        "  runtime          {} {} on {}, {}",
        doc["runtime"].as_str().unwrap_or_default(),
        doc["runtime_version"].as_str().unwrap_or_default(),
        doc["host"].as_str().unwrap_or_default(),
        doc["device"].as_str().unwrap_or_default()
    );
    let units = &doc["summary"]["units"];
    let _ = writeln!(
        text,
        "  units            {} succeeded, {} failed, {} skipped, {} unreported",
        units["succeeded"], units["failed"], units["skipped"], units["unreported"]
    );
    let _ = writeln!(
        text,
        "  derivatives      {}",
        doc["derivatives"].as_array().map_or(0, Vec::len)
    );
    if let Some(d) = doc["results_digest"].as_str() {
        let _ = writeln!(text, "  results          {d}");
    }
    if let Some(e) = doc["error"].as_str() {
        let _ = writeln!(text, "  error            {e}");
    }
    if let Some(f) = crate::failures::line(&doc["failure"]) {
        let _ = writeln!(text, "  failure          {f}");
    }
    for f in doc["summary"]["failures_by_cause"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let _ = writeln!(
            text,
            "  units failed     {} for {} ({})",
            f["units"],
            f["cause"].as_str().unwrap_or_default(),
            f["kind"].as_str().unwrap_or_default()
        );
    }
    text
}

// ------------------------------------------------------------------ the run

/// One work unit of a run.
#[derive(Debug, Clone)]
struct Unit {
    /// `sub-<s>`, `sub-<s>_ses-<t>` or `stack-<id>`.
    id: String,
    subject: Option<String>,
    session: Option<String>,
    stack_id: Option<i64>,
    series_id: Option<i64>,
    subject_id: Option<i64>,
    session_day: Option<String>,
    stacks: Vec<i64>,
}

impl Unit {
    /// What a derivative of the unit belongs to.
    fn belongs(&self) -> Option<Belongs> {
        let subject_id = Some(self.subject_id?);
        Some(match (self.stack_id, &self.session_day) {
            (Some(stack), _) => Belongs {
                scope: "stack".into(),
                stack_id: Some(stack),
                series_id: self.series_id,
                subject_id,
                session_day: None,
            },
            (None, Some(day)) => Belongs {
                scope: "session".into(),
                stack_id: None,
                series_id: None,
                subject_id,
                session_day: Some(day.clone()),
            },
            (None, None) => Belongs {
                scope: "subject".into(),
                stack_id: None,
                series_id: None,
                subject_id,
                session_day: None,
            },
        })
    }

    /// The values a path template's words take for the unit.
    fn vars(&self) -> Vec<(&'static str, String)> {
        let mut v = Vec::new();
        if let Some(s) = &self.subject {
            v.push(("subject", s.clone()));
        }
        if let Some(s) = &self.session {
            v.push(("session", s.clone()));
        }
        if let Some(s) = self.stack_id {
            v.push(("stack", s.to_string()));
        }
        v
    }

    fn doc(&self) -> Value {
        json!({
            "unit": self.id, "subject": self.subject, "session": self.session,
            "stack_id": self.stack_id, "subject_id": self.subject_id,
            "session_day": self.session_day, "stacks": self.stacks,
        })
    }
}

/// The detail a queued run was given; a run from the keyboard holds every
/// class. A pipeline reads pixels, which open at detail quasi.
fn detail_allows_pixels() -> Result<(), Exit> {
    if let Ok(d) = std::env::var("NILS_JOB_DETAIL") {
        let detail = crate::grants::Detail::parse(d.trim()).unwrap_or_default();
        if detail < crate::grants::Detail::Quasi {
            return Err(usage(
                "a pipeline reads the pixels, which are quasi-identifying; the run needs detail quasi",
            ));
        }
    }
    Ok(())
}

/// Parse `id=value` words.
fn pairs(words: &[String]) -> Result<Vec<(String, String)>, Exit> {
    words
        .iter()
        .map(|w| {
            w.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.to_string()))
                .filter(|(k, _)| !k.is_empty())
                .ok_or_else(|| usage(format!("--param {w}: write it as id=value")))
        })
        .collect()
}

/// A short random word, so two registries on one host never name a
/// container alike.
fn nonce() -> String {
    let mut b = [0u8; 4];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
    hex::encode(b)
}

/// The stacks of a frozen handle.
pub(crate) fn handle_stacks(store: &mut Store, handle: i64) -> Result<Vec<i64>, Exit> {
    let h = nils_ask::handle::get(store, handle)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| usage(format!("no handle {handle}")))?;
    if h.grain != nils_ask::ast::Grain::Stack {
        return Err(usage(format!(
            "handle {handle} is a {} set; a run is over stacks, and --select freezes a wider selection into them",
            h.grain.name()
        )));
    }
    if !h.has_rows() || h.withdrawn_at.is_some() || h.truncated {
        return Err(usage(format!(
            "handle {handle} keeps no complete list of stacks; run its question again"
        )));
    }
    let mut stacks: Vec<i64> = nils_ask::handle::keys(store, handle)
        .map_err(|e| fail(e.to_string()))?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    stacks.sort_unstable();
    stacks.dedup();
    Ok(stacks)
}

/// Whether a release may write `out` for a run's input: the run is running
/// and `out` is its input folder under its working place. Otherwise a
/// release writes to an export place only (Wave 5 section 10.2).
pub(crate) fn release_target(store: &mut Store, run_id: i64, out: &Path) -> Result<(), String> {
    let r = rows::run(store, run_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("--into-run {run_id}: no such pipeline run"))?;
    if r.status != "running" {
        return Err(format!("--into-run {run_id}: the run is {}", r.status));
    }
    // a run's input is scratch, under its scratch place (record 49 R7)
    let place = r
        .scratch_place_id
        .or(r.place_id)
        .and_then(|id| nils_registry::place::show(store, id).ok().flatten())
        .ok_or_else(|| format!("--into-run {run_id}: the run has no working place"))?;
    let want = Path::new(&place.path)
        .join(RUNS)
        .join(run_id.to_string())
        .join("input");
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canon(out) != canon(&want) {
        return Err(format!(
            "--into-run {run_id}: a run's input is released into its own folder, {}",
            want.display()
        ));
    }
    Ok(())
}

/// Wait for a child, beating the job's heart; a cancel kills it.
fn wait_beating(
    child: &mut std::process::Child,
    store: &mut Store,
    job_id: i64,
    progress: &Value,
) -> Result<(std::process::ExitStatus, bool), String> {
    let mut beaten = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok((status, false));
        }
        if beaten.elapsed() >= Duration::from_secs(5) {
            beaten = Instant::now();
            if let Ok(job::Asked::Cancel) = job::beat(store, job_id, Some(progress)) {
                let _ = child.kill();
                let status = child.wait().map_err(|e| e.to_string())?;
                return Ok((status, true));
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The input of a run, as materialised.
struct Materialised {
    units: Vec<Unit>,
    mounts: Vec<Mount>,
    release_id: Option<i64>,
    stopped: bool,
    /// What the run was given of the source places (record 43 review).
    scope: Value,
    /// For the stacks layout, each unit's entry of `stacks.json`, in the
    /// order of `units`, and the sources it lists: what a unit that runs
    /// apart is given of it (record 49 A1).
    entries: Vec<Value>,
    listed: Vec<Value>,
    /// With the geometry, the rest of each session the run holds part of
    /// (record 55 E2, P1b), `stacks.json`'s `context`.
    context: Vec<Value>,
    /// With `x-nils.input.rows`, `stacks.json`'s `rows` (record 55 D3);
    /// null otherwise.
    rows: Value,
    /// For the bids layout, the converter its release ran with, where it
    /// was found and how (2026-10-12); null otherwise.
    converter: Value,
    /// Record 55 E5: for a run that declares the slice cache, each entry's
    /// files in the order `stacks.json` lists them, as `source_file` names
    /// them with the size and time it holds, so a unit's `read` hashes are
    /// kept for the right files; empty otherwise.
    files: Vec<Vec<(i64, String, i64, i64)>>,
}

/// At most this many folders are bound one by one for a stacks input; a
/// selection whose files lie in more is given the source places' roots
/// instead, which the run records and its report says.
pub(crate) const MAX_BINDS: usize = 2000;

/// `stacks.json`: each stack's files, and the frames of a multi-frame
/// file; since record 43 also each stack's orientation, its `body_part` and
/// `technique` as the registry holds them now, and how many slices its
/// files hold, so an image seeds and picks slices without reading every
/// header. Each folder that holds a file of the selection is bound
/// read-only at `/source/<n>`, one bind per folder and never a whole source
/// place, unless the folders are more than [`MAX_BINDS`]. The registry is
/// read a few hundred stacks at a time, not stack by stack. With `header`
/// (record 50, `x-nils.input.header`) each entry also carries the stack's
/// header facts, [`stack_headers`]; with `geometry` (record 55 E2,
/// `x-nils.input.geometry`) each entry carries the stack's geometry and
/// each of its files their own, [`stack_geometry`] and [`file_geometry`];
/// without either the file is as it was.
#[allow(clippy::too_many_arguments)]
fn materialise_stacks(
    store: &mut Store,
    stacks: &[i64],
    input: &Path,
    header: bool,
    geometry: bool,
    contrast: bool,
    identity: bool,
    rows: bool,
) -> Result<Materialised, String> {
    let d = store.dialect();
    let err = |e: nils_registry::Error| e.to_string();
    type Info = (i64, i64, String, Option<String>);
    let mut info: BTreeMap<i64, Info> = BTreeMap::new();
    let mut axes: BTreeMap<i64, BTreeMap<String, String>> = BTreeMap::new();
    let mut files_of: BTreeMap<i64, Vec<StackFile>> = BTreeMap::new();
    let mut geometry_of: BTreeMap<i64, Value> = BTreeMap::new();
    let mut study_uid_of: BTreeMap<i64, String> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT st.id, st.series_id, se.subject_id, st.modality, st.orientation, \
             st.image_orientation_patient, fp.slice_thickness, fp.spacing_between_slices, \
             se.study_id, st.image_type, fp.sop_class_uid, st.n_instances FROM {} st \
             JOIN {} se ON se.id = st.series_id LEFT JOIN {} fp ON fp.stack_id = st.id \
             WHERE st.id IN ({list})",
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("stack_fingerprint"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let id = r.int(0).map_err(err)?;
            info.insert(
                id,
                (
                    r.int(1).map_err(err)?,
                    r.int(2).map_err(err)?,
                    r.text(3).map_err(err)?.to_string(),
                    r.opt_text(4).map_err(err)?.map(str::to_string),
                ),
            );
            if geometry {
                geometry_of.insert(id, stack_facts(&r, 5).map_err(err)?);
            }
        }
        // the axes an image reads, as the registry holds them now
        let sql = format!(
            "SELECT stack_id, axis, value FROM {} WHERE stack_id IN ({list}) \
             AND axis IN ('body_part', 'technique') AND value IS NOT NULL \
             ORDER BY stack_id, axis, value",
            store.qualified("classification_axis"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            axes.entry(r.int(0).map_err(err)?)
                .or_default()
                .entry(r.text(1).map_err(err)?.to_string())
                .or_insert(r.text(2).map_err(err)?.to_string());
        }
        stack_files(store, &list, geometry, identity, &mut files_of)?;
        // record 55 E5: the session a stack is part of, by its DICOM
        // identity, which the slice cache names its files by
        if identity {
            let sql = format!(
                "SELECT st.id, stu.study_instance_uid FROM {} st JOIN {} se ON se.id = st.series_id \
                 JOIN {} stu ON stu.id = se.study_id WHERE st.id IN ({list})",
                store.qualified("stack"),
                store.qualified("series"),
                store.qualified("study"),
            );
            for r in store.query(&sql, &[]).map_err(err)? {
                study_uid_of.insert(r.int(0).map_err(err)?, r.text(1).map_err(err)?.to_string());
            }
        }
    }
    let _ = d;
    // a stack of a sample sealed now says so, so an image leaves it out of
    // its session and answers it `sealed` (record 55 E2, P1b)
    if geometry {
        let (sealed, _) =
            nils_registry::labels::sealed_now(store, stacks, &[]).map_err(|e| e.to_string())?;
        for id in sealed {
            if let Some(g) = geometry_of.get_mut(&id) {
                g["sealed"] = json!(true);
            }
        }
    }
    // the sessions the run's unsealed stacks are part of
    let geometry_of_studies: std::collections::BTreeSet<i64> = geometry_of
        .values()
        .filter(|g| g["sealed"] != true)
        .filter_map(|g| g["study"].as_i64())
        .collect();
    let mut headers = if header {
        let subjects: BTreeMap<i64, i64> = info.iter().map(|(k, v)| (*k, v.1)).collect();
        stack_headers(store, &subjects)?
    } else {
        BTreeMap::new()
    };
    // record 55 E3: the session facts the post-contrast model reads, and
    // the datasets that hold a file of each stack
    let mut contrast_of = if contrast {
        let ids: Vec<i64> = info.keys().copied().collect();
        crate::contrast::with_datasets(store, &ids, &mut headers)?;
        crate::contrast::facts(store, &ids)?
    } else {
        BTreeMap::new()
    };
    // one bind per folder that holds a file of the selection, or the roots
    let folder = |root: &str, path: &str| -> Result<String, String> {
        bind_folder(root, path).ok_or_else(|| {
            format!(
                "the source place {root} holds a ':' or a ',' in its path, which a runtime's mount syntax splits on"
            )
        })
    };
    let mut folders: BTreeMap<(i64, String), (String, String)> = BTreeMap::new();
    let mut roots: BTreeMap<i64, String> = BTreeMap::new();
    let (mut widened, mut at_root) = (0usize, 0usize);
    for files in files_of.values() {
        for (so, root, path, _, _, _) in files {
            roots.entry(*so).or_insert_with(|| root.clone());
            let dir = folder(root, path)?;
            if let std::collections::btree_map::Entry::Vacant(e) = folders.entry((*so, dir.clone()))
            {
                let own = Path::new(path)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if dir != own {
                    widened += 1;
                }
                if dir.is_empty() {
                    at_root += 1;
                }
                e.insert((root.clone(), dir));
            }
        }
    }
    let by_folder = folders.len() <= MAX_BINDS;
    let mut index: BTreeMap<(i64, String), usize> = BTreeMap::new();
    let mut mounts = Vec::new();
    let mut listed = Vec::new();
    let bind = |host: PathBuf, n: usize, mounts: &mut Vec<Mount>, listed: &mut Vec<Value>| {
        let at = format!("/source/{n}");
        listed.push(json!({"id": n, "mount": at}));
        mounts.push(Mount {
            host,
            container: at,
            read_only: true,
        });
    };
    if by_folder {
        for (key, (root, dir)) in &folders {
            let n = index.len();
            index.insert(key.clone(), n);
            bind(Path::new(root).join(dir), n, &mut mounts, &mut listed);
        }
    } else {
        for (so, root) in &roots {
            let n = index.len();
            index.insert((*so, String::new()), n);
            bind(PathBuf::from(root), n, &mut mounts, &mut listed);
        }
    }
    let mut entries: Vec<Value> = Vec::new();
    let mut entry_files: Vec<Vec<(i64, String, i64, i64)>> = Vec::new();
    let mut units: Vec<Unit> = Vec::new();
    for &stack in stacks {
        let Some((series_id, subject_id, modality, orientation)) = info.remove(&stack) else {
            return Err(format!(
                "stack {stack} of the selection is not in the registry"
            ));
        };
        let mut seen: std::collections::BTreeSet<(i64, String)> = Default::default();
        let mut files: Vec<Value> = Vec::new();
        let mut named: Vec<(i64, String, i64, i64)> = Vec::new();
        let mut slices: i64 = 0;
        for (so, root, path, frames, geo, ident) in files_of.remove(&stack).unwrap_or_default() {
            if !seen.insert((so, path.clone())) {
                continue;
            }
            // record 55 E5: the file's SOP Instance UID, and the sha256 of
            // its bytes where it is known and the file on the disk still
            // has the size and time it was learnt at: a stale hash would
            // read another file's pictures from the cache
            let identity = ident.map(|i| {
                let holds = i.sha256.is_some()
                    && std::fs::metadata(Path::new(&root).join(&path)).is_ok_and(|m| {
                        i64::try_from(m.len()).ok() == Some(i.size)
                            && nils_digest::walk::mtime_ns_of(&m) == i.mtime_ns
                    });
                named.push((so, path.clone(), i.size, i.mtime_ns));
                json!({
                    "sop_instance_uid": i.sop_instance_uid,
                    "sha256": if holds { i.sha256 } else { None },
                })
            });
            let (n, rel) = if by_folder {
                let root = &roots[&so];
                let dir = folder(root, &path)?;
                let n = index[&(so, dir.clone())];
                let rel = if dir.is_empty() {
                    path.clone()
                } else {
                    path.strip_prefix(&format!("{dir}/"))
                        .unwrap_or(&path)
                        .to_string()
                };
                (n, rel)
            } else {
                (index[&(so, String::new())], path.clone())
            };
            // a single-frame file is one slice; a multi-frame file the
            // frames that are the stack's
            slices += frames.as_ref().map_or(1, |(_, n)| *n);
            let mut file = json!({"source": n, "path": rel, "frames": frames.map(|(f, _)| f)});
            if let Some(geo) = geo {
                file["geometry"] = geo;
            }
            if let Some(identity) = identity {
                file["identity"] = identity;
            }
            files.push(file);
        }
        if files.is_empty() {
            return Err(format!("stack {stack} has no files the registry can read"));
        }
        let unit = format!("stack-{stack}");
        let ax = axes.remove(&stack).unwrap_or_default();
        entries.push(json!({
            "unit": unit, "stack_id": stack, "series_id": series_id,
            "subject_id": subject_id, "modality": modality, "files": files,
            "orientation": orientation, "body_part": ax.get("body_part"),
            "technique": ax.get("technique"), "slices": slices,
        }));
        if header && let Some(entry) = entries.last_mut() {
            entry["header"] = headers.remove(&stack).unwrap_or(Value::Null);
        }
        if geometry && let Some(entry) = entries.last_mut() {
            entry["geometry"] = geometry_of.remove(&stack).unwrap_or(Value::Null);
        }
        if contrast && let Some(entry) = entries.last_mut() {
            entry["contrast"] = contrast_of.remove(&stack).unwrap_or(Value::Null);
        }
        if identity && let Some(entry) = entries.last_mut() {
            entry["identity"] = json!({"study_instance_uid": study_uid_of.remove(&stack)});
            entry_files.push(named);
        }
        units.push(Unit {
            id: unit,
            subject: None,
            session: None,
            stack_id: Some(stack),
            series_id: Some(series_id),
            subject_id: Some(subject_id),
            session_day: None,
            stacks: vec![stack],
        });
    }
    // the rest of each session the run holds part of, for the student's
    // session top (record 55 E2, P1b): read, never scored
    let mut context = if geometry {
        session_context(store, stacks, &geometry_of_studies)?
    } else {
        Vec::new()
    };
    // record 55 E3: each context entry with its series, header and session
    // facts, as the run's own stacks carry them
    if contrast && !context.is_empty() {
        crate::contrast::context(store, &mut context)?;
    }
    // record 55 D3: the registry's rows a header model reads
    let rows_doc = if rows {
        Some(crate::system1::header_rows(store, stacks)?)
    } else {
        None
    };
    write_stacks_json(
        &input.join("stacks.json"),
        &listed,
        &entries,
        geometry.then_some(context.as_slice()),
        rows_doc.as_ref(),
    )
    .map_err(|e| format!("the run's input: {e}"))?;
    let scope = if by_folder {
        let mut s = json!({"sources": "folders", "binds": mounts.len()});
        // a folder bound wider than its own, and a source root bound whole,
        // are said (record 43 second review)
        if widened > 0 || at_root > 0 {
            s["widened"] = json!(widened);
            s["roots"] = json!(at_root);
            s["why"] = json!(format!(
                "{widened} folder(s) bound through an ancestor, since their names hold a ':' or a ',' that a runtime's mount syntax splits on, and {at_root} source root(s) bound whole, since files lie directly in them"
            ));
        }
        s
    } else {
        json!({
            "sources": "roots", "binds": mounts.len(), "folders": folders.len(),
            "why": format!(
                "the selection's files lie in {} folders, more than {MAX_BINDS} binds, so the source places' roots were given",
                folders.len()
            ),
        })
    };
    Ok(Materialised {
        units,
        mounts,
        release_id: None,
        stopped: false,
        scope,
        entries,
        listed,
        context,
        rows: rows_doc.unwrap_or(Value::Null),
        converter: Value::Null,
        files: entry_files,
    })
}

/// `stacks.json` written from the entries as they are, with no copy of
/// them: compact where it carries the geometry (`context` is given then),
/// since the files' geometry makes it three to four times the size,
/// pretty-printed otherwise as before.
fn write_stacks_json(
    path: &Path,
    sources: &[Value],
    stacks: &[Value],
    context: Option<&[Value]>,
    rows: Option<&Value>,
) -> std::io::Result<()> {
    #[derive(serde::Serialize)]
    struct Doc<'a> {
        contract: &'a str,
        sources: &'a [Value],
        stacks: &'a [Value],
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<&'a [Value]>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<&'a Value>,
    }
    let doc = Doc {
        contract: nils_pipeline::CONTRACT,
        sources,
        stacks,
        context,
        rows,
    };
    let mut w = std::io::BufWriter::new(std::fs::File::create(path)?);
    if context.is_some() || rows.is_some() {
        serde_json::to_writer(&mut w, &doc)?;
    } else {
        serde_json::to_writer_pretty(&mut w, &doc)?;
    }
    std::io::Write::flush(&mut w)
}

/// A unit run apart holds one stack of its session (record 55 E2, P1b):
/// its context is the rest of the session, the run's other stacks of its
/// study that are MR and not sealed, in the context's shape, and the run's
/// context of that study. None for a manifest without the geometry.
fn unit_context(m: &Materialised, i: usize) -> Option<Vec<Value>> {
    let entry = m.entries.get(i)?;
    let g = entry.get("geometry").filter(|g| g.is_object())?;
    let Some(study) = g["study"].as_i64().filter(|_| g["sealed"] != true) else {
        return Some(Vec::new());
    };
    let mut out: Vec<Value> = m
        .entries
        .iter()
        .enumerate()
        .filter(|(j, e)| {
            *j != i && e["geometry"]["study"].as_i64() == Some(study) && e["geometry"]["sealed"] != true
        })
        .filter_map(|(_, e)| {
            // the image's modality: the stack's own, else its fingerprint's
            let modality = e["modality"]
                .as_str()
                .filter(|t| !t.is_empty())
                .or_else(|| e["header"]["fingerprint"]["modality"].as_str())
                .unwrap_or("");
            let files: Vec<Value> = e["files"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| json!({"geometry": f["geometry"]}))
                .collect();
            modality.eq_ignore_ascii_case("MR").then(|| {
                let mut c = json!({"stack_id": e["stack_id"], "modality": modality, "geometry": e["geometry"], "files": files});
                // record 55 E3: with the session facts, as the context has them
                if e.get("contrast").is_some() {
                    c["series_id"] = e["series_id"].clone();
                    c["header"] = e["header"].clone();
                    c["contrast"] = e["contrast"].clone();
                }
                c
            })
        })
        .collect();
    out.extend(
        m.context
            .iter()
            .filter(|c| c["geometry"]["study"].as_i64() == Some(study))
            .cloned(),
    );
    out.sort_by_key(|c| c["stack_id"].as_i64());
    Some(out)
}

/// The bids layout: a release of the stacks with the picks applied into
/// the run's input folder, by the release machinery itself as a process of
/// its own, and the units read back from what it wrote.
#[allow(clippy::too_many_arguments)]
fn materialise_bids(
    home: &Home,
    registry: &mut Registry,
    run_id: i64,
    job_id: i64,
    level: Level,
    stacks: &[i64],
    run_dir: &Path,
    input: &Path,
    args: &RunArgs,
) -> Result<Materialised, String> {
    let chosen = run_dir.join("selection.txt");
    let list: String = stacks.iter().map(|s| format!("{s}\n")).collect();
    std::fs::write(&chosen, list).map_err(|e| format!("the run's folder: {e}"))?;
    // record 55 (rulings of 2026-10-12, item 6): a frozen list of a
    // dataset's or a cohort's main scans is released under the picks it was
    // resolved from, whatever the scope's rules say now, so the run
    // reproduces; any other, as a release of picked stacks reads them
    let reading = rows::run(registry.store(), run_id)
        .map_err(|e| e.to_string())?
        .and_then(|r| r.handle_id)
        .and_then(|h| crate::main_input::reading_of(registry.store(), h));
    let picks = run_dir.join("picks.json");
    if let Some(doc) = &reading {
        std::fs::write(&picks, doc.to_string()).map_err(|e| format!("the run's folder: {e}"))?;
    }
    let exe = std::env::current_exe().map_err(|e| format!("this binary: {e}"))?;
    let log = std::fs::File::create(run_dir.join("release.log"))
        .map_err(|e| format!("the run's folder: {e}"))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--registry")
        .arg(home.dir())
        .args(["release", "--out"])
        .arg(input)
        .args(["--layout", "bids", "--picked", "--select"])
        .arg(&chosen)
        .args(["--name", &format!("pipeline-run-{run_id}")])
        .args([
            "--pack",
            &args.pack,
            "--json",
            "--into-run",
            &run_id.to_string(),
        ])
        .arg("--dcm2niix")
        .arg(&args.dcm2niix)
        // the release is a job of its own, never the run's row
        .env_remove(job::ADOPT_VAR)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::from(log));
    if let Some(dir) = &args.pack_dir {
        cmd.arg("--pack-dir").arg(dir);
    }
    if reading.is_some() {
        cmd.arg("--picks-read").arg(&picks);
    }
    let mut child = cmd.spawn().map_err(|e| format!("the release: {e}"))?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut s) = stdout {
            let _ = std::io::Read::read_to_string(&mut s, &mut text);
        }
        text
    });
    let (status, stopped) = wait_beating(
        &mut child,
        registry.store(),
        job_id,
        &json!({"run": run_id, "phase": "materialise", "layout": "bids"}),
    )?;
    let text = reader.join().unwrap_or_default();
    if stopped {
        return Ok(Materialised {
            units: Vec::new(),
            mounts: Vec::new(),
            release_id: None,
            stopped: true,
            scope: Value::Null,
            entries: Vec::new(),
            listed: Vec::new(),
            context: Vec::new(),
            rows: Value::Null,
            converter: Value::Null,
            files: Vec::new(),
        });
    }
    if !status.success() {
        let tail = std::fs::read_to_string(run_dir.join("release.log")).unwrap_or_default();
        let last = tail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim_start_matches("nils: ")
            .to_string();
        return Err(format!("the release of the input failed: {last}"));
    }
    let report: Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("the release's report: {e}"))?;
    let release_id = report["release_id"]
        .as_i64()
        .ok_or("the release's report names no release")?;
    rows::set_input_release(registry.store(), run_id, release_id).map_err(|e| e.to_string())?;
    let mut m = released(registry, release_id, level)?;
    // 2026-10-12: which converter the release ran, and how it was found
    m.converter = json!({
        "version": report["converter"],
        "path": report["converter_path"],
        "found_by": report["converter_found_by"],
    });
    Ok(m)
}

/// The units of a run's bids input, read back from the release that wrote
/// it: how a run materialises its input, and how a resumed run finds the
/// units of the input it released before (record 49 A1).
fn released(
    registry: &mut Registry,
    release_id: i64,
    level: Level,
) -> Result<Materialised, String> {
    let units = released_of(registry, release_id, level)?;
    if units.is_empty() {
        return Err(
            "the release of the input holds no subject's images: the selection's stacks were not picked, or went to sourcedata; nils pick run picks them".into(),
        );
    }
    Ok(Materialised {
        units,
        mounts: Vec::new(),
        release_id: Some(release_id),
        stopped: false,
        scope: json!({"sources": "release"}),
        entries: Vec::new(),
        listed: Vec::new(),
        context: Vec::new(),
        rows: Value::Null,
        converter: Value::Null,
        files: Vec::new(),
    })
}

/// One unit of the bids input a run released before, as a run taken up
/// again goes on with it (2026-10-12): its name, its subject and its
/// session's day, its stacks, and the roles it lacks as the runner judges
/// them ([`units_missing`]).
pub(crate) struct ReleasedUnit {
    pub(crate) id: String,
    pub(crate) subject_id: Option<i64>,
    pub(crate) session_day: Option<String>,
    pub(crate) stacks: Vec<i64>,
    pub(crate) missing: Vec<String>,
}

/// The units of the bids input a run released, each with the roles it
/// lacks of those the descriptor names, for the pre-flight of a run taken
/// up again (2026-10-12): the units [`released`] reads back, judged as the
/// runner judges them when it goes on. None where the release holds none.
pub(crate) fn released_units(
    registry: &mut Registry,
    release_id: i64,
    level: Level,
    roles: &[String],
    main: Option<&Value>,
) -> Result<Vec<ReleasedUnit>, String> {
    let units = released_of(registry, release_id, level)?;
    let mut missing = if roles.is_empty() {
        BTreeMap::new()
    } else {
        units_missing(registry.store(), Some(release_id), &units, roles, main)?
    };
    Ok(units
        .into_iter()
        .map(|u| ReleasedUnit {
            missing: missing.remove(&u.id).unwrap_or_default(),
            id: u.id,
            subject_id: u.subject_id,
            session_day: u.session_day,
            stacks: u.stacks,
        })
        .collect())
}

/// The units a release placed in its subjects' folders, by the run's name
/// for each.
fn released_of(
    registry: &mut Registry,
    release_id: i64,
    level: Level,
) -> Result<Vec<Unit>, String> {
    let store = registry.store();
    let d = store.dialect();
    let err = |e: nils_registry::Error| e.to_string();
    let sql = format!(
        "SELECT rs.stack_id, rs.dir, se.subject_id, se.study_id FROM {} r \
         JOIN {} rs ON rs.dataset_id = r.dataset_id \
         JOIN {} st ON st.id = rs.stack_id JOIN {} se ON se.id = st.series_id \
         WHERE r.id = {} ORDER BY rs.stack_id",
        store.qualified("release"),
        store.qualified("release_stack"),
        store.qualified("stack"),
        store.qualified("series"),
        d.param(1, Type::Int)
    );
    let placed = store.query(&sql, &[Param::Int(release_id)]).map_err(err)?;
    let labels = nils_session::labels_by_study(store, &nils_registry::session::Scheme::default())
        .unwrap_or_default();
    let mut units: BTreeMap<String, Unit> = BTreeMap::new();
    for r in &placed {
        let stack = r.int(0).map_err(err)?;
        let dir = r.text(1).map_err(err)?;
        let subject_id = r.int(2).map_err(err)?;
        let study_id = r.int(3).map_err(err)?;
        let mut parts = dir.split('/');
        let Some(subject) = parts.next().and_then(|p| p.strip_prefix("sub-")) else {
            // routed to sourcedata or derivatives: not a unit's input
            continue;
        };
        let session = parts.next().and_then(|p| p.strip_prefix("ses-"));
        let (id, session) = match (level, session) {
            (Level::Session, Some(s)) => (format!("sub-{subject}_ses-{s}"), Some(s.to_string())),
            _ => (format!("sub-{subject}"), None),
        };
        let day = if session.is_some() {
            labels.get(&study_id).map(|l| l.first.to_string())
        } else {
            None
        };
        let u = units.entry(id.clone()).or_insert(Unit {
            id,
            subject: Some(subject.to_string()),
            session,
            stack_id: None,
            series_id: None,
            subject_id: Some(subject_id),
            session_day: day,
            stacks: Vec::new(),
        });
        u.stacks.push(stack);
    }
    Ok(units.into_values().collect())
}

/// The image format of a file by its name: everything after the first `.`
/// of the file name (`nii.gz`, `mgz`), or nothing for a name without one.
fn image_format(name: &str) -> String {
    name.split_once('.')
        .map(|(_, ext)| ext.to_string())
        .unwrap_or_default()
}

/// Write the fed image over the files the release placed for a role's
/// stack (after the third review, finding 7): the image files of the stem
/// in `folder`, never its sidecar, and only where their format is the fed
/// image's (`.nii.gz` over `.nii.gz`); answers the files written, or the
/// two formats where they differ (the fed image's, the placed file's) and
/// nothing is written. An error is the folder or a copy failing.
fn bind_role_files(
    folder: &Path,
    stem: &str,
    from: &Path,
) -> Result<Result<Vec<String>, (String, String)>, String> {
    let fed_format = image_format(
        &from
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let mut targets: Vec<(PathBuf, String)> = Vec::new();
    for e in std::fs::read_dir(folder)
        .map_err(|e| format!("{}: {e}", folder.display()))?
        .flatten()
    {
        let name = e.file_name().to_string_lossy().into_owned();
        // the image files of the stem, never its sidecar
        if !name.starts_with(&format!("{stem}.")) || name.ends_with(".json") {
            continue;
        }
        // what follows `<stem>.` is the placed file's format
        let placed_format = name[stem.len() + 1..].to_string();
        if placed_format != fed_format {
            return Ok(Err((format!(".{fed_format}"), format!(".{placed_format}"))));
        }
        targets.push((e.path(), name));
    }
    let mut files = Vec::new();
    for (at, name) in targets {
        let _ = std::fs::remove_file(&at);
        std::fs::copy(from, &at)
            .map_err(|e| format!("{}: binding {}: {e}", at.display(), from.display()))?;
        files.push(name);
    }
    Ok(Ok(files))
}

/// After review (finding 3 of the second review): a pick role a workflow
/// link feeds takes the fed step's run's image as the role's file for each
/// unit: the newest image that run made of the unit (its stack, its session
/// or its subject) is written over the file the release placed for the
/// role's stack, and the manifest says so under `bound_roles`. A unit the
/// fed run made no image of is answered under its id with the link's
/// words, for the run to skip it; a unit whose role the release placed
/// nothing for is left to [`units_missing`]'s words.
#[allow(clippy::type_complexity)]
fn bind_roles(
    registry: &mut Registry,
    x: &Execution<'_>,
    m: &Materialised,
    input: &Path,
    links: &[Fed],
    d: &Descriptor,
) -> Result<(Vec<Value>, BTreeMap<String, Vec<String>>), String> {
    let mut bound: Vec<Value> = Vec::new();
    let mut unbound: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (Some(release_id), false) = (m.release_id, links.is_empty()) else {
        return Ok((bound, unbound));
    };
    let err = |e: nils_registry::Error| e.to_string();
    let store = registry.store();
    // where the release wrote each raw stack: its folder and its stem
    let dialect = store.dialect();
    let sql = format!(
        "SELECT rs.stack_id, rs.dir, rs.stem FROM {} r \
         JOIN {} rs ON rs.dataset_id = r.dataset_id WHERE r.id = {} AND rs.route = 'raw'",
        store.qualified("release"),
        store.qualified("release_stack"),
        dialect.param(1, Type::Int)
    );
    let mut placed: BTreeMap<i64, (String, String)> = BTreeMap::new();
    for r in store.query(&sql, &[Param::Int(release_id)]).map_err(err)? {
        if let Some(stem) = r.opt_text(2).map_err(err)? {
            placed.insert(
                r.int(0).map_err(err)?,
                (r.text(1).map_err(err)?.to_string(), stem.to_string()),
            );
        }
    }
    // which of the units' stacks stand for which role
    let all: Vec<i64> = m
        .units
        .iter()
        .flat_map(|u| u.stacks.iter().copied())
        .collect();
    let main = rows::run(store, x.run_id)
        .map_err(err)?
        .and_then(|r| r.handle_id)
        .and_then(|h| crate::main_input::entry_of(store, h));
    let picks: BTreeMap<i64, BTreeSet<String>> = match main {
        Some(main) => crate::main_input::stack_roles(&main, &all),
        None => match release_reading(store, release_id)?.picks() {
            Some(picks) => crate::preflight::pick_roles(store, &all, &picks)?
                .into_iter()
                .map(|(s, p)| (s, p.roles))
                .collect(),
            None => BTreeMap::new(),
        },
    };
    let places = crate::reuse::places(store)?;
    for fed in links {
        let l = &fed.link;
        let mut made: Vec<derivative::Derivative> = Vec::new();
        for run in &fed.runs {
            made.extend(
                derivative::list(
                    store,
                    &derivative::Filter {
                        run_id: Some(*run),
                        limit: 10_000,
                        ..Default::default()
                    },
                )
                .map_err(err)?
                .into_iter()
                .filter(|dv| {
                    dv.withdrawn_at.is_none()
                        && matches!(dv.kind.as_str(), "image" | "output")
                        && fed.takes(dv)
                }),
            );
        }
        for u in &m.units {
            let role_stacks: Vec<i64> = u
                .stacks
                .iter()
                .filter(|s| picks.get(s).is_some_and(|roles| roles.contains(&l.input)))
                .copied()
                .collect();
            if role_stacks.is_empty() {
                continue;
            }
            let Some(dv) = made
                .iter()
                .filter(|dv| {
                    crate::reuse::belongs(dv, &u.stacks, u.subject_id, u.session_day.as_deref())
                })
                .max_by_key(|dv| dv.id)
            else {
                unbound.entry(u.id.clone()).or_default().push(format!(
                    "{}: {} made no image of it for the {} role",
                    l.words(&d.name),
                    fed.words(),
                    l.input
                ));
                continue;
            };
            let Some(root) = places.get(&dv.place_id) else {
                unbound.entry(u.id.clone()).or_default().push(format!(
                    "{}: the place {} of {} is gone",
                    l.words(&d.name),
                    dv.place_id,
                    dv.path
                ));
                continue;
            };
            let from = Path::new(root).join(&dv.path);
            for s in role_stacks {
                let Some((dir, stem)) = placed.get(&s) else {
                    continue;
                };
                let folder = input.join(dir);
                // after the third review (finding 7): bound only where the
                // image formats agree; converting is out of scope, so a unit
                // whose formats differ is skipped with both named
                let files = match bind_role_files(&folder, stem, &from)? {
                    Ok(files) => files
                        .into_iter()
                        .map(|name| format!("{dir}/{name}"))
                        .collect::<Vec<_>>(),
                    Err(formats) => {
                        unbound.entry(u.id.clone()).or_default().push(format!(
                            "{}: {} made {} as {}, and the release placed the {} role as {}; converting between the two formats is not done",
                            l.words(&d.name),
                            fed.words(),
                            dv.path,
                            formats.0,
                            l.input,
                            formats.1
                        ));
                        break;
                    }
                };
                bound.push(json!({
                    "unit": u.id, "role": l.input, "stack": s, "from": l.from_id, "run": l.run,
                    "derivative": dv.id, "path": dv.path, "sha256": dv.sha256, "files": files,
                }));
            }
        }
    }
    Ok((bound, unbound))
}

/// The units of a bids input that lack a role the descriptor names, by
/// the unit's id, with why (record 49 A3): what the release wrote of the
/// stacks picked for each role in the unit, judged as the pre-flight
/// judges it, so the two agree. A run over a frozen list of a dataset's or
/// a cohort's main scans (`main`, what its handle records) takes each of
/// its stacks as a main scan of the list's role, as it was resolved
/// (record 55, rulings of 2026-10-12, item 6); of a list of several roles
/// (2026-10-12), of the role each stack was taken for, as the list records
/// it.
fn units_missing(
    store: &mut Store,
    release_id: Option<i64>,
    units: &[Unit],
    roles: &[String],
    main: Option<&Value>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let Some(release_id) = release_id else {
        return Ok(BTreeMap::new());
    };
    let err = |e: nils_registry::Error| e.to_string();
    let d = store.dialect();
    let sql = format!(
        "SELECT rs.stack_id, rs.stem, rs.route FROM {} r \
         JOIN {} rs ON rs.dataset_id = r.dataset_id WHERE r.id = {}",
        store.qualified("release"),
        store.qualified("release_stack"),
        d.param(1, Type::Int)
    );
    // each stack's stem in the raw tree, which says what it stands as
    let mut stems: BTreeMap<i64, Option<String>> = BTreeMap::new();
    for r in store.query(&sql, &[Param::Int(release_id)]).map_err(err)? {
        let raw = r.text(2).map_err(err)? == "raw";
        let stem = r.opt_text(1).map_err(err)?;
        stems.insert(
            r.int(0).map_err(err)?,
            stem.filter(|_| raw).map(str::to_string),
        );
    }
    let all: Vec<i64> = units
        .iter()
        .flat_map(|u| u.stacks.iter().copied())
        .collect();
    let picks: BTreeMap<i64, std::collections::BTreeSet<String>> = match main {
        // each stack as the main scan of the role it was taken for: the
        // list's one role, or of several (2026-10-12) its own
        Some(m) => crate::main_input::stack_roles(m, &all),
        // the picks its input release froze when it was made (record 55,
        // rulings of 2026-10-12, item 6, carried to releases); of a
        // release from before, the live picks of what it recorded, and
        // where they are gone, every unit says so
        None => {
            let reading = release_reading(store, release_id)?;
            let Some(picks) = reading.picks() else {
                let why = reading.gone().unwrap_or_default();
                return Ok(units
                    .iter()
                    .map(|u| {
                        let unknown = roles
                            .iter()
                            .map(|role| {
                                format!(
                                    "which of its stacks stands for {role} is not known: the picks its input release read are gone ({why})"
                                )
                            })
                            .collect();
                        (u.id.clone(), unknown)
                    })
                    .collect());
            };
            crate::preflight::pick_roles(store, &all, &picks)?
                .into_iter()
                .map(|(s, p)| (s, p.roles))
                .collect()
        }
    };
    let mut out = BTreeMap::new();
    for u in units {
        let mut held: BTreeMap<String, Vec<Option<String>>> = BTreeMap::new();
        for s in &u.stacks {
            let Some(roles_of) = picks.get(s) else {
                continue;
            };
            for role in roles_of {
                held.entry(role.clone())
                    .or_default()
                    .push(stems.get(s).cloned().flatten());
            }
        }
        let why = crate::preflight::roles_missing(roles, &held);
        if !why.is_empty() {
            out.insert(u.id.clone(), why);
        }
    }
    Ok(out)
}

/// Record 55, decision 6, and its rulings of 2026-10-12, item 6, carried
/// to releases: the picks a release read, as a reader after it reads them:
/// the list it froze when it was made; of a release from before, the live
/// picks of what it recorded (the pack's where it recorded none), unless
/// they are gone (one place of truth, 2026-10-11).
pub(crate) fn release_reading(
    store: &mut Store,
    release_id: i64,
) -> Result<nils_registry::pick_truth::OfRelease, String> {
    Ok(nils_registry::pick_truth::of_release(store, release_id)
        .map_err(|e| e.to_string())?
        .unwrap_or(nils_registry::pick_truth::OfRelease::Recorded(
            nils_registry::pick_truth::Of::Pack,
        )))
}

/// Record 48, D1 of the move: each unit of a run by the name the run gave
/// it, with the stacks it was given, read from the registry and never from
/// a file the run's container could reach: a stack's unit is its own stack,
/// a subject's or a session's the stacks the run's release placed in its
/// folder (`sub-<s>/` or `sub-<s>/ses-<t>/`), none where the release no
/// longer says. Empty for a run from before its units were kept (record 49).
pub(crate) fn unit_stacks(
    store: &mut Store,
    run_id: i64,
) -> Result<Vec<(String, Vec<i64>)>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let Some(run) = rows::run(store, run_id).map_err(err)? else {
        return Ok(Vec::new());
    };
    let names: Vec<String> = rows::units(store, run_id)
        .map_err(err)?
        .into_iter()
        .map(|u| u.unit)
        .collect();
    let placed = match run.input_release_id {
        Some(release) if names.iter().any(|n| n.starts_with("sub-")) => {
            released_stacks(store, release).unwrap_or_default()
        }
        _ => Vec::new(),
    };
    Ok(names
        .into_iter()
        .map(|name| {
            if let Some(stack) = name
                .strip_prefix("stack-")
                .and_then(|s| s.parse::<i64>().ok())
            {
                return (name, vec![stack]);
            }
            let folder = match name.split_once("_ses-") {
                Some((subject, session)) => format!("{subject}/ses-{session}/"),
                None => format!("{name}/"),
            };
            let stacks = placed
                .iter()
                .filter(|(_, dir, _)| format!("{dir}/").starts_with(&folder))
                .map(|(s, ..)| *s)
                .collect();
            (name, stacks)
        })
        .collect())
}

/// Record 48, D1 of the move: the stacks of the list a run was frozen
/// over, as long as its handle keeps them; none once it is withdrawn.
pub(crate) fn frozen_stacks(store: &mut Store, run_id: i64) -> Result<Vec<i64>, String> {
    let run = rows::run(store, run_id).map_err(|e| e.to_string())?;
    let Some(handle) = run.and_then(|r| r.handle_id) else {
        return Ok(Vec::new());
    };
    Ok(nils_ask::handle::keys(store, handle)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(k, _)| k)
        .collect())
}

/// Record 55 H3 (section 2.2): where a mask a bids unit made is looked for
/// its grid scan, from the stacks the run's release placed under the
/// unit's folder (`sub-<s>/ses-<t>/`, or `sub-<s>/`), each with the NIfTI
/// the release made of it where it is still in the run's input: with the
/// output's declared grid role, the stacks picked for that role; without,
/// every stack of the unit. None where the run released no input.
pub(crate) fn grid_candidates(
    store: &mut Store,
    run_id: i64,
    path: &str,
    role: Option<&str>,
) -> crate::derivatives::Where {
    use crate::derivatives::Where;
    let Ok(Some(run)) = rows::run(store, run_id) else {
        return Where::Unknown;
    };
    let Some(release_id) = run.input_release_id else {
        return Where::Unknown;
    };
    let segs: Vec<&str> = path.split('/').collect();
    let Some(i) = segs
        .iter()
        .position(|s| s.starts_with("sub-") && !s.contains('_'))
    else {
        return Where::Unknown;
    };
    let unit = match segs.get(i + 1).filter(|s| s.starts_with("ses-")) {
        Some(ses) => format!("{}/{ses}/", segs[i]),
        None => format!("{}/", segs[i]),
    };
    let input = run
        .scratch_place_id
        .or(run.place_id)
        .and_then(|id| nils_registry::place::show(store, id).ok().flatten())
        .map(|p| {
            Path::new(&p.path)
                .join(RUNS)
                .join(run_id.to_string())
                .join("input")
        });
    let placed = match released_stacks(store, release_id) {
        Ok(p) => p,
        Err(_) => return Where::Unknown,
    };
    let stacks: Vec<(i64, Option<PathBuf>)> = placed
        .into_iter()
        .filter(|(_, dir, _)| format!("{dir}/").starts_with(&unit))
        .map(|(s, dir, stem)| (s, input_nifti(input.as_deref(), &dir, stem.as_deref())))
        .collect();
    by_role(store, release_id, stacks, role)
}

/// Each stack a release placed, with its folder and its file stem.
fn released_stacks(
    store: &mut Store,
    release_id: i64,
) -> Result<Vec<(i64, String, Option<String>)>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let d = store.dialect();
    let sql = format!(
        "SELECT rs.stack_id, rs.dir, rs.stem FROM {} r \
         JOIN {} rs ON rs.dataset_id = r.dataset_id WHERE r.id = {} ORDER BY rs.stack_id",
        store.qualified("release"),
        store.qualified("release_stack"),
        d.param(1, Type::Int)
    );
    store
        .query(&sql, &[Param::Int(release_id)])
        .map_err(err)?
        .iter()
        .map(|r| {
            Ok((
                r.int(0).map_err(err)?,
                r.text(1).map_err(err)?.to_string(),
                r.opt_text(2).map_err(err)?.map(str::to_string),
            ))
        })
        .collect()
}

/// The NIfTI a release made of a stack in a run's input, where it is.
fn input_nifti(input: Option<&Path>, dir: &str, stem: Option<&str>) -> Option<PathBuf> {
    let (input, stem) = (input?, stem?);
    ["nii.gz", "nii"]
        .iter()
        .map(|ext| input.join(dir).join(format!("{stem}.{ext}")))
        .find(|p| p.is_file())
}

/// The grid candidates of a unit's stacks: those picked for `role` where
/// the output declares one, else every one.
fn by_role(
    store: &mut Store,
    release_id: i64,
    stacks: Vec<(i64, Option<PathBuf>)>,
    role: Option<&str>,
) -> crate::derivatives::Where {
    use crate::derivatives::Where;
    let Some(role) = role else {
        return Where::Inputs(stacks);
    };
    let ids: Vec<i64> = stacks.iter().map(|(s, _)| *s).collect();
    // the picks the release froze; of one from before whose picks are
    // gone, no stack is said to be of the role, and the mask's grid is
    // the input its file's grid matches
    let picks = match release_reading(store, release_id) {
        Ok(reading) => match reading.picks() {
            Some(picks) => crate::preflight::pick_roles(store, &ids, &picks).unwrap_or_default(),
            None => return Where::Inputs(stacks),
        },
        Err(_) => BTreeMap::new(),
    };
    Where::Role(
        stacks
            .into_iter()
            .filter(|(s, _)| picks.get(s).is_some_and(|p| p.roles.contains(role)))
            .collect(),
    )
}

/// Where a mask a unit made is looked for its grid scan (record 55 H3): a
/// stack unit's own stack; a bids unit's stacks, by the output's grid role
/// or by matching their NIfTI.
fn unit_grid(
    store: &mut Store,
    x: &Execution<'_>,
    u: &Unit,
    role: Option<&str>,
) -> crate::derivatives::Where {
    use crate::derivatives::Where;
    if let Some(s) = u.stack_id {
        return Where::Own(s);
    }
    let Some(release_id) = rows::run(store, x.run_id)
        .ok()
        .flatten()
        .and_then(|r| r.input_release_id)
    else {
        return Where::Unknown;
    };
    let input = Path::new(&x.scratch.path)
        .join(RUNS)
        .join(x.run_id.to_string())
        .join("input");
    let placed = released_stacks(store, release_id).unwrap_or_default();
    let stacks: Vec<(i64, Option<PathBuf>)> = placed
        .into_iter()
        .filter(|(s, _, _)| u.stacks.contains(s))
        .map(|(s, dir, stem)| (s, input_nifti(Some(&input), &dir, stem.as_deref())))
        .collect();
    by_role(store, release_id, stacks, role)
}

/// What became of one unit after the container exited.
struct Outcome {
    status: &'static str,
    error: Option<String>,
    metrics: Value,
    /// Files relative to the output folder.
    files: Vec<String>,
}

/// The lane a run is scheduled in (record 49 A1, A2): its budget, as set
/// and as this machine allows it, and the card a GPU unit leases.
#[derive(Debug, Clone)]
pub(crate) struct Lane {
    pub(crate) ledger: lane::Ledger,
    /// The cores and GB of memory set, before the machine had its say.
    pub(crate) set: (u32, u64),
    /// Why the budget is below the setting, where it is.
    pub(crate) why: Option<String>,
    /// The card a GPU unit leases; none where the lane uses no card.
    pub(crate) card: Option<u32>,
}

impl Lane {
    pub(crate) fn doc(&self) -> Value {
        json!({
            "cores": self.ledger.cores,
            "memory_gb": self.ledger.memory_mib / 1024,
            "set": {"cores": self.set.0, "memory_gb": self.set.1},
            "why": self.why,
            "gpu_card": self.card,
        })
    }
}

/// The two working places a run writes (record 49 R7).
#[derive(Debug, Clone)]
pub(crate) struct Places {
    /// Its outputs: the derivatives and tables it registers.
    pub(crate) output: nils_registry::place::Place,
    /// Its scratch: its input, each unit's folder, its log and apptainer's
    /// images.
    pub(crate) scratch: nils_registry::place::Place,
    /// Whether each was named by the lane, or is the first active working
    /// place because it names none.
    pub(crate) named: (bool, bool),
}

impl Places {
    pub(crate) fn doc(&self) -> Value {
        json!({
            "output": self.output.name, "scratch": self.scratch.name,
            "output_set": self.named.0, "scratch_set": self.named.1,
        })
    }
}

/// The lane's output and scratch places (record 49 R7): each the active
/// working place the lane names, or the first active working place where
/// it names none, as every run wrote before. A place the lane names that
/// is not an active working place is refused in words naming the cure,
/// never swapped for another.
pub(crate) fn run_places(store: &mut Store) -> Result<Places, String> {
    let first = crate::derivatives::working(store)
        .map_err(|m| m.replacen("derivatives are off", "pipelines are off", 1))?;
    let named = |store: &mut Store,
                 key: &str,
                 flag: &str|
     -> Result<Option<nils_registry::place::Place>, String> {
        let Some(name) = meta_text(store, key)?
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        else {
            return Ok(None);
        };
        let active = nils_registry::place::active(store).map_err(|e| e.to_string())?;
        active
            .into_iter()
            .find(|p| p.name == name && p.role == nils_registry::place::Role::Working)
            .map(Some)
            .ok_or_else(|| {
                format!(
                    "pipelines are off: the lane's {flag} place is {name}, which is not an active working place; nils place add {name} <path> --role working adds it, or nils pipeline lane --{flag}-place default goes back to the first working place"
                )
            })
    };
    let output = named(store, OUTPUT_PLACE_KEY, "output")?;
    let scratch = named(store, SCRATCH_PLACE_KEY, "scratch")?;
    Ok(Places {
        named: (output.is_some(), scratch.is_some()),
        output: output.unwrap_or_else(|| first.clone()),
        scratch: scratch.unwrap_or(first),
    })
}

/// One `registry_meta` row, read through a store.
fn meta_text(store: &mut Store, key: &str) -> Result<Option<String>, String> {
    let sql = format!(
        "SELECT value FROM {} WHERE key = {}",
        store.qualified("registry_meta"),
        store.dialect().param(1, Type::Text)
    );
    store
        .query_opt(&sql, &[Param::from(key)])
        .map_err(|e| e.to_string())?
        .map(|r| r.text(0).map(str::to_string))
        .transpose()
        .map_err(|e| e.to_string())
}

/// The lane as the registry's settings and this machine make it.
pub(crate) fn lane_of(registry: &mut Registry) -> Lane {
    let mut num = |key: &str| {
        registry
            .meta_value(key)
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    let cores = num(LANE_CORES_KEY).map_or(lane::DEFAULT_CORES, |c| c.clamp(1, 65_536) as u32);
    let memory = num(LANE_MEMORY_KEY).map_or(lane::DEFAULT_MEMORY_GB, |m| m.max(1));
    let card = match registry
        .meta_value(GPU_CARD_KEY)
        .ok()
        .flatten()
        .as_deref()
        .map(str::trim)
    {
        None | Some("") => Some(0),
        Some("none") => None,
        Some(n) => n.parse().ok(),
    };
    let (ledger, why) = lane::budget(cores, memory, lane::host());
    Lane {
        ledger,
        set: (cores, memory),
        why,
        card,
    }
}

/// What one scheduling unit of a pipeline asks of the lane.
fn ask_of(d: &Descriptor) -> lane::Ask {
    lane::Ask {
        cores: d.needs.cores,
        memory_mib: lane::mib_of_gb(d.needs.memory_gb),
    }
}

/// Refuse a pipeline whose unit could never fit in the lane.
fn fits(d: &Descriptor, p: &Pipeline, lane: &Lane) -> Result<(), Exit> {
    match lane_refusal(d, p, lane) {
        None => Ok(()),
        Some(words) => Err(usage(words)),
    }
}

/// Why a unit could never fit in the lane, in the words a refusal says;
/// none where it could.
pub(crate) fn lane_refusal(d: &Descriptor, p: &Pipeline, lane: &Lane) -> Option<String> {
    if lane.ledger.could(ask_of(d)) {
        return None;
    }
    Some(format!(
        "{} asks {} cores and {} GB of memory for each of its {}, and the pipeline lane has {} cores and {} GB{}; nils pipeline lane --cores <n> --memory-gb <n> sets it",
        p.label(),
        d.needs.cores,
        d.needs.memory_gb,
        if d.units == Units::Apart {
            "units"
        } else {
            "runs"
        },
        lane.ledger.cores,
        lane.ledger.memory_mib / 1024,
        lane.why
            .as_deref()
            .map(|w| format!(" ({w})"))
            .unwrap_or_default()
    ))
}

/// The lane's card where it is smaller than a unit's need, with its size in
/// MiB, as `nvidia-smi` says it (record 55 A5); none where it holds the
/// need, or its size is not known.
pub(crate) fn card_too_small(d: &Descriptor, lane: &Lane) -> Option<(u32, u64)> {
    let card = lane.card?;
    let smi = runtime::which("nvidia-smi", std::env::var_os("PATH").as_deref())?;
    let total = lane::gpu_total_mib(&smi, card).ok();
    (!lane::card_holds(total, lane::mib_of_gb(d.needs.gpu_memory_gb)))
        .then(|| (card, total.unwrap_or(0)))
}

/// Why a unit whose card is optional may run on the CPU though the card is
/// large enough (record 55, after review): its free memory is below a
/// unit's need now, and a unit that cannot lease it within the grace runs
/// on the CPU. None where the card has room, or cannot be asked.
pub(crate) fn card_full_words(d: &Descriptor, lane: &Lane) -> Option<String> {
    let card = lane.card?;
    let smi = runtime::which("nvidia-smi", std::env::var_os("PATH").as_deref())?;
    let free = lane::gpu_free_mib(&smi, card).ok()?;
    let need = lane::mib_of_gb(d.needs.gpu_memory_gb);
    (free < need).then(|| {
        format!(
            "card {card} has {free} MiB free now, and a unit needs {need} MiB: a unit that cannot lease it within {} s runs on the CPU",
            card_grace().as_secs()
        )
    })
}

/// Why a unit runs on the CPU though a card is offered: the lane's card is
/// smaller than the unit's need, so a lease on it would never come.
pub(crate) fn too_small_words(d: &Descriptor, card: u32, total_mib: u64) -> String {
    format!(
        "card {card} holds {} GB, and a unit needs {} GB of a card",
        descriptor::number_text(total_mib as f64 / 1024.0),
        descriptor::number_text(d.needs.gpu_memory_gb)
    )
}

/// The device a run takes and whether it takes the GPU: a GPU where the
/// pipeline asks, the runtime offers one and the lane names a card that
/// can hold a unit's need. An optional need runs on the CPU where the card
/// is smaller than that (record 55 A5), and a required one is refused.
fn device_for(
    d: &Descriptor,
    p: &Pipeline,
    rt: &runtime::Cli,
    lane: &Lane,
) -> Result<(String, bool), Exit> {
    let offered = rt.gpu().filter(|_| lane.card.is_some());
    let small = match (d.gpu, offered) {
        (Gpu::Optional | Gpu::Required, Some(_)) => card_too_small(d, lane),
        _ => None,
    };
    match (d.gpu, offered) {
        (Gpu::None, _) | (Gpu::Optional, None) => Ok(("cpu".to_string(), false)),
        (Gpu::Optional, Some(_)) if small.is_some() => Ok(("cpu".to_string(), false)),
        (Gpu::Required, Some(_)) if small.is_some() => {
            let (card, total) = small.unwrap_or_default();
            Err(usage(format!(
                "{} needs a GPU, and {}; nils pipeline lane --gpu-card <n> names a larger one",
                p.label(),
                too_small_words(d, card, total)
            )))
        }
        (Gpu::Optional | Gpu::Required, Some(g)) => Ok((g.to_string(), true)),
        (Gpu::Required, None) if rt.gpu().is_some() => Err(usage(format!(
            "{} needs a GPU, and the pipeline lane uses no card here; nils pipeline lane --gpu-card <n> names the one it leases",
            p.label()
        ))),
        (Gpu::Required, None) => Err(usage(format!(
            "{} needs a GPU, and {} here offers none{}",
            p.label(),
            rt.kind.name(),
            if rt.kind == runtime::Kind::Podman {
                " (podman passes one through a CDI specification, which this host has not got)"
            } else {
                ""
            }
        ))),
    }
}

/// A secret a run was given (record 49 R3): what the descriptor declares,
/// the file the site set, and what is looked for in what the run leaves.
#[derive(Debug, Clone)]
pub(crate) struct Given {
    secret: descriptor::Secret,
    path: PathBuf,
    held: secrets::Held,
}

/// The file the site set for a secret, where it set one.
pub(crate) fn secret_path(registry: &mut Registry, id: &str) -> Option<String> {
    registry
        .meta_value(&format!("{SECRET_PREFIX}{id}"))
        .ok()
        .flatten()
        .filter(|p| !p.trim().is_empty())
}

/// The secrets a pipeline reads, each read now from the file the site set;
/// one it needs and the site has not set, or that cannot be read, is
/// refused before anything runs.
fn secrets_for(registry: &mut Registry, d: &Descriptor, p: &Pipeline) -> Result<Vec<Given>, Exit> {
    let mut out = Vec::new();
    for s in &d.secrets {
        // record 57 P5: a licence file placed in the software place
        // (licences/<app>/) stands where the site set no file
        let path = secret_path(registry, &s.id).or_else(|| {
            crate::software::licence_file_for(registry, p, d, s).map(|f| f.display().to_string())
        });
        match path {
            None if s.optional => {}
            None => {
                return Err(usage(format!(
                    "{} reads the secret {}, which this site has not set: nils software accept {}@{} --file <path> places it in the software place, or nils pipeline secret set {} --file <path>",
                    p.label(),
                    s.id,
                    p.name,
                    p.tool_version,
                    s.id
                )));
            }
            Some(path) => {
                let held = secrets::read(&s.id, Path::new(&path)).map_err(usage)?;
                out.push(Given {
                    secret: s.clone(),
                    path: PathBuf::from(path),
                    held,
                });
            }
        }
    }
    Ok(out)
}

/// Whether a job is running now, as a claim judges it (`job::is_live`):
/// not over, and heard from within the freshness a claim allows or its
/// process on this host still there.
fn job_alive(store: &mut Store, id: i64) -> bool {
    matches!(job::show(store, id), Ok(Some(j)) if job::is_live(&j))
}

/// How often the lane takes a run up again by itself before it leaves the
/// run to a person, so a run that brings its engine down is not started
/// forever.
pub(crate) const AUTO_RESUMES: i64 = 3;

/// The folder beside a unit's log that holds an apptainer container's
/// `/tmp` and `/var/tmp` (its `--workdir`): converted volumes and other
/// clinical-derived files while the unit runs, gone when it ends.
const WORKDIR: &str = "tmp";

/// How long a unit whose card is optional waits for its lease before it
/// runs on the CPU (record 55, after review): 60 s, or `NILS_CARD_GRACE_S`.
fn card_grace() -> Duration {
    let s = std::env::var("NILS_CARD_GRACE_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(60);
    Duration::from_secs(s)
}

/// How long a unit that runs alone and asks the lane's cores waits for half
/// of them before it takes what is free (record 55 T24, second review):
/// 120 s, or `NILS_LANE_GRACE_S`.
fn lane_grace() -> Duration {
    let s = std::env::var("NILS_LANE_GRACE_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(120);
    Duration::from_secs(s)
}

/// How long a run that runs on the card waits for room another tenant
/// holds before it fails, saying why (record 55, third review): 30 minutes,
/// or `NILS_CARD_STALL_S`.
fn card_stall() -> Duration {
    let s = std::env::var("NILS_CARD_STALL_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(1800);
    Duration::from_secs(s)
}

/// Remove every apptainer work folder of a run (its own and each unit's):
/// what a stopped, cancelled or interrupted unit left in its `/tmp`.
pub(crate) fn drop_workdirs(run_dir: &Path) {
    let _ = std::fs::remove_dir_all(run_dir.join(WORKDIR));
    if let Ok(units) = std::fs::read_dir(run_dir.join("units")) {
        for u in units.flatten() {
            let _ = std::fs::remove_dir_all(u.path().join(WORKDIR));
        }
    }
}

/// A new run's folder made its own (2026-10-17), and its output folder
/// (Wave 7b): where `dir` is there
/// already, it holds another run's files, and it is moved aside to
/// `<id>.before-<when>` beside it, never deleted. Answers where it went;
/// a folder that cannot be moved refuses the run, which never starts among
/// another run's files.
fn set_aside(dir: &Path) -> Result<Option<PathBuf>, String> {
    if std::fs::symlink_metadata(dir).is_err() {
        return Ok(None);
    }
    let when: String = nils_registry::time::now_iso()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut to = dir.with_file_name(format!("{name}.before-{when}"));
    let mut n = 1;
    while std::fs::symlink_metadata(&to).is_ok() {
        n += 1;
        to = dir.with_file_name(format!("{name}.before-{when}-{n}"));
    }
    std::fs::rename(dir, &to).map_err(|e| {
        format!(
            "the folder {} holds another run's files, and could not be moved aside: {e}",
            dir.display()
        )
    })?;
    Ok(Some(to))
}

/// What a run says of the files another run left in its folder.
fn moved_words(dir: &Path, to: &Path) -> String {
    format!(
        "this run's folder {} held another run's files, which were moved to {}",
        dir.display(),
        to.display()
    )
}

/// A run's scratch folder, `<scratch place>/runs/<id>`, where its place is
/// still known.
fn run_folder(store: &mut Store, r: &Run) -> Option<PathBuf> {
    let id = r.scratch_place_id.or(r.place_id)?;
    let place = nils_registry::place::show(store, id).ok().flatten()?;
    Some(PathBuf::from(place.path).join(RUNS).join(r.id.to_string()))
}

/// The registry key, with this host's name after it, that holds when the
/// pipeline lane last swept the work folders of this host's runs that
/// ended: lanes and their work folders are each host's own.
const SWEPT_KEY: &str = "pipelines.workdirs_swept_at";

/// At the pipeline lane's start: the work folders of this host's runs that
/// ended since the last sweep (one query), whatever ended them, are
/// removed, and the sweep's time is kept so the next one visits only the
/// runs that end after it. The first sweep also visits the interrupted
/// runs, whose end has no time; after it, the lane that marks a run
/// interrupted clears its folders then.
pub(crate) fn sweep_workdirs(store: &mut Store) -> Result<usize, String> {
    let host = job::hostname();
    let key = format!("{SWEPT_KEY}.{host}");
    let since = meta_text(store, &key)?;
    let now = nils_registry::time::now_iso();
    let runs = rows::runs_ended_since(store, &host, since.as_deref()).map_err(|e| e.to_string())?;
    let mut swept = 0;
    for r in &runs {
        if let Some(dir) = run_folder(store, r) {
            drop_workdirs(&dir);
            swept += 1;
        }
    }
    nils_registry::home::set_meta(store, &key, &now).map_err(|e| e.to_string())?;
    Ok(swept)
}

/// Record 49 A1: a run whose engine went away with units in flight (its
/// job is over, or no longer heard from, and the run still says running)
/// is marked interrupted and queued once to be taken up again, under what
/// the job that ran it recorded: its principal, its detail and its actor.
/// A run someone asked to stop is closed as cancelled instead. Answers how
/// many were queued.
pub(crate) fn take_up_interrupted(store: &mut Store) -> Result<usize, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let mut queued = 0;
    for r in rows::runs_in(store, "running").map_err(err)? {
        if r.job_id.is_some_and(|j| job_alive(store, j)) {
            continue;
        }
        // what its units left in their /tmp goes, whatever comes next
        if let Some(dir) = run_folder(store, &r) {
            drop_workdirs(&dir);
        }
        let old = r.job_id.and_then(|j| job::show(store, j).ok().flatten());
        if old
            .as_ref()
            .is_some_and(|j| matches!(j.state, job::State::Cancelling | job::State::Cancelled))
        {
            let now = nils_registry::time::now_iso();
            rows::finish(
                store,
                r.id,
                &rows::Finish {
                    status: "cancelled",
                    finished_at: &now,
                    exit_code: None,
                    results_digest: None,
                    summary: &json!({"phase": "run"}),
                    error: Some("stopped with its engine; nils run --resume goes on"),
                    failure: None,
                },
            )
            .map_err(err)?;
            continue;
        }
        let why = format!(
            "the engine that ran it went away{}",
            r.job_id.map(|j| format!(" (job {j})")).unwrap_or_default()
        );
        if !rows::interrupt(store, r.id, &why).map_err(err)? {
            continue;
        }
        let resumes = r.resumes.unwrap_or(0);
        if resumes >= AUTO_RESUMES {
            let d = store.dialect();
            let sql = format!(
                "UPDATE {} SET error = {} WHERE id = {}",
                store.qualified("pipeline_run"),
                d.param(1, Type::Text),
                d.param(2, Type::Int)
            );
            store
                .execute(
                    &sql,
                    &[
                        Param::from(format!(
                            "{why}, after being taken up again {resumes} times; nils run --resume {} takes it up by hand",
                            r.id
                        )),
                        Param::Int(r.id),
                    ],
                )
                .map_err(err)?;
            continue;
        }
        let mut extra = old.map(|j| j.args).unwrap_or_else(|| json!({}));
        if let Some(map) = extra.as_object_mut() {
            for key in [
                "argv",
                "queued",
                "then",
                "chain_before",
                "chain_after",
                "run",
            ] {
                map.remove(key);
            }
        } else {
            extra = json!({});
        }
        // a run started at the keyboard recorded no detail, since the
        // keyboard holds every class; its resume is given the least a run
        // needs, quasi, and never more (a door's run keeps what it recorded)
        if extra.get("detail").is_none() && extra.get("roles").is_none() {
            extra["detail"] = json!("quasi");
        }
        let principal = extra["principal"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| r.principal.clone());
        let argv = vec!["run".to_string(), "--resume".to_string(), r.id.to_string()];
        job::enqueue_with(
            store,
            &argv,
            Some(&format!("run {} taken up again", r.id)),
            Some(&principal),
            extra,
        )
        .map_err(|e| e.to_string())?;
        queued += 1;
    }
    Ok(queued)
}

/// `nils run`.
pub(crate) fn run_command(home: &Home, args: RunArgs) -> Result<(), Exit> {
    // Wave 7b section 7: a name that is a family's, not a pipeline's,
    // routes each unit and runs one member run per member
    if let Some(name) = args.pipeline.as_deref()
        && args.resume.is_none()
        && args.go_on.is_none()
        && args.family.is_none()
    {
        let mut registry = crate::open(home)?;
        if rows::resolve(registry.store(), name)?.is_none()
            && let Some(s) = crate::pipeline_family::get(registry.store(), name).map_err(fail)?
        {
            drop(registry);
            return crate::pipeline_family::run(home, &args, &s);
        }
    }
    if args.preflight {
        return crate::preflight::command(home, &args);
    }
    detail_allows_pixels()?;
    if let Some(run) = args.resume {
        return resume_command(home, &args, run);
    }
    let name = args.pipeline.clone().ok_or_else(|| {
        usage("run <pipeline>: the pipeline to run, or --resume <run> to take a run up again")
    })?;
    let mut registry = crate::open(home)?;
    let p = rows::resolve(registry.store(), &name)?.ok_or_else(|| {
        usage(format!(
            "no pipeline {name} in the catalog; nils pipeline list names them"
        ))
    })?;
    if p.state != "active" {
        return Err(usage(format!("{} is {}", p.label(), p.state)));
    }
    // 2026-10-12: a version a later one replaces runs only when asked for
    // by name at the keyboard
    if !args.replaced
        && let Some(refused) = crate::versions::refused(registry.store(), &p)
    {
        return Err(usage(format!(
            "{refused}; nils run {} --replaced runs it still",
            p.label()
        )));
    }
    let d = descriptor::from_value(p.descriptor.clone()).map_err(|e| {
        fail(format!(
            "{}: the catalog's descriptor no longer checks: {e}",
            p.label()
        ))
    })?;
    // record 55 T24: a unit that asks every core of the lane asks this
    // lane's, and its threads parameter is told as many
    let d = d.sized_to(lane_of(&mut registry).ledger.cores);
    // the parts of a split run that go on after one was taken up again run
    // with the parameters it recorded
    let mut params = match &args.go_on {
        Some(g) => g.params.clone(),
        None => {
            let given = pairs(&args.params)?;
            let mut params = d.resolve(&given).map_err(usage)?;
            // record 55 T24, second review: threads the caller set win; else
            // each unit is told the cores it takes at its start, and the run
            // records none
            d.open_lane_threads(&mut params, &given);
            params
        }
    };

    // where it goes and what runs it; either absent is the capability off
    let places = run_places(registry.store()).map_err(usage)?;
    let detected = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
    let rt = detected
        .runtime
        .clone()
        .ok_or_else(|| usage(detected.reason.clone().unwrap_or_default()))?;
    // the lane it runs in: a unit that could never fit, a GPU it cannot
    // lease and a secret the site has not set are refused before anything
    let lane = lane_of(&mut registry);
    // 2026-10-12: what the run needs of this machine, checked before
    // anything is frozen or claimed; a check that fails refuses it by its
    // kind, and nothing is queued
    let checked = crate::requirements::checks(
        &mut registry,
        home.dir(),
        &crate::requirements::Asked {
            pipeline: &p,
            descriptor: &d,
            dcm2niix: &args.dcm2niix,
            units: 1,
            detected: Some(&detected),
            releases: true,
        },
    );
    if let Some(failed) = crate::requirements::refusing(&checked) {
        return Err(fail(failed.refusal()));
    }
    let (device, gpu) = device_for(&d, &p, &rt, &lane)?;
    fits(&d, &p, &lane)?;
    let given = secrets_for(&mut registry, &d, &p)?;

    // the typed inputs: models, a label set, derivatives of a kind
    let model_inputs: Vec<&descriptor::TypedInput> =
        d.inputs.iter().filter(|t| t.ty == "model").collect();
    if args.models.len() > model_inputs.len() {
        return Err(usage(format!(
            "{} reads {} model(s); {} were given",
            p.label(),
            model_inputs.len(),
            args.models.len()
        )));
    }
    if let Some(missing) = model_inputs
        .iter()
        .skip(args.models.len())
        .find(|t| !t.optional)
    {
        return Err(usage(format!(
            "{} needs a model for its input {}: --model <id|digest|name@version>",
            p.label(),
            missing.id
        )));
    }
    let mut models = Vec::new();
    for reference in &args.models {
        let m = nils_registry::model::resolve(registry.store(), reference)?
            .ok_or_else(|| usage(format!("no registered model answers to {reference}")))?;
        // record 55 D4: a retired model's files stay where its card says,
        // so a past result can be run again; its answers are review items,
        // never staged, since only an admitted or promoted model's are
        if m.state == "retired" {
            eprintln!(
                "model {} is retired: it runs again to reproduce a past result, and what it proposes is asked, never staged",
                m.label()
            );
        }
        // record 43: a run may raise a model's threshold, never lower it
        if let Some(t) = args.threshold {
            nils_registry::proposals::threshold(&m, Some(t)).map_err(|e| usage(e.to_string()))?;
        }
        models.push(m);
    }
    // wave 7b §18: an admitted or promoted model's code is trusted by its
    // admission, where the descriptor names the parameter that says so and
    // the caller left it; the parts of a split run keep what the first set
    if args.go_on.is_none() {
        let given = pairs(&args.params)?;
        let admitted: Vec<bool> = models.iter().map(|m| m.answers()).collect();
        for id in d.trust_admitted(&mut params, &given, &admitted) {
            eprintln!(
                "{id} is true: the model's admission trusts the code it brings, after its bytes match"
            );
        }
    }
    if let Some(t) = args.threshold
        && !(t > 0.0 && t <= 1.0)
    {
        return Err(usage(format!(
            "--threshold is a probability above 0 and at most 1, not {t}"
        )));
    }
    let label_input = d.inputs.iter().find(|t| t.ty == "label_set");
    let label_set = match (args.labels, label_input) {
        (Some(_), None) => {
            return Err(usage(format!("{} reads no label set", p.label())));
        }
        (None, Some(t)) if !t.optional => {
            return Err(usage(format!(
                "{} needs a label set for its input {}: --labels <id>",
                p.label(),
                t.id
            )));
        }
        (Some(id), Some(_)) => {
            let set = nils_registry::labels::get(registry.store(), id)
                .map_err(|e| fail(e.to_string()))?
                .ok_or_else(|| usage(format!("no label set {id}")))?;
            if set.sealed {
                return Err(usage(format!(
                    "label set {id} was drawn from a sealed certification sample and is never training data (record 40 R3); a pipeline that reads a label set may train on it"
                )));
            }
            Some(set)
        }
        _ => None,
    };

    // the selection, frozen, and the stacks it holds: whole, or with
    // --chunk in parts of at most that many stacks, each part a run of its
    // own over a handle of its own; a handle the same way (2026-10-17), as
    // the arrival score and a step's door queue a large one; and the parts
    // of a split run still to run after one of them was taken up again
    let (parts, todo): (Vec<(i64, Option<String>)>, Vec<usize>) = match (
        &args.go_on,
        &args.select,
        args.handle,
    ) {
        (Some(g), _, _) => (
            g.handles
                .iter()
                .map(|h| (*h, g.selection.clone()))
                .collect(),
            g.next.clone(),
        ),
        (None, Some(spec), _) => {
            drop(registry);
            let parts = freeze_parts(home, &args, spec, &d.roles)?;
            registry = crate::open(home)?;
            let n = parts.len();
            (
                parts.into_iter().map(|h| (h, Some(spec.clone()))).collect(),
                (0..n).collect(),
            )
        }
        (None, None, Some(h)) => {
            let parts = match args.chunk {
                Some(chunk) => split_handle(registry.store(), h, chunk)?,
                None => vec![h],
            };
            let n = parts.len();
            (
                parts.into_iter().map(|h| (h, None)).collect(),
                (0..n).collect(),
            )
        }
        (None, None, None) => {
            return Err(usage(
                "a run is over a frozen selection: --select selection:<name>@<v>, a dataset's or a cohort's main scans (--select main:<role>@dataset:<name>, or main@dataset:<name> for each role the pipeline needs), or --handle <id>",
            ));
        }
    };
    let count = parts.len();
    // what each part's job records of the parts, so a part taken up again
    // goes on with those after it
    let split = GoOn {
        handles: parts.iter().map(|(h, _)| *h).collect(),
        chunk: args
            .chunk
            .or_else(|| args.go_on.as_ref().and_then(|g| g.chunk)),
        ..GoOn::default()
    };
    let pack = pack_of_bids(home, &args, &d);
    // the trust check of a model's bytes, made once for all the parts
    let verified = Verified::default();
    // one part's run over its handle: claimed, made, executed and
    // concluded; with the run it made and the job it claimed. `made` is
    // called once the part's run is made (or it is not to be), so parts
    // run at once are made in their order; `hold` leaves the job a worker
    // took to the caller, which finishes it once every part is over
    let run_part = |registry: &mut Registry,
                    i: usize,
                    more: bool,
                    hold: bool,
                    started_in_turn: &dyn Fn(bool)|
     -> (Result<(), Exit>, Option<i64>, Option<i64>) {
        let (handle_id, selection) = (parts[i].0, &parts[i].1);
        // the part's run, once it is made, which the words of a part that
        // did not end name
        let mut made: Option<i64> = None;
        let mut claimed: Option<i64> = None;
        let ran = (|| -> Result<(), Exit> {
            let stacks = handle_stacks(registry.store(), handle_id)?;
            if stacks.is_empty() {
                return Err(usage(
                    "the selection holds no stacks; there is nothing to run",
                ));
            }
            // 2026-10-12: the disk its units take, now their count is known
            if d.layout == Layout::Stacks
                && let Some(c) = crate::requirements::disk(registry, &d, stacks.len())
                && !c.ok
            {
                return Err(fail(c.refusal()));
            }
            // 2026-10-12: a run none of whose units could run would make
            // nothing: refused before it is made, with why, as its
            // pre-flight says it
            if let Some(refused) =
                crate::preflight::units_refusal(registry, &d, handle_id, pack.as_ref(), None, None)
                    .map_err(fail)?
            {
                return Err(fail(refused.words));
            }

            // the job: beside every run whose stacks are not these, and
            // refused while a running run holds one of them
            let who = crate::actor();
            let actor = nils_registry::actor::current();
            let job_id = claim_run(
                registry.store(),
                &p,
                handle_id,
                json!({
                    "pipeline": p.label(), "select": selection, "handle": handle_id,
                    "part": (count > 1).then(|| split.part(i)),
                    "params": args.params, "models": args.models, "labels": args.labels,
                    // Wave 7b A7: a step of a workflow run, whose other steps
                    // share the handle
                    "workflow_run": args.chain.as_ref().map(|c| c.workflow_run),
                }),
            )?;
            claimed = Some(job_id);
            let params_value = Value::Object(params.clone());
            let model_ids: Vec<i64> = models.iter().map(|m| m.id).collect();
            let started = nils_registry::time::now_iso();
            let run_id = rows::start(
                registry.store(),
                &rows::NewRun {
                    pipeline_id: p.id,
                    job_id: Some(job_id),
                    handle_id: Some(handle_id),
                    selection: selection.as_deref(),
                    params: &params_value,
                    runtime: rt.kind.name(),
                    runtime_version: &rt.version,
                    host: &job::hostname(),
                    device: &device,
                    model_ids: &model_ids,
                    label_set_id: label_set.as_ref().map(|s| s.id),
                    place_id: Some(places.output.id),
                    scratch_place_id: (places.scratch.id != places.output.id)
                        .then_some(places.scratch.id),
                    principal: &who,
                    actor: Some(&actor),
                    started_at: &started,
                },
            )?;
            made = Some(run_id);
            started_in_turn(false);
            rows::set_units(registry.store(), run_id, d.units.name(), args.threshold)?;
            // Wave 7b A1: the person's choices at the pack stay with the run,
            // so a resume judges under the same ones
            if crate::pipeline_pack::pack_of(&p).is_some() {
                let asked = crate::pipeline_pack::PersonAsk::of_args(
                    args.goal.as_deref(),
                    &args.switches,
                    args.let_run,
                )
                .map_err(fail)?;
                let mut kept = asked.doc();
                kept["given"] = json!(
                    pairs(&args.params)?
                        .into_iter()
                        .map(|(k, _)| k)
                        .collect::<Vec<_>>()
                );
                rows::set_person(registry.store(), run_id, &kept)?;
            }
            let _ = job::set_arg(registry.store(), job_id, "run", json!(run_id));
            // Wave 7b A7: a step of a workflow run says which, and the
            // workflow run learns its run as soon as it is claimed
            if let Some(c) = &args.chain {
                rows::set_workflow_step(registry.store(), run_id, c.workflow_run, c.step as i64)?;
                crate::workflow_run::record_step_run(
                    registry.store(),
                    c.workflow_run,
                    c.step,
                    run_id,
                )
                .map_err(fail)?;
            }
            let licence_env = crate::software::licence_env(registry, &p);

            let outcome = execute(
                home,
                registry,
                &Execution {
                    pipeline: &p,
                    descriptor: &d,
                    params: &params,
                    run_id,
                    job_id,
                    output: &places.output,
                    scratch: &places.scratch,
                    runtime: &rt,
                    gpu,
                    device: &device,
                    stacks: &stacks,
                    models: &models,
                    label_set: label_set.as_ref(),
                    threshold: args.threshold,
                    who: &who,
                    actor: &actor,
                    args: &args,
                    lane: &lane,
                    secrets: &given,
                    licence_env: &licence_env,
                    resume: None,
                    verified: &verified,
                },
            );
            conclude(
                registry,
                &Conclusion {
                    pipeline: &p,
                    run_id,
                    job_id,
                    handle_id,
                    runtime: rt.kind.name(),
                    device: &device,
                    who: &who,
                    json: args.json,
                    quiet: args.chain.is_some(),
                    resumed: false,
                    more,
                    hold,
                },
                outcome,
            )
        })();
        // the turn passes on, with word of a part that did not begin or end well
        started_in_turn(ran.is_err());
        (ran, made, claimed)
    };
    // record 55 §18 (wave 7b): the parts of a split run whose units run
    // together in one container on the CPU run several at once, as many as
    // the lane's budget holds of the part's declared cores and memory; the
    // lane's admission still starts each container only where it fits
    let at_once = parts_at_once(&d, &lane, todo.len(), args.parts_at_once);
    if at_once > 1 {
        return run_parts_at_once(
            home,
            &mut registry,
            &run_part,
            &PartsPlan {
                pipeline: &p,
                parts: &parts,
                todo: &todo,
                split: &split,
                at_once,
            },
        );
    }
    // the job a worker took, which the parts take up in turn and which
    // stays running between them
    let mut kept: Option<i64> = args.go_on.as_ref().and_then(|g| g.kept);
    for (k, &i) in todo.iter().enumerate() {
        let handle_id = parts[i].0;
        // a stop asked between two parts stops the parts from this one
        if let Some(j) = kept.take()
            && matches!(job::beat(registry.store(), j, None), Ok(job::Asked::Cancel))
        {
            let _ = job::finish(
                registry.store(),
                j,
                job::State::Cancelled,
                Some("stopped between two parts"),
            );
            return Err(Exit {
                code: crate::STOPPED,
                message: format!(
                    "stopped before part {} of {count}; the parts not run are handles {}: nils run {} --handle <id> runs one",
                    i + 1,
                    split.words(&todo[k..]),
                    p.label()
                ),
            });
        }
        let (ran, made, claimed) = run_part(&mut registry, i, k + 1 < todo.len(), false, &|_| {});
        kept = claimed.filter(|j| adopted(*j));
        if let Err(e) = ran {
            if count == 1 {
                return Err(e);
            }
            let left = &todo[k + 1..];
            let rest = if left.is_empty() {
                "it was the last part".to_string()
            } else {
                format!(
                    "the parts not run are handles {}: {}nils run {} --handle <id> runs one",
                    split.words(left),
                    made.map(|run| format!(
                        "nils run --resume {run} takes this part up again and goes on with them, and "
                    ))
                    .unwrap_or_default(),
                    p.label()
                )
            };
            return Err(Exit {
                code: e.code,
                message: format!(
                    "{} (part {} of {count}, handle {handle_id}); {rest}",
                    e.message,
                    i + 1
                ),
            });
        }
    }
    Ok(())
}

/// How many parts of a split run go at once (wave 7b §18): one, unless
/// there are several parts, each one container on the CPU (units together,
/// no GPU, not every core of the lane); then as many as the lane's budget
/// holds of the cores and memory a part declares, at most `asked`.
fn parts_at_once(d: &Descriptor, lane: &Lane, parts: usize, asked: Option<usize>) -> usize {
    if parts < 2 || d.units != Units::Together || d.gpu != Gpu::None || d.needs.lane_cores {
        return 1;
    }
    let ask = ask_of(d);
    let by_cores = (lane.ledger.cores / ask.cores.max(1)) as usize;
    let by_memory = (lane.ledger.memory_mib / ask.memory_mib.max(1)) as usize;
    let fits = by_cores.min(by_memory).max(1);
    fits.min(parts).min(asked.unwrap_or(usize::MAX)).max(1)
}

/// A part that ended: its place in the parts to run, how, and its run.
type PartEnded = (usize, Result<(), Exit>, Option<i64>);

/// What the parts run at once share: the pipeline, every part, those to run
/// in their order, how they are named and how many go at once.
struct PartsPlan<'a> {
    pipeline: &'a Pipeline,
    parts: &'a [(i64, Option<String>)],
    todo: &'a [usize],
    split: &'a GoOn,
    at_once: usize,
}

/// One part's run as `run_command` makes it: the registry, the part's
/// place in the parts, whether more follow, whether the job is held for
/// the caller, and what to call once its run is made.
type RunPart<'a> = dyn Fn(
        &mut Registry,
        usize,
        bool,
        bool,
        &dyn Fn(bool),
    ) -> (Result<(), Exit>, Option<i64>, Option<i64>)
    + Sync
    + 'a;

/// The threads of [`run_parts_at_once`]: `at_once` of them take the parts
/// in `todo` in order, a part's turn coming once the one before has made
/// its run (or failed, or stopped the rest). What ended, in part order, and
/// whether the parts were stopped.
fn schedule_parts(
    home: &Home,
    todo: &[usize],
    at_once: usize,
    of: usize,
    adopt: Option<i64>,
    run_part: &RunPart<'_>,
) -> (Vec<PartEnded>, bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex};
    let count = of;
    let next = Mutex::new(0usize);
    // the parts whose run is made, in their order
    let turn = (Mutex::new(0usize), Condvar::new());
    let stop = AtomicBool::new(false);
    let ended: Mutex<Vec<PartEnded>> = Mutex::new(Vec::new());
    let wait_turn = |k: usize| {
        let (lock, cv) = &turn;
        let mut at = lock.lock().unwrap_or_else(|e| e.into_inner());
        while *at < k {
            at = cv.wait(at).unwrap_or_else(|e| e.into_inner());
        }
    };
    let pass_turn = |k: usize| {
        let (lock, cv) = &turn;
        let mut at = lock.lock().unwrap_or_else(|e| e.into_inner());
        if *at == k {
            *at = k + 1;
        }
        cv.notify_all();
    };
    std::thread::scope(|scope| {
        for _ in 0..at_once {
            scope.spawn(|| {
                let mut own = crate::open(home);
                loop {
                    let k = {
                        let mut n = next.lock().unwrap_or_else(|e| e.into_inner());
                        if *n >= todo.len() || stop.load(Ordering::SeqCst) {
                            return;
                        }
                        *n += 1;
                        *n - 1
                    };
                    wait_turn(k);
                    let reg = match own.as_mut() {
                        Ok(r) => r,
                        Err(e) => {
                            stop.store(true, Ordering::SeqCst);
                            pass_turn(k);
                            let e = Exit {
                                code: e.code,
                                message: e.message.clone(),
                            };
                            ended
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push((k, Err(e), None));
                            return;
                        }
                    };
                    // a stop asked of the job stops the parts not yet begun
                    if stop.load(Ordering::SeqCst)
                        || adopt.is_some_and(|j| {
                            matches!(job::beat(reg.store(), j, None), Ok(job::Asked::Cancel))
                        })
                    {
                        stop.store(true, Ordering::SeqCst);
                        pass_turn(k);
                        return;
                    }
                    let i = todo[k];
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run_part(reg, i, true, true, &|failed| {
                            // a part that fails stops the next before its turn
                            // comes, so the next never starts after it
                            if failed {
                                stop.store(true, Ordering::SeqCst);
                            }
                            pass_turn(k);
                        })
                    }));
                    pass_turn(k);
                    let (ran, made) = match outcome {
                        Ok((ran, made, _)) => (ran, made),
                        Err(_) => (
                            Err(fail(format!("part {} of {count} panicked", i + 1))),
                            None,
                        ),
                    };
                    if ran.is_err() {
                        stop.store(true, Ordering::SeqCst);
                    }
                    ended
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((k, ran, made));
                }
            });
        }
    });
    let mut ended = ended.into_inner().unwrap_or_else(|e| e.into_inner());
    ended.sort_by_key(|(k, _, _)| *k);
    (ended, stop.load(Ordering::SeqCst))
}

/// The parts of a split run, `plan.at_once` at a time, each on a thread of
/// its own with a registry connection of its own (wave 7b §18). They are
/// handed out and their runs made in their order, so run numbers, handles
/// and results read as the parts one after another would; each part is a
/// run of its own with its own folder and log. A part that fails or is
/// stopped stops the parts not yet begun, and those already going end as
/// they end. The job a worker took is every part's and ends once all are
/// over: done, or as the first part in order that did not end done.
fn run_parts_at_once(
    home: &Home,
    registry: &mut Registry,
    run_part: &RunPart<'_>,
    plan: &PartsPlan<'_>,
) -> Result<(), Exit> {
    let count = plan.parts.len();
    let todo = plan.todo;
    eprintln!(
        "{} parts, {} at once within the lane's budget",
        todo.len(),
        plan.at_once
    );
    // the worker's job this process runs as (after the third review, finding
    // 6: as the parts in turn read it, never the variable by hand)
    let adopt: Option<i64> = job::adopted_job();
    let (ended, stopped) = schedule_parts(home, todo, plan.at_once, count, adopt, run_part);
    let done: std::collections::BTreeSet<usize> = ended
        .iter()
        .filter(|(_, r, _)| r.is_ok())
        .map(|(k, _, _)| *k)
        .collect();
    let first = ended.into_iter().find(|(_, r, _)| r.is_err());
    let left: Vec<usize> = (0..todo.len())
        .filter(|k| !done.contains(k))
        .map(|k| todo[k])
        .collect();
    let failed = first.as_ref().map(|(_, r, _)| match r {
        Err(e) => e.code,
        Ok(()) => 0,
    });
    // the job every part held: over now
    if let Some(j) = adopt {
        let (state, why) = match (&first, stopped) {
            (None, false) => (job::State::Done, None),
            (None, true) => (
                job::State::Cancelled,
                Some("stopped between two parts".to_string()),
            ),
            (Some(_), _) if failed == Some(crate::STOPPED) => (
                job::State::Cancelled,
                Some("a part was stopped".to_string()),
            ),
            (Some((_, Err(e), _)), _) => (job::State::Failed, Some(e.message.clone())),
            (Some(_), _) => (job::State::Failed, None),
        };
        job::finish(registry.store(), j, state, why.as_deref()).map_err(|e| fail(e.to_string()))?;
    }
    let p = plan.pipeline;
    match first {
        None if left.is_empty() => Ok(()),
        None => Err(Exit {
            code: crate::STOPPED,
            message: format!(
                "stopped between two parts; the parts not run are handles {}: nils run {} --handle <id> runs one",
                plan.split.words(&left),
                p.label()
            ),
        }),
        Some((k, Err(e), made)) => {
            let i = todo[k];
            let others: Vec<usize> = left.into_iter().filter(|x| *x != i).collect();
            let rest = if others.is_empty() {
                "every other part ended done".to_string()
            } else {
                format!(
                    "the parts not done are handles {}: {}nils run {} --handle <id> runs one",
                    plan.split.words(&others),
                    made.map(|run| format!(
                        "nils run --resume {run} takes this part up again, and "
                    ))
                    .unwrap_or_default(),
                    p.label()
                )
            };
            Err(Exit {
                code: e.code,
                message: format!(
                    "{} (part {} of {count}, handle {}); {rest}",
                    e.message,
                    i + 1,
                    plan.parts[i].0
                ),
            })
        }
        Some(_) => Ok(()),
    }
}

/// A selection frozen for a run: whole, one handle of every stack it
/// holds however many, or with `--chunk` one handle per part of at most
/// that many stacks, in stack order. Each part is frozen from the saved
/// selection as a page after the last stack of the one before.
fn freeze_parts(
    home: &Home,
    args: &RunArgs,
    spec: &str,
    needs: &[String],
) -> Result<Vec<i64>, Exit> {
    use crate::ask_cli::{Freeze, freeze};
    // record 55 (rulings of 2026-10-12, item 6): a dataset's or a cohort's
    // main scans, resolved once and split as they are frozen; without a
    // role, those of each role the pipeline needs (`needs`)
    if let Some(main) = crate::main_input::Spec::parse(spec) {
        let main = main.map_err(usage)?;
        return crate::main_input::freeze_parts(
            home,
            &main,
            args.pack_dir.clone(),
            &args.pack,
            args.chunk,
            needs,
        );
    }
    let grain = nils_ask::ast::Grain::Stack;
    let Some(chunk) = args.chunk else {
        let f = freeze(
            home,
            spec,
            grain,
            args.pack_dir.clone(),
            &args.pack,
            Freeze::Whole,
        )?;
        return Ok(vec![f.handle]);
    };
    if chunk == 0 {
        return Err(usage("--chunk is a number of stacks, at least 1"));
    }
    let mut parts = Vec::new();
    let mut after = None;
    let mut stacks = 0usize;
    loop {
        let f = freeze(
            home,
            spec,
            grain,
            args.pack_dir.clone(),
            &args.pack,
            Freeze::Page {
                after,
                limit: chunk,
            },
        )?;
        if f.keys == 0 {
            // an empty selection is refused as a whole one is, by its handle
            if parts.is_empty() {
                parts.push(f.handle);
            }
            break;
        }
        parts.push(f.handle);
        stacks += f.keys;
        if (f.keys as u64) < chunk {
            break;
        }
        after = f.last;
        if after.is_none() {
            break;
        }
    }
    if parts.len() > 1 {
        eprintln!(
            "{spec}: {stacks} stack(s) in {} parts of at most {chunk}, handles {}",
            parts.len(),
            parts
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(parts)
}

/// A handle run with `--chunk` (2026-10-17): as it is where it holds at
/// most that many stacks, else one handle per part of at most that many,
/// in stack order. Each part holds its stacks with their subjects, is
/// pinned as the handle is (its selections, pack version, epoch, scheme
/// and principal) and records which part of which handle it is. A
/// handle of a scope's main scans of several roles is split only as it is
/// frozen, in whole visits (`--select main@...:NAME --chunk N`).
fn split_handle(store: &mut Store, handle: i64, chunk: u64) -> Result<Vec<i64>, Exit> {
    if chunk == 0 {
        return Err(usage("--chunk is a number of stacks, at least 1"));
    }
    let stacks = handle_stacks(store, handle)?;
    let size = usize::try_from(chunk).unwrap_or(usize::MAX);
    if stacks.len() <= size {
        return Ok(vec![handle]);
    }
    let h = nils_ask::handle::get(store, handle)
        .map_err(|e| fail(e.to_string()))?
        .ok_or_else(|| usage(format!("no handle {handle}")))?;
    if let Some(entry) = crate::main_input::entry_in(&h.selection_versions)
        && entry["roles"].is_array()
    {
        return Err(usage(format!(
            "handle {handle} holds the main scans of several roles, which are split in whole visits as they are frozen: nils run --select {} --chunk {chunk}",
            entry["of"].as_str().unwrap_or("main@dataset:<name>")
        )));
    }
    let subjects: HashMap<i64, Option<i64>> = nils_ask::handle::keys(store, handle)
        .map_err(|e| fail(e.to_string()))?
        .into_iter()
        .collect();
    let parts: Vec<&[i64]> = stacks.chunks(size).collect();
    let of = parts.len();
    let mut out = Vec::with_capacity(of);
    for (i, part) in parts.iter().enumerate() {
        let rows: Vec<nils_registry::store::Row> = part
            .iter()
            .map(|s| {
                use nils_registry::store::{Cell, Row};
                Row(vec![
                    Cell::Int(*s),
                    subjects
                        .get(s)
                        .copied()
                        .flatten()
                        .map_or(Cell::Null, Cell::Int),
                ])
            })
            .collect();
        let mut pins = h.selection_versions.as_array().cloned().unwrap_or_default();
        // a scope's main scans of one role say their part, as a run's
        // --select main:... records it
        for pin in &mut pins {
            if pin["main"].is_string() && pin["picks"].is_object() {
                pin["part"] = json!({"part": i + 1, "of": of});
            }
        }
        pins.push(json!({"handle": handle, "part": {"part": i + 1, "of": of}}));
        let (id, _) = crate::operations::save_stacks(
            store,
            rows,
            &h.principal,
            h.epoch,
            &crate::operations::Pinned {
                versions: Value::Array(pins),
                pack_version: h.pack_version.as_deref(),
                scheme_digest: h.scheme_digest.as_deref(),
            },
        )
        .map_err(fail)?;
        out.push(id);
    }
    eprintln!(
        "handle {handle}: {} stack(s) in {of} parts of at most {chunk}, handles {}",
        stacks.len(),
        out.iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(out)
}

/// The run that pins a handle, the first where there are several.
fn run_over(store: &mut Store, handle: i64) -> Result<Option<i64>, Exit> {
    let sql = format!(
        "SELECT MIN(id) FROM {} WHERE handle_id = {}",
        store.qualified("pipeline_run"),
        store.dialect().param(1, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(handle)])
        .map_err(|e| fail(e.to_string()))?
        .map_or(Ok(None), |r| r.opt_int(0))
        .map_err(|e| fail(e.to_string()))
}

/// The registry locks runs take around what they read and then write that
/// every run shares, so runs that go on side by side take those steps one
/// run at a time (see `nils_registry::lock`): the claim, which looks at
/// the stacks the running runs hold; the lane, whose budget a unit is
/// started within; the intake of what a run made into the review spine,
/// the models and the embeddings; the release of a bids input; and the
/// build of an image apptainer keeps.
pub(crate) const CLAIM_LOCK: &str = "pipeline-claim";
pub(crate) const LANE_LOCK: &str = "pipeline-lane";
pub(crate) const INTAKE_LOCK: &str = "pipeline-intake";
pub(crate) const RELEASE_LOCK: &str = "pipeline-release";

/// The lock of one image apptainer keeps, by its digest.
fn image_lock(digest: &str) -> String {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    format!("pipeline-image-{}", &hex[..hex.len().min(16)])
}

/// How long a run waits between looks at a lock another run holds.
const LOCK_LOOK: Duration = Duration::from_millis(50);

/// Take a registry lock, waiting while another run holds it and beating
/// the job's heart meanwhile, so a wait is never read as a run that went
/// away. Where `cancellable`, a cancel asked while it waits answers none.
fn wait_lock(
    store: &mut Store,
    name: &str,
    job_id: Option<i64>,
    cancellable: bool,
) -> Result<Option<nils_registry::lock::Held>, String> {
    let mut beaten = Instant::now();
    loop {
        if let Some(held) = nils_registry::lock::try_take(store, name)
            .map_err(|e| format!("the registry's {name} lock: {e}"))?
        {
            return Ok(Some(held));
        }
        if let Some(job_id) = job_id
            && beaten.elapsed() >= Duration::from_secs(5)
        {
            beaten = Instant::now();
            if let Ok(job::Asked::Cancel) = job::beat(store, job_id, None)
                && cancellable
            {
                return Ok(None);
            }
        }
        std::thread::sleep(LOCK_LOOK);
    }
}

/// Run `f` under a registry lock, taken as [`wait_lock`] takes it and
/// given back however `f` ends. The registry's metadata is read again
/// first, since another run may have moved its epoch.
fn with_lock<T>(
    registry: &mut Registry,
    name: &str,
    job_id: Option<i64>,
    f: impl FnOnce(&mut Registry) -> Result<T, String>,
) -> Result<T, String> {
    let held = wait_lock(registry.store(), name, job_id, false)?
        .ok_or_else(|| format!("the registry's {name} lock was not taken"))?;
    let _ = registry.refresh_meta();
    let out = f(registry);
    let released = nils_registry::lock::release(registry.store(), held)
        .map_err(|e| format!("the registry's {name} lock: {e}"));
    let out = out?;
    released?;
    Ok(out)
}

/// Whether a running pipeline job holds any of the stacks of `handle`:
/// the words that say which run and how many, where it does. A job that
/// has not said which stacks it runs over holds them all until it does.
/// Wave 7b A7 (section 8.4): the steps of one workflow run share the
/// handle by design, so a run that is a step of the same workflow run
/// (`workflow_run` in its args) never holds this one off.
fn holds_stacks(
    store: &mut Store,
    handle: i64,
    workflow_run: Option<i64>,
    job_id: i64,
    args: &Value,
) -> Result<Option<String>, job::Error> {
    if let (Some(ours), Some(theirs)) = (workflow_run, args["workflow_run"].as_i64())
        && ours == theirs
    {
        return Ok(None);
    }
    let who = match (
        args["run"].as_i64().or(args["resume"].as_i64()),
        args["pipeline"].as_str(),
    ) {
        (Some(run), Some(p)) => format!("run {run} of {p} (job {job_id})"),
        (Some(run), None) => format!("run {run} (job {job_id})"),
        (None, Some(p)) => format!("a run of {p} (job {job_id})"),
        (None, None) => format!("pipeline job {job_id}"),
    };
    let cure = format!(
        "runs over the same stacks run one after another: wait for it to end, or nils jobs cancel {job_id} if it is not running anywhere"
    );
    let Some(theirs) = args["handle"].as_i64() else {
        return Ok(Some(format!(
            "{who} is running and has not yet said which stacks it runs over; {cure}"
        )));
    };
    let d = store.dialect();
    let sql = format!(
        "SELECT COUNT(DISTINCT a.key), MIN(a.key) FROM {m} a JOIN {m} b \
         ON b.handle_id = {} AND b.key = a.key WHERE a.handle_id = {}",
        d.param(1, Type::Int),
        d.param(2, Type::Int),
        m = store.qualified("handle_member"),
    );
    let row = store.query_opt(&sql, &[Param::Int(theirs), Param::Int(handle)])?;
    let (shared, first) = match row {
        Some(r) => (r.int(0)?, r.opt_int(1)?),
        None => (0, None),
    };
    if shared == 0 {
        return Ok(None);
    }
    Ok(Some(format!(
        "{who} holds {shared} of the stacks this run would take{}; {cure}",
        first
            .map(|s| format!(", stack {s} the first"))
            .unwrap_or_default()
    )))
}

/// Claim the job a run is (record 43 R1, as widened on 2026-09-30): runs
/// whose stacks no other running run holds go on side by side, and a run
/// that would take a stack a running run holds is refused, naming that
/// run. The claim is taken under the claim lock, so two runs over the
/// same stacks that start at once never both pass.
fn claim_run(store: &mut Store, p: &Pipeline, handle: i64, args: Value) -> Result<i64, Exit> {
    let lock = wait_lock(store, CLAIM_LOCK, None, false)
        .map_err(fail)?
        .ok_or_else(|| fail("the claim lock was not taken"))?;
    let workflow_run = args["workflow_run"].as_i64();
    let claimed = job::claim_beside(
        store,
        &job::Claim {
            kind: "pipeline",
            name: &p.label(),
            args,
        },
        &mut |store: &mut Store, id: i64, its: &Value| {
            holds_stacks(store, handle, workflow_run, id, its)
        },
    );
    let released = nils_registry::lock::release(store, lock);
    let id = claimed.map_err(|e| match e {
        job::Error::Busy { .. } | job::Error::Held { .. } => Exit {
            code: crate::BUSY,
            message: e.to_string(),
        },
        other => fail(other.to_string()),
    })?;
    released.map_err(|e| fail(e.to_string()))?;
    Ok(id)
}

/// The pack the release of a bids input names its stacks by, which its
/// pre-flight plans the units with; none for any other input, which reads
/// no pack.
fn pack_of_bids(home: &Home, args: &RunArgs, d: &Descriptor) -> Option<nils_pack::Pack> {
    if d.layout != Layout::Bids {
        return None;
    }
    crate::pack_dir(home, args.pack_dir.clone())
        .ok()
        .and_then(|dir| crate::ask_cli::load_pack(&dir, &args.pack).ok())
}

/// Why a run cannot be taken up again, where it cannot (record 49 A1): one
/// that is done or partial has nothing left to run, one that failed after
/// its containers ended is its review items, and one that runs is running.
pub(crate) fn resume_refused(store: &mut Store, r: &Run) -> Option<Exit> {
    match r.status.as_str() {
        "done" | "partial" => Some(usage(format!(
            "run {} is {}: nothing is left to run",
            r.id, r.status
        ))),
        "failed" if r.results_digest.is_some() => Some(usage(format!(
            "run {} failed after its containers ended, and its failures are review items; start a new run",
            r.id
        ))),
        "running" => r.job_id.filter(|j| job_alive(store, *j)).map(|j| Exit {
            code: crate::BUSY,
            message: format!("run {} is running, as job {j}", r.id),
        }),
        _ => None,
    }
}

/// `nils run --resume <run>` (record 49 A1): a run that stopped, was
/// cancelled or whose engine went away is taken up again with everything
/// it recorded. The units it finished are kept, with their derivatives and
/// their outcomes; the units in flight run again from a clean folder.
fn resume_command(home: &Home, args: &RunArgs, run_id: i64) -> Result<(), Exit> {
    let mut registry = crate::open(home)?;
    let r = rows::run(registry.store(), run_id)?
        .ok_or_else(|| usage(format!("no pipeline run {run_id}")))?;
    if let Some(refused) = resume_refused(registry.store(), &r) {
        return Err(refused);
    }
    let p = rows::get(registry.store(), r.pipeline_id)?
        .ok_or_else(|| fail(format!("run {run_id} names a pipeline the catalog lost")))?;
    let d = descriptor::from_value(p.descriptor.clone())
        .map_err(|e| {
            fail(format!(
                "{}: the catalog's descriptor no longer checks: {e}",
                p.label()
            ))
        })?
        .sized_to(lane_of(&mut registry).ledger.cores);
    let params = r
        .params
        .as_object()
        .cloned()
        .ok_or_else(|| fail(format!("run {run_id} recorded no parameters")))?;
    let place = r
        .place_id
        .and_then(|id| {
            nils_registry::place::show(registry.store(), id)
                .ok()
                .flatten()
        })
        .ok_or_else(|| usage(format!("run {run_id} has no working place to go on in")))?;
    // its scratch where it kept it: a run from before, or one whose lane
    // named no other, kept it with its outputs
    let scratch = match r.scratch_place_id {
        None => place.clone(),
        Some(id) => nils_registry::place::show(registry.store(), id)
            .ok()
            .flatten()
            .ok_or_else(|| usage(format!("run {run_id}'s scratch place {id} is gone")))?,
    };
    let detected = runtime::detect(choice(&mut registry), std::env::var_os("PATH").as_deref());
    let rt = detected
        .runtime
        .clone()
        .ok_or_else(|| usage(detected.reason.clone().unwrap_or_default()))?;
    if rt.kind.name() != r.runtime {
        return Err(usage(format!(
            "run {run_id} ran under {}, and pipelines here run under {} now; its units would not be alike, so start a new run",
            r.runtime,
            rt.kind.name()
        )));
    }
    let lane = lane_of(&mut registry);
    let (mut device, mut gpu) = device_for(&d, &p, &rt, &lane)?;
    // a run that went to the CPU off a full optional card goes on there,
    // so a run never mixes devices
    if r.device == "cpu" && d.gpu == Gpu::Optional {
        (device, gpu) = ("cpu".to_string(), false);
    }
    if (r.device == "cpu") != (device == "cpu") {
        return Err(usage(format!(
            "run {run_id} ran on {}, and would go on on {device}; start a new run",
            r.device
        )));
    }
    fits(&d, &p, &lane)?;
    let given = secrets_for(&mut registry, &d, &p)?;
    let mut models = Vec::new();
    for id in r
        .model_ids
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
    {
        models.push(
            nils_registry::model::get(registry.store(), id)?
                .ok_or_else(|| fail(format!("run {run_id} read model {id}, which is gone")))?,
        );
    }
    let label_set = match r.label_set_id {
        Some(id) => Some(
            nils_registry::labels::get(registry.store(), id)
                .map_err(|e| fail(e.to_string()))?
                .ok_or_else(|| fail(format!("run {run_id} read label set {id}, which is gone")))?,
        ),
        None => None,
    };
    let handle_id = r
        .handle_id
        .ok_or_else(|| fail(format!("run {run_id} pinned no handle")))?;
    let stacks = handle_stacks(registry.store(), handle_id)?;
    // the containers its engine left running are stopped first, as taking
    // it up always did, so none of them goes on to its end while it is
    // checked: their units run again either way
    let left = rows::units(registry.store(), run_id)?;
    stop_left(
        &rt,
        d.units == Units::Together,
        &left,
        &PathBuf::from(&place.path)
            .join(derivative::TREE)
            .join(&p.name)
            .join(run_id.to_string()),
    );
    // 2026-10-12: taken up again only where it would run, as a new run is
    // checked: what it needs of this machine (the converter only where its
    // bids input is still to be released), then the units it still has to
    // run; where it could run none of them and made nothing, it is refused
    // before a job is claimed, with why
    let checked = crate::requirements::checks(
        &mut registry,
        home.dir(),
        &crate::requirements::Asked {
            pipeline: &p,
            descriptor: &d,
            dcm2niix: &args.dcm2niix,
            units: 1,
            detected: Some(&detected),
            releases: r.input_release_id.is_none(),
        },
    );
    if let Some(failed) = crate::requirements::refusing(&checked) {
        return Err(fail(failed.refusal()));
    }
    let pack = pack_of_bids(home, args, &d);
    if let Some(refused) = crate::preflight::units_refusal(
        &mut registry,
        &d,
        handle_id,
        pack.as_ref(),
        None,
        Some(run_id),
    )
    .map_err(fail)?
    {
        return Err(fail(refused.words));
    }
    // 2026-10-17: a part of a split run, as its job recorded the parts: the
    // resume records them again, and goes on with those after it
    let split = r
        .job_id
        .and_then(|j| job::show(registry.store(), j).ok().flatten())
        .map(|j| j.args["part"].clone())
        .filter(Value::is_object);
    let who = crate::actor();
    let actor = nils_registry::actor::current();
    let mut claimed = json!({
        "pipeline": p.label(), "resume": run_id, "handle": handle_id,
        "workflow_run": r.workflow_run_id,
    });
    if let Some(part) = &split {
        claimed["part"] = part.clone();
    }
    let job_id = claim_run(registry.store(), &p, handle_id, claimed)?;
    rows::resume(registry.store(), run_id, job_id)?;
    let _ = job::set_arg(registry.store(), job_id, "run", json!(run_id));
    let licence_env = crate::software::licence_env(&mut registry, &p);
    let outcome = execute(
        home,
        &mut registry,
        &Execution {
            pipeline: &p,
            descriptor: &d,
            params: &params,
            run_id,
            job_id,
            output: &place,
            scratch: &scratch,
            runtime: &rt,
            gpu,
            device: &device,
            stacks: &stacks,
            models: &models,
            label_set: label_set.as_ref(),
            threshold: r.threshold,
            who: &who,
            actor: &actor,
            args,
            lane: &lane,
            secrets: &given,
            licence_env: &licence_env,
            resume: Some(&r),
            verified: &Verified::default(),
        },
    );
    // the parts after it that no run holds yet go on once it ends, one
    // after another, as the split run would have run them; a part that a
    // run holds already (run by hand) is left as it is
    let mut go_on = split.as_ref().and_then(GoOn::recorded);
    let mut after = 0;
    if let Some(g) = go_on.as_mut() {
        after = g
            .handles
            .iter()
            .position(|h| *h == handle_id)
            .map_or(g.handles.len(), |at| at + 1);
        for i in after..g.handles.len() {
            match run_over(registry.store(), g.handles[i])? {
                Some(other) => eprintln!(
                    "part {} of {} (handle {}) is run {other}, which is left as it is",
                    i + 1,
                    g.handles.len(),
                    g.handles[i]
                ),
                None => g.next.push(i),
            }
        }
    }
    let go_on = go_on.filter(|g| !g.next.is_empty());
    let concluded = conclude(
        &mut registry,
        &Conclusion {
            pipeline: &p,
            run_id,
            job_id,
            handle_id,
            runtime: rt.kind.name(),
            device: &device,
            who: &who,
            json: args.json,
            quiet: args.chain.is_some(),
            resumed: true,
            more: go_on.is_some(),
            hold: false,
        },
        outcome,
    );
    let Some(mut go_on) = go_on else {
        return concluded;
    };
    if let Err(e) = concluded {
        return Err(Exit {
            code: e.code,
            message: format!(
                "{}; the parts after it, handles {}, go on once it is taken up again and ends",
                e.message,
                go_on.words(&go_on.next)
            ),
        });
    }
    eprintln!(
        "run {run_id} was part {after} of {}: the parts after it go on, handles {}",
        go_on.handles.len(),
        go_on.words(&go_on.next)
    );
    go_on.params = params;
    go_on.selection = r.selection.clone();
    go_on.kept = Some(job_id).filter(|j| adopted(*j));
    drop(registry);
    run_command(
        home,
        RunArgs {
            pipeline: Some(p.id.to_string()),
            resume: None,
            select: None,
            handle: None,
            params: Vec::new(),
            models: models.iter().map(|m| m.id.to_string()).collect(),
            labels: r.label_set_id,
            threshold: r.threshold,
            pack: args.pack.clone(),
            pack_dir: args.pack_dir.clone(),
            dcm2niix: args.dcm2niix.clone(),
            chunk: None,
            parts_at_once: args.parts_at_once,
            preflight: false,
            // the parts go on with the version the run was split for
            replaced: true,
            // record 57 P2: and with the goal and switches the run was given
            goal: args.goal.clone(),
            switches: args.switches.clone(),
            let_run: args.let_run,
            json: args.json,
            go_on: Some(go_on),
            family: None,
            chain: args.chain.clone(),
        },
    )
}

/// `nils pipeline ingest <run>`: a closed run's proposals taken in after
/// it closed, as its intake takes them ([`finalize`]): read again from the
/// results.json of its output folder, or of each unit that ended where its
/// units ran apart; only those on the axes its descriptor declares, of its
/// own stacks, and of the models it was given or made, at the threshold it
/// was run with. A run whose proposals are in already takes nothing. The
/// run's summary says what was taken, and when.
fn ingest_run(registry: &mut Registry, run_id: i64) -> Result<Value, Exit> {
    let r = rows::run(registry.store(), run_id)?
        .ok_or_else(|| usage(format!("no pipeline run {run_id}")))?;
    // only a run whose containers ended and whose results were read
    let closed = matches!(r.status.as_str(), "done" | "partial")
        || (r.status == "failed" && r.results_digest.is_some());
    if !closed {
        return Err(usage(format!(
            "run {run_id} is {}: only a run that ended with its results read is taken in again",
            r.status
        )));
    }
    let p = rows::get(registry.store(), r.pipeline_id)?
        .ok_or_else(|| fail(format!("run {run_id} names a pipeline the catalog lost")))?;
    let d = descriptor::from_value(p.descriptor.clone()).map_err(|e| {
        fail(format!(
            "{}: the catalog's descriptor no longer checks: {e}",
            p.label()
        ))
    })?;
    let held = nils_registry::proposals::held(registry.store(), run_id)
        .map_err(|e| fail(e.to_string()))?;
    let mut doc = json!({"run": run_id, "pipeline": p.label(), "taken": 0, "in_already": held});
    if held > 0 {
        doc["line"] = json!(format!(
            "run {run_id}: its proposals are in already, as {held} review item(s); nothing was taken"
        ));
        return Ok(doc);
    }
    let place = r
        .place_id
        .and_then(|id| {
            nils_registry::place::show(registry.store(), id)
                .ok()
                .flatten()
        })
        .ok_or_else(|| usage(format!("run {run_id} has no working place to read from")))?;
    let output = r
        .output
        .as_deref()
        .ok_or_else(|| usage(format!("run {run_id} names no output folder")))?;
    let out = PathBuf::from(&place.path).join(output);
    let units = rows::units(registry.store(), run_id)?;
    let folders: Vec<PathBuf> = if r.units.as_deref() == Some("apart") {
        units
            .iter()
            .filter(|u| u.state == "over")
            .map(|u| out.join(&u.unit))
            .collect()
    } else {
        vec![out.clone()]
    };
    let mut proposals: Vec<Value> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    for f in &folders {
        match read_results(f) {
            (Some(results), _) => proposals.extend(results.proposals),
            (None, Some(why)) => unreadable.push(why),
            (None, None) => {}
        }
    }
    if let Some(why) = unreadable.first() {
        return Err(fail(format!(
            "run {run_id}: {} results.json file(s) could not be read, the first as {why}; nothing was taken",
            unreadable.len()
        )));
    }
    let declared: Vec<Value> = proposals
        .iter()
        .filter(|p| {
            p["axis"]
                .as_str()
                .is_some_and(|a| d.proposals.iter().any(|x| x == a))
        })
        .cloned()
        .collect();
    doc["given"] = json!(proposals.len());
    doc["declared"] = json!(declared.len());
    if declared.is_empty() {
        doc["line"] = json!(format!(
            "run {run_id}: it proposed nothing on the axes it declares ({} proposal(s) in all); nothing was taken",
            proposals.len()
        ));
        return Ok(doc);
    }
    // its own stacks, and the models it was given or made, as at its intake
    let ours: std::collections::BTreeSet<i64> = match r.handle_id {
        Some(h) => handle_stacks(registry.store(), h)?.into_iter().collect(),
        None => return Err(fail(format!("run {run_id} pinned no handle"))),
    };
    let speaks_for: std::collections::BTreeSet<i64> = r
        .model_ids
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .chain(
            r.summary["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["model"]["id"].as_i64()),
        )
        .collect();
    let parsed = nils_registry::proposals::parse(&json!({ "proposals": declared }))
        .map_err(|e| fail(format!("run {run_id}: {e}")))?;
    let who = crate::actor();
    let taken = with_lock(registry, INTAKE_LOCK, None, |registry| {
        nils_registry::proposals::ingest(
            registry,
            &nils_registry::proposals::Run {
                id: run_id,
                job_id: r.job_id,
                principal: &who,
                stacks: Some(&ours),
                models: Some(&speaks_for),
            },
            &parsed,
            r.threshold,
        )
        .map_err(|e| format!("run {run_id}: {e}"))
    })
    .map_err(fail)?;
    // the run's summary says what its proposals came to now
    let mut summary = r.summary.clone();
    if !summary.is_object() {
        summary = json!({});
    }
    let mut proposed = summary["proposals"].clone();
    if !proposed.is_object() {
        proposed = json!({});
    }
    proposed["given"] = json!(proposals.len());
    proposed["declared"] = json!(declared.len());
    proposed["taken"] = json!(taken.members);
    proposed["ingested"] = taken.to_json();
    if let Some(o) = proposed.as_object_mut()
        && let Some(refused) = o.remove("refused")
    {
        o.insert("refused_at_intake".into(), refused);
    }
    proposed["taken_later"] = json!({"at": nils_registry::time::now_iso(), "by": who});
    summary["proposals"] = proposed;
    rows::set_summary(registry.store(), run_id, &summary)?;
    crate::audit(
        registry,
        nils_registry::audit::Action::PipelineRun,
        json!({"run": run_id, "pipeline": p.label(), "proposals_taken_in": true}),
        Some(json!({"members": taken.members, "items": taken.groups.len()})),
    )?;
    doc["taken"] = json!(taken.members);
    doc["ingested"] = taken.to_json();
    doc["line"] = json!(format!(
        "run {run_id}: took in {} proposal(s) as {} review item(s), of {} on the axes it declares",
        taken.members,
        taken.groups.len(),
        declared.len()
    ));
    Ok(doc)
}

/// What closing a run needs to say.
struct Conclusion<'a> {
    pipeline: &'a Pipeline,
    run_id: i64,
    job_id: i64,
    handle_id: i64,
    runtime: &'a str,
    device: &'a str,
    who: &'a str,
    json: bool,
    /// Wave 7b A7: a step of a workflow run prints nothing of its own; the
    /// workflow run prints the chain once every step is over.
    quiet: bool,
    resumed: bool,
    /// Parts of the same split run follow in this process.
    more: bool,
    /// Parts of the same split run go on at once in this process: the job a
    /// worker took is every part's, and the caller finishes it.
    hold: bool,
}

/// Whether the job is the one a worker took for this process, which every
/// part of a split run takes up in turn.
fn adopted(job_id: i64) -> bool {
    job::adopted_job() == Some(job_id)
}

/// Close a run, audit it, finish its job and say how it ended.
fn conclude(
    registry: &mut Registry,
    c: &Conclusion<'_>,
    outcome: Result<Ended, String>,
) -> Result<(), Exit> {
    let run_id = c.run_id;
    let (status, summary, digest, exit_code, error) = match outcome {
        Ok(o) => o,
        Err(e) => ("failed", json!({}), None, None, Some(e)),
    };
    // 2026-10-12: a run that failed says why, by its kind, beside its error
    let failure = (status == "failed").then(|| {
        crate::failures::of_run(error.as_deref().unwrap_or_default(), exit_code, &summary)
    });
    let finished = nils_registry::time::now_iso();
    // record 57 P5: the install that made it, installed or registered, with
    // its version and fingerprint
    let mut summary = summary;
    if summary.is_object()
        && let Some(u) = crate::software::for_run(registry, c.pipeline)
            .ok()
            .flatten()
    {
        summary["software"] = u.doc();
    }
    rows::finish(
        registry.store(),
        run_id,
        &rows::Finish {
            status,
            finished_at: &finished,
            exit_code,
            results_digest: digest.as_deref(),
            summary: &summary,
            error: error.as_deref(),
            failure: failure.as_ref(),
        },
    )?;
    // the device it ran on, as recorded: a run sent to the CPU says so
    let device = rows::run(registry.store(), run_id)
        .ok()
        .flatten()
        .map_or_else(|| c.device.to_string(), |r| r.device);
    // record 49 A3: a run that loaded measures changed what the ask
    // answers, so it moves the epoch and the ask's catalog is built again
    let measures_loaded = summary["numbers"]["measures"].as_u64().unwrap_or(0) > 0;
    // the epoch is read and moved, so runs that close at once take turns
    with_lock(registry, INTAKE_LOCK, Some(c.job_id), |registry| {
        nils_registry::audit::record_judging(
            registry,
            &nils_registry::audit::Entry {
                principal: c.who,
                action: nils_registry::audit::Action::PipelineRun,
                scope: json!({
                    "run": run_id, "pipeline": c.pipeline.label(), "handle": c.handle_id,
                    "units": summary["units"], "derivatives": summary["derivatives"],
                    "resumed": c.resumed,
                }),
                policy: None,
                job_id: Some(c.job_id),
                details: Some(json!({
                    "status": status, "runtime": c.runtime, "device": device,
                    "results": digest, "review_items": summary["review_items"],
                    "measures": summary["numbers"]["measures"],
                })),
            },
            measures_loaded,
        )
        .map_err(|e| e.to_string())
    })
    .map_err(fail)?;
    let job_state = match status {
        "done" | "partial" => job::State::Done,
        "cancelled" => job::State::Cancelled,
        _ => job::State::Failed,
    };
    let _ = job::set_result(
        registry.store(),
        c.job_id,
        &json!({
            "run": run_id, "status": status, "summary": summary, "results_digest": digest,
            "failure": failure,
        }),
    );
    // 2026-10-17: a part of a split run a worker runs, with parts after
    // it: the job is every part's, and stays running until the last one
    // ends, so nothing reads it as over between two parts
    if !((c.hold || c.more && job_state == job::State::Done) && adopted(c.job_id)) {
        job::finish(registry.store(), c.job_id, job_state, error.as_deref())
            .map_err(|e| fail(e.to_string()))?;
    }
    let r = rows::run(registry.store(), run_id)?
        .ok_or_else(|| fail(format!("no pipeline run {run_id}")))?;
    let doc = run_doc(registry.store(), &r).map_err(fail)?;
    if c.quiet {
        // the workflow run says it
    } else if c.json {
        print(&doc)?;
    } else {
        print_run(&doc);
    }
    match status {
        // a partial run completed: its failed units are review items
        "done" | "partial" => Ok(()),
        "cancelled" => Err(Exit {
            code: crate::STOPPED,
            message: format!(
                "run {run_id} was cancelled; the units it finished are kept, and nils run --resume {run_id} goes on"
            ),
        }),
        _ => Err(fail(format!(
            "run {run_id} {}{}",
            error.unwrap_or_else(|| "failed".into()),
            failure
                .as_ref()
                .and_then(crate::failures::line)
                .map(|l| format!("\n  {l}"))
                .unwrap_or_default()
        ))),
    }
}

/// What a run is given once it is checked.
struct Execution<'a> {
    pipeline: &'a Pipeline,
    descriptor: &'a Descriptor,
    params: &'a serde_json::Map<String, Value>,
    run_id: i64,
    job_id: i64,
    /// The working place of its outputs: the derivatives and tables it
    /// registers, under `derivatives/<pipeline>/<run>/` (record 49 R7).
    output: &'a nils_registry::place::Place,
    /// The working place of its scratch: its input, each unit's folder,
    /// its log and apptainer's images, under `runs/<run>/` and `images/`.
    scratch: &'a nils_registry::place::Place,
    runtime: &'a runtime::Cli,
    gpu: bool,
    device: &'a str,
    stacks: &'a [i64],
    models: &'a [nils_registry::model::Model],
    label_set: Option<&'a nils_registry::labels::LabelSet>,
    /// The caller's threshold for the run's proposals, which only raises a
    /// model card's (record 43).
    threshold: Option<f64>,
    who: &'a str,
    actor: &'a Value,
    args: &'a RunArgs,
    /// The lane's budget and card (record 49 A1, A2).
    lane: &'a Lane,
    /// The secrets it was given (record 49 R3).
    secrets: &'a [Given],
    /// The variables a tool's terms want set beside its licence file
    /// (record 57 P5: `FS_ALLOW_DEEP`), from the pack of the install it uses.
    licence_env: &'a [(String, String)],
    /// The run as it stood, where this takes it up again.
    resume: Option<&'a Run>,
    /// The models whose bytes were found to be the admitted ones, once for
    /// all the parts of a run command.
    verified: &'a Verified,
}

/// The models whose files were hashed and found to be the bytes admitted.
/// The check holds the lock while it hashes, so the parts of a run that go
/// at once wait for the first one's verdict and hash a bundle once.
#[derive(Default)]
struct Verified(Mutex<BTreeSet<i64>>);

impl Verified {
    /// Hash `host` with `hash` unless this model's file was found right
    /// already; an error says what the file is and what it was admitted as.
    fn check(
        &self,
        model: (i64, &str),
        host: &Path,
        hash: &dyn Fn(&Path) -> Result<String, String>,
    ) -> Result<(), String> {
        let mut known = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (id, digest) = model;
        if known.contains(&id) {
            return Ok(());
        }
        let got = hash(host).map_err(|e| format!("{}: {e}", host.display()))?;
        if got != digest.strip_prefix("sha256:").unwrap_or(digest) {
            return Err(format!(
                "its file is sha256:{got}, not the digest {digest} it was admitted with; its code is not run"
            ));
        }
        known.insert(id);
        Ok(())
    }
}

/// How a run ended: its status, its summary, the digest of what it said,
/// the container's exit code and the error.
type Ended = (
    &'static str,
    Value,
    Option<String>,
    Option<i64>,
    Option<String>,
);

/// One container the lane schedules: the whole run where its units run
/// together, one unit where they run apart.
struct Batch {
    /// Indices into the run's units.
    units: Vec<usize>,
    /// The unit's id where it runs apart.
    key: Option<String>,
    /// The folder it writes, and the same under the working place.
    out: PathBuf,
    rel_out: String,
    log: PathBuf,
    /// Whether its units were judged again as the chain released them
    /// (after review, finding 10).
    judged_at_start: bool,
    /// After the third review (finding 9): by unit, the optional inputs
    /// the step before fed nothing for, with why, as the chain's gate
    /// found when it released the batch.
    unfed: BTreeMap<String, Vec<(String, String)>>,
}

/// What a reuse check answers for each unit of a batch: the unit, whether
/// its kept files are as registered, and the result it reuses.
type Checked = Vec<(usize, Result<(), String>, crate::reuse::Candidate)>;

/// A batch whose kept result is being checked, file by file, in a thread
/// of the pool, in place of a container (after review, finding 9).
struct Checking {
    batch: Batch,
    handle: std::thread::JoinHandle<Checked>,
}

/// A batch whose container runs.
struct Running {
    batch: Batch,
    child: std::process::Child,
    inv: Invocation,
    /// The cores and memory it holds of the lane.
    ask: lane::Ask,
}

/// What every batch of a run shares.
struct Shared {
    run_dir: PathBuf,
    input: PathBuf,
    inputs: PathBuf,
    /// Models and a label set, each at /inputs/<id>.
    typed: Vec<Mount>,
    /// The folders of the derivative inputs under the run's inputs.
    derivative_dirs: Vec<(String, PathBuf)>,
    manifest: Value,
    local_image: Option<PathBuf>,
    /// Whether the runtime holds a container to its cores and its memory
    /// here (record 49, after review).
    limits: (bool, bool),
    /// What the first container's log says before its own words: where the
    /// files another run left in this run's folder went (2026-10-17).
    said: Mutex<Option<String>>,
    /// Record 57 P2: the knobs the pack chose for each unit, by the unit's
    /// id, which a unit that runs apart is given in place of the run's;
    /// under a lock, since a chained unit is judged again as it starts
    /// (after review, finding 10).
    unit_params: Mutex<BTreeMap<String, serde_json::Map<String, Value>>>,
    /// Section 8.5 (G6), after review (finding 9): the kept result each unit
    /// may reuse, by the unit's id, its files not yet checked; the
    /// scheduler checks them in its pool where the unit would start.
    reuse: Mutex<BTreeMap<String, crate::reuse::Candidate>>,
    /// The registry's places by id, read once for the run.
    places: BTreeMap<i64, String>,
}

/// What judges a unit by its pack (record 57 P2 to P4): at the run's start
/// for every unit, and again for a chained unit as the chain releases it
/// (after review, finding 10), when the step before has made the results
/// its facts read (an upstream registration's aligned FLAIR makes
/// `aligned(flair, t1w)` true for SAMSEG at its start).
struct Judge<'a> {
    pack: nils_pipeline::pack::PipelinePack,
    person: nils_pipeline::pack::rules::Person,
    host: crate::pipeline_pack::HostFacts,
    roles_of: BTreeMap<i64, BTreeSet<String>>,
    converter: Option<(String, String)>,
    /// `(run, unit, key)` of the units that made the derivative inputs.
    upstream_units: Vec<(i64, String, String)>,
    d: &'a Descriptor,
    params: &'a serde_json::Map<String, Value>,
    pipeline: &'a Pipeline,
    /// Wave 7b section 7: the family's routing where this run is a
    /// member's under a family, recorded on every unit.
    family: Option<&'a crate::pipeline_family::Route>,
}

impl Judge<'_> {
    /// A unit as the pack judges it: its stacks by role.
    fn unit_of(&self, u: &Unit) -> crate::pipeline_pack::UnitOf {
        let mut by_role: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        for s in &u.stacks {
            match (self.d.layout, self.roles_of.get(s)) {
                (Layout::Stacks, _) => by_role.entry("stack".into()).or_default().push(*s),
                (Layout::Bids, Some(roles)) => {
                    for r in roles {
                        by_role.entry(r.clone()).or_default().push(*s);
                    }
                }
                (Layout::Bids, None) => {}
            }
        }
        crate::pipeline_pack::UnitOf {
            by_role,
            subject_id: u.subject_id,
            session_day: u.session_day.clone(),
        }
    }

    fn judge(
        &self,
        store: &mut Store,
        units: &[crate::pipeline_pack::UnitOf],
    ) -> Vec<nils_pipeline::pack::rules::Judgement> {
        crate::pipeline_pack::judge_units(
            store,
            &self.pack,
            self.d,
            self.params,
            units,
            &self.host,
            &self.person,
        )
    }

    /// The keys of the same-named units upstream, or of every unit where
    /// the names differ (a run over other units).
    fn upstream_of(&self, unit: &str) -> Vec<String> {
        let same: BTreeSet<String> = self
            .upstream_units
            .iter()
            .filter(|(_, u, _)| u == unit)
            .map(|(_, _, k)| k.clone())
            .collect();
        if !same.is_empty() {
            return same.into_iter().collect();
        }
        self.upstream_units
            .iter()
            .map(|(_, _, k)| k.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// The two keys of a unit's results and their members.
    fn keys(
        &self,
        store: &mut Store,
        u: &Unit,
        of: &crate::pipeline_pack::UnitOf,
        j: &nils_pipeline::pack::rules::Judgement,
    ) -> (String, String, Value) {
        let roles: BTreeSet<String> = of.by_role.keys().cloned().collect();
        let inputs = crate::pipeline_pack::input_identities(store, &u.stacks);
        crate::pipeline_pack::keys(&crate::pipeline_pack::KeysOf {
            pipeline: self.pipeline,
            pack: Some(&self.pack),
            judgement: Some(j),
            params: self.params,
            roles: &roles,
            converter: self.converter.clone(),
            upstream: self.upstream_of(&u.id),
            inputs,
        })
    }

    /// What the unit records of its judgement: the judgement, the key
    /// members, the main scan a cohort check stratifies it by, and when it
    /// was judged (`run`: at the run's start; `start`: as the unit started).
    fn record(
        &self,
        u: &Unit,
        of: &crate::pipeline_pack::UnitOf,
        j: &nils_pipeline::pack::rules::Judgement,
        members: Value,
        judged_at: &str,
    ) -> Value {
        let mut doc = j.doc();
        doc["key_members"] = members;
        // Wave 7b section 7: a member's run under a family records on
        // every unit the member and the rule that chose, and the family
        if let Some(route) = self.family
            && let Some((variant, fam)) = route.record(&crate::pipeline_family::unit_key(
                u.subject_id,
                u.session_day.as_deref(),
            ))
        {
            doc["variant"] = variant;
            doc["family"] = fam;
            if doc["status"] == json!("ready") {
                doc["status"] = json!("routed");
            }
        }
        // the main scan of the unit, which a cohort check stratifies it
        // by: the first of the pipeline's roles that has a stack, else the
        // first stack the unit read
        let main_role = self
            .d
            .roles
            .iter()
            .find(|r| of.by_role.get(*r).is_some_and(|s| !s.is_empty()));
        doc["stratum_stack"] = match (main_role, u.stacks.first()) {
            (Some(r), _) => {
                json!({"stack": of.by_role[r][0], "from": format!("the {r} stack")})
            }
            (None, Some(s)) => json!({"stack": s, "from": "the unit's first input stack"}),
            _ => Value::Null,
        };
        doc["judged_at"] = json!(judged_at);
        doc
    }
}

/// A unit the pack skipped: over, in the pack's words.
fn pack_skipped(j: &nils_pipeline::pack::rules::Judgement) -> Value {
    let words = j.words.clone().unwrap_or_default();
    json!({
        "status": "skipped",
        "error": words,
        "need": j.need, "words": words, "missing": [words],
        "metrics": {}, "files": [], "refused": [],
        "failure": nils_pipeline::failure::Cause::Input.doc(),
        "pack_skipped": true,
    })
}

/// A unit's id as a folder name: the characters a unit's id is made of.
fn folder_word(id: &str) -> Result<&str, String> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !id.starts_with('.');
    if ok {
        Ok(id)
    } else {
        Err(format!(
            "the unit {id} is not a name a folder can take, so its units cannot run apart"
        ))
    }
}

/// Materialise, schedule the containers in the lane, take in what each
/// left as it ends, and close with what the run as a whole said.
fn execute(home: &Home, registry: &mut Registry, x: &Execution<'_>) -> Result<Ended, String> {
    let heart = Heart::start(registry, x.job_id);
    let ended = execute_run(home, registry, x);
    if let Some(h) = heart {
        h.stop();
    }
    ended
}

/// A run's heart, beaten from a thread of its own on a connection of its
/// own for as long as the run goes on, so a step that runs long between
/// the run's own beats (the intake of thousands of proposals, a release, a
/// wait for a lock) never reads as a run whose engine went away. The run's
/// own beats still carry its progress and hear a cancel; this one only
/// says the run is there.
struct Heart {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: std::thread::JoinHandle<()>,
}

impl Heart {
    /// Start beating; none where no second connection opens, and the run
    /// goes on with its own beats alone.
    fn start(registry: &Registry, job_id: i64) -> Option<Heart> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let mut store = registry.open_reader().ok()?;
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        // six beats in the freshness a claim allows: ten seconds, as the
        // worker beats
        let every =
            Duration::from_millis((job::fresh_secs().saturating_mul(1000) / 6).clamp(250, 10_000));
        let thread = std::thread::spawn(move || {
            let mut beaten = Instant::now();
            while !stopped.load(Ordering::SeqCst) {
                if beaten.elapsed() >= every {
                    beaten = Instant::now();
                    let _ = job::beat(&mut store, job_id, None);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        Some(Heart { stop, thread })
    }

    fn stop(self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = self.thread.join();
    }
}

/// [`execute`] under the run's heart.
fn execute_run(home: &Home, registry: &mut Registry, x: &Execution<'_>) -> Result<Ended, String> {
    let p = x.pipeline;
    let d = x.descriptor;
    // record 49 R7: the outputs under the output place, the scratch under
    // the scratch place (one place where the lane names no other)
    let working = PathBuf::from(&x.output.path);
    let scratch_root = PathBuf::from(&x.scratch.path);
    let run_dir = scratch_root.join(RUNS).join(x.run_id.to_string());
    // 2026-10-17: a new run's folder is its own. One that is there already
    // holds another run's files (a registry made again numbers its runs
    // from 1 in the same working place): it is moved aside, never deleted,
    // and the run says so; a run taken up again goes on in its own
    let moved = match x.resume {
        None => set_aside(&run_dir)?,
        Some(_) => None,
    };
    if let Some(to) = &moved {
        eprintln!("nils: run {}: {}", x.run_id, moved_words(&run_dir, to));
    }
    // a resumed run's units in flight start again from a clean /tmp
    drop_workdirs(&run_dir);
    let input = run_dir.join("input");
    let inputs = run_dir.join("inputs");
    let rel_out = format!("{}/{}/{}", derivative::TREE, p.name, x.run_id);
    let out = working.join(&rel_out);
    let err = |e: nils_registry::Error| e.to_string();
    let mut moved_out: Option<PathBuf> = None;
    match x.resume {
        None => {
            for dir in [&input, &inputs] {
                std::fs::create_dir_all(dir).map_err(|e| format!("the run's folder: {e}"))?;
            }
            // Wave 7b: so is its output folder, when no row of this registry
            // names it: what is there is what a registry before this one left
            // under the same run number, and it is moved aside as the run's
            // folder is, never deleted. A folder a row names is this
            // registry's own, and the run is refused as it always was.
            if rows::output_is_named(registry.store(), &rel_out, x.run_id).map_err(err)? {
                return Err(format!(
                    "the output folder {rel_out} is there already; a run writes a folder of its own"
                ));
            }
            if let Some(to) = set_aside(&out)? {
                eprintln!("nils: run {}: {}", x.run_id, moved_words(&out, &to));
                moved_out = Some(to);
            }
            std::fs::create_dir_all(&out).map_err(|e| format!("the output folder: {e}"))?;
            rows::set_output(registry.store(), x.run_id, &rel_out).map_err(err)?;
        }
        Some(r) => {
            let before = rows::units(registry.store(), x.run_id).map_err(err)?;
            stop_left(
                x.runtime,
                x.descriptor.units == Units::Together,
                &before,
                &out,
            );
            // together, the units are all in flight or all past their
            // container: the first run again from a clean folder, the second
            // taken in from what the container left
            let past = |u: &rows::Unit| u.state == "over" || u.state == "registering";
            if d.units == Units::Together && !before.iter().any(past) && out.exists() {
                std::fs::remove_dir_all(&out).map_err(|e| format!("the output folder: {e}"))?;
            }
            // a bids input a release did not finish is released again
            if d.layout == Layout::Bids && r.input_release_id.is_none() && input.exists() {
                std::fs::remove_dir_all(&input).map_err(|e| format!("the run's input: {e}"))?;
            }
            for dir in [&input, &inputs, &out] {
                std::fs::create_dir_all(dir).map_err(|e| format!("the run's folder: {e}"))?;
            }
            if r.output.is_none() {
                rows::set_output(registry.store(), x.run_id, &rel_out).map_err(err)?;
            }
        }
    }
    let _ = job::beat(
        registry.store(),
        x.job_id,
        Some(&json!({"run": x.run_id, "phase": "materialise", "layout": d.layout.name()})),
    );

    let released_before = x.resume.and_then(|r| r.input_release_id);
    let m = match (d.layout, released_before) {
        (Layout::Stacks, _) => materialise_stacks(
            registry.store(),
            x.stacks,
            &input,
            d.header,
            d.geometry,
            d.contrast,
            d.asks_cache(x.params),
            d.rows,
        )?,
        (Layout::Bids, Some(release)) => released(registry, release, d.level)?,
        (Layout::Bids, None) => {
            // a release is a job of its own kind, one at a time: the runs
            // that release an input take turns rather than refuse each other
            match wait_lock(registry.store(), RELEASE_LOCK, Some(x.job_id), true)? {
                None => Materialised {
                    units: Vec::new(),
                    mounts: Vec::new(),
                    release_id: None,
                    stopped: true,
                    scope: Value::Null,
                    entries: Vec::new(),
                    listed: Vec::new(),
                    context: Vec::new(),
                    rows: Value::Null,
                    converter: Value::Null,
                    files: Vec::new(),
                },
                Some(lock) => {
                    let m = materialise_bids(
                        home, registry, x.run_id, x.job_id, d.level, x.stacks, &run_dir, &input,
                        x.args,
                    );
                    let released = nils_registry::lock::release(registry.store(), lock)
                        .map_err(|e| format!("the registry's release lock: {e}"));
                    let m = m?;
                    released?;
                    m
                }
            }
        }
    };
    if m.stopped {
        return Ok((
            "cancelled",
            json!({"phase": "materialise"}),
            None,
            None,
            None,
        ));
    }

    // the typed inputs, each read-only, and the manifest
    let mut typed: Vec<Mount> = Vec::new();
    let model_inputs: Vec<&descriptor::TypedInput> =
        d.inputs.iter().filter(|t| t.ty == "model").collect();
    let mut models_doc: Vec<Value> = Vec::new();
    for (model, t) in x.models.iter().zip(&model_inputs) {
        // a model a run fitted is that run's derivative of kind model
        // (record 43): its folder is the input, read-only
        let artifact = lane_places(x)
            .into_iter()
            .map(|pl| {
                model_artifact(registry.store(), model.id, pl.id)
                    .map(|a| a.map(|rel| (Path::new(&pl.path).join(&rel), rel)))
            })
            .find_map(Result::transpose)
            .transpose()?;
        // wave 7b §18: code trusted by the model's admission runs only from
        // the bytes that were admitted, so the file is the registered one
        let trusted = t.trust.as_ref().is_some_and(|id| {
            matches!(x.params.get(id), Some(Value::Bool(true)))
                || x.params.get(id).and_then(Value::as_str) == Some("true")
        });
        if trusted && let Some((host, _)) = &artifact {
            x.verified
                .check((model.id, &model.digest), host, &|path| {
                    nils_pipeline::files::sha256_file(path)
                        .map(|(_, got)| got)
                        .map_err(|e| e.to_string())
                })
                .map_err(|why| format!("model {}: {why}", model.label()))?;
        }
        if let Some((host, _)) = &artifact
            && let Some(dir) = host.parent().filter(|d| d.is_dir())
        {
            typed.push(Mount {
                host: dir.to_path_buf(),
                container: format!("/inputs/{}", t.id),
                read_only: true,
            });
        }
        models_doc.push(json!({
            "input": t.id, "model_id": model.id, "name": model.name, "version": model.version,
            "digest": model.digest, "kind": model.kind, "card": model.card,
            "encoder_model_ids": model.encoder_model_ids,
            "artifact": artifact.as_ref().and_then(|(_, a)| Path::new(a).file_name())
                .map(|f| format!("/inputs/{}/{}", t.id, f.to_string_lossy())),
        }));
    }
    let mut label_doc = Value::Null;
    if let (Some(set), Some(t)) = (x.label_set, d.inputs.iter().find(|t| t.ty == "label_set")) {
        label_doc = set.as_json();
        label_doc["input"] = json!(t.id);
        let dir = set
            .place_id
            .and_then(|id| {
                nils_registry::place::show(registry.store(), id)
                    .ok()
                    .flatten()
            })
            .zip(set.path.as_deref())
            .map(|(pl, rel)| Path::new(&pl.path).join(rel));
        if let Some(dir) = dir {
            let dir = if dir.is_file() {
                dir.parent().map(Path::to_path_buf).unwrap_or(dir)
            } else {
                dir
            };
            if dir.is_dir() {
                typed.push(Mount {
                    host: dir,
                    container: format!("/inputs/{}", t.id),
                    read_only: true,
                });
            }
        }
    }
    // the derivatives a run takes, each linked into its input's own folder
    // under the run's inputs (record 43 review): the container sees those
    // files and no other, and no bind reaches the derivatives tree
    let mut derivative_doc = serde_json::Map::new();
    let mut derivative_dirs: Vec<(String, PathBuf)> = Vec::new();
    // after review (finding 3 of the second review): what the workflow's
    // links bind: a typed input to the fed step's run, a role to its image
    let (bound, role_links) = bound_of(registry.store(), x.args.chain.as_ref(), d)?;
    let (bound_roles, roles_unbound) = bind_roles(registry, x, &m, &input, &role_links, d)?;
    let mut taken = derivative_inputs(registry.store(), d, x.stacks, &lane_places(x), &bound)?;
    // after the third review (finding 9): an optional input whose link comes
    // from a step that fed nothing takes nothing, never another run's
    for u in x.args.chain.iter().flat_map(|c| c.unfed.iter()) {
        taken.insert(u.input.clone(), Vec::new());
    }
    let mut made_by = MadeBy::default();
    for t in d.inputs.iter().filter(|t| t.ty.starts_with("derivative:")) {
        let kind = t.ty.trim_start_matches("derivative:");
        let rows = taken.get(&t.id).cloned().unwrap_or_default();
        if rows.is_empty()
            && !t.optional
            && let Some(fed) = bound.get(&t.id)
        {
            return Err(format!(
                "{}: {} made no {kind} derivative of the selection's stacks for the input {} of {}, which the link feeds and which takes no other run's",
                fed.link.words(&d.name),
                fed.words(),
                t.id,
                p.label(),
            ));
        }
        if rows.is_empty() && !t.optional {
            return Err(format!(
                "{} needs {kind} derivatives of the selection's stacks for its input {}, and none is registered that its check leaves {}",
                p.label(),
                t.id,
                match t.checked.as_deref().unwrap_or("not_rejected") {
                    "accepted" => "it: it takes accepted ones only",
                    "any" => "it",
                    _ => "it: a rejected one is not taken",
                }
            ));
        }
        let into = inputs.join(&t.id);
        std::fs::create_dir_all(&into).map_err(|e| format!("the run's inputs: {e}"))?;
        derivative_dirs.push((t.id.clone(), into.clone()));
        let mut listed = Vec::new();
        let mut linked: std::collections::BTreeSet<String> = Default::default();
        for row in rows {
            let root = lane_places(x)
                .into_iter()
                .find(|pl| pl.id == row.place_id)
                .map_or_else(|| working.clone(), |pl| PathBuf::from(&pl.path));
            let rel = link_input(&root, &row.path, &into)?;
            // two rows of one file are one input
            if !linked.insert(rel.clone()) {
                continue;
            }
            let made = made_by.of(registry.store(), &row);
            listed.push(json!({
                "id": row.id, "kind": row.kind, "stack_id": row.stack_id,
                "subject_id": row.subject_id, "model_id": row.model_id,
                "preprocess_version": row.preprocess_version,
                "path": rel, "sha256": row.sha256,
                "made_by": made,
            }));
        }
        derivative_doc.insert(t.id.clone(), json!(listed));
    }
    let manifest = json!({
        "contract": nils_pipeline::CONTRACT,
        "run": x.run_id,
        "pipeline": {"name": p.name, "version": p.version, "descriptor_digest": p.descriptor_digest, "image": p.image},
        "params": Value::Object(x.params.clone()),
        "level": d.level.name(),
        "layout": d.layout.name(),
        "units": m.units.iter().map(Unit::doc).collect::<Vec<_>>(),
        "models": models_doc,
        "label_set": label_doc,
        "derivatives": derivative_doc,
        // after review (finding 3 of the second review): the workflow's
        // links into this run, each bound to the fed step's run, and the
        // role files the chain bound for each unit
        "links": x.args.chain.as_ref().map_or_else(Vec::new, |c| c.links.iter().map(ChainLink::doc).collect::<Vec<_>>()),
        "bound_roles": bound_roles,
        // after the third review (finding 9): the optional inputs fed by
        // no step, with why
        "unfed": fed_notes(x.args.chain.as_ref(), None),
        // the secrets it reads, by id and where it sees them: never a path
        // on the host or a byte of the file (record 49 R3)
        "secrets": x.secrets.iter().map(|g| json!({"id": g.secret.id, "mount": g.secret.mount})).collect::<Vec<_>>(),
        // record 57 P5: the install that made it, installed or registered,
        // with its version and fingerprint, so a result says what made it
        "software": crate::software::for_run(registry, p).ok().flatten().map(|u| u.doc()),
    });
    std::fs::write(
        inputs.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| format!("the run's folder: {e}"))?;

    // the units, as the lane keeps them: a unit over stays over; one in
    // flight when the run stopped runs again
    let names: Vec<String> = m.units.iter().map(|u| u.id.clone()).collect();
    for u in rows::ensure_units(registry.store(), x.run_id, &names).map_err(err)? {
        if u.state == "running" {
            rows::unit_requeued(registry.store(), u.id).map_err(err)?;
        }
    }
    // record 49 A3: a unit whose input lacks a role the descriptor names,
    // as the release wrote it, is not run: it is over as skipped, missing
    // an input, as the pre-flight said it would be, and no review item
    let mut missing = if d.layout == Layout::Bids && !d.roles.is_empty() {
        let main = rows::run(registry.store(), x.run_id)
            .map_err(err)?
            .and_then(|r| r.handle_id)
            .and_then(|h| crate::main_input::entry_of(registry.store(), h));
        units_missing(
            registry.store(),
            m.release_id,
            &m.units,
            &d.roles,
            main.as_ref(),
        )?
    } else {
        BTreeMap::new()
    };
    // record 55 H3, section 6.5 (the T33 pipe test): nor is a unit the
    // inputs hold no derivative of for an input that needs one, its check
    // having left out every one there is
    let place_ids: Vec<i64> = lane_places(x).iter().map(|pl| pl.id).collect();
    let stacks_of: Vec<&[i64]> = m.units.iter().map(|u| u.stacks.as_slice()).collect();
    let lacking = derivatives_missing(registry.store(), d, &stacks_of, &place_ids, &bound)?;
    for (u, why) in m.units.iter().zip(lacking) {
        if !why.is_empty() {
            missing.entry(u.id.clone()).or_default().extend(why);
        }
    }
    // nor a unit whose role a link feeds and the fed run made no image for
    // (after review, finding 3 of the second review): skipped with the
    // link's words
    for (unit, why) in roles_unbound {
        missing.entry(unit).or_default().extend(why);
    }
    if !missing.is_empty() {
        let now = nils_registry::time::now_iso();
        for u in rows::units(registry.store(), x.run_id).map_err(err)? {
            if let Some(why) = missing.get(&u.unit)
                && (u.state == "queued" || u.state == "running")
            {
                let outcome = json!({
                    "status": "skipped",
                    "error": format!("missing an input: {}", why.join("; ")),
                    "missing": why, "metrics": {}, "files": [], "refused": [],
                    "failure": nils_pipeline::failure::Cause::Input.doc(),
                });
                rows::unit_over(registry.store(), u.id, None, &outcome, &now).map_err(err)?;
            }
        }
    }
    // record 57 P2 to P4 (Wave 7b A1): the pack judges every unit that has
    // its inputs, as the pre-flight did: a hard need skips it in the pack's
    // words, the rules choose its knobs, and the unit records who chose,
    // its caveats and the two keys its results carry
    let mut unit_params: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
    let mut reuse_candidates: BTreeMap<String, crate::reuse::Candidate> = BTreeMap::new();
    let places = crate::reuse::places(registry.store())?;
    if let Some(e) = crate::pipeline_pack::pack_error(x.pipeline) {
        return Err(e);
    }
    let judge: Option<Judge<'_>> = match crate::pipeline_pack::pack_of(x.pipeline) {
        None => None,
        Some(pack) => {
            // a resume goes on under the choices the run began with
            let person_ask = match x.resume.map(|r| &r.person) {
                Some(kept) if kept.is_object() => crate::pipeline_pack::PersonAsk::of_body(kept)?,
                _ => crate::pipeline_pack::PersonAsk::of_args(
                    x.args.goal.as_deref(),
                    &x.args.switches,
                    x.args.let_run,
                )?,
            };
            let given = match x.resume.map(|r| &r.person) {
                Some(kept) if kept.is_object() => kept["given"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|k| k.as_str().map(|k| (k.to_string(), String::new())))
                    .collect(),
                _ => pairs(&x.args.params).map_err(|e| e.message)?,
            };
            let person = person_ask.person(&pack, x.who, &given)?;
            let roles_of = match d.layout {
                Layout::Bids => crate::preflight::roles_of_stacks(registry.store(), x.stacks)?,
                Layout::Stacks => BTreeMap::new(),
            };
            // the runs that made the derivative inputs (the manifest lists
            // them under `derivatives`; Wave 7b A7 fixed the key read here,
            // which left every upstream key out before), and their units'
            // keys
            let made_by_runs: Vec<i64> = manifest["derivatives"]
                .as_object()
                .into_iter()
                .flat_map(|o| o.values())
                .flat_map(|v| v.as_array().cloned().unwrap_or_default())
                .filter_map(|e| e["made_by"]["run"].as_i64())
                .collect::<BTreeSet<i64>>()
                .into_iter()
                .collect();
            Some(Judge {
                pack,
                person,
                host: crate::pipeline_pack::host_facts(registry),
                roles_of,
                converter: m.converter["version"]
                    .as_str()
                    .map(|v| ("dcm2niix".to_string(), v.to_string())),
                upstream_units: rows::unit_keys(registry.store(), &made_by_runs)
                    .unwrap_or_default(),
                d,
                params: x.params,
                pipeline: x.pipeline,
                family: x.args.family.as_ref(),
            })
        }
    };
    // record 57 P2 to P4 (Wave 7b A1): the pack judges every unit that has
    // its inputs, as the pre-flight did: a hard need skips it in the pack's
    // words, the rules choose its knobs, and the unit records who chose,
    // its caveats and the two keys its results carry
    if let Some(jd) = &judge {
        // a unit that is over keeps the judgement it was run under: only
        // the units still to run are judged (a resume never rewrites a
        // finished unit's flags and checks)
        let still_to_run: std::collections::BTreeSet<String> =
            rows::units(registry.store(), x.run_id)
                .map_err(err)?
                .into_iter()
                .filter(|u| u.state == "queued" || u.state == "running")
                .map(|u| u.unit)
                .collect();
        let with_inputs: Vec<&Unit> = m
            .units
            .iter()
            .filter(|u| !missing.contains_key(&u.id) && still_to_run.contains(&u.id))
            .collect();
        let of: Vec<crate::pipeline_pack::UnitOf> =
            with_inputs.iter().map(|u| jd.unit_of(u)).collect();
        let judged = jd.judge(registry.store(), &of);
        if d.units == Units::Together
            && let Some(words) = crate::pipeline_pack::together_disagree(&judged)
        {
            return Err(words);
        }
        // record 58 P5: the curation state of the units' inputs as they
        // run, read once for every stack of the run (after review, finding
        // 5), which the provenance door compares with their state later
        let every_stack: Vec<i64> = with_inputs
            .iter()
            .flat_map(|u| u.stacks.iter().copied())
            .collect::<BTreeSet<i64>>()
            .into_iter()
            .collect();
        let curation = crate::provenance::Curation::read(
            registry,
            &every_stack,
            x.label_set,
            &x.args.pack_dir_or_default(home),
        );
        // a read that failed is recorded as such on every unit, never as a
        // state with nothing in it (after review, finding 4 of the second
        // review)
        let curation_of = |stacks: &[i64]| match &curation {
            Ok(c) => c.of(stacks),
            Err(e) => crate::provenance::Curation::unknown(e),
        };
        let now = nils_registry::time::now_iso();
        let row_ids: BTreeMap<String, i64> = rows::units(registry.store(), x.run_id)
            .map_err(err)?
            .into_iter()
            .map(|u| (u.unit, u.id))
            .collect();
        for ((u, of), j) in with_inputs.iter().zip(&of).zip(&judged) {
            let Some(&row_id) = row_ids.get(&u.id) else {
                continue;
            };
            let (ck, rk, members) = jd.keys(registry.store(), u, of, j);
            let mut doc = jd.record(u, of, j, members, "run");
            // after the third review (finding 9): the optional inputs the
            // chain feeds nothing for, and why
            let fed = fed_notes(x.args.chain.as_ref(), None);
            if !fed.is_null() {
                doc["fed"] = fed;
            }
            // section 8.5 (G6): a step whose result key matches a kept result
            // is not run again; the unit records "reused from run N". After
            // review (finding 9): the kept result is found here without
            // reading a file; its files are checked where the unit would
            // start, in the scheduler's pool, where units run apart (units
            // together are checked here, since one container runs them all).
            // After the second review (finding 8): where none is reused, the
            // unit records why under `reuse.considered`, on both paths
            let mut reused: Option<crate::reuse::Reused> = None;
            if j.status != nils_pipeline::pack::rules::Status::Skipped && x.resume.is_none() {
                let of_unit = crate::reuse::Of {
                    run_id: x.run_id,
                    pipeline: x.pipeline,
                    unit: &u.id,
                    stacks: &u.stacks,
                    subject_id: u.subject_id,
                    session_day: u.session_day.as_deref(),
                };
                let refused = match d.units {
                    Units::Apart => {
                        match crate::reuse::candidate(registry.store(), &places, &of_unit, &rk)? {
                            Ok(c) => {
                                reuse_candidates.insert(u.id.clone(), c);
                                Vec::new()
                            }
                            Err(refused) => refused,
                        }
                    }
                    Units::Together => {
                        match crate::reuse::kept_for(registry.store(), &places, &of_unit, &rk)? {
                            Ok(r) => {
                                reused = Some(r);
                                Vec::new()
                            }
                            Err(refused) => refused,
                        }
                    }
                };
                if !refused.is_empty() {
                    doc["reuse"] = json!({"considered": refused});
                }
            }
            rows::set_unit_pack(registry.store(), row_id, &doc, Some(&ck), Some(&rk))
                .map_err(err)?;
            rows::set_unit_curation(registry.store(), row_id, &curation_of(&u.stacks))
                .map_err(err)?;
            if let Some(reused) = reused {
                rows::unit_reused(registry.store(), row_id, reused.run, &reused.outcome, &now)
                    .map_err(err)?;
                continue;
            }
            if j.status == nils_pipeline::pack::rules::Status::Skipped {
                rows::unit_over(registry.store(), row_id, None, &pack_skipped(j), &now)
                    .map_err(err)?;
            } else {
                // together, one container takes the knobs every ready unit
                // agreed on (together_disagree refused a run where they
                // differ), kept under the empty key
                if d.units == Units::Together {
                    unit_params
                        .entry(String::new())
                        .or_insert_with(|| j.params());
                }
                unit_params.insert(u.id.clone(), j.params());
            }
        }
    }
    let unit_rows = rows::units(registry.store(), x.run_id).map_err(err)?;
    let row_of: BTreeMap<String, (i64, String)> = unit_rows
        .iter()
        .map(|u| (u.unit.clone(), (u.id, u.state.clone())))
        .collect();
    let state_of = |id: &str| row_of.get(id).map_or("queued", |(_, s)| s.as_str());

    // the image apptainer runs, built once from the pinned digest
    let local_image = if x.runtime.kind == runtime::Kind::Apptainer {
        // two runs of one image never build or replace it at once: the one
        // that comes second finds it built
        let lock = wait_lock(
            registry.store(),
            &image_lock(&x.pipeline.image_digest),
            Some(x.job_id),
            true,
        )?;
        let built = match lock {
            None => None,
            Some(lock) => {
                let built = ensure_image(registry, x, &scratch_root, &run_dir);
                let released = nils_registry::lock::release(registry.store(), lock)
                    .map_err(|e| format!("the registry's image lock: {e}"));
                let built = built?;
                released?;
                built
            }
        };
        match built {
            Some(path) => Some(path),
            None => {
                return Ok(("cancelled", json!({"phase": "image"}), None, None, None));
            }
        }
    } else {
        None
    };
    let shared = Shared {
        run_dir: run_dir.clone(),
        input: input.clone(),
        inputs: inputs.clone(),
        typed,
        derivative_dirs,
        manifest,
        local_image,
        limits: runtime::limits_here(x.runtime),
        said: Mutex::new({
            let said: Vec<String> = [
                moved.as_ref().map(|to| moved_words(&run_dir, to)),
                moved_out.as_ref().map(|to| moved_words(&out, to)),
            ]
            .into_iter()
            .flatten()
            .collect();
            (!said.is_empty()).then(|| said.join("; "))
        }),
        unit_params: Mutex::new(unit_params),
        reuse: Mutex::new(reuse_candidates),
        places,
    };

    // the batches: those whose container ended and whose taking in a stop
    // cut short are taken in again now; the rest still to run
    let batch_of = |units: Vec<usize>| -> Result<Batch, String> {
        Ok(match d.units {
            Units::Together => Batch {
                units,
                key: None,
                out: out.clone(),
                rel_out: rel_out.clone(),
                log: run_dir.join("log.txt"),
                judged_at_start: false,
                unfed: BTreeMap::new(),
            },
            Units::Apart => {
                let key = folder_word(&m.units[units[0]].id)?.to_string();
                Batch {
                    units,
                    out: out.join(&key),
                    rel_out: format!("{rel_out}/{key}"),
                    log: run_dir.join("units").join(&key).join("log.txt"),
                    key: Some(key),
                    judged_at_start: false,
                    unfed: BTreeMap::new(),
                }
            }
        })
    };
    let group = |want: &dyn Fn(&str) -> bool| -> Vec<Vec<usize>> {
        let idx: Vec<usize> = (0..m.units.len())
            .filter(|i| want(state_of(&m.units[*i].id)))
            .collect();
        match d.units {
            Units::Together if idx.is_empty() => Vec::new(),
            Units::Together => vec![idx],
            Units::Apart => idx.into_iter().map(|i| vec![i]).collect(),
        }
    };
    let mut taken_files: std::collections::BTreeSet<PathBuf> = Default::default();
    for units in group(&|s| s == "registering") {
        let code = units
            .first()
            .and_then(|i| unit_rows.iter().find(|u| u.unit == m.units[*i].id))
            .and_then(|u| u.exit_code)
            .and_then(|c| i32::try_from(c).ok());
        let b = batch_of(units)?;
        take_in(registry, x, &m, &b, code, None, &row_of, &mut taken_files)?;
    }
    let mut pending: std::collections::VecDeque<Batch> = Default::default();
    for units in group(&|s| s == "queued" || s == "running") {
        pending.push_back(batch_of(units)?);
    }

    let mut running: Vec<Running> = Vec::new();
    let scheduled = schedule(
        registry,
        x,
        &m,
        &shared,
        &row_of,
        &mut pending,
        &mut running,
        &mut taken_files,
        judge.as_ref(),
    );
    // whatever ended the schedule, no container outlives it: a stop, a
    // cancel and an error each give their units back to the queue
    if !running.is_empty() {
        let mut pulse = Pulse::new(x.job_id, json!({"run": x.run_id, "phase": "stop"}));
        for mut r in running.drain(..) {
            x.runtime.stop(&r.inv);
            let _ = runtime::kill(&mut r.child);
            let _ = r.child.wait();
            // its /tmp held clinical-derived files: gone with the container
            if let Some(w) = &r.inv.workdir {
                let _ = std::fs::remove_dir_all(w);
            }
            // record 49 R3: a unit stopped mid-way is swept as one that
            // ended; where the sweep cannot vouch for its folder, the
            // folder goes, since a resume runs the unit again from a clean
            // one
            match sweep_batch(registry, x, &r.batch, &mut pulse) {
                Ok((_, shut)) if shut.is_empty() => {}
                _ if r.batch.key.is_some() => {
                    let _ = std::fs::remove_dir_all(&r.batch.out);
                    let _ = std::fs::remove_file(&r.batch.log);
                }
                _ => {}
            }
            for i in &r.batch.units {
                if let Some((id, _)) = row_of.get(&m.units[*i].id) {
                    let _ = rows::unit_requeued(registry.store(), *id);
                }
            }
        }
    }
    match scheduled? {
        Scheduled::Cancelled => {
            let over = rows::units(registry.store(), x.run_id)
                .map_err(err)?
                .iter()
                .filter(|u| u.state == "over")
                .count();
            return Ok((
                "cancelled",
                json!({"phase": "run", "units": {"total": m.units.len(), "over": over}}),
                None,
                None,
                None,
            ));
        }
        Scheduled::Done => {}
    }
    let mut ended = finalize(registry, x, &m, &working, &out, &rel_out, &mut taken_files)?;
    // record 55 F2 and F5: a run that proposes the post-contrast axis gives
    // T1 answers the time rule anchors on, so its subjects are labelled again
    if x.descriptor.proposals.iter().any(|a| a == "post_contrast") {
        ended.1["contrast_label"] = crate::contrast::after_answers(home, registry, x.stacks);
    }
    // record 55 E5: the slice cache's versions no answering model reads
    // are dropped once they have gone unread 30 days (ruling R4)
    if !d.cache.is_empty() {
        ended.1["slice_cache"] = crate::slice_cache::after_run(registry.store(), &working);
    }
    // record 56 section 3: a run that gave System 1's opinions runs the
    // gate over its stacks of datasets flagged as having damaged headers
    if !x.descriptor.opinions.is_empty() && matches!(ended.0, "done" | "partial") {
        ended.1["gate"] = crate::review_rule::after_opinions(
            home,
            registry,
            x.stacks,
            x.who,
            x.args.pack_dir.as_deref(),
        );
    }
    // record 55 E3: what the cascade deferred goes to the full pipeline
    if x.pipeline.name == crate::operations::POST_CONTRAST_PIPELINE
        && matches!(ended.0, "done" | "partial")
    {
        ended.1["fallback"] = crate::operations::fallback(registry, x.run_id, x.job_id, x.who);
    }
    Ok(ended)
}

/// How the schedule ended.
enum Scheduled {
    Done,
    Cancelled,
}

/// What the lane's running units hold on this host, across every run that
/// is going on (2026-09-30): the cores, the memory and the card memory
/// each unit declared when it started, as its row keeps them. A run whose
/// engine went away holds nothing, though its rows still say running; this
/// run's own units are counted as they stand.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LaneHeld {
    cores: u32,
    memory_mib: u64,
    gpu_mib: u64,
    /// Of `gpu_mib`, what this run's own units hold.
    own_gpu_mib: u64,
    runs: usize,
}

fn lane_held(store: &mut Store, run_id: i64, host: &str, card: u32) -> Result<LaneHeld, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let d = store.dialect();
    // one container holds its unit's ask once, however many units it runs
    // together: the rows are counted by container, not by unit
    let sql = format!(
        "SELECT c.run_id, r.job_id, CAST(COALESCE(SUM(c.cores), 0) AS BIGINT), \
         CAST(COALESCE(SUM(c.memory_mb), 0) AS BIGINT), \
         CAST(COALESCE(SUM(CASE WHEN c.gpu_card = {} THEN c.gpu_memory_mb ELSE 0 END), 0) AS BIGINT) \
         FROM (SELECT DISTINCT run_id, container, cores, memory_mb, gpu_card, gpu_memory_mb \
         FROM {} WHERE state = 'running' AND host = {}) c \
         JOIN {} r ON r.id = c.run_id WHERE r.status = 'running' \
         GROUP BY c.run_id, r.job_id",
        d.param(1, Type::Int),
        store.qualified("pipeline_unit"),
        d.param(2, Type::Text),
        store.qualified("pipeline_run"),
    );
    let rows = store
        .query(&sql, &[Param::Int(i64::from(card)), Param::from(host)])
        .map_err(err)?;
    let mut held = LaneHeld::default();
    for r in rows {
        let run = r.int(0).map_err(err)?;
        let job_id = r.opt_int(1).map_err(err)?;
        if run != run_id && !job_id.is_some_and(|j| job_alive(store, j)) {
            continue;
        }
        let n = |i: usize| r.int(i).map(|v| v.max(0) as u64).map_err(err);
        held.cores = held
            .cores
            .saturating_add(n(2)?.min(u64::from(u32::MAX)) as u32);
        held.memory_mib += n(3)?;
        held.gpu_mib += n(4)?;
        if run == run_id {
            held.own_gpu_mib += n(4)?;
        }
        held.runs += 1;
    }
    Ok(held)
}

/// What the scheduler of one run carries between its looks at the lane.
struct Lanes {
    ask: lane::Ask,
    gpu_need: Option<u64>,
    smi: Option<PathBuf>,
    card: u32,
    host: String,
    /// The lane as every run holds it, as last read.
    seen: lane::Ledger,
    /// When the lane was last read, and the card last asked.
    looked: Option<Instant>,
    card_asked: Option<Instant>,
    waiting: Option<String>,
    /// The card is optional: a unit that cannot lease it within `grace`
    /// runs on the CPU rather than wait (record 55, after review).
    optional: bool,
    grace: Duration,
    /// Since when the card has been too full for a unit.
    card_short: Option<Instant>,
    /// A unit of the run started on the card: the run never leaves it.
    on_card: bool,
    /// Why the whole run went to the CPU, where it did; each unit's log
    /// says so.
    to_cpu: Option<String>,
    /// Since when a unit that runs alone has waited for half its cores.
    lane_short: Option<Instant>,
    lane_grace: Duration,
    /// How long a run on the card waits for room another tenant holds.
    card_stall: Duration,
}

/// Start the next batch if it fits the lane as every run holds it, under
/// the lane lock, so two runs never both take the last of the budget or
/// the same free memory of the card: the lane is read, the card asked, the
/// container spawned and its units marked running before the lock is
/// given back. Answers none, and why in `l.waiting`, when it does not fit.
#[allow(clippy::too_many_arguments)]
fn start_next(
    registry: &mut Registry,
    x: &Execution<'_>,
    m: &Materialised,
    shared: &Shared,
    row_of: &BTreeMap<String, (i64, String)>,
    pending: &mut std::collections::VecDeque<Batch>,
    l: &mut Lanes,
) -> Result<Option<Running>, String> {
    let held = lane_held(registry.store(), x.run_id, &l.host, l.card)?;
    l.looked = Some(Instant::now());
    l.seen = x.lane.ledger;
    l.seen.held_cores = held.cores;
    l.seen.held_memory_mib = held.memory_mib;
    // record 55 T24, after review: a unit that asks the lane's cores takes
    // those free now, so units that share the lane never wait for all of
    // it; one that runs alone waits a while for half of it first
    let free = l.seen.cores.saturating_sub(l.seen.held_cores);
    let together = pending.front().is_some_and(|b| b.key.is_none());
    let waited = l
        .lane_short
        .is_some_and(|since| since.elapsed() >= l.lane_grace);
    let Some(cores) = x
        .descriptor
        .cores_at_start(x.params, free, together, waited)
    else {
        l.lane_short.get_or_insert_with(Instant::now);
        l.waiting = Some(format!(
            "the lane's budget: {free} of {} cores free; {}",
            l.seen.cores,
            if together {
                format!(
                    "a unit that runs alone waits up to {} s for {} of the {} it may take",
                    l.lane_grace.as_secs(),
                    x.descriptor.needs.cores.max(1).div_ceil(2),
                    x.descriptor.needs.cores
                )
            } else {
                "a unit waits for one".to_string()
            }
        ));
        return Ok(None);
    };
    let ask = lane::Ask {
        cores,
        memory_mib: l.ask.memory_mib,
    };
    if !l.seen.fits(ask) {
        l.waiting = Some(format!(
            "the lane's budget: {} of {} cores and {} of {} MiB held{}",
            l.seen.held_cores,
            l.seen.cores,
            l.seen.held_memory_mib,
            l.seen.memory_mib,
            if held.runs > 1 {
                format!(", by {} runs", held.runs)
            } else {
                String::new()
            }
        ));
        return Ok(None);
    }
    let mut gpu_mib = None;
    if let Some(need) = l.gpu_need {
        l.card_asked = Some(Instant::now());
        let card = l.card;
        let free = match &l.smi {
            Some(smi) => lane::gpu_free_mib(smi, card),
            None => Err("nvidia-smi is not on the search path".into()),
        };
        let held_gpu = held.gpu_mib;
        let own = held.own_gpu_mib;
        // every lease counts whole: the card's memory in use cannot be told
        // apart by tenant, so subtracting it from the leases would let a
        // foreign tenant's memory cancel this lane's own and overfill the
        // card. Counting a lease that has loaded twice only waits longer.
        // what keeps the lease: none, this run's own leases (which end with
        // its units), or memory another tenant or another run holds
        let short = match free {
            Ok(free) if lane::lease_fits(free, held_gpu, need) => None,
            Ok(free) if own > 0 && free.saturating_add(own) >= need => Some((
                false,
                format!(
                    "card {card}: {free} MiB free and {own} MiB held by this run's own units, and a unit needs {need} MiB: it waits for them"
                ),
            )),
            Ok(free) => Some((
                true,
                format!(
                    "card {card}: {free} MiB free, {own} MiB of it this run's, and a unit needs {need} MiB: another tenant or run holds the rest"
                ),
            )),
            Err(e) => Some((true, format!("card {card}: {e}"))),
        };
        match short {
            None => {
                l.card_short = None;
                gpu_mib = Some(need);
                l.on_card = true;
            }
            // an optional card another tenant keeps too full sends the
            // whole run to the CPU, before its first unit starts, after a
            // short wait; a run never mixes devices
            Some((true, why)) if l.optional && !l.on_card => {
                let since = *l.card_short.get_or_insert_with(Instant::now);
                if since.elapsed() < l.grace {
                    l.waiting = Some(why);
                    return Ok(None);
                }
                let words = format!(
                    "{why}, for {} s: the run runs on the CPU",
                    since.elapsed().as_secs()
                );
                rows::set_device(registry.store(), x.run_id, "cpu").map_err(|e| e.to_string())?;
                l.gpu_need = None;
                l.to_cpu = Some(words);
            }
            // once on the card (or the card required), another tenant's
            // shortfall is waited for a bounded time, then the run fails
            // and says why
            Some((true, why)) => {
                let since = *l.card_short.get_or_insert_with(Instant::now);
                if since.elapsed() >= l.card_stall {
                    return Err(format!(
                        "{why}, for {} s: the run gives up on the card (NILS_CARD_STALL_S); start it again when the card has room",
                        since.elapsed().as_secs()
                    ));
                }
                l.waiting = Some(why);
                return Ok(None);
            }
            Some((false, why)) => {
                l.card_short = None;
                l.waiting = Some(why);
                return Ok(None);
            }
        }
    }
    let note = l.to_cpu.clone();
    let Some(b) = pending.pop_front() else {
        return Ok(None);
    };
    let mut inv = invocation_of(x, m, &b, shared, ask.cores, gpu_mib.is_some())?;
    inv.card = gpu_mib.map(|_| l.card);
    // the run's folder said once, in the first container's log
    let said = shared.said.lock().ok().and_then(|mut s| s.take());
    inv.note = match (said, note) {
        (Some(said), Some(note)) => Some(format!("{said}; {note}")),
        (said, note) => said.or(note),
    };
    if let Some(dir) = b.log.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("the run's folder: {e}"))?;
    }
    let child = runtime::spawn(x.runtime, &inv, &b.log)
        .map_err(|e| format!("{} could not start the container: {e}", x.runtime.name()))?;
    let now = nils_registry::time::now_iso();
    for i in &b.units {
        if let Some((id, _)) = row_of.get(&m.units[*i].id) {
            rows::unit_started(
                registry.store(),
                *id,
                &rows::UnitStart {
                    cores: i64::from(ask.cores),
                    memory_mb: l.ask.memory_mib as i64,
                    gpu_card: gpu_mib.map(|_| i64::from(l.card)),
                    gpu_memory_mb: gpu_mib.map(|g| g as i64),
                    device: if gpu_mib.is_some() { x.device } else { "cpu" },
                    container: &inv.name,
                    pid: Some(i64::from(child.id())),
                    host: &l.host,
                    started_at: &now,
                },
            )
            .map_err(|e| e.to_string())?;
        }
    }
    l.seen.take(ask);
    l.waiting = None;
    l.lane_short = None;
    Ok(Some(Running {
        batch: b,
        child,
        inv,
        ask,
    }))
}

/// Run the batches within the lane's budget, a GPU batch only under a lease
/// on the lane's card, and take in each as it ends. The budget and the
/// lease are the lane's, held across every run that goes on at once: what
/// the other runs' units hold is read from the registry before a unit
/// starts. Those still running when it returns are the caller's to stop.
#[allow(clippy::too_many_arguments)]
fn schedule(
    registry: &mut Registry,
    x: &Execution<'_>,
    m: &Materialised,
    shared: &Shared,
    row_of: &BTreeMap<String, (i64, String)>,
    pending: &mut std::collections::VecDeque<Batch>,
    running: &mut Vec<Running>,
    taken_files: &mut std::collections::BTreeSet<PathBuf>,
    judge: Option<&Judge<'_>>,
) -> Result<Scheduled, String> {
    let d = x.descriptor;
    let ask = ask_of(d);
    // the batches whose kept results are being checked in the pool
    let mut checks: Vec<Checking> = Vec::new();
    // the least a unit starts with: a unit that asks the lane's cores takes
    // one where no more is free
    let least = lane::Ask {
        cores: if d.needs.lane_cores { 1 } else { ask.cores },
        memory_mib: ask.memory_mib,
    };
    // this run's own units, which alone may fill the lane
    let mut ledger = x.lane.ledger;
    let total = m.units.len();
    let mut beaten: Option<Instant> = None;
    let mut l = Lanes {
        ask,
        gpu_need: x.gpu.then(|| lane::mib_of_gb(d.needs.gpu_memory_gb)),
        smi: runtime::which("nvidia-smi", std::env::var_os("PATH").as_deref()),
        // the one card a GPU unit leases and is given: the lane's, 0 where
        // it names none (record 49, after review)
        card: x.lane.card.unwrap_or(0),
        host: job::hostname(),
        seen: x.lane.ledger,
        looked: None,
        card_asked: None,
        waiting: None,
        optional: d.gpu == Gpu::Optional,
        grace: card_grace(),
        card_short: None,
        // a resumed run whose units ran on the card stays there
        on_card: x.gpu
            && rows::units(registry.store(), x.run_id)
                .map(|us| us.iter().any(|u| u.gpu_card.is_some()))
                .unwrap_or(false),
        to_cpu: None,
        lane_short: None,
        lane_grace: lane_grace(),
        card_stall: card_stall(),
    };
    let mut watch = ChainWatch::default();
    loop {
        // start what fits
        while !pending.is_empty() {
            // Wave 7b A7 (section 8.4): a step of a workflow starts a unit
            // when the same unit of the step before is over; one skipped
            // or failed there is skipped here with the same reason
            let chained = x.args.chain.as_ref().filter(|c| !c.after.is_empty());
            if let Some(c) = chained {
                match chain_gate(registry, c, d, m, row_of, pending, &mut watch)? {
                    Gate::Ready => {}
                    Gate::Waiting(words) => {
                        l.waiting = Some(words);
                        break;
                    }
                    Gate::Empty => break,
                }
            }
            // after review (finding 10): a chained unit's pack facts are read
            // again as the chain releases it, now that the step before has
            // made the results they read; a unit the pack skips now is over
            if let (Some(jd), Some(_)) = (judge, chained)
                && d.units == Units::Apart
                && pending.front().is_some_and(|b| !b.judged_at_start)
                && let Some(mut b) = pending.pop_front()
            {
                b.judged_at_start = true;
                judge_again(registry, x, m, shared, row_of, jd, &mut b)?;
                if b.units.is_empty() {
                    continue;
                }
                pending.push_front(b);
            }
            // after review (finding 9): a unit whose kept result may be
            // reused checks that result's files in the pool, in place of a
            // container, and never takes the lane's budget for it
            if pending.front().is_some_and(|b| {
                shared
                    .reuse
                    .lock()
                    .is_ok_and(|r| b.units.iter().all(|i| r.contains_key(&m.units[*i].id)))
            }) && let Some(b) = pending.pop_front()
            {
                let candidates: Vec<(usize, crate::reuse::Candidate)> = {
                    let r = shared
                        .reuse
                        .lock()
                        .map_err(|_| "the reuse candidates were poisoned".to_string())?;
                    b.units
                        .iter()
                        .filter_map(|i| r.get(&m.units[*i].id).map(|c| (*i, c.clone())))
                        .collect()
                };
                let handle = std::thread::spawn(move || {
                    candidates
                        .into_iter()
                        .map(|(i, c)| {
                            let v = c.verify();
                            (i, v, c)
                        })
                        .collect::<Checked>()
                });
                checks.push(Checking { batch: b, handle });
                continue;
            }
            if !ledger.fits(least) {
                l.waiting = Some(format!(
                    "the lane's budget: {} of {} cores and {} of {} MiB held",
                    ledger.held_cores, ledger.cores, ledger.held_memory_mib, ledger.memory_mib
                ));
                break;
            }
            // while it waits, the lane is read again twice a second and the
            // card asked every two seconds, not on every turn
            if l.waiting.is_some()
                && l.looked
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
            {
                break;
            }
            if l.gpu_need.is_some()
                && l.card_asked
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
            {
                break;
            }
            let lock = wait_lock(registry.store(), LANE_LOCK, Some(x.job_id), false)?
                .ok_or("the lane lock was not taken")?;
            let started = start_next(registry, x, m, shared, row_of, pending, &mut l);
            let released = nils_registry::lock::release(registry.store(), lock)
                .map_err(|e| format!("the registry's lane lock: {e}"));
            if let Some(r) = started? {
                ledger.take(r.ask);
                running.push(r);
            }
            released?;
            if l.waiting.is_some() {
                break;
            }
        }
        // the heart, and a cancel
        if beaten.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            beaten = Some(Instant::now());
            if l.looked
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
                && let Ok(held) = lane_held(registry.store(), x.run_id, &l.host, l.card)
            {
                l.seen.held_cores = held.cores;
                l.seen.held_memory_mib = held.memory_mib;
            }
            let in_flight: usize = running.iter().map(|r| r.batch.units.len()).sum();
            let queued: usize = pending.iter().map(|b| b.units.len()).sum::<usize>()
                + checks.iter().map(|c| c.batch.units.len()).sum::<usize>();
            let progress = json!({
                "run": x.run_id, "phase": "run", "units": total,
                "over": total - in_flight - queued, "running": in_flight, "queued": queued,
                "waiting": l.waiting,
                "lane": {
                    "cores": l.seen.cores, "held_cores": l.seen.held_cores,
                    "memory_mb": l.seen.memory_mib, "held_memory_mb": l.seen.held_memory_mib,
                    "run_cores": ledger.held_cores, "run_memory_mb": ledger.held_memory_mib,
                },
            });
            if let Ok(job::Asked::Cancel) = job::beat(registry.store(), x.job_id, Some(&progress)) {
                return Ok(Scheduled::Cancelled);
            }
        }
        // take in what ended
        let mut i = 0;
        while i < running.len() {
            match running[i].child.try_wait() {
                Ok(Some(status)) => {
                    let r = running.remove(i);
                    ledger.give(r.ask);
                    // what the container kept in its /tmp is gone with it
                    if let Some(w) = &r.inv.workdir {
                        let _ = std::fs::remove_dir_all(w);
                    }
                    take_in(
                        registry,
                        x,
                        m,
                        &r.batch,
                        status.code(),
                        signal_of(&status),
                        row_of,
                        taken_files,
                    )?;
                }
                Ok(None) => i += 1,
                Err(e) => return Err(format!("a container of the run: {e}")),
            }
        }
        // take in the kept results whose files were checked: good, the
        // unit is reused; changed, the unit runs after all
        let mut i = 0;
        while i < checks.len() {
            if !checks[i].handle.is_finished() {
                i += 1;
                continue;
            }
            let c = checks.remove(i);
            let verdicts = c
                .handle
                .join()
                .map_err(|_| "a reuse check's thread ended in a panic".to_string())?;
            let now = nils_registry::time::now_iso();
            let mut rerun: Vec<usize> = Vec::new();
            for (u, verdict, cand) in verdicts {
                let id = &m.units[u].id;
                match verdict {
                    Ok(()) => {
                        if let Some((row_id, _)) = row_of.get(id) {
                            rows::unit_reused(
                                registry.store(),
                                *row_id,
                                cand.run,
                                &cand.outcome(),
                                &now,
                            )
                            .map_err(|e| e.to_string())?;
                        }
                    }
                    Err(why) => {
                        if let Ok(mut r) = shared.reuse.lock() {
                            r.remove(id);
                        }
                        // the refusal is the unit's record (after review,
                        // finding 8 of the second review)
                        if let Some((row_id, _)) = row_of.get(id)
                            && let Ok(Some(row)) = rows::unit(registry.store(), *row_id)
                        {
                            let mut doc = row.pack.clone();
                            doc["reuse"] = json!({"considered": cand.refused(&why)});
                            rows::set_unit_pack(
                                registry.store(),
                                *row_id,
                                &doc,
                                row.comparability_key.as_deref(),
                                row.result_key.as_deref(),
                            )
                            .map_err(|e| e.to_string())?;
                        }
                        rerun.push(u);
                    }
                }
            }
            if !rerun.is_empty() {
                let mut b = c.batch;
                b.units = rerun;
                pending.push_front(b);
            }
        }
        if running.is_empty() && pending.is_empty() && checks.is_empty() {
            return Ok(Scheduled::Done);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// After review (finding 10): judge the units of a batch again as the
/// chain releases them, with the facts as they stand now that the step
/// before made its results; the unit's record is updated with `judged_at:
/// start` and keeps the judgement at the run's start as `forecast`; its
/// knobs and keys follow, and so does the kept result it may reuse; a unit
/// the pack skips now is over, and leaves the batch.
fn judge_again(
    registry: &mut Registry,
    x: &Execution<'_>,
    m: &Materialised,
    shared: &Shared,
    row_of: &BTreeMap<String, (i64, String)>,
    jd: &Judge<'_>,
    b: &mut Batch,
) -> Result<(), String> {
    let err = |e: nils_registry::Error| e.to_string();
    let now = nils_registry::time::now_iso();
    let mut keep: Vec<usize> = Vec::new();
    for &i in &b.units {
        let u = &m.units[i];
        let Some((row_id, _)) = row_of.get(&u.id) else {
            keep.push(i);
            continue;
        };
        // the one row judged again (after review, finding 6 of the second
        // review: not every row of the run per batch)
        let Some(was) = rows::unit(registry.store(), *row_id).map_err(err)? else {
            keep.push(i);
            continue;
        };
        let of = jd.unit_of(u);
        let Some(j) = jd.judge(registry.store(), std::slice::from_ref(&of)).pop() else {
            keep.push(i);
            continue;
        };
        let (ck, rk, members) = jd.keys(registry.store(), u, &of, &j);
        let mut doc = jd.record(u, &of, &j, members, "start");
        // after the third review (finding 9): what fed nothing for this
        // unit, the step's unfed links and the gate's notes
        let fed = fed_notes(x.args.chain.as_ref(), b.unfed.get(&u.id));
        if !fed.is_null() {
            doc["fed"] = fed;
        }
        doc["forecast"] = json!({
            "judged_at": was.pack["judged_at"], "status": was.pack["status"], "words": was.pack["words"],
            "caveats": was.pack["caveats"], "knobs": was.pack["knobs"], "facts": was.pack["facts"],
        });
        rows::set_unit_pack(registry.store(), *row_id, &doc, Some(&ck), Some(&rk)).map_err(err)?;
        if j.status == nils_pipeline::pack::rules::Status::Skipped {
            rows::unit_over(registry.store(), *row_id, None, &pack_skipped(&j), &now)
                .map_err(err)?;
            if let Ok(mut r) = shared.reuse.lock() {
                r.remove(&u.id);
            }
            continue;
        }
        if let Ok(mut p) = shared.unit_params.lock() {
            p.insert(u.id.clone(), j.params());
        }
        // the kept result under the key as it stands now
        if was.result_key.as_deref() != Some(rk.as_str()) && x.resume.is_none() {
            let c = crate::reuse::candidate(
                registry.store(),
                &shared.places,
                &crate::reuse::Of {
                    run_id: x.run_id,
                    pipeline: x.pipeline,
                    unit: &u.id,
                    stacks: &u.stacks,
                    subject_id: u.subject_id,
                    session_day: u.session_day.as_deref(),
                },
                &rk,
            )?;
            if let Ok(mut r) = shared.reuse.lock() {
                match c {
                    Ok(c) => {
                        r.insert(u.id.clone(), c);
                    }
                    Err(refused) => {
                        r.remove(&u.id);
                        if !refused.is_empty() {
                            doc["reuse"] = json!({"considered": refused});
                            rows::set_unit_pack(
                                registry.store(),
                                *row_id,
                                &doc,
                                Some(&ck),
                                Some(&rk),
                            )
                            .map_err(err)?;
                        }
                    }
                };
            }
        }
        keep.push(i);
    }
    b.units = keep;
    Ok(())
}

/// What the chain's gate says of the batch at the front of the queue.
enum Gate {
    /// The front batch may start: every unit of it is over and made a
    /// result at the step before.
    Ready,
    /// No pending batch may start yet; the words say what is waited for.
    Waiting(String),
    /// Nothing is pending any more (every waiting unit was skipped).
    Empty,
}

/// What a step of a workflow run knows of the runs before it, read again
/// once a second: each run's status, its units' states and outcomes, and
/// which unit holds which stack (after review, finding 8), so a step of
/// another grain (a session step after a stack step, or the reverse) is
/// fed by the stacks its units hold, not by the units' names.
#[derive(Default)]
struct ChainWatch {
    looked: Option<Instant>,
    runs: BTreeMap<i64, WatchedRun>,
}

/// One run before this step: whether it is over (after the third review,
/// finding 2: its row says done, partial, failed, cancelled, interrupted or
/// gone, or its step is over while the row still says running), its
/// status, its units' states and outcomes, and the unit that holds each
/// stack.
#[derive(Default)]
struct WatchedRun {
    status: String,
    over: bool,
    units: BTreeMap<String, (String, Value)>,
    held: BTreeMap<i64, String>,
}

impl ChainWatch {
    /// Read the runs before again, once a second: `workflow_run` is the run
    /// whose steps they are, so a step that is over (its thread ended, its
    /// row terminal) makes its run over whatever the run row says.
    fn refresh(
        &mut self,
        store: &mut Store,
        workflow_run: i64,
        after: &[i64],
    ) -> Result<(), String> {
        if self
            .looked
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return Ok(());
        }
        self.looked = Some(Instant::now());
        // the steps of the workflow run that are over, by their run
        let steps_over: BTreeSet<i64> = nils_registry::workflow::run(store, workflow_run)
            .map_err(|e| e.to_string())?
            .map(|r| {
                r.steps
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|s| {
                        crate::workflow_run::step_is_over(s["status"].as_str().unwrap_or("queued"))
                    })
                    .filter_map(|s| s["run"].as_i64())
                    .collect()
            })
            .unwrap_or_default();
        for run in after {
            let status = rows::run(store, *run)
                .map_err(|e| e.to_string())?
                .map_or("gone".to_string(), |r| r.status);
            let units: BTreeMap<String, (String, Value)> = rows::units(store, *run)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|u| (u.unit, (u.state, u.outcome)))
                .collect();
            // the stacks each unit holds, read when the units appear and
            // again only when their count changes (a unit's stacks never do)
            let seen = self.runs.entry(*run).or_default();
            if seen.units.len() != units.len() || (seen.held.is_empty() && !units.is_empty()) {
                seen.held = unit_stacks(store, *run)?
                    .into_iter()
                    .flat_map(|(unit, stacks)| stacks.into_iter().map(move |s| (s, unit.clone())))
                    .collect();
            }
            seen.over = status != "running" || steps_over.contains(run);
            seen.status = status;
            seen.units = units;
        }
        Ok(())
    }

    /// What one unit of a run before says: none where it is over and made
    /// a result, else why this unit waits or is skipped.
    fn unit_says(
        run: i64,
        w: &WatchedRun,
        name: &str,
        state: &str,
        outcome: &Value,
    ) -> Option<Upstream> {
        if state == "over" {
            return match outcome["status"].as_str().unwrap_or("failed") {
                "succeeded" | "reused" => None,
                other => Some(Upstream::Skipped {
                    run,
                    status: other.to_string(),
                    words: outcome["error"]
                        .as_str()
                        .or(outcome["words"].as_str())
                        .unwrap_or("no reason recorded")
                        .to_string(),
                }),
            };
        }
        if !w.over {
            return Some(Upstream::NotYet);
        }
        // the run is over with this unit not: interrupted, or its step ended
        // (after the third review, finding 2), never "not yet" for ever
        Some(Upstream::Skipped {
            run,
            status: w.status.clone(),
            words: format!(
                "the step before was {} while its unit {name} was {state}",
                if w.status == "running" {
                    "over"
                } else {
                    w.status.as_str()
                }
            ),
        })
    }

    /// What the runs before say of one unit, by its name where a run
    /// before has a unit of that name, else by the stacks it holds: fed
    /// when every stack of it was in a unit before that is over and made a
    /// result; skipped with the reason where one of them was skipped or
    /// failed, or no unit before held it once that run is over; not yet
    /// while the run before still runs. A run in `optional` (after the
    /// third review, finding 9: every link from it feeds an optional typed
    /// input) never skips the unit: what it would have said is answered as
    /// a note with `Ready`, for the unit to record and run without it.
    fn of(&self, unit: &str, stacks: &[i64], optional: &BTreeSet<i64>) -> Upstream {
        let mut notes: Vec<(i64, String)> = Vec::new();
        for (run, w) in &self.runs {
            let says = self.one(*run, w, unit, stacks);
            match says {
                Upstream::Ready(_) => {}
                Upstream::NotYet => return Upstream::NotYet,
                Upstream::Skipped { run, words, .. } if optional.contains(&run) => {
                    notes.push((run, words));
                }
                skipped => return skipped,
            }
        }
        Upstream::Ready(notes)
    }

    /// What one run before says of the unit.
    fn one(&self, run: i64, w: &WatchedRun, unit: &str, stacks: &[i64]) -> Upstream {
        if let Some((state, outcome)) = w.units.get(unit) {
            return Self::unit_says(run, w, unit, state, outcome)
                .unwrap_or(Upstream::Ready(Vec::new()));
        }
        if stacks.is_empty() || w.held.is_empty() {
            if !w.over {
                // its units may not be made yet
                return Upstream::NotYet;
            }
            return Upstream::Skipped {
                run,
                status: w.status.clone(),
                words: format!("the step before (run {run}) had no unit {unit}"),
            };
        }
        for s in stacks {
            match w.held.get(s) {
                None if !w.over => return Upstream::NotYet,
                None => {
                    return Upstream::Skipped {
                        run,
                        status: w.status.clone(),
                        words: format!("the step before (run {run}) had no unit holding stack {s}"),
                    };
                }
                Some(name) => {
                    let Some((state, outcome)) = w.units.get(name) else {
                        return Upstream::NotYet;
                    };
                    if let Some(says) = Self::unit_says(run, w, name, state, outcome) {
                        return says;
                    }
                }
            }
        }
        Upstream::Ready(Vec::new())
    }
}

enum Upstream {
    /// Fed, with the notes of the optional runs that fed nothing for the
    /// unit: `(run, why)`.
    Ready(Vec<(i64, String)>),
    NotYet,
    Skipped {
        run: i64,
        status: String,
        words: String,
    },
}

/// Wave 7b A7 (section 8.4): rotate the queue so that a batch whose units
/// are over at the steps before stands at the front; skip, with the same
/// reason, the units skipped or failed there; say what is waited for where
/// none may start.
fn chain_gate(
    registry: &mut Registry,
    c: &Chain,
    d: &Descriptor,
    m: &Materialised,
    row_of: &BTreeMap<String, (i64, String)>,
    pending: &mut std::collections::VecDeque<Batch>,
    watch: &mut ChainWatch,
) -> Result<Gate, String> {
    watch.refresh(registry.store(), c.workflow_run, &c.after)?;
    // after the third review (finding 9): the runs before whose every link
    // into this step feeds an optional typed input never skip a unit; the
    // unit runs without what they fed, recording why
    let optional: BTreeSet<i64> = c
        .after
        .iter()
        .copied()
        .filter(|run| {
            let links: Vec<&ChainLink> = c.links.iter().filter(|l| l.run == *run).collect();
            !links.is_empty()
                && links
                    .iter()
                    .all(|l| !l.role && d.inputs.iter().any(|t| t.id == l.input && t.optional))
        })
        .collect();
    let n = pending.len();
    let mut waiting = 0usize;
    for _ in 0..n {
        let Some(mut b) = pending.pop_front() else {
            break;
        };
        let mut skipped: Option<(String, i64, String, String)> = None;
        let mut not_yet = false;
        let mut unfed: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for &i in &b.units {
            let id = &m.units[i].id;
            match watch.of(id, &m.units[i].stacks, &optional) {
                Upstream::Ready(notes) => {
                    for (run, why) in notes {
                        for l in c.links.iter().filter(|l| l.run == run) {
                            unfed.entry(id.clone()).or_default().push((
                                l.input.clone(),
                                format!("fed by no step for this unit: {why}"),
                            ));
                        }
                    }
                }
                Upstream::NotYet => not_yet = true,
                Upstream::Skipped { run, status, words } => {
                    skipped = Some((id.clone(), run, status, words));
                    break;
                }
            }
        }
        if let Some((unit, run, status, words)) = skipped {
            // every unit of the batch is skipped downstream with the reason
            let now = nils_registry::time::now_iso();
            let error = format!("skipped at step {} (run {run}): {words}", c.step);
            for &i in &b.units {
                if let Some((row_id, _)) = row_of.get(&m.units[i].id) {
                    let outcome = json!({
                        "status": "skipped",
                        "error": error,
                        "words": words, "missing": [error],
                        "metrics": {}, "files": [], "refused": [],
                        "failure": nils_pipeline::failure::Cause::Input.doc(),
                        "chain_skipped": {"step": c.step, "run": run, "unit": unit, "status": status},
                    });
                    rows::unit_over(registry.store(), *row_id, None, &outcome, &now)
                        .map_err(|e| e.to_string())?;
                }
            }
            continue;
        }
        if not_yet {
            waiting += 1;
            pending.push_back(b);
            continue;
        }
        // what fed nothing for each unit, on its record now (judged again,
        // the record keeps it), so the unit says why it ran without it
        for (unit, notes) in &unfed {
            if let Some((row_id, _)) = row_of.get(unit)
                && let Some(u) = rows::unit(registry.store(), *row_id).map_err(|e| e.to_string())?
            {
                let mut pack = u.pack.clone();
                if !pack.is_object() {
                    pack = json!({});
                }
                pack["fed"] = fed_notes(Some(c), Some(notes));
                rows::set_unit_pack(
                    registry.store(),
                    *row_id,
                    &pack,
                    u.comparability_key.as_deref(),
                    u.result_key.as_deref(),
                )
                .map_err(|e| e.to_string())?;
            }
        }
        b.unfed = unfed;
        pending.push_front(b);
        return Ok(Gate::Ready);
    }
    if pending.is_empty() {
        return Ok(Gate::Empty);
    }
    Ok(Gate::Waiting(format!(
        "step {}: {waiting} unit(s) wait for the step before (run {})",
        c.step + 1,
        c.after
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// The signal a container's process was ended by, where one ended it: how
/// the kernel's OOM killer ends a process apptainer runs (2026-10-12).
fn signal_of(status: &std::process::ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        std::os::unix::process::ExitStatusExt::signal(status)
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

/// The container of one batch: its folders made fresh where it runs one
/// unit apart, its mounts, its command line and who it runs as.
fn invocation_of(
    x: &Execution<'_>,
    m: &Materialised,
    b: &Batch,
    shared: &Shared,
    cores: u32,
    on_card: bool,
) -> Result<Invocation, String> {
    let d = x.descriptor;
    let p = x.pipeline;
    let mut mounts: Vec<Mount>;
    let participants: Vec<String>;
    let mut env: Vec<(String, String)> = vec![
        ("NILS_RUN_ID".into(), x.run_id.to_string()),
        ("NILS_PIPELINE".into(), p.label()),
        ("NILS_IMAGE_DIGEST".into(), p.image_digest.clone()),
        ("NILS_CORES".into(), cores.to_string()),
        ("NILS_MEMORY_GB".into(), format!("{}", d.needs.memory_gb)),
    ];
    if on_card {
        // record 55 T25: the card memory the unit is leased, which its tool
        // plans within (the post-contrast fallback sizes its waves to it)
        env.push((
            "NILS_GPU_MEMORY_GB".into(),
            descriptor::number_text(d.needs.gpu_memory_gb),
        ));
    }
    match &b.key {
        None => {
            mounts = vec![
                Mount {
                    host: shared.input.clone(),
                    container: "/input".into(),
                    read_only: true,
                },
                Mount {
                    host: shared.inputs.clone(),
                    container: "/inputs".into(),
                    read_only: true,
                },
                Mount {
                    host: b.out.clone(),
                    container: "/output".into(),
                    read_only: false,
                },
            ];
            mounts.extend(m.mounts.iter().cloned());
            mounts.extend(shared.typed.iter().cloned());
            let mut s: Vec<String> = m.units.iter().filter_map(|u| u.subject.clone()).collect();
            s.sort();
            s.dedup();
            participants = s;
        }
        Some(key) => {
            let i = b.units[0];
            let u = &m.units[i];
            let udir = shared.run_dir.join("units").join(key);
            if udir.exists() {
                std::fs::remove_dir_all(&udir).map_err(|e| format!("the unit's folder: {e}"))?;
            }
            if b.out.exists() {
                std::fs::remove_dir_all(&b.out)
                    .map_err(|e| format!("the unit's output folder: {e}"))?;
            }
            let (uin, uinputs) = (udir.join("input"), udir.join("inputs"));
            for dir in [&uin, &uinputs, &b.out] {
                std::fs::create_dir_all(dir).map_err(|e| format!("the unit's folder: {e}"))?;
            }
            let mut sources: Vec<Mount> = Vec::new();
            match d.layout {
                Layout::Stacks => {
                    let entry = m.entries.get(i).cloned().unwrap_or(Value::Null);
                    let used: std::collections::BTreeSet<i64> = entry["files"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|f| f["source"].as_i64())
                        .collect();
                    let listed: Vec<Value> = m
                        .listed
                        .iter()
                        .filter(|l| l["id"].as_i64().is_some_and(|n| used.contains(&n)))
                        .cloned()
                        .collect();
                    let context = unit_context(m, i);
                    write_stacks_json(
                        &uin.join("stacks.json"),
                        &listed,
                        std::slice::from_ref(&entry),
                        context.as_deref(),
                        (!m.rows.is_null()).then_some(&m.rows),
                    )
                    .map_err(|e| format!("the unit's input: {e}"))?;
                    sources = m
                        .mounts
                        .iter()
                        .filter(|mt| {
                            mt.container
                                .strip_prefix("/source/")
                                .and_then(|n| n.parse::<i64>().ok())
                                .is_some_and(|n| used.contains(&n))
                        })
                        .cloned()
                        .collect();
                }
                Layout::Bids => unit_input(&shared.input, &uin, u, d.level)?,
            }
            let mut manifest = shared.manifest.clone();
            manifest["units"] = json!([u.doc()]);
            manifest["unit"] = json!(u.id);
            std::fs::write(
                uinputs.join("manifest.json"),
                serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
            )
            .map_err(|e| format!("the unit's folder: {e}"))?;
            mounts = vec![
                Mount {
                    host: uin,
                    container: "/input".into(),
                    read_only: true,
                },
                Mount {
                    host: uinputs,
                    container: "/inputs".into(),
                    read_only: true,
                },
                Mount {
                    host: b.out.clone(),
                    container: "/output".into(),
                    read_only: false,
                },
            ];
            mounts.extend(sources);
            mounts.extend(shared.typed.iter().cloned());
            for (id, dir) in &shared.derivative_dirs {
                mounts.push(Mount {
                    host: dir.clone(),
                    container: format!("/inputs/{id}"),
                    read_only: true,
                });
            }
            participants = u.subject.iter().cloned().collect();
            env.push(("NILS_UNIT".into(), u.id.clone()));
        }
    }
    // record 55 E5: the slice cache, read-write, for a pipeline that
    // declares the model inputs it keeps there; in the site's working
    // place, since the pictures are clinical
    if d.asks_cache(x.params) {
        let dir = crate::slice_cache::folder(Path::new(&x.output.path));
        std::fs::create_dir_all(&dir).map_err(|e| format!("the slice cache's folder: {e}"))?;
        mounts.push(Mount {
            host: dir,
            container: descriptor::CACHE_PATH.into(),
            read_only: false,
        });
    }
    // a secret, read-only, in this pipeline's containers alone
    for g in x.secrets {
        mounts.push(Mount {
            host: g.path.clone(),
            container: g.secret.mount.clone(),
            read_only: true,
        });
        if let Some(var) = &g.secret.env {
            env.push((var.clone(), g.secret.mount.clone()));
        }
    }
    // record 57 P5: what the tool's terms want set beside its licence file
    for (k, v) in x.licence_env {
        env.push((k.clone(), v.clone()));
    }
    // the threads parameter told the cores this start was given; record 57
    // P2: a unit that runs apart takes the knobs its pack chose for it
    let chosen = shared
        .unit_params
        .lock()
        .map_err(|_| "the units' knobs were poisoned".to_string())?;
    let unit_params = chosen
        .get(b.key.as_deref().unwrap_or(""))
        .unwrap_or(x.params);
    let argv = d.argv(&d.params_for_cores(unit_params, cores), &participants)?;
    drop(chosen);
    // who the process is: for podman the engine's own account, which
    // `--userns keep-id` maps to itself, never the group of a shared
    // (setgid) working place, which keep-id does not map and which would
    // put the outputs on a sub-gid or keep the container from starting;
    // for docker, whose daemon maps nothing, the output folder's owner
    let user = {
        #[cfg(unix)]
        {
            if x.runtime.name() == "podman" {
                Some(this_account())
            } else {
                use std::os::unix::fs::MetadataExt;
                std::fs::metadata(&b.out).ok().map(|m| (m.uid(), m.gid()))
            }
        }
        #[cfg(not(unix))]
        {
            None
        }
    };
    Ok(Invocation {
        name: format!("nils-run-{}-{}", x.run_id, nonce()),
        image: p.image.clone(),
        argv,
        mounts,
        gpu: on_card,
        card: None,
        local_image: shared.local_image.clone(),
        // apptainer's /tmp and /var/tmp, on disk beside the unit's log;
        // podman's and docker's are the container's own
        workdir: (x.runtime.kind == runtime::Kind::Apptainer)
            .then(|| b.log.parent().map(|d| d.join(WORKDIR)))
            .flatten(),
        note: None,
        env,
        user,
        // the unit's declared cores and memory, enforced where the runtime
        // can; the lane's budget counts them either way
        cpus: shared.limits.0.then_some(cores),
        memory_mib: shared.limits.1.then(|| lane::mib_of_gb(d.needs.memory_gb)),
    })
}

/// A bids unit's own input: the dataset's top-level files, and only its
/// subject's folder, or of that only the top-level files and its session's
/// folder, linked from the run's release (a copy across filesystems).
fn unit_input(input: &Path, into: &Path, u: &Unit, level: Level) -> Result<(), String> {
    let e = |e: std::io::Error| format!("the unit's input: {e}");
    for entry in std::fs::read_dir(input).map_err(e)? {
        let entry = entry.map_err(e)?;
        if entry.file_type().map_err(e)?.is_file() {
            link_file(&entry.path(), &into.join(entry.file_name()))?;
        }
    }
    let Some(subject) = &u.subject else {
        return Err(format!("the unit {} names no subject", u.id));
    };
    let sub = format!("sub-{subject}");
    match (level, &u.session) {
        (Level::Session, Some(session)) => {
            let from = input.join(&sub);
            std::fs::create_dir_all(into.join(&sub)).map_err(e)?;
            for entry in std::fs::read_dir(&from).map_err(e)? {
                let entry = entry.map_err(e)?;
                if entry.file_type().map_err(e)?.is_file() {
                    link_file(&entry.path(), &into.join(&sub).join(entry.file_name()))?;
                }
            }
            let ses = format!("ses-{session}");
            link_tree(&from.join(&ses), &into.join(&sub).join(&ses))
        }
        _ => link_tree(&input.join(&sub), &into.join(&sub)),
    }
}

/// Link a folder's regular files, folder by folder; a link is left out.
fn link_tree(from: &Path, to: &Path) -> Result<(), String> {
    let e = |e: std::io::Error| format!("the unit's input: {e}");
    std::fs::create_dir_all(to).map_err(e)?;
    for entry in std::fs::read_dir(from).map_err(e)? {
        let entry = entry.map_err(e)?;
        let kind = entry.file_type().map_err(e)?;
        if kind.is_dir() {
            link_tree(&entry.path(), &to.join(entry.file_name()))?;
        } else if kind.is_file() {
            link_file(&entry.path(), &to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// A hard link, or a copy where the two are not on one filesystem.
fn link_file(from: &Path, to: &Path) -> Result<(), String> {
    match std::fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("the unit's input: {e}")),
        Err(e) => Err(format!("the unit's input: {e}")),
    }
}

/// Stop what a run that went away left running: each unit in flight, by its
/// container's name, and its process where it is this host's and is still
/// the unit's (its command line names the container or the unit's folder).
fn stop_left(rt: &runtime::Cli, together: bool, units: &[rows::Unit], out: &Path) {
    let host = job::hostname();
    for u in units.iter().filter(|u| u.state == "running") {
        if let Some(name) = &u.container {
            rt.stop(&Invocation {
                name: name.clone(),
                image: String::new(),
                argv: Vec::new(),
                mounts: Vec::new(),
                gpu: false,
                card: None,
                local_image: None,
                workdir: None,
                note: None,
                env: Vec::new(),
                user: None,
                cpus: None,
                memory_mib: None,
            });
        }
        let Some(pid) = u.pid.filter(|_| u.host.as_deref() == Some(host.as_str())) else {
            continue;
        };
        if job::process_alive(pid) != Some(true) {
            continue;
        }
        let line = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let line = String::from_utf8_lossy(&line);
        let folder = out.join(&u.unit).display().to_string();
        let ours = u.container.as_deref().is_some_and(|n| line.contains(n))
            || line.contains(&folder)
            || (together && line.contains(&out.display().to_string()));
        if ours {
            // the runtime's process led a group of its own
            if let Ok(leader) = u32::try_from(pid) {
                runtime::kill_group(leader);
            }
        }
    }
}

/// The image apptainer runs (record 49 A2; record 57 P5): the install in
/// the software place whose digest is the pipeline's, installed or
/// registered, verified before it is run; else built once from the pinned
/// digest into the place as an install, `<app>/<version>/image`, with its
/// manifest, wrappers and module, so the next run and people find it there
/// (the image cache of Wave 7a becomes the place; a verified copy left in
/// the old cache is adopted). Answers none when the run was cancelled while
/// it built.
fn ensure_image(
    registry: &mut Registry,
    x: &Execution<'_>,
    working: &Path,
    run_dir: &Path,
) -> Result<Option<PathBuf>, String> {
    let form = image_form(registry);
    if let Some(used) = crate::software::for_run(registry, x.pipeline)?
        && used.manifest.kind.on_disk()
    {
        if let Some(why) = crate::software::failed_verification(&used) {
            return Err(why);
        }
        // used only as verified (record 49, after review): what fails its
        // recorded fingerprint is not run
        if crate::software::still_verified(&used)? {
            return Ok(Some(PathBuf::from(&used.runnable)));
        }
        return Err(format!(
            "the {} at {} is not what was verified: nils software verify {} says what changed",
            used.words(),
            used.runnable,
            used.manifest.spec()
        ));
    }
    // a verified copy in the old image cache is adopted into the place
    let cache = runtime::local_image(&working.join(IMAGES), &x.pipeline.image_digest, form);
    if runtime::image_verified(&cache, form)
        && let Ok(used) = crate::software::adopt_built(registry, x.pipeline, &cache, form, x.who)
    {
        let mut record = cache.file_name().unwrap_or_default().to_os_string();
        record.push(".digest");
        let _ = std::fs::remove_file(cache.with_file_name(record));
        return Ok(Some(PathBuf::from(&used.runnable)));
    }
    let staging = crate::software::staging_dir(registry, x.pipeline)?;
    let part = staging.join(format!(".image.part-{}", nonce()));
    let log = std::fs::File::create(run_dir.join("image.log"))
        .map_err(|e| format!("the run's folder: {e}"))?;
    let err_log = log
        .try_clone()
        .map_err(|e| format!("the run's folder: {e}"))?;
    let _ = job::beat(
        registry.store(),
        x.job_id,
        Some(&json!({"run": x.run_id, "phase": "image", "image": x.pipeline.image})),
    );
    let mut child = std::process::Command::new(&x.runtime.program)
        .args(runtime::build_argv(&x.pipeline.image, &part, form))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(err_log))
        .spawn()
        .map_err(|e| format!("apptainer could not start a build: {e}"))?;
    let (status, stopped) = wait_beating(
        &mut child,
        registry.store(),
        x.job_id,
        &json!({"run": x.run_id, "phase": "image", "image": x.pipeline.image}),
    )?;
    let clean = |p: &Path| {
        let _ = std::fs::remove_dir_all(p);
        let _ = std::fs::remove_file(p);
    };
    if stopped {
        clean(&part);
        return Ok(None);
    }
    if !status.success() || !part.exists() {
        clean(&part);
        return Err(format!(
            "apptainer could not build {} from its digest; its log is {RUNS}/{}/image.log",
            x.pipeline.image, x.run_id
        ));
    }
    // into the place, as an install: another run that built it meanwhile
    // is found there, and this build is let go
    let used = crate::software::adopt_built(registry, x.pipeline, &part, form, x.who)
        .inspect_err(|_| clean(&part))?;
    Ok(Some(PathBuf::from(&used.runnable)))
}

/// What a run's `results.json` in a folder says, read only where it lies
/// inside the folder, never through a link the container planted (record
/// 43 review); or why it cannot be read; or neither, where there is none.
fn read_results(out: &Path) -> (Option<nils_pipeline::Results>, Option<String>) {
    let results_path = out.join(nils_pipeline::results::FILE);
    if std::fs::symlink_metadata(&results_path).is_err() {
        return (None, None);
    }
    match nils_pipeline::files::read_inside(out, nils_pipeline::results::FILE)
        .and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))
        .and_then(|t| nils_pipeline::results::parse(&t))
    {
        Ok(r) => (Some(r), None),
        Err(e) => (None, Some(e)),
    }
}

/// What one unit's container said of it, or what its templates find.
#[allow(clippy::too_many_arguments)]
fn outcome_of(
    u: &Unit,
    reported: &Option<nils_pipeline::Results>,
    by_unit: &HashMap<String, usize>,
    unreadable: &Option<String>,
    exit_code: Option<i64>,
    unit_outputs: &[descriptor::Output],
    run_outputs: &[descriptor::Output],
    out: &Path,
) -> Outcome {
    let is_run_file = |rel: &str| nils_pipeline::files::which_output(run_outputs, rel).is_some();
    if let Some(e) = unreadable {
        return Outcome {
            status: "failed",
            error: Some(e.clone()),
            metrics: json!({}),
            files: Vec::new(),
        };
    }
    if let Some(r) = reported {
        return match by_unit.get(&u.id).and_then(|&i| r.units.get(i)) {
            None => Outcome {
                status: "unreported",
                error: Some("the pipeline's results say nothing of this unit".into()),
                metrics: json!({}),
                files: Vec::new(),
            },
            Some(e) => {
                let mut files: Vec<String> = e
                    .derivatives
                    .iter()
                    .filter(|f| !is_run_file(f))
                    .cloned()
                    .collect();
                if e.status == nils_pipeline::results::Status::Succeeded && files.is_empty() {
                    files = found_for(unit_outputs, out, u);
                }
                Outcome {
                    status: e.status.name(),
                    error: e.error.clone(),
                    metrics: e.metrics.clone(),
                    files: if e.status == nils_pipeline::results::Status::Succeeded {
                        files
                    } else {
                        Vec::new()
                    },
                }
            }
        };
    }
    if exit_code != Some(0) {
        return Outcome {
            status: "failed",
            error: Some(format!(
                "the container exited {} and wrote no results.json",
                exit_code.map_or("by a signal".to_string(), |c| c.to_string())
            )),
            metrics: json!({}),
            files: Vec::new(),
        };
    }
    let files = found_for(unit_outputs, out, u);
    if files.is_empty() {
        Outcome {
            status: "failed",
            error: Some("no declared output was found for this unit".into()),
            metrics: json!({}),
            files,
        }
    } else {
        Outcome {
            status: "succeeded",
            error: None,
            metrics: json!({}),
            files,
        }
    }
}

/// Take in what one batch left: its units marked registering, the secrets
/// swept out, then each unit's files hashed by the engine and registered
/// naming the run, and the unit marked over with its outcome. Taking in is
/// idempotent (a file registered already is found, not registered again),
/// so a resume that finds a unit registering takes it in again and runs
/// nothing.
/// A job's heart kept beating through long work that is not a container:
/// hashing and sweeping what one left (record 49, after review), so a long
/// intake is never taken for a stale job.
struct Pulse {
    job_id: i64,
    last: Instant,
    progress: Value,
}

impl Pulse {
    fn new(job_id: i64, progress: Value) -> Pulse {
        Pulse {
            job_id,
            last: Instant::now(),
            progress,
        }
    }

    /// Beat, at most every five seconds.
    fn tick(&mut self, store: &mut Store) {
        if self.last.elapsed() >= Duration::from_secs(5) {
            self.last = Instant::now();
            let _ = job::beat(store, self.job_id, Some(&self.progress));
        }
    }
}

/// The run's own folder beside its output, which no container sees: where
/// the sweep writes the copies it renames over what it rewrites.
fn scratch_of(x: &Execution<'_>, b: &Batch) -> PathBuf {
    let run_rel = match &b.key {
        Some(_) => b
            .rel_out
            .rsplit_once('/')
            .map_or(b.rel_out.as_str(), |(r, _)| r),
        None => b.rel_out.as_str(),
    };
    PathBuf::from(&x.output.path).join(format!("{run_rel}.nils"))
}

/// Sweep what a batch's container left of the secrets it was given (record
/// 49 R3): its output folder, its `results.json` and its log. Answers what
/// the sweep did, as the unit's row keeps it, and what it could not read.
fn sweep_batch(
    registry: &mut Registry,
    x: &Execution<'_>,
    b: &Batch,
    pulse: &mut Pulse,
) -> Result<(Vec<Value>, Vec<String>), String> {
    let mut swept: Vec<Value> = Vec::new();
    let mut shut: Vec<String> = Vec::new();
    if x.secrets.is_empty() {
        return Ok((swept, shut));
    }
    let held: Vec<secrets::Held> = x.secrets.iter().map(|g| g.held.clone()).collect();
    let scratch = scratch_of(x, b);
    let results_path = b.out.join(nils_pipeline::results::FILE);
    let done = secrets::sweep(
        &b.out,
        &held,
        &[results_path.as_path()],
        &scratch,
        &mut || pulse.tick(registry.store()),
    )
    .map_err(|e| format!("the sweep for secrets: {e}"))?;
    for s in done.done {
        match s {
            secrets::Swept::Removed(rel) => swept.push(json!({
                "file": rel.to_string_lossy(),
                "why": "it held a secret input, and was removed",
            })),
            secrets::Swept::Unscannable(rel, why) => swept.push(json!({
                "file": rel.to_string_lossy(),
                "why": format!("it could not be read inside to sweep it for the secret inputs ({why}), and was removed"),
            })),
            secrets::Swept::Redacted(rel) => {
                swept.push(json!({"file": rel.to_string_lossy(), "redacted": true}));
            }
        }
    }
    shut.extend(
        done.unreadable
            .iter()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    secrets::redact_file(&b.log, &held, &scratch).map_err(|e| format!("the run's log: {e}"))?;
    Ok((swept, shut))
}

#[allow(clippy::too_many_arguments)]
fn take_in(
    registry: &mut Registry,
    x: &Execution<'_>,
    m: &Materialised,
    b: &Batch,
    code: Option<i32>,
    signal: Option<i32>,
    row_of: &BTreeMap<String, (i64, String)>,
    taken_files: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<(), String> {
    let d = x.descriptor;
    let exit_code = code.map(i64::from);
    let now = nils_registry::time::now_iso();
    for &i in &b.units {
        if let Some((id, _)) = row_of.get(&m.units[i].id) {
            rows::unit_registering(registry.store(), *id, exit_code).map_err(|e| e.to_string())?;
        }
    }
    // record 49 R3: nothing a container left is read before what holds a
    // secret is swept from it
    let mut pulse = Pulse::new(x.job_id, json!({"run": x.run_id, "phase": "intake"}));
    let (swept, shut) = sweep_batch(registry, x, b, &mut pulse)?;
    let (reported, unreadable) = read_results(&b.out);
    // each unit's entry by its name, found once and not searched for per
    // unit: a run of hundreds of thousands of stacks is as many entries
    let mut by_unit: HashMap<String, usize> = HashMap::new();
    for (i, e) in reported.iter().flat_map(|r| r.units.iter()).enumerate() {
        by_unit
            .entry(nils_pipeline::results::normalise(&e.unit_id))
            .or_insert(i);
    }
    // record 55 E5: what the image read whole is kept as each file's
    // content, so the next run reads the slice cache instead of the file
    if !m.files.is_empty()
        && shut.is_empty()
        && let Some(r) = &reported
    {
        let learnt = learnt_of(x.run_id, m, &b.units, r, &by_unit);
        if let Err(e) = nils_registry::content::learn(registry.store(), &learnt) {
            eprintln!(
                "nils: run {}: what its image read could not be kept: {e}",
                x.run_id
            );
        }
    }
    let unit_outputs: Vec<descriptor::Output> =
        d.outputs.iter().filter(|o| !o.run_level).cloned().collect();
    let run_outputs: Vec<descriptor::Output> =
        d.outputs.iter().filter(|o| o.run_level).cloned().collect();
    // a container that failed as a whole, with no results.json to say more
    let whole = reported.is_none() && (exit_code != Some(0) || unreadable.is_some());
    let cards: Vec<Value> = reported
        .as_ref()
        .map(|r| r.models.clone())
        .unwrap_or_default();
    let log_rel = b
        .log
        .strip_prefix(&x.scratch.path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("{RUNS}/{}/log.txt", x.run_id));
    // 2026-10-12: a container that failed as a whole is read for its cause
    // in the end of its log, where the runtime and the tool said it
    let log_tail = whole
        .then(|| nils_pipeline::failure::log_tail(&b.log))
        .flatten();
    for (n, &i) in b.units.iter().enumerate() {
        let u = &m.units[i];
        let o = outcome_of(
            u,
            &reported,
            &by_unit,
            &unreadable,
            exit_code,
            &unit_outputs,
            &run_outputs,
            &b.out,
        );
        let Some((row_id, _)) = row_of.get(&u.id) else {
            continue;
        };
        let mut doc = if shut.is_empty() {
            match register_unit(
                registry,
                x,
                u,
                o,
                &b.out,
                &unit_outputs,
                &cards,
                taken_files,
                &now,
                &mut pulse,
            ) {
                Ok(doc) => doc,
                Err(e) => {
                    json!({"status": "failed", "error": e, "metrics": {}, "files": [], "refused": []})
                }
            }
        } else {
            // record 49 R3: what could not be read could not be swept for
            // the secret, so nothing of the batch is vouched for
            json!({
                "status": "failed",
                "error": format!(
                    "{} of what the container left could not be read, even given back to the engine, so it could not be swept for the secret inputs; nothing of it is registered",
                    shut.len()
                ),
                "metrics": {}, "files": [],
                "refused": shut.iter().map(|p| json!({
                    "unit": u.id, "file": p,
                    "why": "it could not be read to sweep it for the secret inputs",
                })).collect::<Vec<_>>(),
            })
        };
        doc["whole"] = json!(whole);
        if let Some(e) = &unreadable {
            doc["unreadable"] = json!(e);
        }
        doc["log"] = json!(log_rel);
        doc["out"] = json!(b.rel_out);
        if n == 0 && !swept.is_empty() {
            doc["swept"] = json!(swept);
        }
        if whole {
            crate::failures::mark_unit(&mut doc, exit_code, signal, log_tail.as_deref());
        } else {
            crate::failures::mark_unit(&mut doc, None, None, None);
        }
        rows::unit_over(registry.store(), *row_id, exit_code, &doc, &now)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Record 55 E5: the files each unit of a batch says its image read whole,
/// by their place in the unit's list in `stacks.json`, with the sha256 it
/// gave and the size and time `source_file` held when the run began. A
/// place past the unit's list is left out.
fn learnt_of(
    run_id: i64,
    m: &Materialised,
    units: &[usize],
    reported: &nils_pipeline::Results,
    by_unit: &HashMap<String, usize>,
) -> Vec<nils_registry::content::Learnt> {
    let mut out = Vec::new();
    for &i in units {
        let (Some(u), Some(files)) = (m.units.get(i), m.files.get(i)) else {
            continue;
        };
        let Some(e) = by_unit
            .get(&nils_pipeline::results::normalise(&u.id))
            .and_then(|k| reported.units.get(*k))
        else {
            continue;
        };
        for (at, sha) in &e.read {
            if let Some((source_id, path, size, mtime_ns)) = files.get(*at) {
                out.push(nils_registry::content::Learnt {
                    source_id: *source_id,
                    path: path.clone(),
                    size: *size,
                    mtime_ns: *mtime_ns,
                    sha256: sha.clone(),
                    run_id: Some(run_id),
                });
            }
        }
    }
    out
}

/// Whether the run registered a file at this path already: a resume that
/// takes a unit in again finds it rather than registering it twice.
fn registered_already(store: &mut Store, run_id: i64, path: &str) -> Result<Option<i64>, String> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE run_id = {} AND path = {}",
        store.qualified("derivative"),
        d.param(1, Type::Int),
        d.param(2, Type::Text)
    );
    store
        .query_opt(&sql, &[Param::Int(run_id), Param::from(path)])
        .and_then(|r| r.map(|r| r.int(0)).transpose())
        .map_err(|e| e.to_string())
}

/// Hash and register one unit's files; answers the unit's outcome as its
/// row keeps it.
#[allow(clippy::too_many_arguments)]
fn register_unit(
    registry: &mut Registry,
    x: &Execution<'_>,
    u: &Unit,
    mut o: Outcome,
    out: &Path,
    unit_outputs: &[descriptor::Output],
    cards: &[Value],
    taken_files: &mut std::collections::BTreeSet<PathBuf>,
    now: &str,
    pulse: &mut Pulse,
) -> Result<Value, String> {
    let working = PathBuf::from(&x.output.path);
    let mut hashed: Vec<Value> = Vec::new();
    let mut refused: Vec<Value> = Vec::new();
    let mut tables: Vec<Value> = Vec::new();
    let mut masks: Vec<Value> = Vec::new();
    let (mut registered, mut bytes_total, mut embedded, mut cached) =
        (0usize, 0u64, 0usize, 0usize);
    let Some(belongs) = u.belongs() else {
        return Ok(json!({
            "status": "failed", "error": "the unit belongs to no subject the registry holds",
            "metrics": o.metrics, "files": [], "refused": [],
        }));
    };
    let vars = u.vars();
    let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    for rel in &o.files {
        // a unit's file is one its own templates find, never another's
        let Some(declared) = nils_pipeline::files::unit_output(unit_outputs, rel, &pairs) else {
            refused.push(json!({
                "unit": u.id, "file": rel,
                "why": "not a file this unit's declared outputs name",
            }));
            continue;
        };
        let file = match nils_pipeline::files::inside(out, rel) {
            Ok(f) => f,
            Err(e) => {
                refused.push(json!({"unit": u.id, "file": rel, "why": e}));
                continue;
            }
        };
        if !taken_files.insert(file.clone()) {
            refused.push(json!({
                "unit": u.id, "file": rel,
                "why": "the same file as another output of the run, by another name",
            }));
            continue;
        }
        let (bytes, sha) =
            nils_pipeline::files::sha256_file_ticking(&file, &mut || pulse.tick(registry.store()))
                .map_err(|e| format!("{rel}: {e}"))?;
        let kind = declared.kind.as_str();
        let media = nils_pipeline::files::media_type(rel, declared.media_type.as_deref());
        // the row names the file where it is, never a link to it
        let path = place_path(&working, &file)?;
        if kind == nils_registry::embedding::KIND {
            // an encoder a card names is registered once, by whichever run
            // meets it first, so the two are taken one run at a time
            match with_lock(registry, INTAKE_LOCK, Some(x.job_id), |registry| {
                register_embedding(
                    registry,
                    x,
                    u,
                    &declared.encoders,
                    &file,
                    &path,
                    bytes,
                    &sha,
                    cards,
                    now,
                )
            }) {
                Ok(nils_registry::embedding::Registered::New(_)) => {
                    embedded += 1;
                    registered += 1;
                    bytes_total += bytes;
                }
                Ok(nils_registry::embedding::Registered::Kept { .. }) => cached += 1,
                Err(why) => {
                    refused.push(json!({"unit": u.id, "file": rel, "why": why}));
                    continue;
                }
            }
        } else {
            let id = match registered_already(registry.store(), x.run_id, &path)? {
                // taken in before a resume: the row stands
                Some(id) => id,
                None => derivative::insert_of_run(
                    registry.store(),
                    &derivative::New {
                        kind,
                        belongs: &belongs,
                        place_id: x.output.id,
                        path: &path,
                        bytes: bytes as i64,
                        sha256: &sha,
                        media_type: &media,
                        registered_by: x.who,
                        actor: Some(x.actor),
                        model_id: None,
                        run_id: None,
                        preprocess_version: None,
                        supersedes_id: None,
                        created_at: now,
                    },
                    x.run_id,
                )
                .map_err(|e| e.to_string())?,
            };
            registered += 1;
            bytes_total += bytes;
            // record 55 H3: a mask's grid, its scan and its parts, read as
            // it is registered; a mask that cannot be read so is kept, and
            // the unit's outcome says why
            if kind == "mask" {
                let at = unit_grid(registry.store(), x, u, declared.grid.as_deref());
                let sets: Vec<crate::mask::SetSpec> = if declared.parts.is_empty() {
                    vec![crate::mask::SetSpec::whole()]
                } else {
                    declared
                        .parts
                        .iter()
                        .map(crate::mask::SetSpec::of)
                        .collect()
                };
                let read = derivative::get(registry.store(), id)
                    .map_err(|e| e.to_string())
                    .and_then(|d| d.ok_or_else(|| format!("derivative {id} went")))
                    .and_then(|d| crate::derivatives::read_mask(registry, &d, at, sets));
                match read {
                    Ok(r) => masks.push(json!({"file": rel, "derivative": id, "read": r})),
                    Err(why) => masks.push(json!({"file": rel, "derivative": id, "why": why})),
                }
            }
            // a table's rows are the unit's measures (record 49 A3), read
            // when the run is closed, so a resume reads what it kept
            if declared.table.is_some() {
                tables.push(
                    json!({"output": declared.id, "file": rel, "path": path, "derivative": id}),
                );
            }
        }
        hashed.push(json!({"path": rel, "sha256": sha}));
    }
    hashed.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    o.files = hashed
        .iter()
        .filter_map(|h| h["path"].as_str().map(str::to_string))
        .collect();
    Ok(json!({
        "status": o.status, "error": o.error, "metrics": o.metrics,
        "files": hashed, "refused": refused, "registered": registered,
        "bytes": bytes_total, "embedded": embedded, "cached": cached, "tables": tables,
        "masks": masks,
    }))
}

/// Close a run over what its units left: the run's own files where its
/// units ran together and the container completed, the seeds, the
/// proposals, the review items, the digest and the summary.
fn finalize(
    registry: &mut Registry,
    x: &Execution<'_>,
    m: &Materialised,
    working: &Path,
    out: &Path,
    rel_out: &str,
    taken_files: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<Ended, String> {
    let p = x.pipeline;
    let d = x.descriptor;
    let now = nils_registry::time::now_iso();
    let unit_rows = rows::units(registry.store(), x.run_id).map_err(|e| e.to_string())?;
    // the device the run ran on, as recorded: a full optional card may
    // have sent it to the CPU before its first unit
    let on_card = x.gpu
        && rows::run(registry.store(), x.run_id)
            .ok()
            .flatten()
            .is_some_and(|r| r.device != "cpu");
    let by_name: BTreeMap<&str, &rows::Unit> =
        unit_rows.iter().map(|u| (u.unit.as_str(), u)).collect();
    let apart = d.units == Units::Apart;
    let mut digest_units: Vec<Value> = Vec::new();
    let mut refused: Vec<Value> = Vec::new();
    let (mut registered, mut bytes_total, mut embedded, mut cached) =
        (0usize, 0u64, 0usize, 0usize);
    let mut statuses: Vec<(usize, String, Option<String>, Value, bool)> = Vec::new();
    for (i, u) in m.units.iter().enumerate() {
        let Some(row) = by_name.get(u.id.as_str()) else {
            continue;
        };
        let o = &row.outcome;
        let status = o["status"].as_str().unwrap_or("failed").to_string();
        // a unit that reused a kept result digests as the unit that made
        // it did (its status and its files with their hashes), so a re-run
        // that ran nothing gives the digest of the run it repeats
        let kept = row
            .reused_from_run
            .and_then(|from| rows::units(registry.store(), from).ok())
            .and_then(|us| us.into_iter().find(|k| k.unit == u.id))
            .filter(|k| {
                k.outcome["files"]
                    .as_array()
                    .is_some_and(|f| f.iter().all(Value::is_object))
            });
        let (digest_status, digest_files) = match &kept {
            Some(k) => (
                k.outcome["status"].as_str().unwrap_or("failed").to_string(),
                k.outcome["files"].clone(),
            ),
            None => (status.clone(), o["files"].clone()),
        };
        digest_units.push(json!({"unit": u.id, "status": digest_status, "files": digest_files}));
        refused.extend(o["refused"].as_array().cloned().unwrap_or_default());
        for s in o["swept"].as_array().into_iter().flatten() {
            if s["why"].is_string() {
                refused.push(json!({
                    "unit": if apart { u.id.as_str() } else { "run" },
                    "file": s["file"], "why": s["why"],
                }));
            }
        }
        let n = |k: &str| o[k].as_u64().unwrap_or(0);
        registered += n("registered") as usize;
        bytes_total += n("bytes");
        embedded += n("embedded") as usize;
        cached += n("cached") as usize;
        statuses.push((
            i,
            status,
            o["error"].as_str().map(str::to_string),
            o["metrics"].clone(),
            o["whole"].as_bool().unwrap_or(false),
        ));
    }
    digest_units.sort_by(|a, b| a["unit"].as_str().cmp(&b["unit"].as_str()));
    // record 49 A3: the rows of the run's tables, as measures, and what
    // reading them came to; a unit's own tables are read here from where
    // it registered them, so a run taken up again reads what it kept
    let mut measured: Vec<crate::measures::Pending> = Vec::new();
    let mut notes = crate::measures::Notes::default();
    for u in &m.units {
        let Some(row) = by_name.get(u.id.as_str()) else {
            continue;
        };
        let Some(belongs) = u.belongs() else {
            continue;
        };
        for t in row.outcome["tables"].as_array().into_iter().flatten() {
            let (Some(output), Some(path), Some(id)) = (
                t["output"].as_str(),
                t["path"].as_str(),
                t["derivative"].as_i64(),
            ) else {
                continue;
            };
            let Some(declared) = d.outputs.iter().find(|o| o.id == output) else {
                continue;
            };
            let read = std::fs::read(working.join(path))
                .map_err(|e| e.to_string())
                .and_then(|b| {
                    crate::measures::unit_table(
                        declared,
                        &b,
                        &u.id,
                        &belongs,
                        id,
                        &mut notes,
                        &mut measured,
                    )
                });
            if let Err(why) = read {
                refused.push(json!({
                    "unit": u.id, "file": t["file"],
                    "why": format!("its rows could not be read: {why}"),
                }));
            }
        }
    }
    // the run's exit: together, its one container's; apart, the first that
    // did not exit 0, or 0
    let exit_code = if apart {
        // a unit never started (one missing an input) has no exit of its own
        unit_rows
            .iter()
            .filter(|u| u.attempts > 0)
            .map(|u| u.exit_code)
            .find(|c| *c != Some(0))
            .unwrap_or(Some(0))
    } else {
        unit_rows
            .iter()
            .find(|u| u.attempts > 0)
            .and_then(|u| u.exit_code)
    };
    let whole_units = statuses.iter().filter(|s| s.4).count();
    let unreadable: Option<String> = unit_rows
        .iter()
        .find_map(|u| u.outcome["unreadable"].as_str().map(str::to_string));
    // together: the container failed as a whole; apart: every unit's did
    // a unit missing an input never ran, and says nothing of the containers
    let missing_input = unit_rows
        .iter()
        .filter(|u| u.outcome["missing"].is_array())
        .count();
    let ran = statuses.len().saturating_sub(missing_input);
    let whole = ran > 0 && whole_units == ran;
    let completed = if apart { !whole } else { exit_code == Some(0) };

    // what the containers said, read again from where each wrote it
    let folders: Vec<PathBuf> = if apart {
        m.units
            .iter()
            .filter(|u| {
                by_name
                    .get(u.id.as_str())
                    .is_some_and(|r| r.state == "over")
            })
            .map(|u| out.join(&u.id))
            .collect()
    } else {
        vec![out.to_path_buf()]
    };
    let reports: Vec<nils_pipeline::Results> =
        folders.iter().filter_map(|f| read_results(f).0).collect();

    // what follows reads and writes what every run shares: a model
    // registered by its name, the review items a newer run supersedes, the
    // measures and the epoch; runs going on side by side take it one run
    // at a time (2026-09-30)
    let (
        run_files,
        models_made,
        seeds_doc,
        results_digest,
        items,
        breached,
        measures_written,
        proposed,
        pack_flags,
    ) = with_lock(registry, INTAKE_LOCK, Some(x.job_id), |registry| {
        // the run's own files (record 43): a model it fitted, registered from
        // its card, and any other run-level output; only a run that completed
        // with its units together
        let run_outputs: Vec<descriptor::Output> =
            d.outputs.iter().filter(|o| o.run_level).cloned().collect();
        let mut run_files: Vec<Value> = Vec::new();
        let mut models_made: Vec<Value> = Vec::new();
        if !apart && exit_code == Some(0) {
            for o in &run_outputs {
                let found = nils_pipeline::files::found(out, &o.template, &[]);
                if o.kind == "model" {
                    match found.as_slice() {
                        [rel] => match register_model(registry, x, o, out, rel, &now) {
                            Ok((model, id, sha)) => {
                                registered += 1;
                                run_files.push(json!({"path": rel, "sha256": sha}));
                                models_made.push(json!({
                                    "output": o.id, "model": model.named(), "derivative": id,
                                    "encoders": model.encoder_model_ids, "trained_on": model.trained_on,
                                }));
                            }
                            Err(why) => refused.push(json!({"output": o.id, "file": rel, "why": why})),
                        },
                        [] => refused.push(json!({"output": o.id, "why": "the run wrote no model"})),
                        many => refused.push(json!({
                            "output": o.id,
                            "why": format!("{} files match; a model output is one file", many.len()),
                        })),
                    }
                    continue;
                }
                for rel in found {
                    let file = match nils_pipeline::files::inside(out, &rel) {
                        Ok(f) => f,
                        Err(why) => {
                            refused.push(json!({"output": o.id, "file": rel, "why": why}));
                            continue;
                        }
                    };
                    if !taken_files.insert(file.clone()) {
                        refused.push(json!({
                            "output": o.id, "file": rel,
                            "why": "the same file as another output of the run, by another name",
                        }));
                        continue;
                    }
                    let (bytes, sha) = nils_pipeline::files::sha256_file(&file)
                        .map_err(|e| format!("{rel}: {e}"))?;
                    let media = nils_pipeline::files::media_type(&rel, o.media_type.as_deref());
                    let id = derivative::insert_of_run(
                        registry.store(),
                        &derivative::New {
                            kind: &o.kind,
                            belongs: &Belongs::run(),
                            place_id: x.output.id,
                            path: &place_path(working, &file)?,
                            bytes: bytes as i64,
                            sha256: &sha,
                            media_type: &media,
                            registered_by: x.who,
                            actor: Some(x.actor),
                            model_id: None,
                            run_id: None,
                            preprocess_version: None,
                            supersedes_id: None,
                            created_at: &now,
                        },
                        x.run_id,
                    )
                    .map_err(|e| e.to_string())?;
                    registered += 1;
                    bytes_total += bytes;
                    // a run's table: each row's unit by its unit column (record
                    // 49 A3), and only a unit that succeeded
                    if o.table.is_some() {
                        let units: Vec<(String, Option<Belongs>)> = statuses
                            .iter()
                            .map(|(i, status, ..)| {
                                let u = &m.units[*i];
                                (
                                    u.id.clone(),
                                    (status == "succeeded").then(|| u.belongs()).flatten(),
                                )
                            })
                            .collect();
                        let read = std::fs::read(&file)
                            .map_err(|e| e.to_string())
                            .and_then(|b| {
                                crate::measures::run_table(
                                    o,
                                    &b,
                                    &units,
                                    id,
                                    &mut notes,
                                    &mut measured,
                                )
                            });
                        if let Err(why) = read {
                            refused.push(json!({
                                "output": o.id, "file": rel,
                                "why": format!("its rows could not be read: {why}"),
                            }));
                        }
                    }
                    run_files.push(json!({"path": rel, "sha256": sha}));
                }
            }
        }
        run_files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));

        // the seeds and the selection it suggests (record 43): kept whole as the
        // run's one derivative of kind seeds, apart from any proposal
        let mut seeds_doc = Value::Null;
        let mut seeds_digest = Value::Null;
        let all_seeds: Vec<&nils_pipeline::results::Seed> =
            reports.iter().flat_map(|r| r.seeds.iter()).collect();
        let any_selection = reports.iter().any(|r| r.selection.is_some());
        if !all_seeds.is_empty() || any_selection {
            let ours: std::collections::BTreeSet<i64> = x.stacks.iter().copied().collect();
            let taken: Vec<&Value> = all_seeds
                .iter()
                .filter(|s| ours.contains(&s.stack_id))
                .map(|s| &s.entry)
                .collect();
            let outside = all_seeds.len() - taken.len();
            let mut selection: Vec<i64> = reports
                .iter()
                .flat_map(|r| r.selection.iter().flatten().copied())
                .filter(|s| ours.contains(s))
                .collect();
            selection.sort_unstable();
            selection.dedup();
            let doc = json!({
                "contract": nils_pipeline::CONTRACT, "run": x.run_id, "pipeline": p.label(),
                "seeds": taken, "selection": {"stacks": selection},
            });
            let text = nils_pipeline::canonical(&doc);
            // into a folder of the engine's own beside the output, which the
            // container never saw, created new and never through a link
            let own = working.join(format!("{rel_out}.nils"));
            match std::fs::symlink_metadata(&own) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(format!(
                        "{rel_out}.nils is there already and is not a folder"
                    ));
                }
                Err(_) => {
                    std::fs::create_dir(&own).map_err(|e| format!("the run's own folder: {e}"))?
                }
            }
            let rel = format!("{rel_out}.nils/{}", nils_pipeline::results::SEEDS_FILE);
            let at = working.join(&rel);
            if std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_file()) {
                // a resume writes what the run says now
                let _ = std::fs::remove_file(&at);
            }
            nils_pipeline::files::write_new(&at, text.as_bytes())
                .map_err(|e| format!("{rel}: {e}"))?;
            let sha = hex_of(&nils_pipeline::sha256(text.as_bytes()));
            let id = derivative::insert_of_run(
                registry.store(),
                &derivative::New {
                    kind: "seeds",
                    belongs: &Belongs::run(),
                    place_id: x.output.id,
                    path: &rel,
                    bytes: text.len() as i64,
                    sha256: &sha,
                    media_type: "application/json",
                    registered_by: x.who,
                    actor: Some(x.actor),
                    model_id: None,
                    run_id: None,
                    preprocess_version: None,
                    supersedes_id: None,
                    created_at: &now,
                },
                x.run_id,
            )
            .map_err(|e| e.to_string())?;
            registered += 1;
            bytes_total += text.len() as u64;
            seeds_doc = json!({
                "seeds": taken.len(), "outside": outside, "selection": selection.len(),
                "derivative": id,
            });
            seeds_digest = json!(sha);
        }

        let proposals: Vec<Value> = reports
            .iter()
            .flat_map(|r| r.proposals.iter().cloned())
            .collect();
        let opinions: Vec<Value> = reports
            .iter()
            .flat_map(|r| r.opinions.iter().cloned())
            .collect();
        let mut digested = json!({
            "units": digest_units, "proposals": proposals, "run": run_files, "seeds": seeds_digest,
        });
        // record 55 D3: a run's opinions are its results too; a run with
        // none digests as before
        if !opinions.is_empty() {
            digested["opinions"] = json!(opinions);
        }
        let results_digest = nils_pipeline::sha256(nils_pipeline::canonical(&digested).as_bytes());

        // the units no one can vouch for become review items; a container
        // that failed as a whole, with no results.json to say more, is one
        // item of the run, not one per unit (wave 43's proof: three failed
        // runs left 2,023 items, each run for a single cause), and so is a run
        // whose every unit's container did
        let mut items: Vec<i64> = Vec::new();
        let label = p.label();
        if whole && !m.units.is_empty() {
            let error = statuses
                .first()
                .and_then(|s| s.2.clone())
                .unwrap_or_else(|| "the run failed".into());
            let log = unit_rows
                .first()
                .and_then(|u| u.outcome["log"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{RUNS}/{}/log.txt", x.run_id));
            let id = nils_registry::review::raise_pipeline_qc(
                registry.store(),
                &nils_registry::review::PipelineQc {
                    run_id: x.run_id,
                    pipeline: &label,
                    unit: "run",
                    stack_id: None,
                    subject_id: None,
                    session_day: None,
                    status: "failed",
                    error: Some(&error),
                    metrics: &json!({
                        "units": m.units.len(), "exit_code": exit_code, "log": log,
                    }),
                    job_id: Some(x.job_id),
                },
                &now,
            )
            .map_err(|e| e.to_string())?;
            items.push(id);
        }
        for (i, status, error, metrics, _) in &statuses {
            if whole || (status != "failed" && status != "unreported") {
                continue;
            }
            let u = &m.units[*i];
            let id = nils_registry::review::raise_pipeline_qc(
                registry.store(),
                &nils_registry::review::PipelineQc {
                    run_id: x.run_id,
                    pipeline: &label,
                    unit: &u.id,
                    stack_id: u.stack_id,
                    subject_id: u.subject_id,
                    session_day: u.session_day.as_deref(),
                    status: if status == "unreported" {
                        "unreported"
                    } else {
                        "failed"
                    },
                    error: error.as_deref(),
                    metrics,
                    job_id: Some(x.job_id),
                },
                &now,
            )
            .map_err(|e| e.to_string())?;
            items.push(id);
        }

        // a file the engine refused, a unit's or the run's own, is a review
        // item too (record 43 review)
        for r in &refused {
            let unit = r["unit"].as_str().unwrap_or("run");
            let u = m.units.iter().find(|u| u.id == unit);
            let id = nils_registry::review::raise_pipeline_qc(
                registry.store(),
                &nils_registry::review::PipelineQc {
                    run_id: x.run_id,
                    pipeline: &label,
                    unit,
                    stack_id: u.and_then(|u| u.stack_id),
                    subject_id: u.and_then(|u| u.subject_id),
                    session_day: u.and_then(|u| u.session_day.as_deref()),
                    status: "refused",
                    error: r["why"].as_str(),
                    metrics: &json!({"file": r["file"], "output": r["output"]}),
                    job_id: Some(x.job_id),
                },
                &now,
            )
            .map_err(|e| e.to_string())?;
            if !items.contains(&id) {
                items.push(id);
            }
        }

        // record 49 A3: the declared checks, held against each unit that
        // succeeded, a breach one pipeline:qc item naming the metric and its
        // value; a unit that has an item already keeps it
        let mut breached: Vec<Value> = Vec::new();
        if !whole {
            let already: std::collections::BTreeSet<String> = statuses
                .iter()
                .filter(|(_, status, ..)| status == "failed" || status == "unreported")
                .map(|(i, ..)| m.units[*i].id.clone())
                .chain(
                    refused
                        .iter()
                        .filter_map(|r| r["unit"].as_str().map(str::to_string)),
                )
                .collect();
            for (i, status, _, metrics, _) in &statuses {
                if status != "succeeded" || d.checks.is_empty() {
                    continue;
                }
                let u = &m.units[*i];
                let mine: BTreeMap<String, f64> = measured
                    .iter()
                    .filter(|p| p.unit_id == u.id)
                    .filter_map(|p| Some((p.name.clone(), p.number?)))
                    .collect();
                let (breaches, taken, unchecked) = crate::measures::hold(&d.checks, metrics, &mine);
                notes.unchecked += unchecked;
                if let Some(b) = u.belongs() {
                    for (metric, value) in taken {
                        measured.push(crate::measures::Pending {
                            unit_id: u.id.clone(),
                            belongs: b.clone(),
                            derivative_id: None,
                            source: nils_registry::measure::METRICS.into(),
                            name: metric,
                            ty: "number".into(),
                            number: Some(value),
                            text: None,
                            unit: None,
                        });
                    }
                }
                if breaches.is_empty() {
                    continue;
                }
                notes.breaches += breaches.len();
                breached.push(json!({"unit": u.id, "breaches": breaches}));
                if already.contains(&u.id) {
                    continue;
                }
                let words = crate::measures::breach_words(&breaches);
                let id = nils_registry::review::raise_pipeline_qc(
                    registry.store(),
                    &nils_registry::review::PipelineQc {
                        run_id: x.run_id,
                        pipeline: &label,
                        unit: &u.id,
                        stack_id: u.stack_id,
                        subject_id: u.subject_id,
                        session_day: u.session_day.as_deref(),
                        status: "breach",
                        error: Some(&words),
                        metrics: &json!({"breaches": breaches, "metrics": metrics}),
                        job_id: Some(x.job_id),
                    },
                    &now,
                )
                .map_err(|e| e.to_string())?;
                if !items.contains(&id) {
                    items.push(id);
                }
            }
        }
        // record 57 P4 (Wave 7b A1): the pack's checks, held after the
        // descriptor's: a flag keeps the result and names the check, its
        // value and its threshold; a fail fails the unit; a drop drops the
        // one value and flags the unit
        let mut pack_flags: Vec<Value> = Vec::new();
        let pack = crate::pipeline_pack::pack_of(p);
        if let Some(pack) = &pack
            && !pack.checks.is_empty()
        {
            let mut dropped: Vec<(String, String)> = Vec::new();
            for (i, status, error, metrics, _) in statuses.iter_mut() {
                if status != "succeeded" {
                    continue;
                }
                let u = &m.units[*i];
                let mine: BTreeMap<String, f64> = measured
                    .iter()
                    .filter(|q| q.unit_id == u.id)
                    .filter_map(|q| Some((q.name.clone(), q.number?)))
                    .collect();
                // a metric a pack check reads from results.json is a measure
                // too, as the descriptor's checks make it one
                if let Some(b) = u.belongs() {
                    for c in &pack.checks {
                        if mine.contains_key(&c.metric)
                            || measured
                                .iter()
                                .any(|q| q.unit_id == u.id && q.name == c.metric)
                        {
                            continue;
                        }
                        if let Some(v) = metrics[&c.metric].as_f64().filter(|v| v.is_finite()) {
                            measured.push(crate::measures::Pending {
                                unit_id: u.id.clone(),
                                belongs: b.clone(),
                                derivative_id: None,
                                source: nils_registry::measure::METRICS.into(),
                                name: c.metric.clone(),
                                ty: "number".into(),
                                number: Some(v),
                                text: None,
                                unit: None,
                            });
                        }
                    }
                }
                let held = crate::pipeline_pack::hold_checks(pack, metrics, &mine);
                if held.flags.is_empty() && held.failed.is_empty() {
                    continue;
                }
                let Some(row) = by_name.get(u.id.as_str()) else {
                    continue;
                };
                let mut record = row.pack.clone();
                if record.is_null() {
                    record = json!({});
                }
                // a unit over before a resume holds the same flags again, and
                // keeps the time it was flagged
                if record["flags"] != json!(held.flags) || record["flagged_at"].is_null() {
                    record["flagged_at"] = json!(now);
                }
                record["flags"] = json!(held.flags);
                rows::set_unit_pack(
                    registry.store(),
                    row.id,
                    &record,
                    row.comparability_key.as_deref(),
                    row.result_key.as_deref(),
                )
                .map_err(|e| e.to_string())?;
                for name in &held.dropped {
                    dropped.push((u.id.clone(), name.clone()));
                }
                if !held.failed.is_empty() {
                    let words = crate::pipeline_pack::flag_words(&held.failed);
                    *status = "failed".into();
                    *error = Some(words.clone());
                    let mut outcome = row.outcome.clone();
                    outcome["status"] = json!("failed");
                    outcome["error"] = json!(words);
                    outcome["failure"] = nils_pipeline::failure::Cause::Pipeline.doc();
                    outcome["pack_failed"] = json!(held.failed);
                    rows::unit_over(registry.store(), row.id, row.exit_code, &outcome, &now)
                        .map_err(|e| e.to_string())?;
                }
                if !held.flags.is_empty() {
                    let words = crate::pipeline_pack::flag_words(&held.flags);
                    pack_flags.push(json!({"unit": u.id, "flags": held.flags, "words": words}));
                    let id = nils_registry::review::raise_pipeline_qc(
                        registry.store(),
                        &nils_registry::review::PipelineQc {
                            run_id: x.run_id,
                            pipeline: &label,
                            unit: &u.id,
                            stack_id: u.stack_id,
                            subject_id: u.subject_id,
                            session_day: u.session_day.as_deref(),
                            status: "flag",
                            error: Some(&words),
                            metrics: &json!({"flags": held.flags, "metrics": metrics}),
                            job_id: Some(x.job_id),
                        },
                        &now,
                    )
                    .map_err(|e| e.to_string())?;
                    if !items.contains(&id) {
                        items.push(id);
                    }
                }
            }
            measured.retain(|q| !dropped.iter().any(|(u, n)| *u == q.unit_id && *n == q.name));
        }
        let measures_written =
            crate::measures::write(registry.store(), x.run_id, p.id, &p.name, &measured, &now)?;
        // record 57, 57-Q8: the cohort checks over every unit of the name
        // that measured the metric, now this run's measures are written; a
        // unit flagged after its run closed says when the flag came
        if let Some(pack) = &pack {
            let cohort: Vec<&nils_pipeline::pack::Check> =
                pack.checks.iter().filter(|c| c.cohort).collect();
            if !cohort.is_empty() {
                // every unit row of the name's recent runs, with its run
                let mut all_units: Vec<(i64, rows::Unit)> = Vec::new();
                for r in rows::runs(registry.store(), Some(p.id), 50).map_err(|e| e.to_string())? {
                    for u in rows::units(registry.store(), r.id).map_err(|e| e.to_string())? {
                        all_units.push((r.id, u));
                    }
                }
                let named: Vec<(i64, String)> = all_units
                    .iter()
                    .map(|(r, u)| (*r, u.unit.clone()))
                    .collect();
                let mut results: Vec<Vec<Value>> = vec![Vec::new(); named.len()];
                for c in &cohort {
                    for (i, v) in
                        crate::pipeline_pack::cohort_check(registry.store(), &p.name, c, &named)
                            .into_iter()
                            .enumerate()
                    {
                        results[i].push(v);
                    }
                }
                for ((run_id, u), checks) in all_units.into_iter().zip(results) {
                    if checks.iter().all(|c| c["status"] == "unchecked") {
                        continue;
                    }
                    let (row_id, unit, ck, rk) = (u.id, u.unit, u.comparability_key, u.result_key);
                    let mut record = if u.pack.is_null() { json!({}) } else { u.pack };
                    let before = record["cohort_checks"].clone();
                    let mut checks = checks;
                    for c in checks.iter_mut() {
                        if c["status"] == "flagged" {
                            let was =
                                before.as_array().into_iter().flatten().find(|b| {
                                    b["metric"] == c["metric"] && b["status"] == "flagged"
                                });
                            c["came_at"] = was
                                .and_then(|b| b["came_at"].clone().into())
                                .unwrap_or_else(|| json!(now));
                            c["after_run_closed"] = json!(run_id != x.run_id);
                            if was.is_none() {
                                pack_flags.push(json!({"unit": unit, "run": run_id, "cohort": c.clone(), "words": c["words"]}));
                            }
                        }
                    }
                    record["cohort_checks"] = json!(checks);
                    rows::set_unit_pack(
                        registry.store(),
                        row_id,
                        &record,
                        ck.as_deref(),
                        rk.as_deref(),
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }

        // the proposals on the axes it declares, through the review spine
        // (record 43 S6): grouped model items, staged at the model's threshold
        let (declared, undeclared): (Vec<Value>, Vec<Value>) =
            proposals.iter().cloned().partition(|p| {
                p["axis"]
                    .as_str()
                    .is_some_and(|a| d.proposals.iter().any(|x| x == a))
            });
        // a run speaks for its own stacks, and for the models it was given or
        // made (record 43 review)
        let ours: std::collections::BTreeSet<i64> = x.stacks.iter().copied().collect();
        let speaks_for: std::collections::BTreeSet<i64> = x
            .models
            .iter()
            .map(|m| m.id)
            .chain(models_made.iter().filter_map(|m| m["model"]["id"].as_i64()))
            .collect();
        let mut proposed = json!({
            "given": proposals.len(), "declared": declared.len(),
            "undeclared": undeclared.len(), "taken": 0,
        });
        if !declared.is_empty() {
            let taken = nils_registry::proposals::parse(&json!({ "proposals": declared }))
                .and_then(|parsed| {
                    nils_registry::proposals::ingest(
                        registry,
                        &nils_registry::proposals::Run {
                            id: x.run_id,
                            job_id: Some(x.job_id),
                            principal: x.who,
                            stacks: Some(&ours),
                            models: Some(&speaks_for),
                        },
                        &parsed,
                        x.threshold,
                    )
                });
            match taken {
                Ok(done) => {
                    proposed["taken"] = json!(done.members);
                    proposed["ingested"] = done.to_json();
                }
                Err(e) => proposed["refused"] = json!(e.to_string()),
            }
        }
        // record 55 D3: System 1's opinions on the axes it declares, kept
        // as a second opinion beside the rules' and never a decision
        if !opinions.is_empty() {
            let (declared, undeclared): (Vec<Value>, Vec<Value>) =
                opinions.iter().cloned().partition(|o| {
                    o["axis"]
                        .as_str()
                        .is_some_and(|a| d.opinions.iter().any(|x| x == a))
                });
            let mut said = json!({
                "given": opinions.len(), "declared": declared.len(),
                "undeclared": undeclared.len(), "written": 0,
            });
            let taken = nils_registry::opinion::parse(&json!({ "opinions": declared })).and_then(
                |parsed| {
                    nils_registry::opinion::ingest(
                        registry.store(),
                        &nils_registry::opinion::Run {
                            id: Some(x.run_id),
                            stacks: Some(&ours),
                            models: Some(&speaks_for),
                        },
                        &parsed,
                    )
                },
            );
            match taken {
                Ok(done) => {
                    said["written"] = json!(done.written);
                    said["ingested"] = done.to_json();
                }
                Err(e) => said["refused"] = json!(e.to_string()),
            }
            proposed["opinions"] = said;
        }
        Ok((
            run_files,
            models_made,
            seeds_doc,
            results_digest,
            items,
            breached,
            measures_written,
            proposed,
            pack_flags,
        ))
    })?;

    let count = |s: &str| statuses.iter().filter(|x| x.1 == s).count();
    // 2026-10-12: why its units failed, by cause
    let failures_by_cause = crate::failures::by_cause(
        unit_rows
            .iter()
            .filter(|u| by_name.contains_key(u.unit.as_str()))
            .map(|u| &u.outcome),
    );
    // 2026-10-12: what its skipped units lacked, in a few words, by reason
    let skipped_by_why = crate::made::skipped_by_why(
        unit_rows
            .iter()
            .filter(|u| by_name.contains_key(u.unit.as_str()))
            .map(|u| &u.outcome),
    );
    let first_log = unit_rows
        .first()
        .and_then(|u| u.outcome["log"].as_str().map(str::to_string));
    let summary = json!({
        "units": {
            "total": m.units.len(), "succeeded": count("succeeded"), "failed": count("failed"),
            "skipped": count("skipped"), "unreported": count("unreported"),
            "reused": count("reused"),
            "missing_input": missing_input,
        },
        "failures_by_cause": failures_by_cause,
        "skipped_by_why": skipped_by_why,
        "derivatives": registered,
        "bytes": bytes_total,
        "embeddings": {"registered": embedded, "kept": cached},
        "run_files": run_files,
        "models": models_made,
        "seeds": seeds_doc,
        "refused_files": refused,
        "review_items": items,
        // record 57 P4: the units the pack's checks flagged, each with the
        // check, its value and its threshold
        "flags": pack_flags,
        "proposals": proposed,
        "results": match (reports.is_empty(), unreadable) {
            (false, _) if apart => json!("results.json, one per unit"),
            (false, _) => json!("results.json"),
            (true, Some(e)) => json!(format!("unreadable: {e}")),
            (true, None) => json!("none: found by the declared templates"),
        },
        // record 49 A3: the run's tables, measures and checks
        "numbers": notes.summary(measures_written, d.checks.len()),
        "breaches": breached,
        "input_release_id": m.release_id,
        // 2026-10-12: the converter a bids input was released with
        "converter": m.converter,
        // what the container was given of the sources: one bind per folder
        // of the selection's files, or the roots past the limit, and why
        "scope": m.scope,
        "log": if apart { format!("{RUNS}/{}/units/<unit>/log.txt", x.run_id) } else { first_log.unwrap_or_else(|| format!("{RUNS}/{}/log.txt", x.run_id)) },
        // record 49: how its units ran, in which lane, what it was taken up
        // again, and the secrets it read, by id alone
        "units_mode": d.units.name(),
        "lane": x.lane.doc(),
        "needs": {"cores": d.needs.cores, "memory_gb": d.needs.memory_gb, "gpu_memory_gb": on_card.then_some(d.needs.gpu_memory_gb)},
        "resumed": x.resume.is_some(),
        "secrets": x.secrets.iter().map(|g| g.secret.id.clone()).collect::<Vec<_>>(),
    });
    if completed {
        // record 43: a run that completed with units that failed or went
        // unreported, or a file it made that was refused, is partial; the
        // units are review items
        let short = count("failed") + count("unreported") > 0 || !refused.is_empty();
        let status = if short { "partial" } else { "done" };
        Ok((status, summary, Some(results_digest), exit_code, None))
    } else {
        let log = if apart {
            format!("{RUNS}/{}/units/<unit>/log.txt", x.run_id)
        } else {
            format!("{RUNS}/{}/log.txt", x.run_id)
        };
        let mut error = format!(
            "the container exited {}; its log is {log} in the working place {}",
            exit_code.map_or("by a signal".to_string(), |c| c.to_string()),
            x.scratch.name
        );
        // 125 is podman's and docker's own failure, as when the image is
        // not in the store it looked in: say which store (wave 43's proof:
        // a scratch HOME has an empty store of its own)
        if exit_code == Some(125) && matches!(x.runtime.name(), "podman" | "docker") {
            error.push_str(&format!(
                "; 125 is {}'s own failure, as when it cannot find or pull {}: it looked in the image store {}{}",
                x.runtime.name(),
                p.image,
                x.runtime.store().as_deref().unwrap_or("it could not name"),
                std::env::var("HOME")
                    .map(|h| format!(" (HOME is {h}; a HOME of its own has a store of its own)"))
                    .unwrap_or_default()
            ));
        }
        Ok((
            "failed",
            summary,
            Some(results_digest),
            exit_code,
            Some(error),
        ))
    }
}

/// The engine process's own uid and gid, the ids podman's `--userns
/// keep-id` maps to themselves.
#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "getuid and getgid read this process and cannot fail"
)]
fn this_account() -> (u32, u32) {
    // SAFETY: neither call takes a pointer, touches memory, or can fail.
    unsafe { (libc::getuid(), libc::getgid()) }
}

/// The uid the engine runs as, whose subordinate ids rootless podman
/// unpacks an image with (2026-10-12).
#[cfg(unix)]
pub(crate) fn engine_uid() -> u32 {
    this_account().0
}

#[cfg(not(unix))]
pub(crate) fn engine_uid() -> u32 {
    0
}

/// A run's seeds, as the runner kept them: the document of its derivative
/// of kind seeds, read back from the working place and checked against the
/// digest the row holds.
fn seeds_of(store: &mut Store, run: i64) -> Result<Value, String> {
    let r = rows::run(store, run)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no pipeline run {run}"))?;
    let rows = derivative::list(
        store,
        &derivative::Filter {
            kind: Some("seeds"),
            run_id: Some(r.id),
            limit: 1,
            ..derivative::Filter::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let row = rows
        .into_iter()
        .find(|d| d.withdrawn_at.is_none())
        .ok_or_else(|| format!("run {run} suggested no seeds"))?;
    let place = nils_registry::place::show(store, row.place_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("the place of derivative {} is gone", row.id))?;
    let bytes = std::fs::read(Path::new(&place.path).join(&row.path))
        .map_err(|e| format!("the seeds of run {run}: {e}"))?;
    if hex_of(&nils_pipeline::sha256(&bytes)) != row.sha256 {
        return Err(format!(
            "the seeds file of run {run} is not the one registered (derivative {})",
            row.id
        ));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("the seeds of run {run}: {e}"))
}

/// The columns of `stack_fingerprint` a stack's header object carries
/// (record 50), each with whether it is text, an integer or a number.
const HEADER_FINGERPRINT: [(&str, char); 22] = [
    ("manufacturer", 't'),
    ("modality", 't'),
    ("orientation", 't'),
    ("fov_x", 'f'),
    ("fov_y", 'f'),
    ("pixel_spacing_row", 'f'),
    ("pixel_spacing_col", 'f'),
    ("rows", 'i'),
    ("columns", 'i'),
    ("n_slices", 'i'),
    ("slice_span_mm", 'f'),
    ("slice_thickness", 'f'),
    ("spacing_between_slices", 'f'),
    ("aspect_ratio", 'f'),
    ("field_strength_normalized", 'f'),
    ("magnetic_field_strength", 'f'),
    ("receive_coil_name", 't'),
    ("text_series_description_ci", 't'),
    ("text_protocol_name_ci", 't'),
    ("text_sequence_name_ci", 't'),
    ("text_series_comments_ci", 't'),
    ("text_body_part_ci", 't'),
];

/// A stack's file as the stacks queries read it: its source's id and root,
/// its path, the frames of a multi-frame file that are the stack's and
/// how many, the file's geometry where it was asked for, and its identity
/// where the slice cache was (record 55 E5).
type StackFile = (
    i64,
    String,
    String,
    Option<(String, i64)>,
    Option<Value>,
    Option<FileIdentity>,
);

/// A file's identity for the slice cache (record 55 E5): its SOP Instance
/// UID, the size and time `source_file` holds, and the sha256 of its bytes
/// where a run learnt it while the file had that size and time.
#[derive(Debug, Clone, PartialEq)]
struct FileIdentity {
    sop_instance_uid: String,
    size: i64,
    mtime_ns: i64,
    sha256: Option<String>,
}

/// The files of the stacks `list` names (a few hundred ids, comma
/// separated) into `files_of`, in the order a manifest lists them: a
/// multi-frame file's frames first, then the files whose every frame is the
/// stack's, each by path, with its [`file_geometry`] where `geometry` asks
/// and its [`FileIdentity`] where `identity` does.
fn stack_files(
    store: &mut Store,
    list: &str,
    geometry: bool,
    identity: bool,
    files_of: &mut BTreeMap<i64, Vec<StackFile>>,
) -> Result<(), String> {
    let err = |e: nils_registry::Error| e.to_string();
    // the instance's columns a file's geometry is made of, after the ones
    // each query reads anyway, then the file's identity
    let geo_columns = if geometry {
        FILE_GEOMETRY
            .iter()
            .map(|c| format!(", i.{c}"))
            .collect::<String>()
    } else {
        String::new()
    };
    let (id_columns, id_join) = if identity {
        (
            ", i.sop_instance_uid, f.size, f.mtime_ns, fc.sha256".to_string(),
            nils_registry::content::join(store, "f", "fc"),
        )
    } else {
        (String::new(), String::new())
    };
    let identity_at = |r: &nils_registry::store::Row,
                       at: usize|
     -> Result<Option<FileIdentity>, nils_registry::Error> {
        if !identity {
            return Ok(None);
        }
        Ok(Some(FileIdentity {
            sop_instance_uid: r.text(at)?.to_string(),
            size: r.int(at + 1)?,
            mtime_ns: r.int(at + 2)?,
            sha256: r.opt_text(at + 3)?.map(str::to_string),
        }))
    };
    let geo_n = if geometry { FILE_GEOMETRY.len() } else { 0 };
    let framed = format!(
        "SELECT fr.stack_id, so.id, so.root, f.path, fr.frames, fr.n_frames{geo_columns}{id_columns} FROM {} fr \
         JOIN {} i ON i.id = fr.instance_id JOIN {} f ON f.id = i.source_file_id \
         JOIN {} so ON so.id = f.source_id {id_join} WHERE fr.stack_id IN ({list}) \
         ORDER BY fr.stack_id, f.path",
        store.qualified("instance_frame"),
        store.qualified("instance"),
        store.qualified("source_file"),
        store.qualified("source"),
    );
    for r in store.query(&framed, &[]).map_err(err)? {
        let geo = if geometry {
            Some(file_geometry(&r, 6).map_err(err)?)
        } else {
            None
        };
        files_of.entry(r.int(0).map_err(err)?).or_default().push((
            r.int(1).map_err(err)?,
            r.text(2).map_err(err)?.to_string(),
            r.text(3).map_err(err)?.to_string(),
            Some((r.text(4).map_err(err)?.to_string(), r.int(5).map_err(err)?)),
            geo,
            identity_at(&r, 6 + geo_n).map_err(err)?,
        ));
    }
    let whole = format!(
        "SELECT i.stack_id, so.id, so.root, f.path{geo_columns}{id_columns} FROM {} i \
         JOIN {} f ON f.id = i.source_file_id JOIN {} so ON so.id = f.source_id {id_join} \
         WHERE i.stack_id IN ({list}) ORDER BY i.stack_id, f.path",
        store.qualified("instance"),
        store.qualified("source_file"),
        store.qualified("source"),
    );
    for r in store.query(&whole, &[]).map_err(err)? {
        let geo = if geometry {
            Some(file_geometry(&r, 4).map_err(err)?)
        } else {
            None
        };
        files_of.entry(r.int(0).map_err(err)?).or_default().push((
            r.int(1).map_err(err)?,
            r.text(2).map_err(err)?.to_string(),
            r.text(3).map_err(err)?.to_string(),
            None,
            geo,
            identity_at(&r, 4 + geo_n).map_err(err)?,
        ));
    }
    Ok(())
}

/// A stack's [`stack_geometry`] and the facts the few-slice student reads
/// beside it (record 55 E2, P1b), from the stacks query's row at `at`:
/// the orientation, thickness and spacing, then the session it is part of,
/// what its images are and how many there are; `sealed` is false until the
/// caller marks it.
fn stack_facts(r: &nils_registry::store::Row, at: usize) -> Result<Value, nils_registry::Error> {
    let mut g = stack_geometry(
        r.opt_text(at)?,
        r.opt_double(at + 1)?,
        r.opt_double(at + 2)?,
    );
    g["study"] = json!(r.int(at + 3)?);
    g["image_type"] = json!(r.opt_text(at + 4)?);
    g["sop_class_uid"] = json!(r.opt_text(at + 5)?);
    g["n_instances"] = json!(r.opt_int(at + 6)?);
    g["sealed"] = json!(false);
    Ok(g)
}

/// The rest of each session the run holds part of (record 55 E2, P1b),
/// for `stacks.json`'s `context`: the few-slice student reads a stack's
/// height against its session's top, the highest box among the study's
/// unsealed MR stacks, and a run may hold part of a study (a `--chunk`
/// page, a selection that is not whole studies). So each other stack of
/// the `studies` that is MR (its own modality, else its fingerprint's) and
/// not sealed now is listed with its modality, its geometry and its files'
/// geometry, in the order and shape the run's own stacks have, and no path:
/// a pipeline computes the top from them and never scores them.
fn session_context(
    store: &mut Store,
    run: &[i64],
    studies: &std::collections::BTreeSet<i64>,
) -> Result<Vec<Value>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let own: std::collections::BTreeSet<i64> = run.iter().copied().collect();
    let studies: Vec<i64> = studies.iter().copied().collect();
    let mut modality: BTreeMap<i64, String> = BTreeMap::new();
    for chunk in studies.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        // the image's modality: the stack's own, else its fingerprint's
        let sql = format!(
            "SELECT st.id, COALESCE(NULLIF(st.modality, ''), fp.modality) FROM {} st \
             JOIN {} se ON se.id = st.series_id LEFT JOIN {} fp ON fp.stack_id = st.id \
             WHERE se.study_id IN ({list}) \
             AND UPPER(COALESCE(NULLIF(st.modality, ''), fp.modality, '')) = 'MR' ORDER BY st.id",
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("stack_fingerprint"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let id = r.int(0).map_err(err)?;
            if !own.contains(&id) {
                modality.insert(
                    id,
                    r.opt_text(1).map_err(err)?.unwrap_or_default().to_string(),
                );
            }
        }
    }
    let ids: Vec<i64> = modality.keys().copied().collect();
    let (sealed, _) =
        nils_registry::labels::sealed_now(store, &ids, &[]).map_err(|e| e.to_string())?;
    let ids: Vec<i64> = ids.into_iter().filter(|id| !sealed.contains(id)).collect();
    let mut geometry_of: BTreeMap<i64, Value> = BTreeMap::new();
    let mut files_of: BTreeMap<i64, Vec<StackFile>> = BTreeMap::new();
    for chunk in ids.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT st.id, st.image_orientation_patient, fp.slice_thickness, fp.spacing_between_slices, \
             se.study_id, st.image_type, fp.sop_class_uid, st.n_instances FROM {} st \
             JOIN {} se ON se.id = st.series_id LEFT JOIN {} fp ON fp.stack_id = st.id \
             WHERE st.id IN ({list})",
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("stack_fingerprint"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            geometry_of.insert(r.int(0).map_err(err)?, stack_facts(&r, 1).map_err(err)?);
        }
        stack_files(store, &list, true, false, &mut files_of)?;
    }
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let mut seen: std::collections::BTreeSet<(i64, String)> = Default::default();
        let files: Vec<Value> = files_of
            .remove(&id)
            .unwrap_or_default()
            .into_iter()
            .filter(|(so, _, path, _, _, _)| seen.insert((*so, path.clone())))
            .map(|(_, _, _, _, geo, _)| json!({"geometry": geo.unwrap_or(Value::Null)}))
            .collect();
        out.push(json!({
            "stack_id": id,
            "modality": modality.remove(&id),
            "geometry": geometry_of.remove(&id).unwrap_or(Value::Null),
            "files": files,
        }));
    }
    Ok(out)
}

/// The columns of `instance` a file's geometry object carries (record 55
/// E2), in the order the stacks queries read them: where its image sits,
/// its spacing, its matrix, its number and how many frames it holds.
const FILE_GEOMETRY: [&str; 6] = [
    "image_position_patient",
    "pixel_spacing",
    "rows",
    "columns",
    "instance_number",
    "number_of_frames",
];

/// The numbers of a value DICOM writes as `a\b\c`, as a JSON list when
/// there are exactly `n` and each is finite; null otherwise, so a pipeline
/// never meets half a position.
fn dicom_numbers(text: Option<&str>, n: usize) -> Value {
    let Some(text) = text else {
        return Value::Null;
    };
    let numbers: Option<Vec<f64>> = text
        .split('\\')
        .map(|v| v.trim().parse::<f64>().ok().filter(|x| x.is_finite()))
        .collect();
    match numbers {
        Some(v) if v.len() == n => json!(v),
        _ => Value::Null,
    }
}

/// A file's geometry object for `stacks.json` (record 55 E2), from the
/// instance's [`FILE_GEOMETRY`] columns starting at `at` in the row: as the
/// registry read them from the file's header, each null where it holds
/// none. A multi-frame file's position is its first frame's, so a pipeline
/// reads the per-frame positions from the file itself.
fn file_geometry(r: &nils_registry::store::Row, at: usize) -> Result<Value, nils_registry::Error> {
    Ok(json!({
        "image_position_patient": dicom_numbers(r.opt_text(at)?, 3),
        "pixel_spacing": dicom_numbers(r.opt_text(at + 1)?, 2),
        "rows": r.opt_int(at + 2)?,
        "columns": r.opt_int(at + 3)?,
        "instance_number": r.opt_int(at + 4)?,
        "number_of_frames": r.opt_int(at + 5)?,
    }))
}

/// A stack's geometry object for `stacks.json` (record 55 E2): the
/// orientation of its images (the stack's own, six numbers) and the slice
/// thickness and spacing of its fingerprint row, each null where the
/// registry holds none (a stack not fingerprinted has neither). The stacks
/// query adds the stack's facts beside it (P1b): `study`, the series'
/// study to group a session by, `image_type`, `sop_class_uid` (the
/// fingerprint's), `n_instances` and whether it is `sealed` now.
fn stack_geometry(
    orientation: Option<&str>,
    thickness: Option<f64>,
    spacing: Option<f64>,
) -> Value {
    json!({
        "image_orientation_patient": dicom_numbers(orientation, 6),
        "slice_thickness": thickness.filter(|x| x.is_finite()),
        "spacing_between_slices": spacing.filter(|x| x.is_finite()),
    })
}

/// Each stack's header object for `stacks.json` (record 50): the named
/// columns of its fingerprint row (null without one), every classification
/// row by axis, the name of its first ingest batch, and the cohorts its
/// subject is an open member of. `stacks` maps each stack to its subject.
/// The registry is read a few hundred stacks at a time.
pub(crate) fn stack_headers(
    store: &mut Store,
    stacks: &BTreeMap<i64, i64>,
) -> Result<BTreeMap<i64, Value>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let ids: Vec<i64> = stacks.keys().copied().collect();
    let mut fingerprint: BTreeMap<i64, Value> = BTreeMap::new();
    let mut classification: BTreeMap<i64, serde_json::Map<String, Value>> = BTreeMap::new();
    let mut batch: BTreeMap<i64, String> = BTreeMap::new();
    let mut cohorts: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    let columns = HEADER_FINGERPRINT
        .iter()
        .map(|(c, _)| format!("f.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    for chunk in ids.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT f.stack_id, {columns} FROM {} f WHERE f.stack_id IN ({list})",
            store.qualified("stack_fingerprint"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let mut row = serde_json::Map::new();
            for (i, (name, ty)) in HEADER_FINGERPRINT.iter().enumerate() {
                let at = i + 1;
                let v = match ty {
                    't' => json!(r.opt_text(at).map_err(err)?),
                    'i' => json!(r.opt_int(at).map_err(err)?),
                    _ => json!(r.opt_double(at).map_err(err)?),
                };
                row.insert((*name).to_string(), v);
            }
            fingerprint.insert(r.int(0).map_err(err)?, Value::Object(row));
        }
        let sql = format!(
            "SELECT stack_id, axis, value, confidence, tier FROM {} \
             WHERE stack_id IN ({list}) ORDER BY stack_id, axis, id",
            store.qualified("classification_axis"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let axis = r.text(1).map_err(err)?.to_string();
            let row = json!({
                "value": r.opt_text(2).map_err(err)?,
                "confidence": r.double(3).map_err(err)?,
                "tier": r.text(4).map_err(err)?,
            });
            let rows = classification
                .entry(r.int(0).map_err(err)?)
                .or_default()
                .entry(axis)
                .or_insert_with(|| json!([]));
            if let Some(a) = rows.as_array_mut() {
                a.push(row);
            }
        }
        let sql = format!(
            "SELECT st.id, b.name FROM {} st JOIN {} b ON b.id = st.first_batch_id \
             WHERE st.id IN ({list})",
            store.qualified("stack"),
            store.qualified("ingest_batch"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            batch.insert(r.int(0).map_err(err)?, r.text(1).map_err(err)?.to_string());
        }
    }
    let mut subjects: Vec<i64> = stacks.values().copied().collect();
    subjects.sort_unstable();
    subjects.dedup();
    let mut open: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for chunk in subjects.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT m.subject_id, c.name FROM {} m JOIN {} c ON c.id = m.cohort_id \
             WHERE m.subject_id IN ({list}) AND m.left_at IS NULL",
            store.qualified("cohort_member"),
            store.qualified("cohort"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            open.entry(r.int(0).map_err(err)?)
                .or_default()
                .push(r.text(1).map_err(err)?.to_string());
        }
    }
    for names in open.values_mut() {
        names.sort();
        names.dedup();
    }
    for (stack, subject) in stacks {
        cohorts.insert(*stack, open.get(subject).cloned().unwrap_or_default());
    }
    Ok(ids
        .iter()
        .map(|id| {
            (
                *id,
                json!({
                    "fingerprint": fingerprint.remove(id).unwrap_or(Value::Null),
                    "classification": Value::Object(classification.remove(id).unwrap_or_default()),
                    "batch": batch.remove(id),
                    "cohorts": cohorts.remove(id).unwrap_or_default(),
                }),
            )
        })
        .collect())
}

/// Where the artifact of a model a run fitted lies under the working place:
/// the newest live derivative of kind model naming it there.
fn model_artifact(
    store: &mut Store,
    model_id: i64,
    place_id: i64,
) -> Result<Option<String>, String> {
    let d = store.dialect();
    let sql = format!(
        "SELECT path FROM {} WHERE kind = 'model' AND model_id = {} AND place_id = {} \
         AND withdrawn_at IS NULL ORDER BY id DESC",
        store.qualified("derivative"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    store
        .query_opt(&sql, &[Param::Int(model_id), Param::Int(place_id)])
        .map_err(|e| e.to_string())?
        .map(|r| r.text(0).map(str::to_string))
        .transpose()
        .map_err(|e| e.to_string())
}

/// The folder, relative to its source root, a file is bound through: its
/// own, or where that path holds a ':' or a ',', which the runtimes' mount
/// syntax splits on, its nearest ancestor that holds neither; the root
/// itself, `""`, for a file directly in it. None when the root's own path
/// holds one.
fn bind_folder(root: &str, path: &str) -> Option<String> {
    let bad = |p: &str| p.contains(':') || p.contains(',');
    if bad(root) {
        return None;
    }
    let mut dir = Path::new(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    loop {
        let text = dir.to_string_lossy().into_owned();
        if !bad(&text) {
            return Some(text);
        }
        dir = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    }
}

/// The live derivatives of the working place each derivative input of a
/// descriptor takes, over the selection's stacks, read a few hundred
/// stacks at a time; and those of the selection's sessions and subjects,
/// which a run of the bids layout registers, so one such run takes what
/// an earlier one made (SynthSeg's label map for segcsvd).
fn derivative_inputs(
    store: &mut Store,
    d: &Descriptor,
    stacks: &[i64],
    places: &[&nils_registry::place::Place],
    bound: &Bound,
) -> Result<BTreeMap<String, Vec<derivative::Derivative>>, String> {
    let place_ids: Vec<i64> = places.iter().map(|pl| pl.id).collect();
    let mut out = derivative_candidates(store, d, stacks, &place_ids)?.rows;
    // record 55 H3, section 6.5: what the input takes of a check's
    // status; a run is never given a rejected result unless its
    // descriptor asks for any
    for t in d.inputs.iter().filter(|t| t.ty.starts_with("derivative:")) {
        let checked = t.checked.as_deref().unwrap_or("not_rejected");
        if let Some(rows) = out.get_mut(&t.id) {
            rows.retain(|r| takes_checked(checked, r.check_status.as_deref()));
            // after review (finding 3 of the second review): an input a
            // link feeds takes the fed run's derivatives and no other's
            if let Some(fed) = bound.get(&t.id) {
                rows.retain(|r| fed.takes(r));
            }
        }
    }
    Ok(out)
}

/// The derivatives each derivative input of a descriptor could take, by
/// the input's id, before their checks are read, and the session of each
/// stack they were read for.
struct Candidates {
    rows: BTreeMap<String, Vec<derivative::Derivative>>,
    sessions: BTreeMap<i64, (i64, String)>,
}

/// What [`derivative_inputs`] reads, its check left to the caller: the
/// derivatives of the stacks, and those of the stacks' sessions and
/// subjects, in these places.
fn derivative_candidates(
    store: &mut Store,
    d: &Descriptor,
    stacks: &[i64],
    place_ids: &[i64],
) -> Result<Candidates, String> {
    let mut rows: BTreeMap<String, Vec<derivative::Derivative>> = BTreeMap::new();
    let wanted = d.inputs.iter().any(|t| t.ty.starts_with("derivative:"));
    let of_stack = if wanted {
        stack_sessions(store, stacks)?
    } else {
        Default::default()
    };
    let sessions: std::collections::BTreeSet<(i64, String)> = of_stack.values().cloned().collect();
    let subjects: Vec<i64> = sessions
        .iter()
        .map(|(s, _)| *s)
        .collect::<std::collections::BTreeSet<i64>>()
        .into_iter()
        .collect();
    // one cache of the producing pipelines' outputs for every input
    let mut typed = crate::typed_outputs::Outputs::default();
    for t in d.inputs.iter().filter(|t| t.ty.starts_with("derivative:")) {
        let kind = t.ty.trim_start_matches("derivative:");
        let mut found = Vec::new();
        for &place in place_ids {
            found.extend(
                derivative::of_stacks(store, kind, stacks, place).map_err(|e| e.to_string())?,
            );
            for r in
                derivative::of_subjects(store, kind, &subjects, place).map_err(|e| e.to_string())?
            {
                let ours = match (r.scope.as_str(), r.subject_id, &r.session_day) {
                    ("subject", Some(_), _) => true,
                    ("session", Some(s), Some(day)) => sessions.contains(&(s, day.clone())),
                    _ => false,
                };
                if ours {
                    found.push(r);
                }
            }
        }
        // Wave 7b section 8.2: an input that asks for a role, a contrast or
        // a grid takes only a derivative whose typed output says the same
        if t.role.is_some() || t.contrast.is_some() || t.grid.is_some() {
            found.retain(|r| typed.takes(store, t, r));
        }
        rows.insert(t.id.clone(), found);
    }
    Ok(Candidates {
        rows,
        sessions: of_stack,
    })
}

/// Whether a derivative input that takes `checked` (accepted, not_rejected
/// or any) takes a result whose check left it so (record 55 H3, section
/// 6.5). An unchecked result is taken by not_rejected and any.
pub(crate) fn takes_checked(checked: &str, status: Option<&str>) -> bool {
    match checked {
        "any" => true,
        "accepted" => status == Some("accepted"),
        _ => status != Some("rejected"),
    }
}

/// Record 55 H3, section 6.5, found by the T33 pipe test: why each unit,
/// given by its stacks, lacks a derivative an input that is not optional
/// needs, in the order the units are given (nothing for a unit that lacks
/// none). A derivative is the unit's when it is of one of its stacks, or
/// of the subject or the session of one of them, as the run's inputs read
/// them; the unit lacks the input when none is registered, or when its
/// check leaves out every one there is, so the run's inputs hold none for
/// it. The pre-flight and the run judge the units by it, so both agree
/// with the inputs: a unit whose only mask was rejected is not ready, and
/// is not run.
pub(crate) fn derivatives_missing(
    store: &mut Store,
    d: &Descriptor,
    units: &[&[i64]],
    place_ids: &[i64],
    bound: &Bound,
) -> Result<Vec<Vec<String>>, String> {
    let mut out: Vec<Vec<String>> = vec![Vec::new(); units.len()];
    let needed: Vec<&descriptor::TypedInput> = d
        .inputs
        .iter()
        .filter(|t| t.ty.starts_with("derivative:") && !t.optional)
        .collect();
    if needed.is_empty() || units.is_empty() {
        return Ok(out);
    }
    let stacks: Vec<i64> = units
        .iter()
        .flat_map(|u| u.iter().copied())
        .collect::<std::collections::BTreeSet<i64>>()
        .into_iter()
        .collect();
    let found = derivative_candidates(store, d, &stacks, place_ids)?;
    for t in needed {
        let kind = t.ty.trim_start_matches("derivative:");
        let checked = t.checked.as_deref().unwrap_or("not_rejected");
        let rows = found.rows.get(&t.id).map(Vec::as_slice).unwrap_or_default();
        for (u, why) in units.iter().zip(out.iter_mut()) {
            let sessions: Vec<&(i64, String)> =
                u.iter().filter_map(|s| found.sessions.get(s)).collect();
            let own: Vec<&derivative::Derivative> = rows
                .iter()
                .filter(|r| match (r.stack_id, r.scope.as_str(), r.subject_id) {
                    (Some(s), ..) => u.contains(&s),
                    (None, "subject", Some(subject)) => sessions.iter().any(|(s, _)| *s == subject),
                    (None, "session", Some(subject)) => r
                        .session_day
                        .as_deref()
                        .is_some_and(|day| sessions.iter().any(|(s, d)| *s == subject && d == day)),
                    _ => false,
                })
                .collect();
            // after review (finding 3 of the second review): an input a
            // link feeds is bound to the fed run; a unit that run made no
            // file of is skipped with the link's words. At the plan (after
            // the third review, finding 4) the binding has no run yet: the
            // input is fed by the step, not missing
            if let Some(fed) = bound.get(&t.id) {
                if fed.is_planned() {
                    continue;
                }
                let made: Vec<&&derivative::Derivative> =
                    own.iter().filter(|r| fed.takes(r)).collect();
                if made
                    .iter()
                    .any(|r| takes_checked(checked, r.check_status.as_deref()))
                {
                    continue;
                }
                why.push(format!(
                    "{}: {} made no {kind} derivative of it for the input {}{}",
                    fed.link.words(&d.name),
                    fed.words(),
                    t.id,
                    if made.is_empty() {
                        String::new()
                    } else {
                        " that its check takes".to_string()
                    }
                ));
                continue;
            }
            if own
                .iter()
                .any(|r| takes_checked(checked, r.check_status.as_deref()))
            {
                continue;
            }
            why.push(derivative_lack(own.len(), kind, &t.id, checked));
        }
    }
    Ok(out)
}

/// Why a unit lacks a derivative an input needs, given how many of the
/// kind it has, none of which the input's check takes: the words the
/// pre-flight and the run say, which [`crate::preflight::brief`] shortens.
pub(crate) fn derivative_lack(found: usize, kind: &str, input: &str, checked: &str) -> String {
    match (found, checked) {
        (0, _) => format!("no {kind} derivative of it for the input {input}"),
        (_, "accepted") => format!(
            "no {kind} derivative of it for the input {input} is accepted by a check, and the input takes accepted ones only"
        ),
        (1, _) => format!(
            "its {kind} derivative for the input {input} was rejected by a check, and the input takes no rejected one"
        ),
        (n, _) => format!(
            "its {n} {kind} derivatives for the input {input} were all rejected by a check, and the input takes no rejected one"
        ),
    }
}

/// The sessions of these stacks, each as its subject and its day, the day
/// a run of the bids layout names a session's derivatives by, by stack.
pub(crate) fn stack_sessions(
    store: &mut Store,
    stacks: &[i64],
) -> Result<BTreeMap<i64, (i64, String)>, String> {
    let err = |e: nils_registry::Error| e.to_string();
    let labels = nils_session::labels_by_study(store, &nils_registry::session::Scheme::default())
        .unwrap_or_default();
    let mut out = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT st.id, se.subject_id, se.study_id FROM {} st JOIN {} se ON se.id = st.series_id \
             WHERE st.id IN ({list})",
            store.qualified("stack"),
            store.qualified("series"),
        );
        for r in store.query(&sql, &[]).map_err(err)? {
            let (Some(subject), Some(study)) =
                (r.opt_int(1).map_err(err)?, r.opt_int(2).map_err(err)?)
            else {
                continue;
            };
            if let Some(l) = labels.get(&study) {
                out.insert(r.int(0).map_err(err)?, (subject, l.first.to_string()));
            }
        }
    }
    Ok(out)
}

/// The working places a run reads derivatives and models from: its output
/// place first, then its scratch place where that is another, which is
/// where a lane that had no output place of its own kept them (record 49
/// R7).
fn lane_places<'a>(x: &Execution<'a>) -> Vec<&'a nils_registry::place::Place> {
    if x.scratch.id == x.output.id {
        vec![x.output]
    } else {
        vec![x.output, x.scratch]
    }
}

/// Link one derivative into a run's input folder at its path under the
/// derivatives tree, by a hard link (a copy where the two are not on one
/// filesystem). The file is taken only where it resolves inside the
/// working place's derivatives tree. Answers the path under the input.
/// What made each derivative a run takes (record 57 P11), for the
/// manifest, so a pipeline can say which result it read: the run, its
/// pipeline's name and version, the parameters it ran with, and the scan
/// the derivative's grid is on with the file name the release that run
/// read gave it (`sub-a_ses-1_acq-Sag+3D+MPRAGE_T1w`, its contrast the
/// suffix and the `acq-` tokens). Each run and release is read once.
#[derive(Default)]
struct MadeBy {
    runs: BTreeMap<i64, Option<(Value, Option<i64>)>>,
    stems: BTreeMap<i64, BTreeMap<i64, Option<String>>>,
}

impl MadeBy {
    /// Null for a derivative no run made, or a run the registry no longer
    /// holds; a grid not read yet, or a scan no release named, is null.
    fn of(&mut self, store: &mut Store, row: &derivative::Derivative) -> Value {
        let Some(run_id) = row.run_id else {
            return Value::Null;
        };
        let made = self
            .runs
            .entry(run_id)
            .or_insert_with(|| {
                let run = rows::run(store, run_id).ok().flatten()?;
                let p = rows::get(store, run.pipeline_id).ok().flatten()?;
                Some((
                    json!({
                        "run": run_id, "pipeline": p.name, "version": p.version,
                        "params": run.params,
                    }),
                    run.input_release_id,
                ))
            })
            .clone();
        let Some((mut doc, release)) = made else {
            return Value::Null;
        };
        let stem = match (release, row.grid_stack_id) {
            (Some(release), Some(stack)) => self
                .stems
                .entry(release)
                .or_insert_with(|| {
                    released_stacks(store, release)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(s, _, stem)| (s, stem))
                        .collect()
                })
                .get(&stack)
                .cloned()
                .flatten(),
            _ => None,
        };
        doc["grid_stack_id"] = json!(row.grid_stack_id);
        doc["grid_stem"] = json!(stem);
        doc
    }
}

fn link_input(working: &Path, path: &str, into: &Path) -> Result<String, String> {
    let rel = path
        .strip_prefix(&format!("{}/", derivative::TREE))
        .ok_or_else(|| format!("{path} is not under the derivatives tree"))?;
    let tree = std::fs::canonicalize(working.join(derivative::TREE))
        .map_err(|e| format!("the derivatives tree: {e}"))?;
    let real = std::fs::canonicalize(working.join(path)).map_err(|e| format!("{path}: {e}"))?;
    if !real.starts_with(&tree) || !real.is_file() {
        return Err(format!("{path} is not a file of the derivatives tree"));
    }
    if Path::new(rel)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!("{path} is not a plain path"));
    }
    let at = into.join(rel);
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("the run's inputs: {e}"))?;
    }
    match std::fs::hard_link(&real, &at) {
        Ok(()) => {}
        // linked already, by an earlier row of the same file: nothing to do;
        // anything else in its place is refused, never copied over
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if !same_file(&real, &at) {
                return Err(format!(
                    "{path}: the run's inputs hold another file at {rel} already"
                ));
            }
        }
        // only across filesystems is it copied, onto nothing
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let bytes = std::fs::read(&real).map_err(|e| format!("{path}: {e}"))?;
            nils_pipeline::files::write_new(&at, &bytes).map_err(|e| format!("{path}: {e}"))?;
        }
        Err(e) => return Err(format!("{path}: {e}")),
    }
    Ok(rel.to_string())
}

/// A file's path under the working place, from where it really is: the row
/// never names a link (record 43 review).
fn place_path(working: &Path, file: &Path) -> Result<String, String> {
    let root = std::fs::canonicalize(working).map_err(|e| format!("the working place: {e}"))?;
    let real = std::fs::canonicalize(file).map_err(|e| format!("{}: {e}", file.display()))?;
    real.strip_prefix(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|_| format!("{} is outside the working place", file.display()))
}

/// Whether two paths are one file on disk.
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::symlink_metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok()
    }
}

/// `sha256:<hex>` to its hex, as the derivative rows keep a digest.
fn hex_of(digest: &str) -> String {
    digest.strip_prefix("sha256:").unwrap_or(digest).to_string()
}

/// Register an embedding a run wrote under the cache's key (record 43 S4):
/// its header names the stack, the encoder and the preprocessing; the
/// encoder is the registry's, or registered from the card the results
/// carry for it. Answers what the cache did, or why the file is refused.
#[allow(clippy::too_many_arguments)]
fn register_embedding(
    registry: &mut Registry,
    x: &Execution<'_>,
    u: &Unit,
    declared: &[String],
    file: &Path,
    path: &str,
    bytes: u64,
    sha: &str,
    cards: &[Value],
    now: &str,
) -> Result<nils_registry::embedding::Registered, String> {
    use nils_registry::embedding;
    let data = std::fs::read(file).map_err(|e| e.to_string())?;
    // read whole, every value finite, before anything is registered
    let h = embedding::decode(&data)?.header;
    if u.stack_id != Some(h.stack_id) {
        return Err(format!(
            "the embedding names stack {}, and the unit is {}",
            h.stack_id, u.id
        ));
    }
    // only an encoder the run was given, or its descriptor declares
    let given = x.models.iter().any(|m| m.digest == h.encoder);
    if !given && !declared.contains(&h.encoder) {
        return Err(format!(
            "the encoder {} is one the run was neither given nor declares for this output",
            h.encoder
        ));
    }
    let encoder = match nils_registry::model::by_digest(registry.store(), &h.encoder)
        .map_err(|e| e.to_string())?
    {
        Some(m) => m,
        None => {
            let card = cards
                .iter()
                .find(|c| c["digest"] == h.encoder.as_str() && c["kind"] == "encoder")
                .ok_or_else(|| {
                    format!(
                        "the encoder {} is not registered, and the results carry no card for it",
                        h.encoder
                    )
                })?;
            let text = |k: &str| card[k].as_str().filter(|s| !s.is_empty());
            embedding::register_encoder(
                registry,
                &embedding::Encoder {
                    name: text("name").ok_or("the encoder's card names no name")?,
                    version: text("version").ok_or("the encoder's card names no version")?,
                    weights_digest: &h.encoder,
                    image_digest: text("image_digest"),
                },
                x.who,
            )
            .map_err(|e| e.to_string())?
        }
    };
    embedding::register(
        registry,
        &embedding::New {
            stack_id: h.stack_id,
            encoder_id: encoder.id,
            preprocess_version: &h.preprocess_version,
            place_id: x.output.id,
            path,
            bytes: bytes as i64,
            sha256: sha,
            registered_by: x.who,
            actor: Some(x.actor),
            run_id: Some(x.run_id),
            created_at: now,
        },
    )
    .map_err(|e| e.to_string())
}

/// Register the model a run fitted (record 43): the artifact hashed by the
/// engine, the card beside it naming that digest, trained on the label set
/// the run was given, its encoders the card's, in state registered. The
/// artifact becomes the run's derivative of kind model, which a later run
/// that reads the model mounts. Answers the model, the derivative's id and
/// the artifact's digest.
fn register_model(
    registry: &mut Registry,
    x: &Execution<'_>,
    o: &descriptor::Output,
    out: &Path,
    rel: &str,
    now: &str,
) -> Result<(nils_registry::model::Model, i64, String), String> {
    let file = nils_pipeline::files::inside(out, rel)?;
    let (bytes, sha) =
        nils_pipeline::files::sha256_file(&file).map_err(|e| format!("{rel}: {e}"))?;
    let card_rel = o.card.as_deref().ok_or("a model output names its card")?;
    let card_file = nils_pipeline::files::inside(out, card_rel)?;
    let text =
        std::fs::read_to_string(&card_file).map_err(|e| format!("the card {card_rel}: {e}"))?;
    let mut card: Value =
        serde_json::from_str(&text).map_err(|e| format!("the card {card_rel}: {e}"))?;
    let digest = format!("sha256:{sha}");
    if card["digest"].as_str() != Some(digest.as_str()) {
        return Err(format!(
            "the card names the digest {}, and the artifact is {digest}",
            card["digest"]
        ));
    }
    // trained on the label set it was given, and no other
    let bare = |d: &str| d.strip_prefix("sha256:").unwrap_or(d).to_string();
    match (x.label_set, card["trained_on"]["label_set"].as_str()) {
        (Some(set), Some(named)) if bare(named) != bare(&set.digest) => {
            return Err(format!(
                "the card says it was trained on {named}, and the run gave it label set {} ({})",
                set.id, set.digest
            ));
        }
        (Some(set), _) => {
            if !card["trained_on"].is_object() {
                card["trained_on"] = json!({});
            }
            card["trained_on"]["label_set"] = json!(format!("sha256:{}", bare(&set.digest)));
            card["trained_on"]["name"] = json!(set.name);
            card["trained_on"]["rows"] = json!(set.rows);
        }
        (None, Some(named)) => {
            return Err(format!(
                "the card says it was trained on {named}, and the run was given no label set"
            ));
        }
        (None, None) => {}
    }
    let model =
        nils_registry::model::register(registry, &card, x.who).map_err(|e| e.to_string())?;
    nils_registry::model::set_job(registry.store(), model.id, x.job_id)
        .map_err(|e| e.to_string())?;
    let media = nils_pipeline::files::media_type(rel, o.media_type.as_deref());
    let id = derivative::insert_of_run(
        registry.store(),
        &derivative::New {
            kind: "model",
            belongs: &Belongs::run(),
            place_id: x.output.id,
            path: &place_path(Path::new(&x.output.path), &file)?,
            bytes: bytes as i64,
            sha256: &sha,
            media_type: &media,
            registered_by: x.who,
            actor: Some(x.actor),
            model_id: Some(model.id),
            run_id: None,
            preprocess_version: None,
            supersedes_id: None,
            created_at: now,
        },
        x.run_id,
    )
    .map_err(|e| e.to_string())?;
    Ok((model, id, sha))
}

/// A unit's files as the declared templates find them.
fn found_for(outputs: &[descriptor::Output], out: &Path, u: &Unit) -> Vec<String> {
    let vars = u.vars();
    let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut files: Vec<String> = outputs
        .iter()
        .flat_map(|o| nils_pipeline::files::found(out, &o.template, &pairs))
        .collect();
    files.sort();
    files.dedup();
    files
}

#[cfg(test)]
mod tests {
    /// Wave 7b review: every `api/pipelines` and `api/pipeline-runs` route
    /// this file serves is in the doors list, which the desk reads to
    /// decide what to show.
    #[test]
    fn every_route_of_the_pipelines_table_is_in_the_doors_list() {
        let doors = crate::serve::doors_list();
        let mut missing = Vec::new();
        let mut seen = 0;
        for line in include_str!("pipelines.rs").lines() {
            let t = line.trim_start();
            let (method, rest) = if let Some(r) = t.strip_prefix("(\"") {
                let Some((m, r)) = r.split_once("\", [") else {
                    continue;
                };
                (m.to_string(), format!("[{r}"))
            } else if t.starts_with("[\"api\", \"pipeline") {
                ("GET".to_string(), t.to_string())
            } else {
                continue;
            };
            let Some(body) = rest
                .strip_prefix("[\"api\", \"pipeline")
                .map(|_| &rest[1..])
                .and_then(|r| r.split_once(']'))
                .map(|(b, _)| b)
            else {
                continue;
            };
            if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "DELETE") {
                continue;
            }
            let segs: Vec<&str> = body.split(',').map(str::trim).collect();
            let shape: Vec<Option<&str>> = segs
                .iter()
                .map(|s| s.strip_prefix('"').map(|w| w.trim_end_matches('"')))
                .collect();
            seen += 1;
            let found = doors.iter().any(|d| {
                let Some((m, p)) = d.split_once(' ') else {
                    return false;
                };
                let ds: Vec<&str> = p.trim_start_matches('/').split('/').collect();
                m == method
                    && ds.len() == shape.len()
                    && ds.iter().zip(&shape).all(|(d, w)| match w {
                        Some(w) => d == w || (d.starts_with('{') && d.ends_with('}')),
                        None => d.starts_with('{') && d.ends_with('}'),
                    })
            });
            if !found {
                missing.push(format!("{method} {body}"));
            }
        }
        assert!(seen >= 15, "the routes were not read ({seen})");
        assert!(missing.is_empty(), "served and not listed: {missing:?}");
    }

    use super::*;

    /// The trust check hashes a model's file once for all the parts of a
    /// run, however many go at once, and a file that is not the admitted
    /// one is refused every time.
    #[test]
    fn the_trust_check_hashes_a_bundle_once_for_all_the_parts() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let verified = Verified::default();
        let hashed = AtomicUsize::new(0);
        let hash = |_: &Path| -> Result<String, String> {
            hashed.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(50));
            Ok("abc".to_string())
        };
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    verified
                        .check((7, "sha256:abc"), Path::new("/model"), &hash)
                        .unwrap();
                });
            }
        });
        assert_eq!(hashed.load(Ordering::SeqCst), 1);
        let wrong = Verified::default();
        for _ in 0..2 {
            let why = wrong
                .check((8, "sha256:def"), Path::new("/model"), &hash)
                .unwrap_err();
            assert!(why.contains("not the digest sha256:def"), "{why}");
        }
        assert_eq!(hashed.load(Ordering::SeqCst), 3);
    }

    /// A part that is refused or fails stops the next before its turn
    /// comes: the turn passed on by a failed part comes with the stop
    /// already set, so with two going at once the second never starts.
    #[test]
    fn a_failed_part_leaves_the_next_unstarted() {
        use std::sync::Mutex;
        let dir = nils_dicom::synth::TempDir::new("parts-stop");
        let home = Home::new(dir.path().join("registry"));
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        drop(
            home.init(&nils_registry::InitOptions {
                backend: nils_registry::Backend::Sqlite,
                dsn: None,
                schema: None,
                scheme: nils_registry::pseudonym::Scheme::Blake2b32,
                key: "k".to_string(),
                display_length: nils_registry::pseudonym::DEFAULT_DISPLAY_LENGTH,
                session_scheme: None,
            })
            .unwrap(),
        );
        for _ in 0..20 {
            let started = Mutex::new(Vec::new());
            let run_part = |_: &mut Registry,
                            i: usize,
                            _: bool,
                            _: bool,
                            turn: &dyn Fn(bool)|
             -> (Result<(), Exit>, Option<i64>, Option<i64>) {
                started.lock().unwrap().push(i);
                if i == 0 {
                    // refused before its run is made: the turn passes on with the news
                    turn(true);
                    return (Err(fail("refused before it started")), None, None);
                }
                turn(false);
                (Ok(()), Some(i as i64), None)
            };
            let (ended, stopped) = schedule_parts(&home, &[0, 1, 2], 2, 3, None, &run_part);
            assert_eq!(
                *started.lock().unwrap(),
                [0],
                "a part started after a failed one"
            );
            assert!(stopped);
            assert_eq!(ended.len(), 1);
            assert!(ended[0].1.is_err());
        }
    }

    /// 2026-10-17: a run over more stacks than a part holds is queued with
    /// `--chunk`: a part is what a GB of the declared memory was seen to
    /// fit, times the GBs, where the units run together; units apart are
    /// never split; the tests set their own size.
    #[test]
    fn a_large_batch_is_queued_in_parts_its_pipeline_s_memory_holds() {
        let base = r#"name: header-model
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/header-model@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
command-line: "score [Manifest] [OutputLocation]"
x-nils:
  analysis-level: stack
  input: {layout: stacks}
  outputs:
    - {id: opinion, kind: output, path-template: "s/{stack}.json", media-type: application/json}
"#;
        let of = |extra: &str| descriptor::parse(&format!("{base}{extra}")).unwrap();
        // System 1's: 4 GB, together
        let s1 = of("  needs: {gpu: none, memory-gb: 4, cores: 4}\n");
        assert_eq!(s1.units, Units::Together);
        assert_eq!(part_stacks_of(&s1, None), Some(5_000));
        assert_eq!(part_stacks_of(&s1, Some(4)), Some(4));
        // less than a GB is a GB's worth
        let small = of("  needs: {memory-gb: 0.5}\n");
        assert_eq!(part_stacks_of(&small, None), Some(1_250));
        // units apart are a container each
        let apart = of("  units: apart\n  needs: {memory-gb: 4}\n");
        assert_eq!(part_stacks_of(&apart, None), None);
        assert_eq!(part_stacks_of(&apart, Some(4)), None);
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let run = words(&["run", "system1-score@1", "--handle", "7", "--model", "3"]);
        // 456,110 new stacks: in parts of 5,000
        assert_eq!(
            with_chunk(run.clone(), 456_110, Some(5_000)),
            words(&[
                "run",
                "system1-score@1",
                "--handle",
                "7",
                "--model",
                "3",
                "--chunk",
                "5000"
            ])
        );
        // 5,220 as before, and as many as a part holds
        assert_eq!(with_chunk(run.clone(), 5_000, Some(5_000)), run);
        assert_eq!(with_chunk(run.clone(), 4_999, Some(5_000)), run);
        assert_eq!(with_chunk(run.clone(), 456_110, None), run);
    }

    /// 2026-10-17: a folder a new run finds at its own path holds another
    /// run's files, and is moved aside whole, never deleted; a second one
    /// in the same second takes a name of its own; no folder, nothing.
    #[test]
    fn a_folder_another_run_left_is_moved_aside_whole() {
        let base = std::env::temp_dir().join(format!("nils-aside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join(RUNS).join("7");
        assert_eq!(set_aside(&dir).unwrap(), None);
        std::fs::create_dir_all(dir.join("input")).unwrap();
        std::fs::write(dir.join("image.log"), "an older registry's run 7").unwrap();
        std::fs::write(dir.join("input/stacks.json"), "{}").unwrap();
        let to = set_aside(&dir).unwrap().unwrap();
        assert!(!dir.exists());
        assert_eq!(to.parent(), dir.parent());
        let name = to.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("7.before-"), "{name}");
        assert_eq!(
            std::fs::read_to_string(to.join("image.log")).unwrap(),
            "an older registry's run 7"
        );
        assert!(to.join("input/stacks.json").is_file());
        // again at once: a name of its own beside the first
        std::fs::create_dir_all(&dir).unwrap();
        let again = set_aside(&dir).unwrap().unwrap();
        assert_ne!(again, to);
        assert!(again.is_dir() && to.is_dir() && !dir.exists());
        assert!(moved_words(&dir, &to).contains("moved to"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A handle of these stacks, each with a subject ten times its id,
    /// pinned with these selections.
    fn handle_of(store: &mut Store, stacks: &[i64], pins: Value) -> i64 {
        use nils_registry::store::{Cell, Row};
        let rows = stacks
            .iter()
            .map(|s| Row(vec![Cell::Int(*s), Cell::Int(s * 10)]))
            .collect();
        crate::operations::save_stacks(
            store,
            rows,
            "ops@lab",
            3,
            &crate::operations::Pinned {
                versions: pins,
                pack_version: Some("mri-1"),
                scheme_digest: None,
            },
        )
        .unwrap()
        .0
    }

    /// 2026-10-17: a handle run with `--chunk` is split in stack order into
    /// handles of at most that many, each pinned as it is with its part
    /// said; one that fits runs as it is; one of a scope's main scans of
    /// several roles is split only as it is frozen, in whole visits.
    #[test]
    fn a_handle_is_split_in_stack_order_into_handles_pinned_as_it_is() {
        let mut store = Store::sqlite_in_memory().unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        let split = |store: &mut Store, h: i64, chunk: u64| {
            split_handle(store, h, chunk).unwrap_or_else(|e| panic!("{}", e.message))
        };
        let pin = json!({"selection": "every", "version": 1, "set": "every"});
        let h = handle_of(&mut store, &[5, 1, 4, 2, 3], json!([pin]));
        assert_eq!(split(&mut store, h, 5), [h]);
        assert_eq!(split(&mut store, h, 9), [h]);
        assert!(split_handle(&mut store, h, 0).is_err());
        let parts = split(&mut store, h, 2);
        assert_eq!(parts.len(), 3);
        let mut held = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            assert_ne!(*part, h);
            let keys = nils_ask::handle::keys(&mut store, *part).unwrap();
            assert!(keys.iter().all(|(s, subject)| *subject == Some(s * 10)));
            held.push(keys.iter().map(|(s, _)| *s).collect::<Vec<_>>());
            let got = nils_ask::handle::get(&mut store, *part).unwrap().unwrap();
            assert_eq!(got.principal, "ops@lab");
            assert_eq!(got.epoch, 3);
            assert_eq!(got.pack_version.as_deref(), Some("mri-1"));
            assert_eq!(
                got.selection_versions,
                json!([pin, {"handle": h, "part": {"part": i + 1, "of": 3}}])
            );
        }
        assert_eq!(held, [vec![1, 2], vec![3, 4], vec![5]]);
        // a scope's main scans of one role say their part; of several, the
        // split is refused and says how to make it
        let one = json!({"main": "t1w", "picks": {}, "of": "main:t1w@dataset:d"});
        let h = handle_of(&mut store, &[1, 2, 3], json!([one]));
        let parts = split(&mut store, h, 2);
        let got = nils_ask::handle::get(&mut store, parts[1])
            .unwrap()
            .unwrap();
        assert_eq!(
            got.selection_versions[0]["part"],
            json!({"part": 2, "of": 2})
        );
        let several = json!({"main": "t1w+flair", "roles": ["t1w", "flair"], "picks": {}, "of": "main@dataset:d"});
        let h = handle_of(&mut store, &[1, 2, 3], json!([several]));
        let refused = split_handle(&mut store, h, 2).unwrap_err();
        assert!(
            refused
                .message
                .contains("--select main@dataset:d --chunk 2"),
            "{}",
            refused.message
        );
    }

    /// Record 55, second review: the lane's start sweeps the work folders
    /// of the runs that ended (done, partial, failed, cancelled,
    /// interrupted), never a running one's, and the next sweep visits only
    /// those that ended after it.
    #[test]
    fn the_sweep_clears_ended_runs_once_and_never_a_running_one() {
        sweep_case(Store::sqlite_in_memory().unwrap(), "sqlite");
        // and on Postgres, where the time is cast to a timestamp
        let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
            .ok()
            .filter(|d| !d.is_empty())
        else {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            return;
        };
        let schema = "nils_sweep_test";
        let mut admin = Store::connect_postgres(&dsn, "public").unwrap();
        admin
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
            ))
            .unwrap();
        drop(admin);
        sweep_case(Store::connect_postgres(&dsn, schema).unwrap(), "postgres");
        let mut admin = Store::connect_postgres(&dsn, "public").unwrap();
        admin
            .batch(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .unwrap();
    }

    fn sweep_case(mut store: Store, backend: &str) {
        let base =
            std::env::temp_dir().join(format!("nils-sweep-{}-{backend}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        let place = nils_registry::place::add(
            &mut store,
            &nils_registry::place::New {
                name: "work",
                role: nils_registry::place::Role::Working,
                path: base.to_str().unwrap(),
                guarantees: json!({}),
                probed: json!({}),
                handling: Value::Null,
                dataset: Value::Null,
            },
        )
        .unwrap();
        let doc = json!({});
        let (pl, _) = rows::add(
            &mut store,
            &rows::New {
                name: "swept",
                tool_version: "1",
                descriptor: &doc,
                descriptor_digest: "d1",
                image: "example.org/swept",
                image_digest: "sha256:00",
                layout: "stacks",
                level: "stack",
                added_by: "test",
                added_at: "2026-10-15T10:00:00Z",
                pack: None,
                pack_digest: None,
                version_digest: "d1",
            },
        )
        .unwrap();
        let params = json!({});
        let host = job::hostname();
        let mut start = |status: Option<&str>| -> i64 {
            let id = rows::start(
                &mut store,
                &rows::NewRun {
                    pipeline_id: pl.id,
                    job_id: None,
                    handle_id: None,
                    selection: None,
                    params: &params,
                    runtime: "apptainer",
                    runtime_version: "1",
                    host: &host,
                    device: "cpu",
                    model_ids: &[],
                    label_set_id: None,
                    place_id: Some(place),
                    scratch_place_id: None,
                    principal: "t",
                    actor: None,
                    started_at: "2026-10-15T10:00:00Z",
                },
            )
            .unwrap();
            if let Some(s) = status {
                rows::finish(
                    &mut store,
                    id,
                    &rows::Finish {
                        status: s,
                        finished_at: &nils_registry::time::now_iso(),
                        exit_code: None,
                        results_digest: None,
                        summary: &json!({}),
                        error: None,
                        failure: None,
                    },
                )
                .unwrap();
            }
            id
        };
        let mut ids: Vec<i64> = [
            Some("done"),
            Some("partial"),
            Some("failed"),
            Some("cancelled"),
            None,
        ]
        .into_iter()
        .map(&mut start)
        .collect();
        // a run whose engine went away: interrupted, its end without a time
        let gone = start(None);
        assert!(rows::interrupt(&mut store, gone, "went away").unwrap());
        ids.insert(4, gone);
        let tmp = |id: i64| {
            base.join(RUNS)
                .join(id.to_string())
                .join("units/stack-1")
                .join(WORKDIR)
        };
        for id in &ids {
            std::fs::create_dir_all(tmp(*id)).unwrap();
        }
        // times are whole seconds: the sweep comes a second after the ends
        std::thread::sleep(Duration::from_millis(1100));
        assert_eq!(sweep_workdirs(&mut store).unwrap(), 5);
        for id in &ids[..5] {
            assert!(!tmp(*id).exists(), "run {id}");
        }
        assert!(tmp(ids[5]).exists(), "a running run keeps its own");
        // the next sweep visits none of those again, the interrupted one
        // included, and the marker is this host's
        std::fs::create_dir_all(tmp(gone)).unwrap();
        assert_eq!(sweep_workdirs(&mut store).unwrap(), 0);
        assert!(tmp(gone).exists());
        assert!(
            meta_text(&mut store, &format!("{SWEPT_KEY}.{host}"))
                .unwrap()
                .is_some()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Record 55, after review: what apptainer units left in their /tmp
    /// (clinical-derived volumes) goes when a run stops, is cancelled or
    /// taken up again, and at an engine's start; nothing else of the run.
    #[test]
    fn a_run_s_work_folders_go_and_nothing_else() {
        let base = std::env::temp_dir().join(format!("nils-workdirs-{}", std::process::id()));
        let run = base.join("runs").join("7");
        for d in [
            "tmp/a",
            "units/stack-1/tmp/b",
            "units/stack-2/tmp",
            "units/stack-2/out",
        ] {
            std::fs::create_dir_all(run.join(d)).unwrap();
        }
        std::fs::write(run.join("units/stack-1/tmp/b/vol.nii.gz"), b"x").unwrap();
        std::fs::write(run.join("units/stack-1/log.txt"), b"log").unwrap();
        drop_workdirs(&run);
        assert!(!run.join("tmp").exists());
        assert!(!run.join("units/stack-1/tmp").exists());
        assert!(!run.join("units/stack-2/tmp").exists());
        assert!(run.join("units/stack-1/log.txt").exists());
        assert!(run.join("units/stack-2/out").exists());
        // a run with no folders is no error
        drop_workdirs(&base.join("runs").join("8"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Record 55 H3, section 6.5: a `derivative:<kind>` input takes what
    /// its check leaves it: by default any result but a rejected one, with
    /// `accepted` only the accepted, with `any` every one.
    #[test]
    fn a_derivative_input_takes_what_its_check_leaves_it() {
        let mut store = Store::sqlite_in_memory().unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        let place = nils_registry::place::add(
            &mut store,
            &nils_registry::place::New {
                name: "work",
                role: nils_registry::place::Role::Working,
                path: "/nowhere",
                guarantees: json!({}),
                probed: json!({}),
                handling: Value::Null,
                dataset: Value::Null,
            },
        )
        .unwrap();
        let pl = nils_registry::place::show(&mut store, place)
            .unwrap()
            .unwrap();
        let t = store.qualified("derivative");
        for (i, status) in [
            (1, "'accepted'"),
            (2, "'rejected'"),
            (3, "NULL"),
            (4, "'flagged'"),
        ] {
            store
                .execute(
                    &format!(
                        "INSERT INTO {t} (kind, scope, stack_id, place_id, path, bytes, sha256, media_type, created_at, check_status) \
                         VALUES ('mask', 'stack', 7, {place}, 'p{i}', 1, 's{i}', 'x', '2026-10-11T10:00:00Z', {status})"
                    ),
                    &[],
                )
                .unwrap();
        }
        let yml = |checked: &str| {
            format!(
                "name: takes\nschema-version: \"0.5\"\ntool-version: \"1\"\ncontainer-image:\n  type: docker\n  image: \"example.org/takes@sha256:59c45f54a1f1dc69134f63bec91a726e41c71c64a16cc21cda0b54526910a3c3\"\nx-nils:\n  analysis-level: stack\n  input: {{layout: stacks}}\n  inputs:\n    - {{id: prior, type: \"derivative:mask\"{checked}}}\n  outputs:\n    - {{id: out, kind: output, path-template: \"stack-{{stack}}/out.txt\"}}\n"
            )
        };
        for (checked, want) in [
            ("", vec!["p1", "p3", "p4"]),
            (", checked: not_rejected", vec!["p1", "p3", "p4"]),
            (", checked: accepted", vec!["p1"]),
            (", checked: any", vec!["p1", "p2", "p3", "p4"]),
        ] {
            let d = nils_pipeline::descriptor::parse(&yml(checked)).unwrap();
            let taken = derivative_inputs(&mut store, &d, &[7], &[&pl], &Bound::new()).unwrap();
            let paths: Vec<&str> = taken["prior"].iter().map(|r| r.path.as_str()).collect();
            assert_eq!(paths, want, "{checked}");
        }
        assert!(takes_checked("not_rejected", None));
        assert!(!takes_checked("not_rejected", Some("rejected")));
        assert!(!takes_checked("accepted", Some("flagged")));
    }

    /// Record 55 E2: a geometry value is DICOM's backslash list of numbers
    /// read whole, with the count it must have, or nothing: a pipeline never
    /// meets half a position, a word or an infinity.
    #[test]
    fn geometry_numbers_are_whole_or_null() {
        assert_eq!(
            dicom_numbers(Some("1\\-2.5\\3e1"), 3),
            json!([1.0, -2.5, 30.0])
        );
        assert_eq!(dicom_numbers(Some(" 0.5 \\ 0.5 "), 2), json!([0.5, 0.5]));
        assert_eq!(
            dicom_numbers(Some("1\\0\\0\\0\\1\\0"), 6)
                .as_array()
                .map(Vec::len),
            Some(6)
        );
        for (text, n) in [
            ("1\\2", 3),
            ("1\\2\\3\\4", 3),
            ("1\\x\\3", 3),
            ("1\\inf\\3", 3),
            ("1\\NaN\\3", 3),
            ("", 2),
            ("1\\\\2", 2),
        ] {
            assert_eq!(dicom_numbers(Some(text), n), Value::Null, "{text:?}");
        }
        assert_eq!(dicom_numbers(None, 3), Value::Null);
        // a stack's thickness and spacing are numbers or null, never NaN
        let g = stack_geometry(Some("0\\1\\0\\0\\0\\-1"), Some(1.5), Some(f64::NAN));
        assert_eq!(
            g["image_orientation_patient"],
            json!([0.0, 1.0, 0.0, 0.0, 0.0, -1.0])
        );
        assert_eq!(g["slice_thickness"], json!(1.5));
        assert_eq!(g["spacing_between_slices"], Value::Null);
        let g = stack_geometry(None, None, None);
        assert!(g.as_object().unwrap().values().all(Value::is_null), "{g}");
    }

    /// The review of record 43: at detail plain a run's document says no
    /// unit by its subject or session, and keeps everything else.
    #[test]
    fn a_run_read_at_plain_detail_names_no_subject_or_session() {
        let mut doc = json!({
            "id": 3, "selection": "selection:every@1",
            "summary": {"refused_files": [
                {"unit": "sub-P1_ses-20220115", "why": "not its own"},
                {"unit": "stack-12", "why": "a link out"},
                {"unit": "sub-P2", "file": "sub-P2/x.nii.gz"},
            ], "units": {"total": 3}},
        });
        plain(&mut doc);
        let refused = doc["summary"]["refused_files"].as_array().unwrap();
        assert!(refused[0]["unit"].is_null() && refused[2]["file"].is_null());
        assert_eq!(refused[1]["unit"], "stack-12");
        assert_eq!(doc["summary"]["units"]["total"], 3);
        assert!(!doc.to_string().contains("sub-"), "{doc}");
    }

    /// Record 49 R4 and R4b, after the assistant's review: below detail
    /// quasi a run's checks and failures are counts by check and by reason,
    /// a count of 1 to 4 scans withheld, and no unit, value or tool's words.
    #[test]
    fn a_run_read_below_quasi_says_counts_by_check_and_reason_and_no_scan() {
        let breach = |unit: &str, check: &str, metric: &str, value: f64| {
            json!({"unit": unit, "breaches": [
                {"metric": metric, "value": value, "check": check, "op": ">=", "threshold": 8},
            ]})
        };
        let mut breaches: Vec<Value> = (1..=6)
            .map(|i| breach(&format!("stack-{i}"), "snr >= 8", "snr", 3.25 + i as f64))
            .collect();
        breaches.push(breach("sub-P7_ses-1", "holes <= 200", "holes", 251.5));
        let mut doc = json!({
            "id": 9, "status": "partial",
            "error": "the tool said: sub-P9 has no /data/raw/P9/x.dcm",
            "unit_states": {"over": 12},
            "units_run": [{"unit": "stack-1", "state": "over", "status": "failed", "exit_code": 3}],
            "summary": {
                "units": {"total": 12, "succeeded": 10, "failed": 2, "skipped": 0, "unreported": 0},
                "numbers": {
                    "tables": {"refused_values": ["volumes: brain_volume: 12.75x", "volumes: brain_volume: 13.5x"]},
                    "checks": {"declared": 2, "breaches": 7, "unchecked": 0},
                },
                "breaches": breaches,
                "refused_files": [
                    {"unit": "stack-11", "file": "stack-11/x.nii.gz", "why": "a link out"},
                    {"output": "model", "why": "the run wrote no model"},
                ],
                "review_items": [1, 2, 3],
            },
        });
        run_totals_only(&mut doc);
        let s = &doc["summary"];
        assert_eq!(s["breaches"], json!([]), "{doc}");
        assert_eq!(
            s["breaches_by_check"],
            json!([
                {"check": "holes <= 200", "metric": "holes", "units": null, "withheld": true},
                {"check": "snr >= 8", "metric": "snr", "units": 6},
            ]),
            "{doc}"
        );
        assert_eq!(
            s["numbers"]["checks"]["breaches"], 7,
            "7 breaches stand for 7 scans"
        );
        assert_eq!(
            s["failures_by_reason"],
            json!([
                {"reason": "failed", "units": null, "withheld": true},
                {"reason": "refused", "units": null, "withheld": true},
            ]),
            "{doc}"
        );
        // a count held is held where the run's totals would say it again
        assert!(s["units"]["failed"].is_null() && s["units"]["succeeded"].is_null());
        assert_eq!(s["units"]["total"], 12);
        assert_eq!(
            s["numbers"]["tables"]["refused_values"],
            json!([{"column": "volumes: brain_volume", "values": null, "withheld": true}])
        );
        assert_eq!(
            s["refused_files"],
            json!([{"output": "model", "why": "the run wrote no model"}])
        );
        assert!(
            s["review_items"].is_null(),
            "3 items stand for 3 units: {doc}"
        );
        assert_eq!(doc["units_run"], json!([]));
        assert_eq!(doc["unit_states"]["over"], 12);
        assert_eq!(s["detail"], "totals");
        let text = doc.to_string();
        for leak in [
            "stack-",
            "sub-",
            "3.25",
            "4.25",
            "251.5",
            "12.75",
            "/data/raw",
        ] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }

        // a job's result is its run's summary; a pipeline:qc item says its
        // status and run alone
        let mut job = json!({
            "kind": "pipeline", "error": "stack-3 failed: /data/raw/P3",
            "result": {"run": 9, "summary": {"breaches": [breach("stack-3", "snr >= 8", "snr", 4.5)],
                        "numbers": {"checks": {"breaches": 1}}}},
        });
        job_totals_only(&mut job);
        assert!(job["result"]["summary"]["numbers"]["checks"]["breaches"].is_null());
        // record 55 K9: a dataset probe's alike subjects are counted, never named
        let mut probe = json!({
            "kind": "ingest",
            "result": {"dataset": "ds", "candidates": [
                {"label": "rule 1", "alike": {"pairs": [{"subjects": ["CODEA1", "CODEB2"]}], "linked": 0, "unmapped": 1}},
                {"label": "rule 2"},
            ]},
        });
        job_totals_only(&mut probe);
        assert_eq!(
            probe["result"]["candidates"][0]["alike"]["pairs"],
            json!([])
        );
        assert_eq!(probe["result"]["candidates"][0]["alike"]["withheld"], 1);
        assert_eq!(probe["result"]["candidates"][0]["alike"]["unmapped"], 1);
        assert!(probe["result"]["candidates"][1]["alike"].is_null());
        assert!(!probe.to_string().contains("CODEA1"));
        // a job with no result yet, or one that is not a document, is read
        // as it stands
        for result in [json!(null), json!("done"), json!([1, 2])] {
            let mut queued = json!({"kind": "ingest", "state": "queued", "result": result});
            let before = queued.clone();
            job_totals_only(&mut queued);
            assert_eq!(queued, before);
        }
        assert!(!job.to_string().contains("stack-3") && !job.to_string().contains("4.5"));
        // below detail quasi a review list says a run's items one a check
        // or a reason with its count, never one a unit
        let qc = |id: i64, unit: &str, status: &str, check: Option<&str>| {
            json!({
                "id": id, "kind": "pipeline:qc", "scope": "run", "status": "open",
                "group_key": format!("run:9|unit:{unit}"),
                "ref": {"run_id": 9, "pipeline": "volumes@1", "unit": unit, "stack_id": id},
                "evidence": {"status": status, "error": "snr is 4.5, and the check is snr >= 8",
                             "metrics": {"breaches": check.map_or(json!([]), |c| json!([{"check": c, "value": 4.5}]))}},
            })
        };
        let mut rows =
            vec![json!({"id": 1, "kind": "classify:conflict", "evidence": {"axis": "a"}})];
        rows.extend((0..6).map(|i| qc(10 + i, &format!("stack-{i}"), "breach", Some("snr >= 8"))));
        rows.push(qc(20, "sub-P1", "breach", Some("holes <= 200")));
        rows.push(qc(21, "sub-P2", "failed", None));
        let grouped = qc_items_grouped(rows);
        assert_eq!(grouped.len(), 4, "{grouped:?}");
        assert_eq!(grouped[0]["evidence"]["axis"], "a");
        let of = |check: Option<&str>, reason: &str| {
            grouped
                .iter()
                .find(|g| {
                    g["evidence"]["check"].as_str() == check && g["evidence"]["status"] == reason
                })
                .cloned()
                .unwrap()
        };
        assert_eq!(of(Some("snr >= 8"), "breach")["units"], 6);
        assert_eq!(of(Some("holes <= 200"), "breach")["withheld"], true);
        assert!(of(None, "failed")["units"].is_null());
        assert_eq!(
            of(None, "failed")["ref"],
            json!({"run_id": 9, "pipeline": "volumes@1"})
        );
        let text = json!(grouped).to_string();
        for leak in ["stack-", "sub-", "4.5", "the check is"] {
            assert!(!text.contains(leak), "{leak} in {text}");
        }
        assert!(grouped[1..].iter().all(|g| g.get("id").is_none()), "{text}");
    }

    /// The second review of record 43: a folder whose name the runtimes'
    /// mount syntax would split is bound through an ancestor, not refused,
    /// and a file directly in its source root binds that root.
    #[test]
    fn a_folder_a_mount_would_misread_is_bound_through_its_parent() {
        assert_eq!(bind_folder("/data", "P1/1/f.dcm").as_deref(), Some("P1/1"));
        assert_eq!(bind_folder("/data", "P1/a:b/f.dcm").as_deref(), Some("P1"));
        assert_eq!(bind_folder("/data", "x,y/a:b/f.dcm").as_deref(), Some(""));
        assert_eq!(bind_folder("/data", "f.dcm").as_deref(), Some(""));
        assert_eq!(bind_folder("/da:ta", "P1/f.dcm"), None);
    }

    /// The second review of record 43: linking an input twice, or a file
    /// already linked, never copies a file onto its own link, which would
    /// empty it.
    #[test]
    fn an_input_linked_twice_keeps_its_bytes() {
        let root = std::env::temp_dir().join(format!("nils-link-input-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let file = root.join("derivatives/p/1/stack-1/x.emb");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"the embedding").unwrap();
        let into = root.join("runs/2/inputs/emb");
        std::fs::create_dir_all(&into).unwrap();
        for _ in 0..2 {
            let rel = link_input(&root, "derivatives/p/1/stack-1/x.emb", &into).unwrap();
            assert_eq!(rel, "p/1/stack-1/x.emb");
        }
        assert_eq!(std::fs::read(&file).unwrap(), b"the embedding");
        assert_eq!(
            std::fs::read(into.join("p/1/stack-1/x.emb")).unwrap(),
            b"the embedding"
        );
        // another file already at the link's place is refused, never overwritten
        let other = root.join("derivatives/p/1/stack-2/x.emb");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, b"another").unwrap();
        std::fs::write(
            into.join("p/1/stack-2/x.emb")
                .parent()
                .map(|p| {
                    std::fs::create_dir_all(p).unwrap();
                    p.join("x.emb")
                })
                .unwrap(),
            b"planted",
        )
        .unwrap();
        assert!(link_input(&root, "derivatives/p/1/stack-2/x.emb", &into).is_err());
        assert_eq!(std::fs::read(&other).unwrap(), b"another");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Record 49 A1: the door queues a resume as it queues a run, and still
    /// takes no flag of a caller's own.
    #[test]
    fn a_door_queues_a_resume_and_nothing_else() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let pack = Path::new("/packs");
        assert_eq!(
            located(Some(pack), words(&["run", "--resume", "7"])).ok(),
            Some(words(&["run", "--resume", "7", "--pack-dir", "/packs"]))
        );
        assert!(located(None, words(&["run"])).is_err());
        assert!(located(None, words(&["run", "--resume"])).is_err());
        assert!(located(None, words(&["run", "--resume", "7", "--dcm2niix", "/x"])).is_err());
        // Wave 7b: --chunk from a door, a whole number of stacks
        assert_eq!(
            located(
                None,
                words(&["run", "p", "--handle", "3", "--chunk", "500"])
            )
            .ok(),
            Some(words(&["run", "p", "--handle", "3", "--chunk", "500"]))
        );
        for bad in ["0", "-1", "x", "1.5"] {
            assert!(
                located(None, words(&["run", "p", "--chunk", bad])).is_err(),
                "{bad}"
            );
        }
        assert!(located(None, words(&["run", "p", "--chunk"])).is_err());
    }

    /// Record 49: a unit's id becomes a folder only where it is a plain word.
    #[test]
    fn a_unit_s_folder_is_a_plain_word() {
        assert!(folder_word("sub-P1_ses-20220115").is_ok());
        assert!(folder_word("stack-12").is_ok());
        for bad in ["", "..", ".hidden", "a/b", "sub-1 x"] {
            assert!(folder_word(bad).is_err(), "{bad}");
        }
    }

    /// After review (finding 8): a step of another grain is fed by the
    /// stacks its units hold, not by the units' names: a session unit over
    /// a stack step's run is ready when every stack of it is over there,
    /// skipped with the reason where one of them was, waits while one has
    /// no unit yet, and is skipped once the run before is over without one;
    /// a unit of the same name is matched by its name as before.
    #[test]
    fn the_chain_feeds_a_unit_of_another_grain_by_its_stacks() {
        let over = |status: &str| {
            (
                "over".to_string(),
                json!({"status": status, "error": format!("{status} upstream")}),
            )
        };
        let none = BTreeSet::new();
        let mut w = ChainWatch::default();
        w.runs.insert(
            7,
            WatchedRun {
                status: "running".into(),
                over: false,
                units: [
                    ("stack-1".to_string(), over("succeeded")),
                    ("stack-2".to_string(), over("reused")),
                    ("stack-3".to_string(), over("failed")),
                    ("stack-4".to_string(), ("running".to_string(), Value::Null)),
                ]
                .into_iter()
                .collect(),
                held: [
                    (1, "stack-1"),
                    (2, "stack-2"),
                    (3, "stack-3"),
                    (4, "stack-4"),
                ]
                .into_iter()
                .map(|(s, u)| (s, u.to_string()))
                .collect(),
            },
        );
        assert!(matches!(
            w.of("sub-1_ses-a", &[1, 2], &none),
            Upstream::Ready(_)
        ));
        match w.of("sub-1_ses-b", &[2, 3], &none) {
            Upstream::Skipped { run, status, words } => {
                assert_eq!(
                    (run, status.as_str(), words.as_str()),
                    (7, "failed", "failed upstream")
                );
            }
            _ => panic!("a session over a failed stack is skipped"),
        }
        assert!(matches!(
            w.of("sub-2_ses-a", &[1, 4], &none),
            Upstream::NotYet
        ));
        assert!(matches!(w.of("sub-3_ses-a", &[9], &none), Upstream::NotYet));
        // the same name still matches by name
        assert!(matches!(w.of("stack-1", &[1], &none), Upstream::Ready(_)));
        assert!(matches!(w.of("stack-4", &[4], &none), Upstream::NotYet));
        // the run before over: a stack no unit held is skipped and says so
        w.runs.get_mut(&7).unwrap().status = "done".into();
        w.runs.get_mut(&7).unwrap().over = true;
        match w.of("sub-3_ses-a", &[1, 9], &none) {
            Upstream::Skipped { words, .. } => {
                assert_eq!(words, "the step before (run 7) had no unit holding stack 9");
            }
            _ => panic!("a stack no unit before held is skipped once that run is over"),
        }
        // a unit whose stacks the registry does not know is matched by name only
        match w.of("sub-4", &[], &none) {
            Upstream::Skipped { words, .. } => {
                assert_eq!(words, "the step before (run 7) had no unit sub-4");
            }
            _ => panic!("no stacks and no such name: skipped"),
        }
    }

    /// After the third review (finding 2): a run before that is
    /// `interrupted`, or whose step is over while its row still says
    /// running, is over to the gate: a unit of it not over is skipped with
    /// the words, never "not yet" for ever. And (finding 9) a run whose
    /// every link feeds an optional input never skips a unit: what it would
    /// have said comes back as a note with `Ready`.
    #[test]
    fn an_over_run_before_skips_its_unfinished_units_and_an_optional_one_notes_them() {
        let running = ("running".to_string(), Value::Null);
        let failed = (
            "over".to_string(),
            json!({"status": "failed", "error": "failed upstream"}),
        );
        let good = ("over".to_string(), json!({"status": "succeeded"}));
        let mut w = ChainWatch::default();
        w.runs.insert(
            7,
            WatchedRun {
                status: "interrupted".into(),
                over: true,
                units: [
                    ("stack-1".to_string(), good.clone()),
                    ("stack-2".to_string(), running.clone()),
                    ("stack-3".to_string(), failed.clone()),
                ]
                .into_iter()
                .collect(),
                held: [(1, "stack-1"), (2, "stack-2"), (3, "stack-3")]
                    .into_iter()
                    .map(|(s, u)| (s, u.to_string()))
                    .collect(),
            },
        );
        let none = BTreeSet::new();
        assert!(matches!(w.of("stack-1", &[1], &none), Upstream::Ready(_)));
        match w.of("stack-2", &[2], &none) {
            Upstream::Skipped { status, words, .. } => {
                assert_eq!(status, "interrupted");
                assert_eq!(
                    words,
                    "the step before was interrupted while its unit stack-2 was running"
                );
            }
            _ => panic!("an interrupted run's unit is skipped, not waited for"),
        }
        // the row says running, but the step is over (its thread ended)
        w.runs.get_mut(&7).unwrap().status = "running".into();
        match w.of("stack-2", &[2], &none) {
            Upstream::Skipped { status, words, .. } => {
                assert_eq!(status, "running");
                assert_eq!(
                    words,
                    "the step before was over while its unit stack-2 was running"
                );
            }
            _ => panic!("a run whose step is over is over"),
        }
        // not over: waited for
        w.runs.get_mut(&7).unwrap().over = false;
        assert!(matches!(w.of("stack-2", &[2], &none), Upstream::NotYet));
        // an optional run: the failed unit is a note, and the unit goes on
        let optional: BTreeSet<i64> = [7].into_iter().collect();
        match w.of("stack-3", &[3], &optional) {
            Upstream::Ready(notes) => {
                assert_eq!(notes, vec![(7, "failed upstream".to_string())]);
            }
            _ => panic!("an optional input's failed unit is a note"),
        }
        match w.of("stack-3", &[3], &none) {
            Upstream::Skipped { words, .. } => assert_eq!(words, "failed upstream"),
            _ => panic!("a required input's failed unit skips"),
        }
        // the notes name the inputs the run feeds, on the unit's record
        let chain = Chain {
            workflow_run: 1,
            step: 1,
            after: vec![7],
            links: vec![ChainLink {
                from_step: 0,
                from_id: "a".into(),
                run: 7,
                output: "out".into(),
                input: "prev".into(),
                role: false,
            }],
            unfed: vec![Unfed {
                input: "mask".into(),
                words: "fed by no step for this unit: step 1 (m) ended done without a run".into(),
            }],
        };
        let fed = fed_notes(
            Some(&chain),
            Some(&vec![(
                "prev".to_string(),
                "fed by no step for this unit: failed upstream".to_string(),
            )]),
        );
        assert_eq!(fed["prev"], "fed by no step for this unit: failed upstream");
        assert_eq!(
            fed["mask"],
            "fed by no step for this unit: step 1 (m) ended done without a run"
        );
        assert!(fed_notes(None, None).is_null());
    }

    /// After the third review (finding 7): a role's file is bound only
    /// where the fed image's format is the placed file's; the sidecar is
    /// never touched, and two formats are named where they differ, with
    /// nothing written.
    #[test]
    fn a_role_is_bound_only_in_the_same_image_format() {
        let dir = nils_dicom::synth::TempDir::new("bind-role-files");
        let folder = dir.path().join("sub-1").join("anat");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("sub-1_FLAIR.nii.gz"), b"released").unwrap();
        std::fs::write(folder.join("sub-1_FLAIR.json"), b"{}").unwrap();
        std::fs::write(folder.join("sub-1_T1w.nii.gz"), b"other").unwrap();
        let made = dir.path().join("made");
        std::fs::create_dir_all(&made).unwrap();
        std::fs::write(made.join("aligned.nii.gz"), b"aligned").unwrap();
        std::fs::write(made.join("aligned.mgz"), b"aligned mgz").unwrap();
        // the same format: the image file is written over, the sidecar kept
        let files = bind_role_files(&folder, "sub-1_FLAIR", &made.join("aligned.nii.gz"))
            .unwrap()
            .unwrap();
        assert_eq!(files, vec!["sub-1_FLAIR.nii.gz".to_string()]);
        assert_eq!(
            std::fs::read(folder.join("sub-1_FLAIR.nii.gz")).unwrap(),
            b"aligned"
        );
        assert_eq!(
            std::fs::read(folder.join("sub-1_FLAIR.json")).unwrap(),
            b"{}"
        );
        assert_eq!(
            std::fs::read(folder.join("sub-1_T1w.nii.gz")).unwrap(),
            b"other"
        );
        // another format: both named, nothing written
        let formats = bind_role_files(&folder, "sub-1_FLAIR", &made.join("aligned.mgz"))
            .unwrap()
            .unwrap_err();
        assert_eq!(formats, (".mgz".to_string(), ".nii.gz".to_string()));
        assert_eq!(
            std::fs::read(folder.join("sub-1_FLAIR.nii.gz")).unwrap(),
            b"aligned"
        );
        assert_eq!(image_format("x.nii.gz"), "nii.gz");
        assert_eq!(image_format("x.mgz"), "mgz");
        assert_eq!(image_format("noformat"), "");
    }
}
