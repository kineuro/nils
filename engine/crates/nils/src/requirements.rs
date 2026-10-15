// SPDX-License-Identifier: AGPL-3.0-only

//! What a run needs of this machine, checked before it is queued
//! (2026-10-12): the converter a bids input is released with, the
//! container runtime and whether it can take the image, the memory a unit
//! asks against what the machine has and what the lane holds, a GPU where
//! one is needed, and the disk a unit's outputs take where the descriptor
//! says. The pre-flight answers each check; the run door and `nils run`
//! refuse a run one of them fails, with its kind and its reason, and queue
//! nothing. No check is slow: nothing is pulled, and an image not pulled
//! yet is information, not a refusal.

use std::path::Path;
use std::time::Duration;

use nils_pipeline::descriptor::{Descriptor, Gpu, Layout, Units};
use nils_pipeline::failure::Cause;
use nils_pipeline::runtime;
use nils_registry::Registry;
use nils_registry::pipeline::Pipeline;
use serde_json::{Value, json};

use crate::serve::Reply;

/// How long a runtime is given to say whether it holds an image.
const IMAGE_CAP: Duration = Duration::from_secs(10);

/// One check: its name, whether it holds, what was found, and the cause
/// where it does not hold.
#[derive(Debug, Clone)]
pub(crate) struct Check {
    pub(crate) check: &'static str,
    pub(crate) ok: bool,
    pub(crate) said: String,
    pub(crate) cause: Option<Cause>,
    pub(crate) extra: Value,
}

impl Check {
    fn ok(check: &'static str, said: String, extra: Value) -> Check {
        Check {
            check,
            ok: true,
            said,
            cause: None,
            extra,
        }
    }

    fn failed(check: &'static str, cause: Cause, said: String, extra: Value) -> Check {
        Check {
            check,
            ok: false,
            said,
            cause: Some(cause),
            extra,
        }
    }

    /// `{check, ok, said, failure, ...}`, as the pre-flight answers it.
    pub(crate) fn doc(&self) -> Value {
        let mut v = json!({
            "check": self.check,
            "ok": self.ok,
            "said": self.said,
            "failure": self.cause.map_or(Value::Null, Cause::refusal_doc),
        });
        if let Value::Object(more) = &self.extra {
            for (k, x) in more {
                v[k] = x.clone();
            }
        }
        v
    }

    /// The refusal of a run this check fails, in words a person reads and
    /// the job keeps: what was found, what to do, and the cause marked.
    pub(crate) fn refusal(&self) -> String {
        let cause = self.cause.unwrap_or(Cause::Pipeline);
        format!(
            "refused before it started: {}; {} {}",
            self.said,
            cause.advice_before(),
            nils_pipeline::failure::mark(cause)
        )
    }
}

/// What the checks are asked of.
pub(crate) struct Asked<'a> {
    pub(crate) pipeline: &'a Pipeline,
    pub(crate) descriptor: &'a Descriptor,
    /// The converter as the run names it, `dcm2niix` by default.
    pub(crate) dcm2niix: &'a Path,
    /// The units the run would have; where it is not known, one, so the
    /// disk is held only to what one unit takes.
    pub(crate) units: usize,
    /// The runtime as the caller found it already, so it is not looked for
    /// twice; none to take the engine's own look.
    pub(crate) detected: Option<&'a runtime::Detected>,
    /// Whether the run releases a bids input, which needs the converter: a
    /// new run does, a run taken up again only where it released none
    /// before (2026-10-12).
    pub(crate) releases: bool,
}

fn found_by_words(by: &str) -> &'static str {
    match by {
        "given" => "as given",
        "setup-given" => "as the site gave it to setup",
        "path" => "on the engine's search path",
        "beside" => "beside nils",
        "setup" => "where the setup record names it",
        "tools" => "in the registry's tools folder",
        _ => "as named",
    }
}

/// The converter a bids input is released with: found, and answering its
/// version.
fn converter(home: &Path, given: &Path) -> Check {
    let found = crate::tools::find_dcm2niix(given, Some(home), std::env::var_os("PATH").as_deref());
    let Some(found) = found else {
        return Check::failed(
            "converter",
            Cause::Dcm2niix,
            "dcm2niix is not on the engine's search path, beside nils, where the setup record \
             names one, or in the registry's tools folder"
                .to_string(),
            json!({"path": null, "found_by": null}),
        );
    };
    let extra = json!({
        "path": found.path.display().to_string(), "found_by": found.by,
        "version": found.version,
    });
    match (&found.version, &found.problem) {
        (Some(version), _) => Check::ok(
            "converter",
            format!(
                "{version} at {} ({})",
                found.path.display(),
                found_by_words(found.by)
            ),
            extra,
        ),
        (None, problem) => Check::failed(
            "converter",
            Cause::Dcm2niix,
            problem
                .clone()
                .unwrap_or_else(|| format!("{} does not run", found.path.display())),
            extra,
        ),
    }
}

/// The name of the account a uid is, as /etc/passwd says it, else as the
/// environment does.
fn account_name(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    passwd
        .lines()
        .filter_map(|l| {
            let mut f = l.split(':');
            let name = f.next()?;
            let id = f.nth(1)?;
            (id == uid.to_string()).then(|| name.to_string())
        })
        .next()
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("LOGNAME").ok())
}

/// Whether an account has subordinate ids in a file of `/etc/subuid`'s
/// form, by its name or its uid.
pub(crate) fn has_subids(text: &str, name: Option<&str>, uid: u32) -> bool {
    text.lines().any(|l| {
        let mut f = l.trim().split(':');
        let who = f.next().unwrap_or_default();
        let count: u64 = f
            .nth(1)
            .and_then(|c| c.trim().parse().ok())
            .unwrap_or_default();
        (Some(who) == name || who == uid.to_string()) && count > 0
    })
}

/// Whether the engine's account has the subordinate ids rootless podman
/// unpacks an image with: none where it cannot be told, as where the
/// machine serves them from elsewhere than the two files.
fn subids_here() -> Option<bool> {
    let nss = std::fs::read_to_string("/etc/nsswitch.conf").unwrap_or_default();
    if nss.lines().any(|l| l.trim_start().starts_with("subid:")) {
        return None;
    }
    let uid = crate::pipelines::engine_uid();
    let name = account_name(uid);
    let has = |file: &str| {
        has_subids(
            &std::fs::read_to_string(file).unwrap_or_default(),
            name.as_deref(),
            uid,
        )
    };
    Some(has("/etc/subuid") && has("/etc/subgid"))
}

/// The runtime, and whether it can take the image or holds it already.
fn runtime_and_image(
    registry: &mut Registry,
    p: &Pipeline,
    detected: &runtime::Detected,
) -> Vec<Check> {
    let detected = detected.clone();
    let Some(rt) = detected.runtime else {
        return vec![Check::failed(
            "runtime",
            Cause::Runtime,
            detected
                .reason
                .unwrap_or_else(|| "no container runtime here".to_string()),
            json!({"runtime": null}),
        )];
    };
    let mut out = vec![Check::ok(
        "runtime",
        format!("{} {}", rt.kind.name(), rt.version),
        json!({"runtime": rt.kind.name(), "version": rt.version}),
    )];
    let here = match rt.kind {
        runtime::Kind::Apptainer => {
            crate::pipelines::run_places(registry.store())
                .ok()
                .map(|places| {
                    let form = crate::pipelines::image_form(registry);
                    let dir = Path::new(&places.scratch.path).join(crate::pipelines::IMAGES);
                    runtime::image_recorded(&runtime::local_image(&dir, &p.image_digest, form))
                })
        }
        _ => runtime::image_here(&rt, &p.image, IMAGE_CAP),
    };
    let extra = json!({"image": p.image, "present": here});
    out.push(match here {
        Some(true) => Check::ok("image", "the image is here".to_string(), extra),
        Some(false) if rt.kind == runtime::Kind::Podman && subids_here() == Some(false) => {
            Check::failed(
                "image",
                Cause::Subuid,
                "rootless podman here has no subordinate ids for the engine's account in \
                 /etc/subuid and /etc/subgid, so it cannot unpack an image whose files belong to \
                 other users"
                    .to_string(),
                extra,
            )
        }
        Some(false) => Check::ok(
            "image",
            "the image is not pulled yet: the run pulls it first".to_string(),
            extra,
        ),
        None => Check::ok(
            "image",
            format!(
                "whether the image is here is not known: {} did not say in time",
                rt.kind.name()
            ),
            extra,
        ),
    });
    out
}

/// The memory a unit asks, against the machine's and the lane's, and a GPU
/// where the pipeline needs one.
fn memory_and_card(
    registry: &mut Registry,
    p: &Pipeline,
    d: &Descriptor,
    detected: &runtime::Detected,
) -> Vec<Check> {
    let mut out = Vec::new();
    let ask = nils_pipeline::lane::Ask {
        cores: d.needs.cores,
        memory_mib: nils_pipeline::lane::mib_of_gb(d.needs.memory_gb),
    };
    let (_, machine_mib) = nils_pipeline::lane::host();
    let gb = |mib: u64| nils_pipeline::descriptor::number_text((mib as f64 / 102.4).round() / 10.0);
    let (lane, _) = crate::preflight::budget(registry);
    let extra = json!({
        "need_gb": d.needs.memory_gb, "machine_gb": gb(machine_mib),
        "lane_gb": gb(lane.ledger.memory_mib), "cores": d.needs.cores, "lane_cores": lane.ledger.cores,
    });
    out.push(if ask.memory_mib > machine_mib {
        Check::failed(
            "memory",
            Cause::Memory,
            format!(
                "a unit asks {} GB of memory, and this machine has {} GB",
                nils_pipeline::descriptor::number_text(d.needs.memory_gb),
                gb(machine_mib)
            ),
            extra,
        )
    } else if let Some(words) = crate::pipelines::lane_refusal(d, p, &lane) {
        Check::failed("memory", Cause::Lane, words, extra)
    } else {
        Check::ok(
            "memory",
            format!(
                "a unit asks {} GB; the lane holds {} GB of this machine's {} GB",
                nils_pipeline::descriptor::number_text(d.needs.memory_gb),
                gb(lane.ledger.memory_mib),
                gb(machine_mib)
            ),
            extra,
        )
    });
    if d.gpu == Gpu::Required {
        let card = detected.runtime.as_ref().and_then(|r| r.gpu.clone());
        out.push(match (card, lane.card) {
            (None, _) => Check::failed(
                "gpu",
                Cause::GpuDriver,
                format!(
                    "{} needs a GPU, and this engine's runtime offers none",
                    p.label()
                ),
                json!({"available": null}),
            ),
            (Some(c), None) => Check::failed(
                "gpu",
                Cause::Lane,
                format!(
                    "{} needs a GPU, and the pipeline lane uses no card here",
                    p.label()
                ),
                json!({"available": c}),
            ),
            (Some(c), Some(n)) => Check::ok(
                "gpu",
                format!("card {n}, {c}"),
                json!({"available": c, "card": n}),
            ),
        });
    }
    out
}

/// The disk the units' outputs take, where the descriptor says, against
/// the output place's free space.
pub(crate) fn disk(registry: &mut Registry, d: &Descriptor, units: usize) -> Option<Check> {
    let places = crate::pipelines::run_places(registry.store()).ok()?;
    let free = crate::places::space(Path::new(&places.output.path)).map(|(free, _)| free);
    let free_gb = free.map(|b| (b as f64 / 1e8).round() / 10.0);
    let units = if d.units == Units::Apart {
        units.max(1)
    } else {
        1
    };
    let need_gb = d.disk_gb.map(|g| g * units as f64);
    let extra = json!({
        "place": places.output.name, "free_gb": free_gb, "need_gb": need_gb,
    });
    let place = &places.output.name;
    Some(match (need_gb, free_gb) {
        (Some(need), Some(free)) if need > free => Check::failed(
            "disk",
            Cause::Disk,
            format!(
                "the units would write about {} GB into the working place {place}, which has {} GB free",
                nils_pipeline::descriptor::number_text(need),
                nils_pipeline::descriptor::number_text(free)
            ),
            extra,
        ),
        (Some(need), Some(free)) => Check::ok(
            "disk",
            format!(
                "the units would write about {} GB into the working place {place}, which has {} GB free",
                nils_pipeline::descriptor::number_text(need),
                nils_pipeline::descriptor::number_text(free)
            ),
            extra,
        ),
        (None, Some(free)) => Check::ok(
            "disk",
            format!(
                "{} GB free in the working place {place}; the pipeline does not say what its outputs take",
                nils_pipeline::descriptor::number_text(free)
            ),
            extra,
        ),
        (_, None) => Check::ok(
            "disk",
            format!("the free space of the working place {place} could not be read"),
            extra,
        ),
    })
}

/// Every check of a run, in the order a person reads them.
pub(crate) fn checks(registry: &mut Registry, home: &Path, a: &Asked<'_>) -> Vec<Check> {
    let mut out = Vec::new();
    if a.descriptor.layout == Layout::Bids && a.releases {
        out.push(converter(home, a.dcm2niix));
    }
    let detected = match a.detected {
        Some(d) => d.clone(),
        None => crate::pipelines::detect_cached(registry),
    };
    out.extend(runtime_and_image(registry, a.pipeline, &detected));
    out.extend(memory_and_card(
        registry,
        a.pipeline,
        a.descriptor,
        &detected,
    ));
    out.extend(disk(registry, a.descriptor, a.units));
    out
}

/// The first check that fails, which refuses the run.
pub(crate) fn refusing(checks: &[Check]) -> Option<&Check> {
    checks.iter().find(|c| !c.ok)
}

/// The pipeline a run's command line names: the word that is no flag and
/// no flag's value.
fn named(command: &[String]) -> Option<&str> {
    let mut it = command.iter().skip(1);
    while let Some(w) = it.next() {
        if w.starts_with("--") {
            it.next();
            continue;
        }
        return Some(w.as_str());
    }
    None
}

/// The value a flag of a command line takes.
fn flag<'a>(command: &'a [String], name: &str) -> Option<&'a str> {
    command
        .iter()
        .position(|w| w == name)
        .and_then(|i| command.get(i + 1))
        .map(String::as_str)
}

/// The run door's checks (2026-10-12): a `run` a door is about to queue,
/// refused with 409, its kind, its reason and every check, where one fails;
/// nothing is queued then. A pipeline the catalog does not hold is left to
/// the run, which refuses it itself.
///
/// A run taken up again (`--resume`) is checked as a new run is, under the
/// pipeline it recorded, the converter only where its bids input is still
/// to be released; one that cannot be taken up (done, failed after its
/// containers ended, or running) is refused as `nils run --resume` refuses
/// it. With `units`, a run that could run none of its units (a run taken
/// up again, of those it still has to run) and has made nothing is refused
/// as well, by the pre-flight's units and in its words, with `cannot`
/// ([`crate::preflight::door_units`]).
pub(crate) fn at_the_door(
    home: &nils_registry::home::Home,
    pack_dir: Option<&Path>,
    registry: &mut Registry,
    command: &[String],
    units: bool,
) -> Result<(), Reply> {
    if command.first().map(String::as_str) != Some("run") {
        return Ok(());
    }
    let resumed = match command.iter().any(|w| w == "--resume") {
        true => {
            let Some(r) = flag(command, "--resume")
                .and_then(|r| r.parse::<i64>().ok())
                .and_then(|id| {
                    nils_registry::pipeline::run(registry.store(), id)
                        .ok()
                        .flatten()
                })
            else {
                return Ok(());
            };
            if let Some(refused) = crate::pipelines::resume_refused(registry.store(), &r) {
                return Err(Reply::error(409, refused.message));
            }
            Some(r)
        }
        false => None,
    };
    let p = match &resumed {
        Some(r) => nils_registry::pipeline::get(registry.store(), r.pipeline_id)
            .ok()
            .flatten(),
        None => named(command)
            .and_then(|name| nils_registry::pipeline::resolve(registry.store(), name).ok())
            .flatten(),
    };
    let Some(p) = p else {
        return Ok(());
    };
    // 2026-10-12: a new run of a version a later one replaces is refused
    // at the door, naming the version that replaces it; only the keyboard
    // runs it still (`nils run <name>@<version> --replaced`). A run taken
    // up again goes on with the version it recorded.
    if resumed.is_none()
        && let Ok(s) = crate::versions::of_one(registry.store(), &p)
        && let Some(words) = s.refusal(&p.label())
    {
        let mut r = Reply::error(409, words);
        r.body["replaced"] = json!({
            "pipeline": p.label(),
            "replaced_by": s.replaced_by.as_ref().map(|(_, l)| l),
            "why": s.why,
            "current": s.current.as_ref().map(|(_, l)| l),
        });
        return Err(r);
    }
    let Ok(d) = nils_pipeline::descriptor::from_value(p.descriptor.clone()) else {
        return Ok(());
    };
    // record 55 T24: as the run sizes it, to this lane's cores
    let d = d.sized_to(crate::pipelines::lane_of(registry).ledger.cores);
    let handle = match &resumed {
        Some(r) => r.handle_id,
        None => flag(command, "--handle").and_then(|h| h.parse::<i64>().ok()),
    };
    let count = match handle {
        Some(h) if d.layout == Layout::Stacks => {
            crate::pipelines::handle_stacks(registry.store(), h)
                .ok()
                .map_or(1, |s| s.len())
        }
        _ => 1,
    };
    let all = checks(
        registry,
        home.dir(),
        &Asked {
            pipeline: &p,
            descriptor: &d,
            dcm2niix: Path::new(crate::tools::DCM2NIIX),
            units: count,
            detected: None,
            releases: resumed
                .as_ref()
                .is_none_or(|r| r.input_release_id.is_none()),
        },
    );
    let docs = || json!(all.iter().map(Check::doc).collect::<Vec<_>>());
    if let Some(failed) = refusing(&all) {
        let mut r = Reply::error(409, failed.refusal());
        r.body["failure"] = failed.cause.map_or(Value::Null, Cause::refusal_doc);
        r.body["requirements"] = docs();
        return Err(r);
    }
    if !units {
        return Ok(());
    }
    if let Some(refused) = crate::preflight::door_units(home, pack_dir, registry, command, &d) {
        let mut r = Reply::error(409, refused.words.clone());
        if let Value::Object(more) = refused.doc() {
            for (k, v) in more {
                r.body[k] = v;
            }
        }
        r.body["requirements"] = docs();
        return Err(r);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_s_subordinate_ids_are_found_by_its_name_or_its_uid() {
        let file = "anna:100000:65536\n1001:200000:65536\nempty:300000:0\n";
        assert!(has_subids(file, Some("anna"), 1000));
        assert!(has_subids(file, Some("bo"), 1001));
        assert!(!has_subids(file, Some("bo"), 1002));
        assert!(!has_subids(file, Some("empty"), 1003));
        assert!(!has_subids("", Some("anna"), 1000));
    }

    #[test]
    fn a_refusal_names_its_cause_for_the_job_that_keeps_only_its_words() {
        let c = Check::failed(
            "disk",
            Cause::Disk,
            "the units would write about 40 GB into the working place work, which has 2 GB free"
                .into(),
            json!({}),
        );
        let words = c.refusal();
        assert!(words.starts_with("refused before it started: the units would write"));
        assert_eq!(
            nils_pipeline::failure::cause_of(&nils_pipeline::failure::Said::error(&words)),
            Cause::Disk
        );
        assert_eq!(c.doc()["failure"]["kind"], "resource");
        assert_eq!(
            named(&[
                "run".into(),
                "--select".into(),
                "selection:a@1".into(),
                "n4".into()
            ]),
            Some("n4")
        );
    }
}
