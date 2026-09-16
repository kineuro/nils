// SPDX-License-Identifier: AGPL-3.0-only

//! The chain (record 26 §7): `POST /api/jobs` and `nils bring-in` may
//! queue a command with `then`, the commands queued one after another when
//! the one before ends done, under the principal, grants and detail
//! recorded on the first. The worker queues the next step; a step the
//! person may not queue stops the chain, and the job's result says which
//! step and why. `bring-in @dataset` is sugar for the whole thread of a
//! dataset: pseudonymise, then digest, fingerprint and classify, the
//! digest sharing the pseudonymise step's batch name.

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
        let (first, then) = bring_in(&place("identified"), Some("batch-1"), Some("mri"), false);
        assert_eq!(first, ["pseudonymize", "@scans", "--name", "batch-1"]);
        assert_eq!(then.len(), 3);
        assert_eq!(then[0], ["digest", "@scans", "--name", "batch-1"]);
        assert_eq!(then[1], ["fingerprint"]);
        assert_eq!(then[2], ["classify", "--pack", "mri"]);
        let (first, then) = bring_in(&place("deidentified"), None, None, false);
        assert_eq!(first[0], "digest");
        assert!(first[3].starts_with("scans-20"), "{first:?}");
        assert_eq!(then.len(), 2);
        assert_eq!(then[1], ["classify"]);
        let (first, _) = bring_in(&place("coded"), None, None, false);
        assert_eq!(first[0], "digest");
        // a tree with files no digest has read: the digest goes first
        let (first, then) = bring_in(&place("identified"), Some("v0"), None, true);
        assert_eq!(first, ["digest", "@scans", "--name", "v0"]);
        assert_eq!(then[0], ["pseudonymize", "@scans", "--name", "v0"]);
        assert_eq!(then[1], ["digest", "@scans", "--name", "v0"]);
        assert_eq!(then.len(), 4);
        // never for a dataset with no pseudonymise step
        let (first, then) = bring_in(&place("deidentified"), None, None, true);
        assert_eq!(first[0], "digest");
        assert_eq!(then.len(), 2);
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
