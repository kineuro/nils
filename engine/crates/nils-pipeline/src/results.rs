// SPDX-License-Identifier: AGPL-3.0-only

//! `results.json` (`contracts/job/v1/results.schema.json`): what a pipeline
//! says of its run, one entry per work unit, and the values it proposes. v0's
//! schema, read the way v0 read it (an object with `units`, or a bare list of
//! units), with the proposals beside. A missing file is not an error: the
//! runner then finds each unit's files by the descriptor's templates.

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

/// The whole file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Results {
    pub units: Vec<Unit>,
    pub proposals: Vec<Value>,
}

/// The file's name under the output folder.
pub const FILE: &str = "results.json";

/// Parse the text of a `results.json`.
pub fn parse(text: &str) -> Result<Results, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("results.json is not JSON: {e}"))?;
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
            let proposals = match o.get("proposals") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(a)) => a.clone(),
                Some(_) => return Err("results.json proposals is a list".into()),
            };
            (units, proposals)
        }
        _ => return Err("results.json is an object or a list of units".into()),
    };
    let mut out = Results {
        units: Vec::with_capacity(units.len()),
        proposals,
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
}
