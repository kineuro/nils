// SPDX-License-Identifier: AGPL-3.0-only

//! What a release calls each subject after `sub-`.
//!
//! A release names a subject by its code unless it is told to name by ID
//! types: an ordered list, in which the first type the subject holds a kept
//! value of gives the name, and the code is the last fallback. The plan here
//! is worked out before a release writes anything, and it is the whole
//! decision: the run only reads it.
//!
//! Three things stop a plan, and each is listed rather than guessed at. A
//! subject that holds several values of the type that would name it needs a
//! person to say which. Two subjects whose names would be equal would share
//! a folder. And a type that keeps no value can name nobody, so asking for
//! it is refused by name.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use nils_registry::Registry;
use nils_registry::linkage::{self, Subkeys};
use nils_registry::personnummer;
use nils_registry::schema::Type;
use nils_registry::store::{Param, Store};

use crate::run::Error;

/// How many subjects a refusal names before it counts the rest: a refusal is
/// read by a person, and the plan lists them all.
const LISTED: usize = 20;

/// A value as a `sub-` label may spell it: BIDS allows letters and digits
/// there and nothing else, so everything else goes.
pub fn clean(value: &str) -> String {
    value.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// A person's word on which value names a subject that holds several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The subject's code.
    pub subject: String,
    /// The value, as kept or already cleaned: it is compared after cleaning.
    pub value: String,
}

impl Choice {
    /// `CODE=VALUE`, as the command line takes one.
    pub fn parse(text: &str) -> Result<Choice, String> {
        split(text, &['='])
    }
}

fn split(text: &str, separators: &[char]) -> Result<Choice, String> {
    let (subject, value) = text
        .split_once(separators)
        .map(|(a, b)| (a.trim(), b.trim()))
        .unwrap_or((text.trim(), ""));
    if subject.is_empty() || value.is_empty() {
        return Err(
            "a choice is a subject code and the value that names it, written CODE=VALUE"
                .to_string(),
        );
    }
    Ok(Choice {
        subject: subject.to_string(),
        value: value.to_string(),
    })
}

/// The choices of a file: one a line, the subject code, then a tab, a comma
/// or an equals sign, then the value. An empty line and a line that starts
/// with `#` are skipped. A line that is neither is named by its number and
/// never quoted, since it may hold an identifier.
pub fn choices_of_file(text: &str) -> Result<Vec<Choice>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.push(split(line, &['\t', ',', '=']).map_err(|e| format!("line {}: {e}", n + 1))?);
    }
    Ok(out)
}

/// What one subject is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// The label after `sub-`.
    pub name: String,
    /// The type the name came from, or none where it is the subject's code.
    pub id_type: Option<String>,
    /// Whether a person chose the value among several.
    pub chosen: bool,
}

/// One type of the list and how many subjects it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeCount {
    pub name: String,
    /// Whether an ID of the type identifies a person wherever it is met.
    pub identifying: bool,
    pub subjects: i64,
}

/// A subject that holds several values of the type that would name it, and
/// no choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedsChoice {
    pub subject_id: i64,
    pub subject: String,
    pub id_type: String,
    /// The values as kept, each once. Never in a refusal's words.
    pub values: Vec<String>,
}

/// A name two or more subjects would share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub name: String,
    /// Each subject's code and the type that named it, none for its code.
    pub subjects: Vec<(String, Option<String>)>,
}

/// How a release names its subjects, worked out before it writes anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// The types, in the order the release gave them.
    pub by: Vec<String>,
    pub types: Vec<TypeCount>,
    /// The subjects the selection reaches.
    pub subjects: i64,
    /// Those no type named, which keep their code.
    pub code: i64,
    /// Those a person's choice named, given now or kept from a version before.
    pub chosen: i64,
    /// The subjects under whose types a value shaped like a personnummer was
    /// met and passed over, by code.
    pub passed_over: Vec<String>,
    pub needs_choice: Vec<NeedsChoice>,
    pub collisions: Vec<Collision>,
    /// The name of every subject that has one, by subject id.
    pub names: HashMap<i64, Named>,
}

impl Plan {
    /// Whether a release under this plan may start.
    pub fn ready(&self) -> bool {
        self.needs_choice.is_empty() && self.collisions.is_empty()
    }

    /// Why a release under this plan does not start, in words that name
    /// subjects by their codes and never spell a value or a name; none
    /// where it may start.
    pub fn refusal(&self) -> Option<String> {
        if self.ready() {
            return None;
        }
        let mut words = format!(
            "the release names its subjects by {} and cannot name them all.",
            self.by.join(", then ")
        );
        if !self.needs_choice.is_empty() {
            let listed: Vec<String> = self
                .needs_choice
                .iter()
                .take(LISTED)
                .map(|n| format!("{} ({} of {})", n.subject, n.values.len(), n.id_type))
                .collect();
            words.push_str(&format!(
                " {} hold several values of the type that would name them: {}{}. Say which \
                 value names each (--name-choice CODE=VALUE or a --name-choices file on the \
                 command line, name_choices in a request); the plan lists the values \
                 (nils release --name-plan, POST /api/releases/names).",
                subjects_word(self.needs_choice.len()),
                listed.join(", "),
                and_more(self.needs_choice.len()),
            ));
        }
        if !self.collisions.is_empty() {
            let listed: Vec<String> = self
                .collisions
                .iter()
                .take(LISTED)
                .map(|c| {
                    c.subjects
                        .iter()
                        .map(|(code, by)| match by {
                            Some(t) => format!("{code} (by {t})"),
                            None => format!("{code} (by its code)"),
                        })
                        .collect::<Vec<_>>()
                        .join(" and ")
                })
                .collect();
            words.push_str(&format!(
                " {} would be shared once cleaned to letters and digits, so two subjects \
                 would be written into one folder: {}{}. Correct the values, name by another \
                 type first, or release the subjects apart.",
                if self.collisions.len() == 1 {
                    "1 name".to_string()
                } else {
                    format!("{} names", self.collisions.len())
                },
                listed.join("; "),
                and_more(self.collisions.len()),
            ));
        }
        words.push_str(" Nothing was written.");
        Some(words)
    }

    /// The counts, for a report, a release row's reader and the audit row:
    /// the types, how many subjects each named, how many kept their code.
    /// No value and no name.
    pub fn counts(&self) -> serde_json::Value {
        serde_json::json!({
            "by": self.by,
            "subjects": self.subjects,
            "types": self.types.iter().map(|t| serde_json::json!({
                "type": t.name,
                "subjects": t.subjects,
            })).collect::<Vec<_>>(),
            "code": self.code,
            "chosen": self.chosen,
            "passed_over": self.passed_over.len(),
        })
    }

    /// The plan as a rehearsal answers it. With `values` the subjects that
    /// need a choice come with the values to choose among and a collision
    /// with the name; without, both are left out and what stays is counts
    /// and subject codes.
    pub fn doc(&self, values: bool) -> serde_json::Value {
        let needs: Vec<serde_json::Value> = self
            .needs_choice
            .iter()
            .map(|n| {
                let mut one = serde_json::json!({
                    "subject": n.subject,
                    "type": n.id_type,
                    "count": n.values.len(),
                });
                if values {
                    one["values"] = serde_json::json!(n.values);
                }
                one
            })
            .collect();
        let collisions: Vec<serde_json::Value> = self
            .collisions
            .iter()
            .map(|c| {
                let mut one = serde_json::json!({
                    "subjects": c.subjects.iter().map(|(code, by)| serde_json::json!({
                        "subject": code,
                        "type": by,
                    })).collect::<Vec<_>>(),
                });
                if values {
                    one["name"] = serde_json::json!(c.name);
                }
                one
            })
            .collect();
        serde_json::json!({
            "by": self.by,
            "subjects": self.subjects,
            "types": self.types.iter().map(|t| serde_json::json!({
                "type": t.name,
                "identifying": t.identifying,
                "subjects": t.subjects,
            })).collect::<Vec<_>>(),
            "code": self.code,
            "chosen": self.chosen,
            "passed_over": self.passed_over.len(),
            "passed_over_subjects": self.passed_over,
            "needs_choice": needs,
            "collisions": collisions,
            "values": if values { "shown" } else { "withheld" },
            "ready": self.ready(),
        })
    }
}

fn subjects_word(n: usize) -> String {
    if n == 1 {
        "1 subject".to_string()
    } else {
        format!("{n} subjects")
    }
}

fn and_more(n: usize) -> String {
    if n > LISTED {
        format!(" and {} more", n - LISTED)
    } else {
        String::new()
    }
}

fn refused(words: String) -> Error {
    Error::Refused(format!("{words} Nothing was written."))
}

/// The types of the list, each checked: named once, a type the store keeps
/// values of, known to the store, and held by somebody. A type that never
/// keeps a value is refused by its name whatever the store holds.
pub fn types_of(store: &mut Store, by: &[String]) -> Result<Vec<linkage::IdType>, Error> {
    let mut seen = BTreeSet::new();
    for name in by {
        if !seen.insert(name.as_str()) {
            return Err(refused(format!(
                "name by {name}: the type is in the list twice. Name each type once, in the \
                 order they are tried."
            )));
        }
        if !linkage::keeps(name) {
            return Err(refused(format!(
                "name by {name}: {} is never kept, not even sealed, so it can name no \
                 subject. Choose an ID type that keeps its values (nils linkage id-type list).",
                if personnummer::is_type(name) {
                    "a personnummer"
                } else {
                    "an ID of this type"
                }
            )));
        }
    }
    let known = linkage::id_types(store)?;
    let mut out = Vec::new();
    for name in by {
        let Some(t) = known.iter().find(|t| &t.name == name) else {
            let kept: Vec<&str> = known
                .iter()
                .filter(|t| linkage::keeps(&t.name))
                .map(|t| t.name.as_str())
                .collect();
            return Err(refused(format!(
                "name by {name}: no ID type is named so. The types that keep values are {}.",
                kept.join(", ")
            )));
        };
        if !linkage::holds_kept_value(store, t.id)? {
            return Err(refused(format!(
                "name by {name}: no subject holds a kept value of {name}, so it would name \
                 nobody and every subject would keep its code. Import the IDs first (nils \
                 linkage import), or take the type out of the list."
            )));
        }
        out.push(t.clone());
    }
    Ok(out)
}

/// The choices kept by the versions before of the release named `release`:
/// for each subject the newest, as the type it was made under and the name
/// it gave.
fn kept_choices(store: &mut Store, release: &str) -> Result<HashMap<i64, (String, String)>, Error> {
    let sql = format!(
        "SELECT n.subject_id, n.id_type, n.name, n.chosen FROM {} n JOIN {} r ON r.id = n.release_id \
         WHERE r.name = {} ORDER BY n.release_id DESC, n.id DESC",
        store.qualified("release_name"),
        store.qualified("release"),
        store.dialect().param(1, Type::Text),
    );
    let mut out = HashMap::new();
    for r in store.query(&sql, &[Param::from(release)])? {
        if r.int(3)? == 0 {
            continue;
        }
        let Some(id_type) = r.opt_text(1)? else {
            continue;
        };
        out.entry(r.int(0)?)
            .or_insert((id_type.to_string(), r.text(2)?.to_string()));
    }
    Ok(out)
}

/// How the release named `release` would name `subjects` (each an id and its
/// code) by the types `by`, with the choices given now. None where `by` is
/// empty: every subject keeps its code and nothing is read.
///
/// Refused, with nothing written, where a type cannot be named by or a
/// choice cannot be taken. A plan that needs a choice or holds a collision
/// is an answer, not a refusal: [`Plan::ready`] says whether a release may
/// start, and [`Plan::refusal`] the words it stops with.
///
/// Every value read is audited in the linkage store under `actor`.
pub fn plan(
    registry: &mut Registry,
    release: &str,
    by: &[String],
    choices: &[Choice],
    subjects: &[(i64, String)],
    actor: &str,
) -> Result<Option<Plan>, Error> {
    if by.is_empty() {
        if !choices.is_empty() {
            return Err(refused(
                "a choice of the value that names a subject was given and the release names \
                 by no ID type. Give the types to name by, or leave the choices out."
                    .to_string(),
            ));
        }
        return Ok(None);
    }
    let key = registry
        .pseudonym_key()
        .map_err(|e| Error::Refused(format!("the linkage store: {e}")))?;
    let keys = Subkeys::derive(&key);
    let mut store = registry
        .open_linkage()
        .map_err(|e| Error::Refused(format!("the linkage store: {e}")))?;
    let types = types_of(&mut store, by)?;

    // The choices given now, by subject code, each compared once cleaned.
    let codes: BTreeSet<&str> = subjects.iter().map(|(_, c)| c.as_str()).collect();
    let mut asked: BTreeMap<&str, String> = BTreeMap::new();
    for c in choices {
        if !codes.contains(c.subject.as_str()) {
            return Err(refused(format!(
                "the choice for {}: the release's selection holds no subject with that code. \
                 A choice names a subject by its subject code.",
                c.subject
            )));
        }
        let value = clean(&c.value);
        if let Some(before) = asked.insert(c.subject.as_str(), value.clone())
            && before != value
        {
            return Err(refused(format!(
                "the choice for {}: two different values were given. Give one.",
                c.subject
            )));
        }
    }
    let kept = kept_choices(registry.store(), release)?;

    let why = format!("release {release}: subjects named by {}", by.join(", "));
    let mut plan = Plan {
        by: by.to_vec(),
        subjects: subjects.len() as i64,
        ..Plan::default()
    };
    let mut passed_over: BTreeSet<&str> = BTreeSet::new();
    // Each type is read only for the subjects no earlier type decided, so a
    // later type's values are opened, and audited, only where they can name.
    let mut open: Vec<&(i64, String)> = subjects.iter().collect();
    for t in &types {
        let ids: Vec<i64> = open.iter().map(|(id, _)| *id).collect();
        let mut held = linkage::kept_values_of_type(&mut store, &keys, &ids, t.id, actor, &why)?;
        let mut named = 0i64;
        let mut still = Vec::new();
        for subject in open {
            let (id, code) = subject;
            // The values that can name: not shaped like a personnummer,
            // something left once cleaned, each cleaned spelling once.
            let mut can: Vec<(String, String)> = Vec::new();
            for value in held.remove(id).unwrap_or_default() {
                let cleaned = clean(&value);
                if personnummer::normalise(&value).is_ok()
                    || personnummer::normalise(&cleaned).is_ok()
                {
                    passed_over.insert(code.as_str());
                    continue;
                }
                if cleaned.is_empty() || can.iter().any(|(_, c)| *c == cleaned) {
                    continue;
                }
                can.push((value, cleaned));
            }
            let choice = asked.get(code.as_str());
            let (name, chosen) = match can.len() {
                0 => {
                    still.push(subject);
                    continue;
                }
                1 => (can.remove(0).1, false),
                _ => {
                    let given = choice.filter(|v| can.iter().any(|(_, c)| c == *v)).cloned();
                    let before = kept
                        .get(id)
                        .filter(|(of, name)| *of == t.name && can.iter().any(|(_, c)| c == name))
                        .map(|(_, name)| name.clone());
                    // A choice given now that is none of the values is
                    // refused below rather than answered by the kept one.
                    match given.or(before.filter(|_| choice.is_none())) {
                        Some(name) => (name, true),
                        None if choice.is_some() => {
                            return Err(refused(format!(
                                "the choice for {code}: the subject holds no such value of \
                                 {}, the type that names it. The plan lists the values it \
                                 holds (nils release --name-plan, POST /api/releases/names).",
                                t.name
                            )));
                        }
                        None => {
                            plan.needs_choice.push(NeedsChoice {
                                subject_id: *id,
                                subject: code.clone(),
                                id_type: t.name.clone(),
                                values: can.into_iter().map(|(v, _)| v).collect(),
                            });
                            continue;
                        }
                    }
                }
            };
            if choice.is_some_and(|v| *v != name) {
                return Err(refused(format!(
                    "the choice for {code}: the subject holds one value of {}, the type that \
                     names it, and the choice is another. Leave the choice out.",
                    t.name
                )));
            }
            named += 1;
            if chosen {
                plan.chosen += 1;
            }
            plan.names.insert(
                *id,
                Named {
                    name,
                    id_type: Some(t.name.clone()),
                    chosen,
                },
            );
        }
        plan.types.push(TypeCount {
            name: t.name.clone(),
            identifying: t.identifying,
            subjects: named,
        });
        open = still;
    }
    // Whoever is left holds no value that can name, and keeps the code.
    for (id, code) in open {
        if asked.contains_key(code.as_str()) {
            return Err(refused(format!(
                "the choice for {code}: the subject holds no value of {} that can name it, \
                 so it keeps its code. Leave the choice out.",
                by.join(" or ")
            )));
        }
        plan.code += 1;
        plan.names.insert(
            *id,
            Named {
                name: code.clone(),
                id_type: None,
                chosen: false,
            },
        );
    }
    plan.passed_over = passed_over.into_iter().map(str::to_string).collect();

    // Two names that differ only in case are one name to a file system that
    // ignores it, and BIDS refuses them, so they are compared without it.
    let mut by_name: BTreeMap<String, Vec<(&str, &Named)>> = BTreeMap::new();
    for (id, code) in subjects {
        if let Some(n) = plan.names.get(id) {
            by_name
                .entry(n.name.to_ascii_lowercase())
                .or_default()
                .push((code.as_str(), n));
        }
    }
    for (_, sharing) in by_name {
        if sharing.len() < 2 {
            continue;
        }
        plan.collisions.push(Collision {
            name: sharing[0].1.name.clone(),
            subjects: sharing
                .iter()
                .map(|(code, n)| (code.to_string(), n.id_type.clone()))
                .collect(),
        });
    }
    Ok(Some(plan))
}

/// Keep what a release called each subject, before it writes a file: the
/// pairing a result that comes back under a name is linked by, and the
/// choices its next version reads.
pub fn keep(store: &mut Store, release: i64, plan: &Plan) -> Result<(), Error> {
    let mut ids: Vec<&i64> = plan.names.keys().collect();
    ids.sort();
    let rows: Vec<Vec<Param>> = ids
        .into_iter()
        .map(|id| {
            let n = &plan.names[id];
            vec![
                Param::Int(release),
                Param::Int(*id),
                Param::from(n.name.as_str()),
                match &n.id_type {
                    Some(t) => Param::from(t.as_str()),
                    None => Param::Null,
                },
                Param::Bool(n.chosen),
            ]
        })
        .collect();
    crate::run::write_rows(
        store,
        "release_name",
        &["release_id", "subject_id", "name", "id_type", "chosen"],
        &rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_cleaned_to_letters_and_digits() {
        assert_eq!(clean("AB_01-x 7"), "AB01x7");
        assert_eq!(clean("_-/ "), "");
        // a letter outside ASCII is not one BIDS takes
        assert_eq!(clean("Å1é"), "1");
    }

    #[test]
    fn a_choice_is_a_code_and_a_value() {
        assert_eq!(
            Choice::parse("abc123=AB_01").unwrap(),
            Choice {
                subject: "abc123".into(),
                value: "AB_01".into()
            }
        );
        assert!(Choice::parse("abc123").is_err());
        assert!(Choice::parse("=x").is_err());
    }

    #[test]
    fn a_file_of_choices_takes_three_separators_and_skips_comments() {
        let got = choices_of_file("# who\n\na1\tX-1\nb2,Y 2\nc3=Z=3\n").unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(
            (got[0].subject.as_str(), got[0].value.as_str()),
            ("a1", "X-1")
        );
        assert_eq!(
            (got[1].subject.as_str(), got[1].value.as_str()),
            ("b2", "Y 2")
        );
        assert_eq!(
            (got[2].subject.as_str(), got[2].value.as_str()),
            ("c3", "Z=3")
        );
        // the line's number, never its text
        let err = choices_of_file("a1\tX\nonly-a-code\n").unwrap_err();
        assert!(err.starts_with("line 2:"), "{err}");
        assert!(!err.contains("only-a-code"), "{err}");
    }

    #[test]
    fn a_plan_that_stops_names_codes_and_never_a_value() {
        let plan = Plan {
            by: vec!["study-id".into()],
            needs_choice: vec![NeedsChoice {
                subject_id: 1,
                subject: "c0de".into(),
                id_type: "study-id".into(),
                values: vec!["SECRET_1".into(), "SECRET_2".into()],
            }],
            collisions: vec![Collision {
                name: "SHARED9".into(),
                subjects: vec![
                    ("aaaa".into(), Some("study-id".into())),
                    ("bbbb".into(), None),
                ],
            }],
            ..Plan::default()
        };
        let words = plan.refusal().unwrap();
        assert!(words.contains("c0de (2 of study-id)"), "{words}");
        assert!(
            words.contains("aaaa (by study-id) and bbbb (by its code)"),
            "{words}"
        );
        assert!(words.ends_with("Nothing was written."), "{words}");
        assert!(
            !words.contains("SECRET") && !words.contains("SHARED9"),
            "{words}"
        );
        let withheld = plan.doc(false).to_string();
        assert!(!withheld.contains("SECRET") && !withheld.contains("SHARED9"));
        let shown = plan.doc(true).to_string();
        assert!(shown.contains("SECRET_1") && shown.contains("SHARED9"));
    }
}
