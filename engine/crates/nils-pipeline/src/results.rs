// SPDX-License-Identifier: AGPL-3.0-only

//! `results.json` (`contracts/job/v1/results.schema.json`): what a pipeline
//! says of its run, one entry per work unit, and the values it proposes. v0's
//! schema, read the way v0 read it (an object with `units`, or a bare list of
//! units), with the proposals beside, and since record 43 the cards of the
//! models the run used or made, the seeds it suggests and the selection it
//! suggests curating. A missing file is not an error: the runner then finds
//! each unit's files by the descriptor's templates.

use serde_json::Value;

/// What became of one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Succeeded,
    Failed,
    Skipped,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Succeeded => "succeeded",
            Status::Failed => "failed",
            Status::Skipped => "skipped",
        }
    }

    pub fn parse(text: &str) -> Option<Status> {
        Some(match text {
            "succeeded" => Status::Succeeded,
            "failed" => Status::Failed,
            "skipped" => Status::Skipped,
            _ => return None,
        })
    }
}

/// One unit as the pipeline reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub unit_id: String,
    pub status: Status,
    /// Files under the output folder, relative to it.
    pub derivatives: Vec<String>,
    pub metrics: Value,
    pub error: Option<String>,
}

/// One seed: a value suggested for a stack, for a person to curate
/// (record 43). Never a proposal: no model decides by it.
#[derive(Debug, Clone, PartialEq)]
pub struct Seed {
    pub stack_id: i64,
    pub axis: String,
    pub value: String,
    pub margin: Option<f64>,
    /// The whole entry as the pipeline wrote it.
    pub entry: Value,
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Results {
    pub units: Vec<Unit>,
    pub proposals: Vec<Value>,
    /// Model cards (`contracts/model/v1`) the run used or made.
    pub models: Vec<Value>,
    pub seeds: Vec<Seed>,
    /// The stacks the pipeline suggests curating.
    pub selection: Option<Vec<i64>>,
}

/// The file's name under the output folder.
pub const FILE: &str = "results.json";

/// Where the runner writes a run's seeds under its output folder, as the
/// one derivative of kind `seeds` (record 43).
pub const SEEDS_FILE: &str = "nils-seeds.json";

/// Parse the text of a `results.json`.
pub fn parse(text: &str) -> Result<Results, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("results.json is not JSON: {e}"))?;
    let list = |o: &serde_json::Map<String, Value>, key: &str| -> Result<Vec<Value>, String> {
        match o.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(a)) => Ok(a.clone()),
            Some(_) => Err(format!("results.json {key} is a list")),
        }
    };
    let (mut models, mut seeds, mut selection) = (Vec::new(), Vec::new(), None);
    let (units, proposals) = match &value {
        Value::Array(a) => (a.clone(), Vec::new()),
        Value::Object(o) => {
            if let Some(v) = o.get("schema_version")
                && v.as_str() != Some("1")
                && v.as_i64() != Some(1)
            {
                return Err(format!("results.json schema_version is 1, not {v}"));
            }
            let units = match o.get("units") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(a)) => a.clone(),
                Some(_) => return Err("results.json units is a list".into()),
            };
            let proposals = list(o, "proposals")?;
            models = list(o, "models")?;
            if models.iter().any(|m| !m.is_object()) {
                return Err("results.json models are model cards, objects".into());
            }
            for (i, e) in list(o, "seeds")?.into_iter().enumerate() {
                let at = format!("results.json seeds[{i}]");
                let stack_id = e["stack_id"]
                    .as_i64()
                    .filter(|s| *s > 0)
                    .ok_or_else(|| format!("{at}.stack_id is a stack's id"))?;
                let axis = e["axis"]
                    .as_str()
                    .filter(|a| {
                        !a.is_empty() && a.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                    })
                    .ok_or_else(|| format!("{at}.axis is an axis of the pack"))?
                    .to_string();
                let value = e["value"]
                    .as_str()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| format!("{at}.value is the value suggested"))?
                    .to_string();
                let margin = match &e["margin"] {
                    Value::Null => None,
                    m => Some(
                        m.as_f64()
                            .ok_or_else(|| format!("{at}.margin is a number"))?,
                    ),
                };
                seeds.push(Seed {
                    stack_id,
                    axis,
                    value,
                    margin,
                    entry: e,
                });
            }
            selection = match o.get("selection") {
                None | Some(Value::Null) => None,
                Some(s) => Some(
                    s["stacks"]
                        .as_array()
                        .ok_or("results.json selection names its stacks")?
                        .iter()
                        .map(|v| {
                            v.as_i64()
                                .filter(|i| *i > 0)
                                .ok_or("results.json selection.stacks are stack ids")
                        })
                        .collect::<Result<Vec<i64>, _>>()?,
                ),
            };
            (units, proposals)
        }
        _ => return Err("results.json is an object or a list of units".into()),
    };
    let mut out = Results {
        units: Vec::with_capacity(units.len()),
        proposals,
        models,
        seeds,
        selection,
    };
    for (i, u) in units.iter().enumerate() {
        let unit_id = match &u["unit_id"] {
            Value::String(s) if !s.is_empty() => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => return Err(format!("results.json units[{i}].unit_id is required")),
        };
        let status_text = u["status"].as_str().unwrap_or_default();
        let status = Status::parse(status_text).ok_or_else(|| {
            format!(
                "results.json units[{i}].status is succeeded, failed or skipped, not {}",
                if status_text.is_empty() {
                    "absent"
                } else {
                    status_text
                }
            )
        })?;
        let derivatives = match &u["derivatives"] {
            Value::Null => Vec::new(),
            Value::Array(a) => a
                .iter()
                .map(|d| {
                    d.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("results.json units[{i}].derivatives are paths"))
                })
                .collect::<Result<_, _>>()?,
            _ => return Err(format!("results.json units[{i}].derivatives is a list")),
        };
        out.units.push(Unit {
            unit_id,
            status,
            derivatives,
            metrics: match &u["metrics"] {
                Value::Null => Value::Object(Default::default()),
                m => m.clone(),
            },
            error: u["error"].as_str().map(str::to_string),
        });
    }
    Ok(out)
}

/// The unit a results entry names, in the runner's own spelling: `sub-a`,
/// `sub-a_ses-b` or `stack-12`; `sub-a/ses-b` and a bare stack id are read
/// too.
pub fn normalise(unit_id: &str) -> String {
    let t = unit_id.trim().trim_end_matches('/');
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        return format!("stack-{t}");
    }
    t.replacen("/ses-", "_ses-", 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_s_schema_and_a_bare_list_both_read() {
        let r = parse(
            r#"{"schema_version": "1", "units": [
                {"unit_id": "sub-01/ses-a", "work_unit": "session", "status": "succeeded", "derivatives": ["x.nii.gz"], "metrics": {"snr": 3}},
                {"unit_id": 12, "status": "failed", "error": "no T1w"}
            ], "proposals": [{"axis": "body_part", "value": "brain", "stack_id": 12}]}"#,
        )
        .unwrap();
        assert_eq!(r.units.len(), 2);
        assert_eq!(r.units[0].metrics["snr"], 3);
        assert_eq!(r.units[1].status, Status::Failed);
        assert_eq!(r.units[1].error.as_deref(), Some("no T1w"));
        assert_eq!(r.proposals.len(), 1);
        assert_eq!(normalise(&r.units[0].unit_id), "sub-01_ses-a");
        assert_eq!(normalise(&r.units[1].unit_id), "stack-12");
        let bare = parse(r#"[{"unit_id": "sub-01", "status": "skipped"}]"#).unwrap();
        assert_eq!(bare.units[0].status, Status::Skipped);
        assert!(parse(r#"{"units": [{"unit_id": "a", "status": "done"}]}"#).is_err());
        assert!(parse(r#"{"schema_version": "2"}"#).is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn seeds_and_a_selection_are_read_apart_from_the_proposals() {
        let r = parse(
            r#"{"units": [], "models": [{"name": "e"}],
                "seeds": [{"stack_id": 4, "axis": "body_part", "value": "brain", "margin": 0.2, "source": "zero_shot"}],
                "selection": {"stacks": [4, 9]}}"#,
        )
        .unwrap();
        assert!(r.proposals.is_empty());
        assert_eq!(r.models.len(), 1);
        assert_eq!(r.seeds[0].stack_id, 4);
        assert_eq!(r.seeds[0].margin, Some(0.2));
        assert_eq!(r.seeds[0].entry["source"], "zero_shot");
        assert_eq!(r.selection, Some(vec![4, 9]));
        assert!(parse(r#"{"seeds": [{"stack_id": 4, "axis": "Body", "value": "x"}]}"#).is_err());
        assert!(parse(r#"{"selection": {"stacks": ["a"]}}"#).is_err());
    }
}
