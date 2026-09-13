// SPDX-License-Identifier: AGPL-3.0-only

//! The profile of one set of a query (`POST /api/ask/profile`), for the
//! desk's charts: how many subjects, sessions and stacks are under it, its
//! stacks by their base, the values of one stack field, and, for a role that
//! may read them, the sex and the age of its subjects and the kinds of their
//! clinical events. Each part is a small query added to a copy of the
//! document and previewed under the caller's own scope and caps, so it
//! stores nothing and reads no more than the caller may. A part that cannot
//! be answered says why, and the others still stand.
//!
//! A member reached by more than one path, such as a subject in two of the
//! cohorts a set starts from, is a row for each path, so every number here
//! counts members by their ids, once each, and never by rows.

use nils_ask::affordance::{self, Preview, Setting};
use nils_ask::ast::{Ask, Grain};
use nils_ask::parse;
use nils_ask::validate::Class;
use nils_registry::Registry;
use nils_registry::store::Store;
use serde_json::{Value, json};

/// The most values a part lists.
const VALUES: u64 = 60;

/// The binding on a group that counts its members once each.
const MEMBERS: &str = "profile_members";

/// A session set with the age of its subject at the session, and the decade
/// of that age, bound.
fn aged(mut sessions: Value) -> Value {
    sessions["bind"] = json!({
        "age": ["age_at", {}, ["field", {}, "subject.birth_date"], ["field", {}, "first"]],
        "decade": ["*", {}, ["round", {"key": true}, ["/", {}, ["-", {}, ["field", {}, "age"], 4.5], 10], 0], 10],
    });
    sessions
}

/// The case ladder that reads a stack's base as a value. A group is keyed by
/// fields and never by an axis, so each value the pack gives the axis is a
/// branch, and a stack with no base is keyed by nothing.
fn base_ladder(values: &[String]) -> Value {
    values.iter().rev().fold(Value::Null, |otherwise, v| {
        let when = json!(["=", {}, ["axis", {}, "base"], v]);
        if otherwise.is_null() {
            json!(["case", {}, when, v])
        } else {
            json!(["case", {}, when, v, otherwise])
        }
    })
}

/// A set name the document leaves free.
fn fresh(base: &Value, stem: &str) -> String {
    let mut name = stem.to_string();
    let mut n = 1;
    while !base["sets"][&name].is_null() {
        name = format!("{stem}_{n}");
        n += 1;
    }
    name
}

/// A group's rows, one `{value, count, subjects}` for each value of `key`:
/// how many members have the value, each once, and how many subjects.
fn values_of(p: &Preview, key: &str) -> Vec<Value> {
    let at = |name: &str| p.columns.iter().position(|c| c == name);
    let (k, c, n) = (at(key), at(MEMBERS), at("_subjects"));
    p.rows
        .iter()
        .map(|row| {
            let cell =
                |i: Option<usize>| i.and_then(|i| row.get(i)).cloned().unwrap_or(Value::Null);
            json!({"value": cell(k), "count": cell(c), "subjects": cell(n)})
        })
        .collect()
}

/// The largest first by `by`; values that tie keep their order.
fn most_first(values: &mut [Value], by: &str) {
    let n = |v: &Value| v[by].as_i64().unwrap_or(0);
    values.sort_by_key(|v| std::cmp::Reverse(n(v)));
}

/// A part as the answer carries it: its value, or why it has none.
fn part(result: Result<Value, String>) -> Value {
    result.unwrap_or_else(|why| json!({"refused": why}))
}

/// The document a profile reads, and what it reads it with.
struct Reading<'a, 's> {
    registry: &'a mut Registry,
    reader: &'a mut Store,
    setting: &'a Setting<'s>,
    /// The document as JSON, keeping nothing.
    base: Value,
}

impl Reading<'_, '_> {
    /// A preview of the document with `sets` added, answering `out`.
    fn preview(&mut self, sets: &[(String, Value)], out: Value) -> Result<Preview, String> {
        let mut doc = self.base.clone();
        for (name, set) in sets {
            doc["sets"][name] = set.clone();
        }
        doc["out"] = out;
        let ask = parse(&doc.to_string()).map_err(|e| e.to_string())?;
        affordance::preview(
            self.registry,
            &ask,
            VALUES,
            self.setting,
            Some(&mut *self.reader),
        )
        .map_err(|e| e.to_string())
    }

    /// How many rows `set` holds, and how many subjects, each once.
    fn count(&mut self, sets: &[(String, Value)], set: &str) -> Result<(i64, i64), String> {
        let p = self.preview(sets, json!({"set": set, "level": "count"}))?;
        let row = p.rows.first().ok_or("the count came back with no row")?;
        let at = |name: &str| {
            p.columns
                .iter()
                .position(|c| c == name)
                .and_then(|i| row.get(i))
                .and_then(Value::as_i64)
                .unwrap_or(0)
        };
        Ok((at("rows"), at("subjects")))
    }

    /// How many members `of` holds, each once: the rows of a group of it by id.
    fn members(&mut self, mut sets: Vec<(String, Value)>, of: &str) -> Result<i64, String> {
        let name = fresh(&self.base, "profile_ids");
        sets.push((
            name.clone(),
            json!({"grain": "group", "group": {"of": of, "by": [["field", {}, "id"]]}}),
        ));
        Ok(self.count(&sets, &name)?.0)
    }

    /// How many members a companion set holds, each once.
    fn members_of(&mut self, stem: &str, set: Value) -> Result<i64, String> {
        let name = fresh(&self.base, stem);
        self.members(vec![(name.clone(), set)], &name)
    }

    /// `of` grouped by `key`, and whether the cap cut the list.
    fn group(
        &mut self,
        mut sets: Vec<(String, Value)>,
        of: &str,
        key: &str,
    ) -> Result<(Vec<Value>, bool), String> {
        let name = fresh(&self.base, "profile_group");
        let mut group = json!({"grain": "group", "group": {"of": of, "by": [["field", {}, key]]}});
        group["bind"][MEMBERS] = json!(["distinct", {"set": of}, ["field", {}, "id"]]);
        sets.push((name.clone(), group));
        let p = self.preview(
            &sets,
            json!({
                "set": name,
                "level": "aggregate",
                "columns": [["field", {}, key], ["field", {}, MEMBERS], ["field", {}, "_subjects"]],
                "order": [[["field", {}, key], "asc"]],
            }),
        )?;
        Ok((values_of(&p, key), p.truncated))
    }
}

/// The subjects, sessions and stacks under a set of one of those grains, each
/// once; for a set of another grain, its rows and their subjects.
fn counts(r: &mut Reading<'_, '_>, grain: Grain, t: &str) -> Result<Value, String> {
    let (rows, subjects) = r.count(&[], t)?;
    Ok(match grain {
        Grain::Subject => {
            let sessions =
                r.members_of("profile_sessions", json!({"grain": "session", "of": t}))?;
            let stacks = r.members_of("profile_stacks", json!({"grain": "stack", "of": t}))?;
            json!({"subjects": subjects, "sessions": sessions, "stacks": stacks})
        }
        Grain::Session => {
            let sessions = r.members(Vec::new(), t)?;
            let stacks = r.members_of("profile_stacks", json!({"grain": "stack", "of": t}))?;
            json!({"subjects": subjects, "sessions": sessions, "stacks": stacks})
        }
        Grain::Stack => {
            let stacks = r.members(Vec::new(), t)?;
            let sessions = r.members_of(
                "profile_sessions",
                json!({"grain": "session", "has": [{"set": t, "min": 1}]}),
            )?;
            json!({"subjects": subjects, "sessions": sessions, "stacks": stacks})
        }
        _ => json!({"rows": rows, "subjects": subjects}),
    })
}

/// The profile of `set`, the document's answer when none is named, with one
/// stack `field` counted by value when one is named. `Err` only when the
/// document has no such set.
pub(crate) fn document(
    registry: &mut Registry,
    reader: &mut Store,
    setting: &Setting<'_>,
    ask: &Ask,
    set: Option<&str>,
    field: Option<&str>,
) -> Result<Value, String> {
    let target = set.unwrap_or(ask.out.set.as_str()).to_string();
    let grain = ask
        .sets
        .get(&target)
        .map(|s| s.grain)
        .ok_or_else(|| format!("the document has no set {target}"))?;
    let mut base = serde_json::to_value(ask).map_err(|e| e.to_string())?;
    if let Some(doc) = base.as_object_mut() {
        doc.remove("keep");
    }
    let (names, scope) = (setting.names, setting.scope);
    let mut r = Reading {
        registry,
        reader,
        setting,
        base,
    };
    let t = target.as_str();
    let has = json!([{"set": t, "min": 1}]);
    let under = matches!(grain, Grain::Subject | Grain::Session | Grain::Stack);

    let counts = if under {
        part(counts(&mut r, grain, t))
    } else {
        part(
            r.count(&[], t)
                .map(|(rows, subjects)| json!({"rows": rows, "subjects": subjects})),
        )
    };

    // its stacks by their base
    let stacks = fresh(&r.base, "profile_stacks");
    let stack_set = match grain {
        Grain::Subject | Grain::Session => Some(json!({"grain": "stack", "of": t})),
        _ => None,
    };
    let ladder = base_ladder(&names.axis_values("base").unwrap_or_default());
    let stack_types = if !under || ladder.is_null() {
        Value::Null
    } else {
        let mut keyed = stack_set
            .clone()
            .unwrap_or_else(|| json!({"grain": "stack", "from": t}));
        keyed["bind"] = json!({"base_value": ladder});
        part(
            r.group(vec![(stacks.clone(), keyed)], &stacks, "base_value")
                .map(|(mut values, _)| {
                    most_first(&mut values, "count");
                    Value::from(values)
                }),
        )
    };

    // one stack field by value, never an identifying one, and only one the role may read
    let field = match field {
        None => Value::Null,
        Some(f) if !under => json!({"name": f, "refused": "no stacks are under this set"}),
        Some(f) => match names.field("stack", f).map(|i| i.class) {
            None => json!({"name": f, "refused": format!("{f} is not a field of a stack")}),
            Some(Class::Identifying) => {
                json!({"name": f, "withheld": "an identifying field is never counted by value"})
            }
            Some(c @ (Class::QuasiIdentifying | Class::Sensitive))
                if !scope.classes.contains(&c) =>
            {
                json!({"name": f, "withheld": "the role may not read this field"})
            }
            Some(_) => {
                let (sets, of) = match &stack_set {
                    Some(s) => (vec![(stacks.clone(), s.clone())], stacks.as_str()),
                    None => (Vec::new(), t),
                };
                match r.group(sets, of, f) {
                    Ok((mut values, truncated)) => {
                        most_first(&mut values, "count");
                        json!({"name": f, "values": values, "truncated": truncated})
                    }
                    Err(why) => json!({"name": f, "refused": why}),
                }
            }
        },
    };

    // the subjects themselves, for what is read of a person
    let people = fresh(&r.base, "profile_subjects");
    let subjects: Option<(Vec<(String, Value)>, String)> = match grain {
        Grain::Subject => Some((Vec::new(), target.clone())),
        Grain::Session | Grain::Stack => Some((
            vec![(people.clone(), json!({"grain": "subject", "has": has}))],
            people,
        )),
        _ => None,
    };

    // sex and age are quasi-identifying, so only a role that may read them has them counted
    let demographics = match &subjects {
        None => Value::Null,
        Some(_) if !scope.classes.contains(&Class::QuasiIdentifying) => json!({
            "withheld": "the sex and the age of a subject are quasi-identifying, counted from the reviewer role"
        }),
        Some((sets, of)) => {
            let sex = part(r.group(sets.clone(), of, "sex").map(|(mut values, _)| {
                most_first(&mut values, "count");
                Value::from(values)
            }));
            let visits = fresh(&r.base, "profile_ages");
            let sessions = match grain {
                Grain::Subject => json!({"grain": "session", "of": t}),
                Grain::Session => json!({"grain": "session", "from": t}),
                _ => json!({"grain": "session", "has": has}),
            };
            let ages = part(
                r.group(vec![(visits.clone(), aged(sessions))], &visits, "decade")
                    .map(|(values, _)| Value::from(values)),
            );
            json!({"sex": sex, "age_decades": ages})
        }
    };

    // the kinds of the subjects' clinical events; a sensitive kind only for a role that may
    // read it, and the answer says it is withheld whether or not the set holds one
    let clinical = match &subjects {
        None => Value::Null,
        Some((sets, of)) => {
            let events = fresh(&r.base, "profile_events");
            let mut sets = sets.clone();
            sets.push((events.clone(), json!({"grain": "event", "of": of})));
            match r.group(sets, &events, "kind") {
                Err(why) => json!({"refused": why}),
                Ok((values, _)) => {
                    let open = scope.classes.contains(&Class::Sensitive);
                    let mut kinds: Vec<Value> = values
                        .into_iter()
                        .filter(|k| {
                            open || !k["value"]
                                .as_str()
                                .and_then(|name| names.kind(name))
                                .is_some_and(|i| i.sensitive)
                        })
                        .collect();
                    most_first(&mut kinds, "subjects");
                    json!({"kinds": kinds, "sensitive_withheld": !open})
                }
            }
        }
    };

    Ok(json!({
        "set": target,
        "grain": grain,
        "counts": counts,
        "stack_types": stack_types,
        "field": field,
        "demographics": demographics,
        "clinical": clinical,
    }))
}

#[cfg(test)]
mod tests {
    use super::{base_ladder, fresh, most_first};
    use serde_json::json;

    #[test]
    fn a_base_is_read_through_a_ladder_of_the_values_the_pack_gives_it() {
        assert_eq!(
            base_ladder(&["T1w".into(), "T2w".into()]),
            json!([
                "case",
                {},
                ["=", {}, ["axis", {}, "base"], "T1w"],
                "T1w",
                ["case", {}, ["=", {}, ["axis", {}, "base"], "T2w"], "T2w"]
            ])
        );
        assert!(base_ladder(&[]).is_null());
    }

    #[test]
    fn a_companion_set_takes_a_name_the_document_leaves_free() {
        let base = json!({"sets": {"people": {}, "profile_group": {}, "profile_group_1": {}}});
        assert_eq!(fresh(&base, "profile_group"), "profile_group_2");
        assert_eq!(fresh(&base, "profile_stacks"), "profile_stacks");
    }

    #[test]
    fn the_largest_value_comes_first_and_ties_keep_their_order() {
        let mut values = vec![
            json!({"value": "a", "count": 1}),
            json!({"value": "b", "count": 5}),
            json!({"value": "c", "count": 1}),
        ];
        most_first(&mut values, "count");
        let order: Vec<&str> = values
            .iter()
            .map(|v| v["value"].as_str().unwrap())
            .collect();
        assert_eq!(order, ["b", "a", "c"]);
    }
}
