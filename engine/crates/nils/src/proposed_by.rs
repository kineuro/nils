// SPDX-License-Identifier: AGPL-3.0-only

//! A run whose plan came from the assistant's proposal, accepted by a
//! person (record 57 P9 and P2). The assistant never sets a value itself:
//! a person accepts its proposal, and the run and its units record the
//! person, with the proposal noted. A run door takes `proposed_by`
//! `{agent, proposal}`; `nils run` and `nils workflow run` take
//! `--proposed-by AGENT --proposal ID`; the run records
//! `{agent, proposal, accepted_by}`, `accepted_by` always the verified
//! person the run is queued as, never the body.

use serde_json::{Value, json};

/// The most characters an agent's name or a proposal's id holds.
const MAX: usize = 200;

/// Who proposed the plan a person accepted, and which proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProposedBy {
    /// The assistant's purpose or name.
    pub agent: String,
    /// The assistant's id of the proposal the person accepted.
    pub proposal: String,
}

fn word(what: &str, v: &str) -> Result<String, String> {
    let t = v.trim();
    if t.is_empty() {
        return Err(format!("proposed_by.{what} is empty: name it"));
    }
    if t.chars().count() > MAX {
        return Err(format!(
            "proposed_by.{what} is over {MAX} characters: give the short name or id"
        ));
    }
    if t.starts_with('-') || t.chars().any(char::is_control) {
        return Err(format!(
            "proposed_by.{what} starts with - or holds a control character: give the name or id as the assistant keeps it"
        ));
    }
    Ok(t.to_string())
}

impl ProposedBy {
    /// From a door's body: `proposed_by` `{agent, proposal}`, both text;
    /// none where the body has no `proposed_by` (or it is null).
    pub(crate) fn of_body(body: &Value) -> Result<Option<ProposedBy>, String> {
        match &body["proposed_by"] {
            Value::Null => Ok(None),
            Value::Object(o) => {
                let text = |k: &str| match o.get(k) {
                    Some(Value::String(s)) => word(k, s),
                    _ => Err(format!(
                        "proposed_by is {{agent, proposal}}, both text: {k} is missing or not text"
                    )),
                };
                Ok(Some(ProposedBy {
                    agent: text("agent")?,
                    proposal: text("proposal")?,
                }))
            }
            _ => Err("proposed_by is {agent, proposal}, both text".into()),
        }
    }

    /// From the command line: `--proposed-by AGENT` and `--proposal ID`,
    /// both or neither.
    pub(crate) fn of_args(
        agent: Option<&str>,
        proposal: Option<&str>,
    ) -> Result<Option<ProposedBy>, String> {
        match (agent, proposal) {
            (None, None) => Ok(None),
            (Some(a), Some(p)) => Ok(Some(ProposedBy {
                agent: word("agent", a)?,
                proposal: word("proposal", p)?,
            })),
            _ => Err(
                "--proposed-by and --proposal go together: the assistant and the id of the proposal the person accepted"
                    .into(),
            ),
        }
    }

    /// As the command line takes it.
    pub(crate) fn args(&self) -> [String; 4] {
        [
            "--proposed-by".into(),
            self.agent.clone(),
            "--proposal".into(),
            self.proposal.clone(),
        ]
    }

    /// As the run and its units record it: the person who accepted it
    /// beside the proposal.
    pub(crate) fn record(&self, accepted_by: &str) -> Value {
        json!({"agent": self.agent, "proposal": self.proposal, "accepted_by": accepted_by})
    }
}

/// The line a run's text says of it: `proposed by <agent> (proposal
/// <id>), accepted by <person>`; none where the run records no proposal.
pub(crate) fn line(recorded: &Value) -> Option<String> {
    let agent = recorded["agent"].as_str()?;
    Some(format!(
        "proposed by {agent} (proposal {}), accepted by {}",
        recorded["proposal"].as_str().unwrap_or("-"),
        recorded["accepted_by"].as_str().unwrap_or("-")
    ))
}

/// A door's `proposed_by` on a command line the jobs door queues (`run`
/// or `workflow run`): appended as the flags, refused on any other verb
/// and where the command names the flags itself.
pub(crate) fn onto_command(body: &Value, command: &mut Vec<String>) -> Result<(), String> {
    let Some(p) = ProposedBy::of_body(body)? else {
        return Ok(());
    };
    let runs = command.first().is_some_and(|v| v == "run")
        || (command.first().is_some_and(|v| v == "workflow")
            && command.get(1).is_some_and(|v| v == "run"));
    if !runs {
        return Err(
            "proposed_by goes with a run: run <pipeline> or workflow run <workflow>".into(),
        );
    }
    if command
        .iter()
        .any(|w| w.starts_with("--proposed-by") || w.starts_with("--proposal"))
    {
        return Err(
            "proposed_by is given once: in the body, or as --proposed-by and --proposal in the command"
                .into(),
        );
    }
    command.extend(p.args());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_args_and_record() {
        assert_eq!(ProposedBy::of_body(&json!({})).unwrap(), None);
        assert_eq!(
            ProposedBy::of_body(&json!({"proposed_by": null})).unwrap(),
            None
        );
        let p = ProposedBy::of_body(
            &json!({"proposed_by": {"agent": " assistant ", "proposal": "c-12"}}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(p.agent, "assistant");
        assert_eq!(
            p.record("anna"),
            json!({"agent": "assistant", "proposal": "c-12", "accepted_by": "anna"})
        );
        assert_eq!(
            ProposedBy::of_args(Some("assistant"), Some("c-12")).unwrap(),
            Some(p.clone())
        );
        assert_eq!(ProposedBy::of_args(None, None).unwrap(), None);
        assert_eq!(
            line(&p.record("anna")).as_deref(),
            Some("proposed by assistant (proposal c-12), accepted by anna")
        );
        assert_eq!(line(&Value::Null), None);
        assert!(ProposedBy::of_args(Some("assistant"), None).is_err());
        for bad in [
            json!({"proposed_by": "assistant"}),
            json!({"proposed_by": {"agent": "assistant"}}),
            json!({"proposed_by": {"agent": "", "proposal": "c"}}),
            json!({"proposed_by": {"agent": "a", "proposal": 3}}),
            json!({"proposed_by": {"agent": "--json", "proposal": "c"}}),
            json!({"proposed_by": {"agent": "a\nb", "proposal": "c"}}),
            json!({"proposed_by": {"agent": "a", "proposal": "x".repeat(201)}}),
        ] {
            assert!(ProposedBy::of_body(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn onto_a_queued_command() {
        let body = json!({"proposed_by": {"agent": "assistant", "proposal": "c-1"}});
        let mut run = vec!["run".to_string(), "p@1".into()];
        onto_command(&body, &mut run).unwrap();
        assert_eq!(
            run,
            [
                "run",
                "p@1",
                "--proposed-by",
                "assistant",
                "--proposal",
                "c-1"
            ]
        );
        let mut wf = vec!["workflow".to_string(), "run".into(), "w@1".into()];
        onto_command(&body, &mut wf).unwrap();
        assert_eq!(wf.len(), 7);
        let mut other = vec!["classify".to_string()];
        assert!(onto_command(&body, &mut other).is_err());
        assert!(onto_command(&body, &mut run).is_err(), "given twice");
        let mut none = vec!["classify".to_string()];
        onto_command(&json!({}), &mut none).unwrap();
        assert_eq!(none, ["classify"]);
    }
}
