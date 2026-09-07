// SPDX-License-Identifier: AGPL-3.0-only

//! Slice 8 (`docs/specs/wave4b-the-ask.md`, §10): options offer typed
//! moves with no query; a catalog value the principal may not see is never
//! enumerated and the author's own set names always are; apply returns a
//! handle and never a document and refuses a stale list; diagnose's funnel
//! names the set where each planted negative of the yardstick falls out;
//! preview, describe and draft; an outdated selection gets its move.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{Lab, fixture, refresh, set_param};
use nils_ask::affordance::{self, AffordanceError, Setting};
use nils_ask::ast::Src;
use nils_ask::diagnose;
use nils_ask::document;
use nils_ask::exec::Bounds;
use nils_ask::moves::{CATALOG, Kind, MOVE_KINDS_CAP, MoveCall};
use nils_ask::validate::{Class, Scope};
use nils_ask::{parse, selection};
use nils_catalog::Caps;
use nils_registry::session::Scheme;
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_affordance_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

fn args(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn subject_id(l: &mut Lab, code: &str) -> i64 {
    let store = l.registry.store();
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE code = {}",
        store.qualified("subject"),
        d.param(1, nils_registry::schema::Type::Text)
    );
    store
        .query(&sql, &[nils_registry::Param::from(code)])
        .unwrap()[0]
        .int(0)
        .unwrap()
}

#[test]
fn options_offer_typed_moves_with_no_query() {
    assert!(CATALOG.len() <= MOVE_KINDS_CAP);
    assert_eq!(Caps::default().move_kinds as usize, MOVE_KINDS_CAP);
    // pure over the document: one backend is every backend
    let labs = labs();
    {
        let l = &labs[0];
        let scope = Scope::default();
        let scheme = Scheme::default();
        let s = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "author",
            bounds: bounds(),
            values_cap: 50,
        };
        let ask = fixture("yardstick");
        let epoch = l.registry.meta().epoch;
        let opts = affordance::options(epoch, &ask, Some("good"), &s).unwrap();
        assert!(!opts.count_on_options && !opts.preview_on_options);
        assert_eq!(opts.set, "good");
        assert_eq!(opts.grain, "session");
        assert!(
            opts.describe
                .starts_with("good: sessions from followups; the nearest edss"),
            "{}",
            opts.describe
        );
        let ids: Vec<u32> = opts.moves.iter().map(|m| m.id).collect();
        assert_eq!(ids, (1..=opts.moves.len() as u32).collect::<Vec<_>>());
        let kinds: BTreeSet<Kind> = opts.moves.iter().map(|m| m.kind).collect();
        assert!(kinds.iter().all(|k| CATALOG.contains(k)));
        for k in [
            Kind::AddWhere,
            Kind::SetWindow,
            Kind::SetPolicy,
            Kind::SetOptional,
            Kind::RemoveRelation,
            Kind::AddNear,
            Kind::AddHas,
            Kind::AddBind,
            Kind::AddPick,
            Kind::AddSet,
            Kind::SetOut,
            Kind::KeepSet,
        ] {
            assert!(kinds.contains(&k), "{}: no {k:?} move", l.name);
        }
        let window = opts
            .moves
            .iter()
            .find(|m| m.kind == Kind::SetWindow)
            .unwrap();
        let relations: Vec<&Value> = window.holes[0].fillers.iter().collect();
        assert!(
            relations.contains(&&json!("near:edss")) && relations.contains(&&json!("near:sdmt")),
            "{relations:?}"
        );
        assert!(window.holes[1].fillers.contains(&json!("3 months")));
        let fields: Vec<String> = opts.exposes["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        for f in [
            "first",
            "last",
            "subject.birth_date",
            "age",
            "edss.date",
            "edss.offset_days",
            "t1.sig",
            "flair.sig",
        ] {
            assert!(
                fields.contains(&f.to_string()),
                "{}: {f} not exposed in {fields:?}",
                l.name
            );
        }
        // a quasi identifier is usable in a predicate by anyone who may ask (rule 15)
        assert!(fields.contains(&"subject.code".to_string()), "{fields:?}");
        assert!(
            !fields.iter().any(|f| f == "t1.offset_days"),
            "{}: an attached partner has no offset: {fields:?}",
            l.name
        );
        // the same document, set and scope: the same token
        let again = affordance::options(epoch, &ask, Some("good"), &s).unwrap();
        assert_eq!(again.token, opts.token);
        let other = affordance::options(epoch, &ask, Some("t1"), &s).unwrap();
        assert_ne!(other.token, opts.token);
        // hidden sets are never offered
        let set_out = opts.moves.iter().find(|m| m.kind == Kind::SetOut).unwrap();
        assert!(
            set_out.holes[0]
                .fillers
                .iter()
                .all(|v| !v.as_str().unwrap().contains("__"))
        );
        assert!(set_out.holes[0].fillers.contains(&json!("answer")));
        assert!(affordance::options(epoch, &ask, Some("nowhere"), &s).is_err());
    }
}

#[test]
fn a_value_the_principal_may_not_see_is_never_enumerated_and_own_names_always_are() {
    let labs = labs();
    {
        let l = &labs[0];
        let scheme = Scheme::default();
        let ask = fixture("yardstick");
        let epoch = l.registry.meta().epoch;
        let kinds_of = |scope: &Scope| -> Vec<String> {
            let s = Setting {
                names: &l.catalog,
                scope,
                scheme: &scheme,
                principal: "author",
                bounds: bounds(),
                values_cap: 50,
            };
            let opts = affordance::options(epoch, &ask, Some("edss"), &s).unwrap();
            let m = opts
                .moves
                .iter()
                .find(|m| m.kind == Kind::AddWhere && m.template.contains("{kind}"))
                .expect("the kind move");
            let kinds: Vec<String> = m.holes[0]
                .fillers
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            // the author's own sets are listed whatever the scope
            let near = opts
                .moves
                .iter()
                .find(|m| m.kind == Kind::AddNear)
                .expect("a near move");
            let partners: Vec<String> = near.holes[0]
                .fillers
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            for own in ["sdmt", "followups", "good", "t1", "flair"] {
                assert!(
                    partners.contains(&own.to_string()),
                    "{}: {own} not among {partners:?}",
                    l.name
                );
            }
            kinds
        };
        let plain = kinds_of(&Scope::default());
        assert!(
            plain.contains(&"EDSS".to_string()) && plain.contains(&"SDMT".to_string()),
            "{plain:?}"
        );
        assert!(
            !plain.iter().any(|k| k.contains("HIV")),
            "{}: a sensitive kind was enumerated: {plain:?}",
            l.name
        );
        let cleared = kinds_of(&Scope {
            federated: false,
            classes: BTreeSet::from([Class::Sensitive]),
        });
        assert!(cleared.contains(&"HIV Status".to_string()), "{cleared:?}");
    }
}

#[test]
fn apply_returns_a_handle_and_refuses_a_stale_list() {
    for mut l in labs() {
        let scope = Scope::default();
        let scheme = Scheme::default();
        let ask = fixture("yardstick");
        let epoch = l.registry.meta().epoch;
        let doc = {
            let s = Setting {
                names: &l.catalog,
                scope: &scope,
                scheme: &scheme,
                principal: "author",
                bounds: bounds(),
                values_cap: 50,
            };
            affordance::post(&mut l.registry, &ask, &s).unwrap()
        };
        let s = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "author",
            bounds: bounds(),
            values_cap: 50,
        };
        let opts = affordance::options(epoch, &doc.ask, Some("good"), &s).unwrap();
        let window = opts
            .moves
            .iter()
            .find(|m| m.kind == Kind::SetWindow)
            .unwrap();
        let call = MoveCall {
            move_id: window.id,
            args: args(&[
                ("relation", json!("near:edss")),
                ("preset", json!("3 months")),
            ]),
        };
        let applied = affordance::apply(
            &mut l.registry,
            doc.id,
            epoch,
            &opts.token,
            "good",
            std::slice::from_ref(&call),
            &s,
        )
        .unwrap();
        assert_ne!(applied.document, doc.id, "{}", l.name);
        assert_eq!(applied.parent, doc.id);
        assert_ne!(
            applied.hash, doc.hash,
            "{}: a window is structural, so the hash moves",
            l.name
        );
        assert_eq!(applied.changed.len(), 1);
        assert_eq!(applied.changed[0].0, "good");
        assert!(
            applied.changed[0].1.contains("-93 to 93 days"),
            "{}",
            applied.changed[0].1
        );
        assert_ne!(applied.options.token, opts.token);
        assert_eq!(applied.options.set, "good");
        // the document is fetched by handle only when something needs it
        let stored = document::get(l.registry.store(), applied.document)
            .unwrap()
            .unwrap();
        assert_eq!(stored.parent_id, Some(doc.id));
        let good = &stored.ask.sets["good"];
        assert!(
            matches!(&good.near[0].window, nils_ask::ast::WindowSpec::Literal(w) if w.days() == (Some(-93), Some(93)))
        );
        // the old list against the new document: stale
        let err = affordance::apply(
            &mut l.registry,
            applied.document,
            epoch,
            &opts.token,
            "good",
            std::slice::from_ref(&call),
            &s,
        )
        .unwrap_err();
        assert!(
            matches!(err, AffordanceError::StaleOptions(_)),
            "{}: {err}",
            l.name
        );
        // another epoch: stale
        let err = affordance::apply(
            &mut l.registry,
            doc.id,
            epoch + 1,
            &opts.token,
            "good",
            std::slice::from_ref(&call),
            &s,
        )
        .unwrap_err();
        assert!(
            matches!(err, AffordanceError::StaleOptions(_)),
            "{}: {err}",
            l.name
        );
        // a move id off the list, an argument off its fillers
        let err = affordance::apply(
            &mut l.registry,
            doc.id,
            epoch,
            &opts.token,
            "good",
            &[MoveCall {
                move_id: 999,
                args: BTreeMap::new(),
            }],
            &s,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                AffordanceError::Move(nils_ask::moves::MoveError::NoSuchMove(999))
            ),
            "{}: {err}",
            l.name
        );
        let err = affordance::apply(
            &mut l.registry,
            doc.id,
            epoch,
            &opts.token,
            "good",
            &[MoveCall {
                move_id: window.id,
                args: args(&[
                    ("relation", json!("near:edss")),
                    ("preset", json!("9 months")),
                ]),
            }],
            &s,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                AffordanceError::Move(nils_ask::moves::MoveError::Arg { .. })
            ),
            "{}: {err}",
            l.name
        );
        // a move that leaves an invalid document stores nothing
        let drop = opts
            .moves
            .iter()
            .find(|m| m.kind == Kind::RemoveRelation)
            .unwrap();
        let before = document_count(&mut l.registry);
        let err = affordance::apply(
            &mut l.registry,
            doc.id,
            epoch,
            &opts.token,
            "good",
            &[MoveCall {
                move_id: drop.id,
                args: args(&[("relation", json!("attach:flair"))]),
            }],
            &s,
        )
        .unwrap_err();
        assert!(
            matches!(err, AffordanceError::Ask(nils_ask::Error::Invalid(_))),
            "{}: {err}",
            l.name
        );
        assert_eq!(document_count(&mut l.registry), before, "{}", l.name);
        // two moves at once, atomically: a column and an order on the answer
        let out_opts = affordance::options(epoch, &doc.ask, Some("answer"), &s).unwrap();
        let col = out_opts
            .moves
            .iter()
            .find(|m| m.kind == Kind::AddColumn)
            .unwrap();
        let ord = out_opts
            .moves
            .iter()
            .find(|m| m.kind == Kind::AddOrder)
            .unwrap();
        let both = affordance::apply(
            &mut l.registry,
            doc.id,
            epoch,
            &out_opts.token,
            "answer",
            &[
                MoveCall {
                    move_id: col.id,
                    args: args(&[("field", json!("n_good"))]),
                },
                MoveCall {
                    move_id: ord.id,
                    args: args(&[("field", json!("n_good")), ("dir", json!("desc"))]),
                },
            ],
            &s,
        )
        .unwrap();
        let stored = document::get(l.registry.store(), both.document)
            .unwrap()
            .unwrap();
        assert_eq!(stored.ask.out.columns.len(), 7);
        assert_eq!(stored.ask.out.order.len(), 2);
        // the same text posted twice is one handle
        let again = affordance::post(&mut l.registry, &ask, &s).unwrap();
        assert_eq!(again.id, doc.id);
    }
}

fn document_count(registry: &mut nils_registry::Registry) -> i64 {
    let store = registry.store();
    store
        .query(
            &format!("SELECT COUNT(*) FROM {}", store.qualified("ask_document")),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap()
}

#[test]
fn the_funnel_names_the_set_where_each_planted_negative_falls_out() {
    for mut l in labs() {
        let scope = Scope::default();
        let scheme = Scheme::default();
        let ask = fixture("yardstick");
        let d = diagnose::diagnose(
            &mut l.registry,
            ask.clone(),
            Vec::new(),
            &l.catalog,
            &scope,
            &scheme,
            bounds(),
            true,
            None,
        )
        .unwrap();
        assert!(d.valid && d.issues.is_empty(), "{}", l.name);
        assert!(d.zero_rows.is_none(), "{}: {:?}", l.name, d.zero_rows);
        assert_eq!(d.cost.class, "medium");
        let stages: Vec<String> = d
            .funnel
            .iter()
            .map(|s| format!("{}/{}", s.set, s.stage))
            .collect();
        assert!(
            stages.contains(&"converted/source".to_string()),
            "{stages:?}"
        );
        assert!(
            stages.iter().any(|s| s.starts_with("good/near edss")),
            "{stages:?}"
        );
        assert!(
            stages.contains(&"answer/has good".to_string()),
            "{stages:?}"
        );
        // the counts never grow along a set's stages
        for set in ["converted", "followups", "good", "answer"] {
            let counts: Vec<i64> = d
                .funnel
                .iter()
                .filter(|s| s.set == set)
                .map(|s| s.subjects)
                .collect();
            assert!(
                counts.windows(2).all(|w| w[0] >= w[1]),
                "{}: {set} {counts:?}",
                l.name
            );
        }
        let last = d.funnel.iter().rfind(|s| s.set == "answer").unwrap();
        assert_eq!(last.subjects, 14, "{}", l.name);
        let cases = l.manifest.cases.clone();
        for c in &cases {
            let id = subject_id(&mut l, &c.code);
            let at = d.falls_out(id);
            let note = format!(
                "{}: {} ({}: {}) falls out at {:?}",
                l.name,
                c.code,
                c.falls_out_at,
                c.note,
                at.map(|s| format!("{}/{}", s.set, s.stage))
            );
            match c.falls_out_at.as_str() {
                "converted" => assert_eq!(at.map(|s| s.set.as_str()), Some("converted"), "{note}"),
                "followups" => assert_eq!(at.map(|s| s.set.as_str()), Some("followups"), "{note}"),
                "good" => {
                    let s = at.expect(&note);
                    assert!(
                        s.set == "good" || (s.set == "answer" && s.stage == "has good"),
                        "{note}"
                    );
                }
                "comparable" => {
                    let s = at.expect(&note);
                    assert!(
                        s.set == "answer" && s.stage.starts_with("where comparable.largest"),
                        "{note}"
                    );
                }
                _ => assert!(at.is_none(), "{note}"),
            }
        }
        // the answer's clauses and what they dropped; the picks and their ties
        assert_eq!(d.drops.len(), 1);
        assert!(d.drops[0].before >= d.drops[0].after, "{:?}", d.drops);
        assert!(d.drops[0].clause.starts_with("where comparable.largest"));
        let ties: BTreeMap<&str, i64> = d.ties.iter().map(|(s, n)| (s.as_str(), *n)).collect();
        assert!(
            ties.contains_key("t1") && ties.contains_key("flair"),
            "{ties:?}"
        );
        assert!(d.coarse.is_empty(), "{:?}", d.coarse);
        // a question nobody can satisfy explains its zero rows in domain words
        let mut none = ask.clone();
        set_param(&mut none, "age_from", json!(200));
        let z = diagnose::diagnose(
            &mut l.registry,
            none,
            Vec::new(),
            &l.catalog,
            &scope,
            &scheme,
            bounds(),
            false,
            None,
        )
        .unwrap();
        let why = z.zero_rows.clone().expect("a zero row explanation");
        assert!(
            why.starts_with("no session of followups survives where age >= {age_from}"),
            "{}: {why}",
            l.name
        );
        // an invalid document diagnoses with no query
        let bad = parse(&json!({
            "ast_version": 1,
            "sets": {"a": {"grain": "subject", "where": [["=", {}, ["field", {}, "nowhere"], 1]]}},
            "out": {"set": "a", "level": "count"}
        }).to_string()).unwrap();
        let b = diagnose::diagnose(
            &mut l.registry,
            bad,
            Vec::new(),
            &l.catalog,
            &scope,
            &scheme,
            bounds(),
            false,
            None,
        )
        .unwrap();
        assert!(!b.valid && !b.issues.is_empty() && b.funnel.is_empty());
        assert_eq!(b.issues[0].path, "sets.a.where[0]");
    }
}

#[test]
fn preview_describe_and_draft() {
    for mut l in labs() {
        let scope = Scope::default();
        let scheme = Scheme::default();
        let s = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "author",
            bounds: bounds(),
            values_cap: 50,
        };
        let ask = fixture("yardstick");
        let p = affordance::preview(&mut l.registry, &ask, 5, &s, None).unwrap();
        assert_eq!(p.rows.len(), 5, "{}", l.name);
        assert_eq!(p.columns.len(), 8);
        let mut counted = ask.clone();
        counted.out.level = nils_ask::ast::Level::Count;
        counted.out.columns.clear();
        counted.out.order.clear();
        let c = affordance::preview(&mut l.registry, &counted, 5, &s, None).unwrap();
        assert_eq!(c.rows.len(), 1);
        assert_eq!(c.rows[0][1], json!(14));
        let d = affordance::describe(&ask, &s).unwrap();
        let by_name: BTreeMap<&str, &str> = d
            .sets
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();
        assert!(
            by_name["converted"]
                .contains("the course changes from {from_type} to {to_type} (adjacent)"),
            "{}",
            by_name["converted"]
        );
        assert!(
            by_name["good"].contains("with t1 from t1"),
            "{}",
            by_name["good"]
        );
        assert!(
            by_name["t1"].contains("one per session by n_instances desc, id asc"),
            "{}",
            by_name["t1"]
        );
        assert!(!by_name.contains_key("answer__same_comparable"));
        assert!(
            d.conventions
                .iter()
                .any(|c| c.contains("a month is 31 days"))
        );
        assert!(
            d.conventions
                .iter()
                .any(|c| c.contains("the level loose compares")),
            "{:?}",
            d.conventions
        );
        assert!(
            d.denominators
                .contains(&("n_good".to_string(), "good".to_string()))
        );
        assert!(
            d.mechanisms.contains(&(
                "good".to_string(),
                "edss".to_string(),
                "nearest within -186 to 186 days".to_string()
            )),
            "{:?}",
            d.mechanisms
        );
        assert_eq!(d.disclosure, "local");
        assert!(
            d.answer
                .starts_with("the answer is answer at the record level with code")
        );
        // draft: a sloppy text is repaired, diagnosed and stored
        let text = json!({
            "ast_version": 1,
            "sets": {"a": {"grain": "subject", "where": ["not_null", ["field", {}, "birth_date"]]}},
            "out": {"set": "a", "level": "count"}
        })
        .to_string();
        let drafted = affordance::draft(&mut l.registry, &text, &s, None).unwrap();
        assert!(!drafted.repairs.is_empty(), "{}", l.name);
        assert!(
            drafted.diagnosis.valid,
            "{}: {:?}",
            l.name, drafted.diagnosis.issues
        );
        assert!(drafted.document.is_some());
        let broken = affordance::draft(&mut l.registry, &json!({"ast_version": 1, "sets": {"a": {"grain": "subject", "from": "nowhere"}}, "out": {"set": "a", "level": "count"}}).to_string(), &s, None).unwrap();
        assert!(!broken.diagnosis.valid && broken.document.is_none());
    }
}

#[test]
fn an_outdated_selection_gets_its_move() {
    for mut l in labs() {
        let scope = Scope::default();
        let scheme = Scheme::default();
        let small = parse(&json!({
            "ast_version": 1,
            "sets": {"a": {"grain": "subject", "where": [["not_null", {}, ["field", {}, "birth_date"]]]}},
            "out": {"set": "a", "level": "count"}
        }).to_string()).unwrap();
        let prepared = nils_ask::prepare(small.clone(), &l.catalog, &scope).unwrap();
        selection::save(
            &mut l.registry,
            "with-birth",
            &prepared.ask,
            &prepared.hash,
            "author",
            None,
            None,
        )
        .unwrap();
        let reader = parse(
            &json!({
                "ast_version": 1,
                "sets": {"r": {"grain": "subject", "from": "selection:with-birth@1"}},
                "out": {"set": "r", "level": "count"}
            })
            .to_string(),
        )
        .unwrap();
        let mut v2 = small.clone();
        v2.sets.get_mut("a").unwrap().where_.push(
            nils_ask::ast::Clause::new("=")
                .arg(nils_ask::ast::Arg::Clause(nils_ask::ast::Clause::field(
                    "sex",
                )))
                .arg(nils_ask::ast::Arg::Text("F".into())),
        );
        let p2 = nils_ask::prepare(v2, &l.catalog, &scope).unwrap();
        let saved = selection::save(
            &mut l.registry,
            "with-birth",
            &p2.ask,
            &p2.hash,
            "author",
            None,
            None,
        )
        .unwrap();
        assert_eq!(saved.version, 2);
        refresh(&mut l);
        let epoch = l.registry.meta().epoch;
        let s = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "author",
            bounds: bounds(),
            values_cap: 50,
        };
        let opts = affordance::options(epoch, &reader, Some("r"), &s).unwrap();
        assert!(
            opts.diagnostics
                .iter()
                .any(|w| w.code == nils_ask::validate::Code::SelectionOutdated),
            "{}: {:?}",
            l.name,
            opts.diagnostics
        );
        let update = opts
            .moves
            .iter()
            .find(|m| m.kind == Kind::UpdateSelection)
            .expect("the update move");
        assert_eq!(update.holes[0].fillers, vec![json!("r")]);
        let doc = affordance::post(&mut l.registry, &reader, &s).unwrap();
        let applied = affordance::apply(
            &mut l.registry,
            doc.id,
            epoch,
            &opts.token,
            "r",
            &[MoveCall {
                move_id: update.id,
                args: args(&[("set", json!("r"))]),
            }],
            &s,
        )
        .unwrap();
        let stored = document::get(l.registry.store(), applied.document)
            .unwrap()
            .unwrap();
        assert!(matches!(
            &stored.ask.sets["r"].from,
            Some(Src::Selection { version: None, .. })
        ));
        assert!(
            !applied
                .options
                .moves
                .iter()
                .any(|m| m.kind == Kind::UpdateSelection),
            "{}",
            l.name
        );
        assert!(
            applied.options.diagnostics.is_empty(),
            "{}: {:?}",
            l.name,
            applied.options.diagnostics
        );
    }
}
