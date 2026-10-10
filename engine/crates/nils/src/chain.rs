// SPDX-License-Identifier: AGPL-3.0-only

//! The chain (record 26 §7): `POST /api/jobs` and `nils bring-in` may
//! queue a command with `then`, the commands queued one after another when
//! the one before ends done, under the principal, grants and detail
//! recorded on the first. The worker queues the next step; a step the
//! person may not queue stops the chain, and the job's result says which
//! step and why. `bring-in @dataset` is sugar for the whole thread of a
//! dataset: pseudonymise, then digest, fingerprint and classify, the
//! digest sharing the pseudonymise step's batch name, and since record 55
//! (H2) a pick run for the dataset where it feeds a cohort and the pack
//! declares picks.

use nils_registry::job::{self, Job};
use nils_registry::place::Place;
use nils_registry::store::Store;
use serde_json::{Value, json};

use crate::grants::{Access, Detail};

/// The commands of `then`, as a body gives them: each a list of words, or
/// one string split on whitespace.
pub(crate) fn parse_then(doc: &Value) -> Result<Vec<Vec<String>>, String> {
    let Some(list) = doc.get("then") else {
        return Ok(Vec::new());
    };
    if list.is_null() {
        return Ok(Vec::new());
    }
    let Some(list) = list.as_array() else {
        return Err("then: a list of command lines, each a list of words".into());
    };
    let mut out = Vec::with_capacity(list.len());
    for (i, command) in list.iter().enumerate() {
        let words: Vec<String> = match command {
            Value::Array(words) => words
                .iter()
                .map(|w| {
                    w.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("then[{i}]: a command line is a list of words"))
                })
                .collect::<Result<_, _>>()?,
            Value::String(line) => line.split_whitespace().map(str::to_string).collect(),
            _ => return Err(format!("then[{i}]: a command line is a list of words")),
        };
        if words.is_empty() {
            return Err(format!("then[{i}]: an empty command line"));
        }
        out.push(words);
    }
    Ok(out)
}

/// Record 55 H2: the subjects of a dataset, whose files a digest read from
/// its pseudonymised tree: what a pick run for the dataset decides.
pub(crate) fn dataset_subjects(
    store: &mut Store,
    place: &Place,
) -> Result<std::collections::BTreeSet<i64>, nils_registry::store::Error> {
    use nils_registry::schema::Type;
    use nils_registry::store::Param;
    let tree = place
        .tree_path("anon")
        .unwrap_or_else(|| std::path::PathBuf::from(&place.path));
    let real = std::fs::canonicalize(&tree).unwrap_or(tree);
    let sql = format!(
        "SELECT id, root_canonical FROM {}",
        store.qualified("source")
    );
    let sources: Vec<i64> = store
        .query(&sql, &[])?
        .iter()
        .filter_map(|r| {
            let root = r.text(1).ok()?;
            std::path::Path::new(root)
                .starts_with(&real)
                .then(|| r.int(0).ok())
                .flatten()
        })
        .collect();
    let sql = format!(
        "SELECT DISTINCT se.subject_id FROM {} sf \
         JOIN {} i ON i.id = sf.instance_id \
         JOIN {} se ON se.id = i.series_id \
         WHERE sf.source_id = {} AND sf.status <> 'gone' AND se.subject_id IS NOT NULL",
        store.qualified("source_file"),
        store.qualified("instance"),
        store.qualified("series"),
        store.dialect().param(1, Type::Int)
    );
    let mut out = std::collections::BTreeSet::new();
    for source in sources {
        for r in store.query(&sql, &[Param::Int(source)])? {
            out.insert(r.int(0)?);
        }
    }
    Ok(out)
}

/// Whether the pack of that name, in that directory, declares picks.
pub(crate) fn pack_has_picks(dir: Option<&std::path::Path>, name: &str) -> bool {
    let Some(dir) = dir else {
        return false;
    };
    crate::packs_in(dir)
        .ok()
        .and_then(|found| {
            found
                .into_iter()
                .find(|p| p.file_name().is_some_and(|f| *f == *name))
        })
        .and_then(|p| nils_pack::load(&p, None).ok())
        .is_some_and(|p| !p.picks.is_empty())
}

/// The pick run a bring-in ends with (record 55 H2): the dataset's subjects
/// decided, when the dataset feeds a cohort and the pack declares picks,
/// which is what says the cohort's sessions have roles to fill.
pub(crate) fn pick_step(place: &Place, pack: Option<&str>) -> Vec<String> {
    let mut pick = vec![
        "pick".to_string(),
        "run".to_string(),
        "--dataset".to_string(),
        place.name.clone(),
    ];
    if let Some(p) = pack {
        pick.extend(["--pack".to_string(), p.to_string()]);
    }
    pick
}

/// What `bring-in @dataset [--name N] [--pack P]` stands for: the first
/// command and the ones after it. An identified dataset is pseudonymised
/// first; any other has no first step and starts with the digest. The
/// digest is named as the pseudonymise step is, so the two batches are one
/// thread (record 26 §14); the name is the dataset's and today's date
/// unless given. With `digest_first`, a digest of the tree goes before
/// the pseudonymise step: the tree holds files no digest has read, as a
/// v0 folder does on its first bring-in, and the pseudonymiser leaves an
/// original the tree holds already as it is only once the registry knows
/// the tree's files (lab 26, defect 7).
pub(crate) fn bring_in(
    place: &Place,
    name: Option<&str>,
    pack: Option<&str>,
    digest_first: bool,
    picks: bool,
) -> (Vec<String>, Vec<Vec<String>>) {
    let at = format!("@{}", place.name);
    let name = name
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}-{}", place.name, nils_registry::time::today()));
    let named = |verb: &str| vec![verb.to_string(), at.clone(), "--name".into(), name.clone()];
    let mut classify = vec!["classify".to_string()];
    if let Some(p) = pack {
        classify.extend(["--pack".to_string(), p.to_string()]);
    }
    let identified = place.dataset["arrives"].as_str() == Some("identified");
    let mut steps = Vec::new();
    if identified {
        if digest_first {
            steps.push(named("digest"));
        }
        steps.push(named("pseudonymize"));
    }
    steps.push(named("digest"));
    steps.push(vec!["fingerprint".to_string()]);
    steps.push(classify);
    // record 55 H2: a dataset that feeds a cohort has its sessions' roles
    // picked as it comes in, when the pack declares picks
    // record 55 H2 (round 4): unless the dataset turned picking after a
    // sort off
    if picks
        && place.dataset["cohort"].is_string()
        && nils_registry::place::picks_after_sort(&place.dataset)
    {
        steps.push(pick_step(place, pack));
    }
    let first = steps.remove(0);
    (first, steps)
}

/// How many files of the dataset's pseudonymised tree no digest has read,
/// when there are any: the tree's files, counted as a probe counts them,
/// against the registry's rows for the tree. None for a dataset that is
/// not pseudonymised, or whose tree the registry has read whole; a count
/// that stopped short says nothing unless it already passed the rows.
pub(crate) fn unread_in_tree(store: &mut Store, place: &Place) -> Option<u64> {
    if place.dataset["arrives"].as_str() != Some("identified") {
        return None;
    }
    let anon = place.tree_path("anon")?;
    let counted = crate::dataset::count(&anon);
    let files = counted["files"].as_u64().unwrap_or(0);
    if files == 0 {
        return None;
    }
    let canonical = std::fs::canonicalize(&anon).ok()?;
    let sql = format!(
        "SELECT COUNT(*) FROM {} f JOIN {} s ON s.id = f.source_id \
         WHERE s.root_canonical = {} AND f.status <> 'gone'",
        store.qualified("source_file"),
        store.qualified("source"),
        store.dialect().param(1, nils_registry::schema::Type::Text)
    );
    let rows = store
        .query_opt(
            &sql,
            &[nils_registry::store::Param::from(
                canonical.display().to_string(),
            )],
        )
        .ok()
        .flatten()
        .and_then(|r| r.int(0).ok())
        .unwrap_or(0)
        .max(0) as u64;
    (files > rows).then_some(files - rows)
}

/// The arguments of `bring-in`, read from its command line: the dataset,
/// and the name and pack if given.
#[derive(Debug)]
pub(crate) struct BringIn {
    pub(crate) dataset: String,
    pub(crate) name: Option<String>,
    pub(crate) pack: Option<String>,
}

impl BringIn {
    pub(crate) fn parse(command: &[String]) -> Result<BringIn, String> {
        let mut dataset = None;
        let mut name = None;
        let mut pack = None;
        let mut rest = command.iter().skip(1);
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--name" => {
                    name = Some(rest.next().ok_or("--name takes a name")?.to_string());
                }
                "--pack" => {
                    pack = Some(rest.next().ok_or("--pack takes a pack")?.to_string());
                }
                a if a.starts_with('@') && dataset.is_none() => {
                    dataset = Some(a.trim_start_matches('@').to_string());
                }
                other => {
                    return Err(format!(
                        "bring-in takes @dataset, --name N and --pack P, not {other}"
                    ));
                }
            }
        }
        Ok(BringIn {
            dataset: dataset.ok_or("bring-in names a dataset as @name")?,
            name,
            pack,
        })
    }
}

/// What a chain step needs of the caller who queued the first job, as the
/// job recorded them: the grants and the detail. A job queued from the
/// keyboard recorded every grant and detail sensitive.
fn recorded_access(job: &Job) -> Access {
    let mut access = Access::default();
    if let Some(grants) = job.args["grants"].as_array() {
        for g in grants.iter().filter_map(Value::as_str) {
            if let Some(named) = Access::named(g) {
                access.add(&named);
            }
        }
    }
    access.detail = Detail::of_job(&job.args);
    access
}

/// A job ended done: the first command of its `then` is queued under the
/// principal, grants, detail and actor the job recorded, with the rest of
/// the chain as its own `then`. A step the recorded grants do not reach
/// stops the chain, and the job's result says which step and why. Answers
/// the queued job's id, or none when the chain ends here.
pub(crate) fn continue_chain(store: &mut Store, job: &Job) -> Result<Option<i64>, String> {
    let mut then = job.then();
    // record 55 H2 (round 4): a sort ends with picking main scans for the
    // subjects it judged, a pipeline step of its own
    if let Some(step) = crate::pick_after::step_after(store, job) {
        then.insert(0, step);
    }
    // Wave 7a (2026-10-10): a read that added, changed and removed nothing
    // leaves nothing to sort, so the sorting steps after it are not queued
    // (a sort judges every stack, and its pick every subject it judged);
    // the read's result says which were left and why
    if job.kind == "digest" && read_nothing(job) {
        let skipped: Vec<Vec<String>> = then.iter().filter(|s| sorting(s)).cloned().collect();
        if !skipped.is_empty() {
            then.retain(|s| !sorting(s));
            let mut result = job.result.clone().unwrap_or_else(|| json!({}));
            if !result.is_object() {
                result = json!({ "result": result });
            }
            result["chain_ended"] = json!({
                "why": "the read added, changed and removed no file and no stack, so there is nothing new to sort",
                "skipped": skipped,
            });
            job::set_result(store, job.id, &result).map_err(|e| e.to_string())?;
        }
    }
    if then.is_empty() {
        return Ok(None);
    }
    let next = then.remove(0);
    let access = recorded_access(job);
    let why = match crate::serve::verb_needs(&next) {
        None => Some(format!(
            "{} is not a verb the chain queues",
            next.first().map(String::as_str).unwrap_or("")
        )),
        Some((grant, _)) if !access.holds(grant) => Some(format!(
            "the {grant} grant, which the caller who queued the chain does not hold"
        )),
        Some((_, detail)) if access.detail < detail => Some(format!(
            "detail {}, and the chain was queued at {}",
            detail.name(),
            access.detail.name()
        )),
        Some(_) => None,
    };
    if let Some(why) = why {
        let mut result = job.result.clone().unwrap_or_else(|| json!({}));
        if !result.is_object() {
            result = json!({ "result": result });
        }
        result["chain_stopped"] = json!({ "step": next, "why": why });
        job::set_result(store, job.id, &result).map_err(|e| e.to_string())?;
        return Ok(None);
    }
    let extra = json!({
        "detail": access.detail.name(),
        "grants": job.args["grants"],
        "actor": job.args["actor"],
        "then": then,
        "chain_before": job.id,
    });
    let id = job::enqueue_with(store, &next, job.name.as_deref(), job.principal(), extra)
        .map_err(|e| e.to_string())?;
    job::set_arg(store, job.id, "chain_after", json!(id)).map_err(|e| e.to_string())?;
    Ok(Some(id))
}

/// A step of a chain that sorts what a read brought: the fingerprints, the
/// sort, the pick after it, and the pictures of what was sorted.
fn sorting(step: &[String]) -> bool {
    matches!(
        step.first().map(String::as_str),
        Some("fingerprint" | "classify" | "pick" | "pyramid" | "preview")
    )
}

/// Whether a digest that ended done read nothing a sort would judge anew:
/// no file added, changed or gone, and no stack made, emptied or folded.
/// The digest keeps its counts as its progress (and a result, where one is
/// written); a count it does not give makes this false, so a chain whose
/// read cannot be judged goes on as before.
fn read_nothing(job: &Job) -> bool {
    const MOVED: [&str; 6] = [
        "ingested",
        "changed",
        "gone",
        "stacks_created",
        "empty_stacks_removed",
        "echo_stacks_folded",
    ];
    let counts = [job.result.as_ref(), job.progress.as_ref()]
        .into_iter()
        .flatten()
        .find(|c| c.get("ingested").is_some_and(Value::is_u64));
    counts.is_some_and(|c| {
        MOVED
            .iter()
            .all(|k| c.get(*k).and_then(Value::as_u64).is_none_or(|n| n == 0))
            && ["ingested", "changed", "gone", "stacks_created"]
                .iter()
                .all(|k| c.get(*k).is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_registry::place::Role;

    fn place(arrives: &str) -> Place {
        Place {
            id: 1,
            name: "scans".into(),
            role: Role::Source,
            path: "/data/scans".into(),
            guarantees: json!({}),
            probed: Value::Null,
            probed_at: None,
            created_at: "t".into(),
            updated_at: None,
            retired_at: None,
            handling: Value::Null,
            dataset: json!({"arrives": arrives}),
        }
    }

    /// A digest that ended done with these counts, as its progress or its result.
    fn digest(progress: Option<Value>, result: Option<Value>) -> Job {
        Job {
            id: 7,
            kind: "digest".into(),
            name: Some("ds-1".into()),
            state: job::State::Done,
            pid: None,
            host: None,
            started_at: "t".into(),
            heartbeat_at: None,
            finished_at: None,
            progress,
            error: None,
            args: json!({}),
            result,
        }
    }

    #[test]
    fn a_read_that_brought_nothing_new_leaves_nothing_to_sort() {
        // a read of 2026-10-10 whose every file the registry held already,
        // through another dataset
        let known = json!({
            "ingested": 0, "duplicate": 7328, "changed": 0, "gone": 0, "held": 0,
            "stacks_created": 0, "empty_stacks_removed": 0, "echo_stacks_folded": 0,
            "identities_attached": 7,
        });
        assert!(read_nothing(&digest(Some(known.clone()), None)));
        // the counts are read from a result too, where a digest wrote one
        assert!(read_nothing(&digest(None, Some(known.clone()))));
        // anything a sort would judge anew keeps the chain going
        for moved in [
            "ingested",
            "changed",
            "gone",
            "stacks_created",
            "empty_stacks_removed",
            "echo_stacks_folded",
        ] {
            let mut c = known.clone();
            c[moved] = json!(1);
            assert!(!read_nothing(&digest(Some(c), None)), "{moved}");
        }
        // a read whose counts are not known goes on as before
        assert!(!read_nothing(&digest(None, None)));
        assert!(!read_nothing(&digest(Some(json!({"batch_id": 3})), None)));
        assert!(!read_nothing(&digest(Some(json!({"ingested": 0})), None)));
    }

    #[test]
    fn the_sorting_steps_are_the_fingerprints_the_sort_the_pick_and_the_pictures() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for step in [
            &["fingerprint"][..],
            &["classify", "--pack", "mri"],
            &["pick", "run", "--after-sort", "4"],
            &["pyramid", "build"],
            &["preview", "build"],
        ] {
            assert!(sorting(&words(step)), "{step:?}");
        }
        for step in [&["backup"][..], &["release", "r1"], &["digest", "@ds"]] {
            assert!(!sorting(&words(step)), "{step:?}");
        }
    }

    #[test]
    fn then_is_a_list_of_command_lines_in_either_spelling() {
        assert_eq!(parse_then(&json!({})).unwrap(), Vec::<Vec<String>>::new());
        assert_eq!(parse_then(&json!({"then": null})).unwrap().len(), 0);
        let then = parse_then(
            &json!({"then": [["digest", "@ds"], "fingerprint", ["classify", "--pack", "mri"]]}),
        )
        .unwrap();
        assert_eq!(then.len(), 3);
        assert_eq!(then[0], ["digest", "@ds"]);
        assert_eq!(then[1], ["fingerprint"]);
        assert_eq!(then[2], ["classify", "--pack", "mri"]);
        assert!(parse_then(&json!({"then": "digest"})).is_err());
        assert!(parse_then(&json!({"then": [[]]})).is_err());
        assert!(parse_then(&json!({"then": [[1]]})).is_err());
    }

    #[test]
    fn bring_in_is_the_thread_of_a_dataset() {
        let (first, then) = bring_in(
            &place("identified"),
            Some("batch-1"),
            Some("mri"),
            false,
            true,
        );
        assert_eq!(first, ["pseudonymize", "@scans", "--name", "batch-1"]);
        assert_eq!(then.len(), 3);
        assert_eq!(then[0], ["digest", "@scans", "--name", "batch-1"]);
        assert_eq!(then[1], ["fingerprint"]);
        assert_eq!(then[2], ["classify", "--pack", "mri"]);
        let (first, then) = bring_in(&place("deidentified"), None, None, false, true);
        assert_eq!(first[0], "digest");
        assert!(first[3].starts_with("scans-20"), "{first:?}");
        assert_eq!(then.len(), 2);
        assert_eq!(then[1], ["classify"]);
        let (first, _) = bring_in(&place("coded"), None, None, false, true);
        assert_eq!(first[0], "digest");
        // a tree with files no digest has read: the digest goes first
        let (first, then) = bring_in(&place("identified"), Some("v0"), None, true, true);
        assert_eq!(first, ["digest", "@scans", "--name", "v0"]);
        assert_eq!(then[0], ["pseudonymize", "@scans", "--name", "v0"]);
        assert_eq!(then[1], ["digest", "@scans", "--name", "v0"]);
        assert_eq!(then.len(), 4);
        // never for a dataset with no pseudonymise step
        let (first, then) = bring_in(&place("deidentified"), None, None, true, true);
        assert_eq!(first[0], "digest");
        assert_eq!(then.len(), 2);
        // record 55 H2: a dataset that feeds a cohort ends with a pick run
        // for the dataset, where the pack declares picks
        let mut fed = place("identified");
        fed.dataset["cohort"] = json!("study-a");
        let (_, then) = bring_in(&fed, Some("b"), Some("mri"), false, true);
        assert_eq!(then.len(), 4);
        assert_eq!(
            then[3],
            ["pick", "run", "--dataset", "scans", "--pack", "mri"]
        );
        let (_, then) = bring_in(&fed, Some("b"), None, false, false);
        assert_eq!(then.len(), 3, "a pack with no picks: no pick run");
        let (_, then) = bring_in(&place("identified"), Some("b"), None, false, true);
        assert_eq!(then.len(), 3, "no cohort: no pick run");
        // record 55 H2 (round 4): a dataset whose picks are off has none
        let mut off = fed.clone();
        off.dataset["picks"] = json!("off");
        let (_, then) = bring_in(&off, Some("b"), Some("mri"), false, true);
        assert_eq!(then.len(), 3, "picks off: no pick run");
    }

    #[test]
    fn the_bring_in_line_is_read_and_refused_with_its_words() {
        let words = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        let b = BringIn::parse(&words("bring-in @ds --name x --pack mri")).unwrap();
        assert_eq!(
            (b.dataset.as_str(), b.name.as_deref(), b.pack.as_deref()),
            ("ds", Some("x"), Some("mri"))
        );
        assert!(
            BringIn::parse(&words("bring-in"))
                .unwrap_err()
                .contains("@name")
        );
        assert!(
            BringIn::parse(&words("bring-in @ds --dry-run"))
                .unwrap_err()
                .contains("--dry-run")
        );
    }
}
