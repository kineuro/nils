// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 H3, job type 3 (check a result; the build spec of 2026-10-11,
//! sections 6 and 7, and its tests for slice E2) on both backends: a check
//! campaign over results, its answers held to section 6.2, two raters'
//! agreed flags as the union of their parts, a disagreement on the verdict
//! sent to the adjudicator, the close into `check` writing each item's
//! decisions and statuses in one transaction (and nothing where an item
//! fails), the part grain's result status once all its parts are resolved,
//! and a direct check that withdraws the earlier decision and is refused
//! while an open check holds the result. Every row is made up.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::campaign::{self, Close, Given, Items, New, Role};
use nils_registry::check::{self, CheckItem, Panel};
use nils_registry::derivative::{self, Part};
use nils_registry::home::{Home, InitOptions};
use nils_registry::schema::{Type, table};
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store};
use serde_json::{Value, json};

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_check_campaign_test";

/// The campaign's own adjudication: on a disagreement, by exact agreement.
static NO_RULES: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| json!({}));

fn postgres_dsn() -> Option<String> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => None,
    }
}

struct Lab {
    name: &'static str,
    registry: Registry,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(dsn) = postgres_dsn().filter(|_| self._guard.is_some()) {
            let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
            store
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
                ))
                .expect("drop");
        }
    }
}

fn lab(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
    let dir = TempDir::new("check-campaign-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-check-test-key").unwrap();
    let registry = home
        .init(&InitOptions {
            backend,
            dsn,
            schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
            scheme: Scheme::Blake2b32,
            key: "k".to_string(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap();
    Lab {
        name,
        registry,
        _dir: dir,
        _guard: None,
    }
}

fn labs() -> Vec<Lab> {
    let mut out = vec![lab("sqlite", Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
            ))
            .expect("drop");
        let mut l = lab("postgres", Backend::Postgres, Some(dsn));
        l._guard = Some(guard);
        out.push(l);
    }
    out
}

fn filler(ty: Type) -> Param {
    match ty {
        Type::Int | Type::Id => Param::Int(1),
        Type::Double => Param::Double(1.0),
        Type::Bool => Param::Bool(false),
        Type::Date => Param::from("2026-01-01"),
        Type::Time => Param::from("00:00:00"),
        Type::Timestamp => Param::from("2026-01-01T00:00:00Z"),
        Type::Json => Param::from("{}"),
        Type::Text => Param::from("x"),
        Type::Bytes => Param::Bytes(vec![0]),
    }
}

fn row(store: &mut Store, name: &str, given: &[(&str, Param)]) -> i64 {
    let t = table(name);
    let mut cols: Vec<&str> = Vec::new();
    let mut vals: Vec<Param> = Vec::new();
    for c in t.columns.iter().filter(|c| c.ty != Type::Id) {
        if let Some((_, v)) = given.iter().find(|(n, _)| *n == c.name) {
            cols.push(c.name);
            vals.push(v.clone());
        } else if c.not_null {
            cols.push(c.name);
            vals.push(filler(c.ty));
        }
    }
    store
        .insert(&Insert::new(t, &cols).returning(&["id"]), &[vals])
        .unwrap()[0]
        .int(0)
        .unwrap()
}

/// One subject: a series of two stacks in frame F1 (the made-on scan and a
/// FLAIR), and a series in frame F2. Answers the three stacks.
fn scans(reg: &mut Registry) -> (i64, i64, i64) {
    let store = reg.store();
    let subject = row(store, "subject", &[("code", Param::from("s1"))]);
    let study = row(
        store,
        "study",
        &[
            ("subject_id", Param::Int(subject)),
            ("study_instance_uid", Param::from("1.2.840.7.0")),
        ],
    );
    let mut out = Vec::new();
    for (uid, frame) in [
        ("1.2.840.7.1", "F1"),
        ("1.2.840.7.2", "F1"),
        ("1.2.840.7.3", "F2"),
    ] {
        let series = row(
            store,
            "series",
            &[
                ("subject_id", Param::Int(subject)),
                ("study_id", Param::Int(study)),
                ("series_instance_uid", Param::from(uid)),
                ("frame_of_reference_uid", Param::from(frame)),
            ],
        );
        out.push(row(
            store,
            "stack",
            &[
                ("series_id", Param::Int(series)),
                ("stack_index", Param::Int(0)),
                ("stack_key", Param::from(format!("{uid}#0"))),
            ],
        ));
    }
    (out[0], out[1], out[2])
}

/// A mask on the made-on scan's own grid with three lesions, labels 1 to 3.
fn mask(reg: &mut Registry, stack: i64, n: usize) -> i64 {
    let store = reg.store();
    let belongs = derivative::belongs(store, Some(stack), None, None, None).unwrap();
    let sha = format!("{n:064x}");
    let id = derivative::insert(
        store,
        &derivative::New {
            kind: "mask",
            belongs: &belongs,
            place_id: 1,
            path: "derivatives/mask/ab/ab",
            bytes: 1,
            sha256: &sha,
            media_type: "application/x-nifti",
            registered_by: "anna@lab",
            actor: None,
            model_id: None,
            run_id: None,
            preprocess_version: None,
            supersedes_id: None,
            created_at: "2026-10-11T10:00:00Z",
        },
    )
    .unwrap();
    derivative::set_grid(store, id, &json!({"dims": [4, 4, 4]}), Some(stack), "same").unwrap();
    let part = |label: i64| Part {
        label,
        name: None,
        voxels: 2,
        volume_mm3: 1.0,
        anchor_mm: [label as f64, 0.0, 0.0],
        box_mm: [[0.0; 3], [1.0; 3]],
        position: label - 1,
        method: "26-connected".into(),
        check_status: None,
        check_reason: None,
        check_decision_id: None,
    };
    derivative::set_parts(
        store,
        id,
        &json!([{"key": "lesions", "words": "Lesions", "kind": "components", "count": 3}]),
        &[("lesions".into(), vec![part(1), part(2), part(3)])],
    )
    .unwrap();
    id
}

fn question(grain: &str) -> Value {
    json!({"kind": "check", "part_set": "lesions", "grain": grain, "reasons": check::presets()["components"]})
}

fn new<'a>(
    name: &'a str,
    q: &'a Value,
    items: Vec<CheckItem>,
    raters: i64,
    adjudicators: Vec<String>,
) -> New<'a> {
    New {
        name,
        owner: "cleo@lab",
        question: q,
        source: json!({"test": true}),
        items: Items::Checks(items),
        handle_id: None,
        content_hash: Some("sha"),
        pack_version: None,
        raters_per_item: raters,
        raters: Vec::new(),
        adjudicators,
        adjudication: &NO_RULES,
        closes_into: "check",
        lease_seconds: 600,
        inputs: Default::default(),
        hold_back: None,
        suggest: Some(campaign::Suggest::None),
        hide_header: false,
    }
}

fn item(d: i64, label: Option<i64>, made_on: i64, flair: i64) -> CheckItem {
    CheckItem {
        derivative_id: d,
        part_set: "lesions".into(),
        part_label: label,
        panels: vec![
            Panel {
                panel: 0,
                role: check::MADE_ON.into(),
                stack: made_on,
                link: "same".into(),
            },
            Panel {
                panel: 1,
                role: "flair".into(),
                stack: flair,
                link: "world".into(),
            },
        ],
    }
}

fn at(minute: u32) -> String {
    format!("2026-10-11T11:{minute:02}:00Z")
}

/// The engine's reading of which results were made from a stack of a
/// sample sealed now, where no sample is: none.
fn none_sealed(_: &mut Registry, _: &[i64]) -> Result<std::collections::BTreeSet<i64>, String> {
    Ok(std::collections::BTreeSet::new())
}

/// Claim the next item and answer it.
#[allow(clippy::too_many_arguments)]
fn read(
    reg: &mut Registry,
    c: i64,
    who: &str,
    role: Role,
    value: &str,
    form: Value,
    why: Option<&str>,
    minute: u32,
) -> Result<campaign::Answered, campaign::Error> {
    let claimed = campaign::claim(reg, c, who, role, &at(minute))
        .unwrap()
        .expect("an item");
    campaign::answer(
        reg,
        &Given {
            assignment: claimed.assignment.id,
            principal: who,
            author_kind: "person",
            model: None,
            value: Some(value),
            form: Some(&form),
            derivative_id: None,
            why,
            unsure: false,
        },
        &at(minute + 1),
    )
}

fn decisions(
    store: &mut Store,
    scope: &str,
    reference: &str,
) -> Vec<(String, Option<String>, bool)> {
    let d = store.dialect();
    let sql = format!(
        "SELECT value, why, CASE WHEN withdrawn_at IS NULL THEN 0 ELSE 1 END FROM {} \
         WHERE scope = {} AND ref = {} AND axis = 'check' ORDER BY id",
        store.qualified("decision"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
    );
    store
        .query(&sql, &[Param::from(scope), Param::from(reference)])
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.opt_text(0).unwrap().unwrap_or_default().to_string(),
                r.opt_text(1).unwrap().map(str::to_string),
                r.int(2).unwrap() == 1,
            )
        })
        .collect()
}

fn part_status(store: &mut Store, d: i64) -> Vec<(i64, Option<String>, Option<String>)> {
    let (_, parts) =
        derivative::parts(store, d, "lesions", derivative::PartOrder::Position, 0, 50).unwrap();
    parts
        .into_iter()
        .map(|p| (p.label, p.check_status, p.check_reason))
        .collect()
}

#[test]
fn a_check_closes_into_a_status_on_its_results_and_their_parts() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, s1, s2) = scans(reg);
        let (d0, d1, d2, d3) = (
            mask(reg, s0, 1),
            mask(reg, s0, 2),
            mask(reg, s0, 3),
            mask(reg, s0, 4),
        );
        let q = question("result");

        // refused as section 8.3 says: a grain the items do not have, a
        // result twice, a panel that is not the made-on scan first
        let part_q = question("part");
        let e = campaign::create(
            reg,
            &new("x", &part_q, vec![item(d0, None, s0, s1)], 1, vec![]),
        )
        .unwrap_err();
        assert!(e.to_string().contains("one part per item"), "{name}: {e}");
        let e = campaign::create(
            reg,
            &new(
                "x",
                &q,
                vec![item(d0, None, s0, s1), item(d0, None, s0, s1)],
                1,
                vec![],
            ),
        )
        .unwrap_err();
        assert!(e.to_string().contains("twice"), "{name}: {e}");
        let mut flipped = item(d0, None, s0, s1);
        flipped.panels.swap(0, 1);
        assert!(campaign::create(reg, &new("x", &q, vec![flipped], 1, vec![])).is_err());
        let axis = json!({"kind": "axis", "axis": "body_part"});
        let mut wrong = new("x", &axis, vec![item(d0, None, s0, s1)], 1, vec![]);
        wrong.closes_into = "none";
        assert!(campaign::create(reg, &wrong).is_err(), "{name}");
        let mut into = new("x", &q, vec![item(d0, None, s0, s1)], 1, vec![]);
        into.closes_into = "decision";
        assert!(campaign::create(reg, &into).is_err(), "{name}");

        // two raters a result, an adjudicator for a disagreement
        let c = campaign::create(
            reg,
            &new(
                "lesions-r1",
                &q,
                vec![
                    item(d0, None, s0, s1),
                    item(d1, None, s0, s1),
                    item(d2, None, s0, s1),
                    item(d3, None, s0, s1),
                ],
                2,
                vec!["cleo@lab".into()],
            ),
        )
        .unwrap();
        assert_eq!(c.grain, "result", "{name}");
        assert_eq!(c.question["kind"], "check", "{name}");
        let items = campaign::items(reg.store(), c.id).unwrap();
        assert_eq!(items[0].derivative_id, Some(d0), "{name}");
        assert_eq!(items[0].part_set.as_deref(), Some("lesions"), "{name}");
        assert_eq!(items[0].part_label, None, "{name}");
        let panels = check::panels_of(reg.store(), c.id, None).unwrap();
        assert_eq!(panels[&items[0].id].len(), 2, "{name}");
        assert_eq!(panels[&items[0].id][1].stack, s1, "{name}");
        // a direct check is refused while an open check holds the result
        let e = check::check_directly(
            reg,
            &check::Direct {
                derivative: d0,
                value: "accepted",
                reasons: &[],
                parts: &[],
                set: None,
                why: None,
                who: "dan@lab",
                kind: "person",
            },
        )
        .unwrap_err();
        assert!(matches!(e, campaign::Error::Refused(_)), "{name}: {e}");

        // an answer is held to section 6.2 and to its item
        let claimed = campaign::claim(reg, c.id, "anna@lab", Role::Rater, &at(0))
            .unwrap()
            .unwrap();
        let try_answer = |reg: &mut Registry, form: Value, value: &str| {
            campaign::answer(
                reg,
                &Given {
                    assignment: claimed.assignment.id,
                    principal: "anna@lab",
                    author_kind: "person",
                    model: None,
                    value: Some(value),
                    form: Some(&form),
                    derivative_id: None,
                    why: None,
                    unsure: false,
                },
                &at(1),
            )
        };
        let e = try_answer(
            reg,
            json!({"parts": [{"label": 9, "reason": "under"}]}),
            "flagged",
        )
        .unwrap_err();
        assert!(e.to_string().contains("no part 9"), "{name}: {e}");
        let e = try_answer(reg, json!({"seen": [1], "shown": [s2]}), "accepted").unwrap_err();
        assert!(e.to_string().contains("scanner frame"), "{name}: {e}");
        let e = try_answer(reg, json!({"reasons": ["under"]}), "accepted").unwrap_err();
        assert!(e.to_string().contains("no reason"), "{name}: {e}");
        // d0: both flag, different parts: the union goes to correction
        try_answer(
            reg,
            json!({"parts": [{"label": 1, "reason": "under"}], "seen": [1, 2, 3], "shown": [s1]}),
            "flagged",
        )
        .unwrap();
        let a = read(
            reg,
            c.id,
            "bo@lab",
            Role::Rater,
            "flagged",
            json!({"parts": [{"label": 2, "reason": "not_lesion"}], "seen": [2]}),
            None,
            2,
        )
        .unwrap();
        assert_eq!(a.state, "agreed", "{name}");
        // d1: accepted against rejected goes to the adjudicator
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "accepted",
            json!({"seen": [1, 2]}),
            None,
            4,
        )
        .unwrap();
        let a = read(
            reg,
            c.id,
            "bo@lab",
            Role::Rater,
            "rejected",
            json!({"reasons": ["under"]}),
            None,
            6,
        )
        .unwrap();
        assert_eq!(a.state, "needs_adjudication", "{name}");
        assert!(a.adjudication.is_some(), "{name}");
        let a = read(
            reg,
            c.id,
            "cleo@lab",
            Role::Adjudicator,
            "accepted",
            json!({"seen": [3]}),
            None,
            8,
        )
        .unwrap();
        assert_eq!(a.state, "adjudicated", "{name}");
        // d2: a missed lesion as a point, and a reason that needs a note
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "flagged",
            json!({"points": [{"reason": "missed", "mm": [1.5, -2.0, 30.0]}]}),
            None,
            10,
        )
        .unwrap();
        let e = read(
            reg,
            c.id,
            "bo@lab",
            Role::Rater,
            "flagged",
            json!({"reasons": ["other"]}),
            None,
            12,
        )
        .unwrap_err();
        assert!(e.to_string().contains("note"), "{name}: {e}");
        // the lease is held: the same item is answered again, with its note
        read(
            reg,
            c.id,
            "bo@lab",
            Role::Rater,
            "flagged",
            json!({"reasons": ["other"]}),
            Some("a ghost in the margin"),
            14,
        )
        .unwrap();
        // d3: both accept
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "accepted",
            json!({"seen": [3]}),
            None,
            16,
        )
        .unwrap();
        read(
            reg,
            c.id,
            "bo@lab",
            Role::Rater,
            "accepted",
            json!({}),
            None,
            18,
        )
        .unwrap();

        // seconds, seen and shown are kept on the answer
        let answers = campaign::answers(reg.store(), c.id).unwrap();
        let first = answers
            .iter()
            .find(|a| a.item_id == items[0].id && a.principal == "anna@lab")
            .unwrap();
        assert!(first.seconds.is_some(), "{name}");
        assert_eq!(
            first.form.as_ref().unwrap()["seen"],
            json!([1, 2, 3]),
            "{name}"
        );
        assert_eq!(first.form.as_ref().unwrap()["shown"], json!([s1]), "{name}");
        // agreement on the verdict, the reasons' overlap reported, and the
        // counts by verdict and reason
        let agreement = campaign::agreement(reg.store(), c.id).unwrap();
        assert_eq!(agreement["items"], 4, "{name}: {agreement}");
        assert_eq!(agreement["exact"], 0.75, "{name}: {agreement}");
        assert_eq!(agreement["reasons"]["items"], 4, "{name}: {agreement}");
        let stats = campaign::stats(reg.store(), c.id).unwrap();
        assert_eq!(stats["all"]["verdicts"]["flagged"], 4, "{name}: {stats}");
        assert_eq!(stats["all"]["verdicts"]["rejected"], 1, "{name}: {stats}");
        assert_eq!(stats["all"]["reasons"]["missed"], 1, "{name}: {stats}");

        // a failure leaves nothing written: d0's part 1 is no part of it now
        let p = reg.store().qualified("derivative_part");
        reg.store()
            .execute(
                &format!("DELETE FROM {p} WHERE derivative_id = {d0} AND label = 1"),
                &[],
            )
            .unwrap();
        // a model or an agent never closes a check into force
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "bot@lab",
                author_kind: "agent",
                model: None,
                picks: None,
                sealed: Some(&none_sealed),
            },
            &at(30),
        )
        .unwrap();
        assert_eq!(closed.resolved, 0, "{name}: {:?}", closed);
        assert_eq!(closed.refused.len(), 4, "{name}");
        // closed by an agent, the campaign is closed; a second campaign over
        // the same results is closed by a person
        let c = campaign::create(
            reg,
            &new(
                "lesions-r2",
                &q,
                vec![
                    item(d0, None, s0, s1),
                    item(d1, None, s0, s1),
                    item(d2, None, s0, s1),
                ],
                1,
                vec![],
            ),
        )
        .unwrap();
        read(reg, c.id, "anna@lab", Role::Rater, "flagged", json!({"parts": [{"label": 1, "reason": "under"}, {"label": 2, "reason": "over"}], "seen": [3]}), None, 32).unwrap_err();
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "flagged",
            json!({"parts": [{"label": 2, "reason": "over"}], "seen": [2, 3]}),
            None,
            34,
        )
        .unwrap();
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "accepted",
            json!({"seen": [1, 2]}),
            None,
            36,
        )
        .unwrap();
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "flagged",
            json!({"reasons": ["under"], "points": [{"reason": "missed", "mm": [0, 0, 0]}]}),
            None,
            38,
        )
        .unwrap();
        // the parts of d0 are read again before the close: part 3 goes, so
        // the item that saw it fails whole
        reg.store()
            .execute(
                &format!("DELETE FROM {p} WHERE derivative_id = {d0} AND label = 3"),
                &[],
            )
            .unwrap();
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
                sealed: Some(&none_sealed),
            },
            &at(40),
        )
        .unwrap();
        assert_eq!(closed.resolved, 2, "{name}: {:?}", closed);
        assert_eq!(closed.refused.len(), 1, "{name}: {:?}", closed);
        assert!(
            closed.refused[0].1.contains("part 3"),
            "{name}: {:?}",
            closed
        );
        let st = reg.store();
        assert!(
            decisions(st, "derivative", &d0.to_string()).is_empty(),
            "{name}"
        );
        assert_eq!(
            derivative::get(st, d0).unwrap().unwrap().check_status,
            None,
            "{name}"
        );
        assert!(
            part_status(st, d0).iter().all(|(_, s, _)| s.is_none()),
            "{name}"
        );
        // d1: accepted, the parts seen accepted, the one never seen nothing
        let one = derivative::get(st, d1).unwrap().unwrap();
        assert_eq!(one.check_status.as_deref(), Some("accepted"), "{name}");
        assert!(
            one.check_decision_id.is_some() && one.checked_at.is_some(),
            "{name}"
        );
        assert_eq!(
            part_status(st, d1),
            vec![
                (1, Some("accepted".into()), None),
                (2, Some("accepted".into()), None),
                (3, None, None)
            ],
            "{name}"
        );
        assert_eq!(
            decisions(st, "derivative", &d1.to_string()),
            vec![("accepted".into(), None, false)],
            "{name}"
        );
        assert_eq!(
            decisions(st, "derivative_part", &format!("{d1}:lesions:2")).len(),
            1,
            "{name}"
        );
        // d2: flagged with its result-level reasons, a point's among them
        let two = derivative::get(st, d2).unwrap().unwrap();
        assert_eq!(two.check_status.as_deref(), Some("flagged"), "{name}");
        assert_eq!(two.check_reasons, json!(["under", "missed"]), "{name}");
        assert_eq!(
            decisions(st, "derivative", &d2.to_string()),
            vec![("flagged".into(), Some("under, missed".into()), false)],
            "{name}"
        );
        // the item's review item is closed by its decision
        let its = campaign::items(st, c.id).unwrap();
        let resolved = its.iter().find(|i| i.derivative_id == Some(d1)).unwrap();
        assert_eq!(resolved.state, "resolved", "{name}");
        let ri = nils_registry::review::item(st, resolved.review_item_id)
            .unwrap()
            .unwrap();
        assert_eq!(ri.status, "accepted", "{name}");
        assert_eq!(ri.scope, "derivative", "{name}");
        let failed = its.iter().find(|i| i.derivative_id == Some(d0)).unwrap();
        // no decision for the failed item: it is kept refused, saying why,
        // and its question stays open for a later close (after the T33
        // pipe test; it was closed as superseded with the campaign before)
        assert_eq!(failed.state, "refused", "{name}");
        assert!(
            failed
                .refused_why
                .as_deref()
                .is_some_and(|w| w.contains("part 3")),
            "{name}: {:?}",
            failed.refused_why
        );
        let ri = nils_registry::review::item(st, failed.review_item_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            ri.status, "open",
            "{name}: nothing written for the failed item"
        );
        assert_eq!(ri.evidence["campaign_state"], "refused", "{name}");

        // part grain: one item per lesion; the result follows once every
        // part item of it is resolved
        let pq = question("part");
        let c = campaign::create(
            reg,
            &new(
                "lesions-p",
                &pq,
                vec![
                    item(d3, Some(1), s0, s1),
                    item(d3, Some(2), s0, s1),
                    item(d3, Some(3), s0, s1),
                    item(d2, Some(1), s0, s1),
                    item(d2, Some(2), s0, s1),
                ],
                1,
                vec![],
            ),
        )
        .unwrap();
        assert_eq!(c.grain, "part", "{name}");
        let e = read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "flagged",
            json!({"parts": [{"label": 2, "reason": "under"}]}),
            None,
            42,
        )
        .unwrap_err();
        assert!(e.to_string().contains("own item"), "{name}: {e}");
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "accepted",
            json!({}),
            None,
            44,
        )
        .unwrap();
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "rejected",
            json!({}),
            None,
            46,
        )
        .unwrap();
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "flagged",
            json!({"reasons": ["under"]}),
            None,
            48,
        )
        .unwrap();
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "accepted",
            json!({}),
            None,
            50,
        )
        .unwrap();
        // d2's second lesion is left unread
        let closed = campaign::close(
            reg,
            &Close {
                campaign: c.id,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
                sealed: Some(&none_sealed),
            },
            &at(52),
        )
        .unwrap();
        assert_eq!(
            (closed.resolved, closed.unresolved),
            (4, 1),
            "{name}: {:?}",
            closed
        );
        let st = reg.store();
        assert_eq!(
            part_status(st, d3),
            vec![
                (1, Some("accepted".into()), None),
                (2, Some("rejected".into()), None),
                (3, Some("flagged".into()), Some("under".into())),
            ],
            "{name}"
        );
        let three = derivative::get(st, d3).unwrap().unwrap();
        assert_eq!(
            three.check_status.as_deref(),
            Some("flagged"),
            "{name}: a part grain never rejects a result"
        );
        assert_eq!(three.check_reasons, json!(["under"]), "{name}");
        // d2 keeps the status its earlier check left: not every part of it
        // was resolved here
        let d2_now = decisions(st, "derivative", &d2.to_string());
        assert_eq!(d2_now.len(), 1, "{name}: {d2_now:?}");
        assert_eq!(
            derivative::get(st, d2)
                .unwrap()
                .unwrap()
                .check_status
                .as_deref(),
            Some("flagged"),
            "{name}"
        );

        // a direct check withdraws the earlier decision, and is withdrawn
        let done = check::check_directly(
            reg,
            &check::Direct {
                derivative: d1,
                value: "flagged",
                reasons: &[],
                parts: &[(2, "over".into())],
                set: Some("lesions"),
                why: Some("one lesion too wide"),
                who: "dan@lab",
                kind: "person",
            },
        )
        .unwrap();
        let st = reg.store();
        assert_eq!(
            decisions(st, "derivative", &d1.to_string())
                .iter()
                .map(|(v, _, w)| (v.as_str(), *w))
                .collect::<Vec<_>>(),
            vec![("accepted", true), ("flagged", false)],
            "{name}"
        );
        let now = derivative::get(st, d1).unwrap().unwrap();
        assert_eq!(now.check_status.as_deref(), Some("flagged"), "{name}");
        assert_eq!(now.check_decision_id, Some(done.decision), "{name}");
        assert_eq!(
            part_status(st, d1)[1],
            (2, Some("flagged".into()), Some("over".into())),
            "{name}"
        );
        assert!(
            check::check_directly(
                reg,
                &check::Direct {
                    derivative: d1,
                    value: "accepted",
                    reasons: &[],
                    parts: &[],
                    set: None,
                    why: None,
                    who: "bot@lab",
                    kind: "agent",
                },
            )
            .is_err(),
            "{name}: a check is a person's"
        );
        let gone =
            check::withdraw(reg, d1, "dan@lab", "person", Some("checked too early")).unwrap();
        assert!(gone.len() >= 2, "{name}: {gone:?}");
        let st = reg.store();
        let after = derivative::get(st, d1).unwrap().unwrap();
        assert_eq!(
            (after.check_status, after.check_decision_id),
            (None, None),
            "{name}"
        );
        assert!(
            part_status(st, d1).iter().all(|(_, s, _)| s.is_none()),
            "{name}"
        );
        assert!(
            decisions(st, "derivative", &d1.to_string())
                .iter()
                .all(|(_, _, w)| *w),
            "{name}: withdrawn, never deleted"
        );
        assert!(
            check::withdraw(reg, d1, "dan@lab", "person", None).is_err(),
            "{name}"
        );
    }
}

fn close_as_person(reg: &mut Registry, c: i64, minute: u32) -> campaign::Closed {
    campaign::close(
        reg,
        &Close {
            campaign: c,
            who: "cleo@lab",
            author_kind: "person",
            model: None,
            picks: None,
            sealed: Some(&none_sealed),
        },
        &at(minute),
    )
    .unwrap()
}

/// Replace an item's stored outcome form, as an older engine or a damaged
/// row would leave it.
fn set_outcome_form(reg: &mut Registry, it: &campaign::Item, form: Value) {
    let mut o = it.outcome.clone();
    o["form"] = form;
    reg.store()
        .update_by_id(
            table("campaign_item"),
            &[("outcome", Param::from(o.to_string()))],
            "id",
            it.id,
        )
        .unwrap();
}

/// The T33 pipe test of type 3: two raters flag the same part for
/// different reasons. The close writes that part once, flagged, its
/// decision's why and its status holding both reasons, beside every other
/// part they flagged or saw; an outcome an older engine left with the part
/// once per reason closes the same way; at part grain the part's status
/// holds both reasons too. An item whose outcome cannot be read is refused
/// and named, counted unresolved, and nothing is written for it.
#[test]
fn a_part_two_raters_flag_for_different_reasons_is_written_once_with_both() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, s1, _) = scans(reg);
        let (d0, d1, d2, d3) = (
            mask(reg, s0, 1),
            mask(reg, s0, 2),
            mask(reg, s0, 3),
            mask(reg, s0, 4),
        );
        let q = question("result");
        let c = campaign::create(
            reg,
            &new(
                "t33",
                &q,
                vec![
                    item(d0, None, s0, s1),
                    item(d1, None, s0, s1),
                    item(d3, None, s0, s1),
                ],
                2,
                vec![],
            ),
        )
        .unwrap();
        let answers = [
            // d0: part 1 for two reasons, part 3 for the same one
            json!({"parts": [{"label": 1, "reason": "under"}, {"label": 3, "reason": "over"}], "seen": [1, 2, 3]}),
            json!({"parts": [{"label": 1, "reason": "not_lesion"}, {"label": 3, "reason": "over"}]}),
            // d1: its outcome is spoiled below
            json!({"parts": [{"label": 2, "reason": "under"}]}),
            json!({"parts": [{"label": 2, "reason": "under"}]}),
            // d3: its outcome is put back in the shape an older engine left
            json!({"parts": [{"label": 2, "reason": "under"}]}),
            json!({"parts": [{"label": 2, "reason": "not_lesion"}]}),
        ];
        for (i, form) in answers.iter().enumerate() {
            let who = if i % 2 == 0 { "anna@lab" } else { "bo@lab" };
            let a = read(
                reg,
                c.id,
                who,
                Role::Rater,
                "flagged",
                form.clone(),
                None,
                2 * i as u32,
            )
            .unwrap();
            if i % 2 == 1 {
                assert_eq!(a.state, "agreed", "{name}: {i}");
            }
        }
        let items = campaign::items(reg.store(), c.id).unwrap();
        let of = |d: i64| items.iter().find(|i| i.derivative_id == Some(d)).unwrap();
        assert_eq!(
            of(d0).outcome["form"]["parts"],
            json!([{"label": 1, "reason": "under, not_lesion"}, {"label": 3, "reason": "over"}]),
            "{name}"
        );
        set_outcome_form(reg, of(d1), json!({"parts": [{"label": 2}]}));
        set_outcome_form(
            reg,
            of(d3),
            json!({"parts": [{"label": 2, "reason": "under"}, {"label": 2, "reason": "not_lesion"}]}),
        );

        let closed = close_as_person(reg, c.id, 20);
        assert_eq!(
            (closed.resolved, closed.unresolved),
            (2, 1),
            "{name}: {closed:?}"
        );
        assert_eq!(closed.refused.len(), 1, "{name}: {closed:?}");
        assert_eq!(closed.refused[0].0, of(d1).id, "{name}");
        let why = &closed.refused[0].1;
        assert!(
            why.contains("cannot be read") && why.contains(&format!("result {d1}")),
            "{name}: {why}"
        );
        assert_eq!(
            closed.as_json()["refused"][0]["item"],
            json!(of(d1).id),
            "{name}"
        );

        let st = reg.store();
        let part = |n: i64, l: i64| format!("{n}:lesions:{l}");
        assert_eq!(
            decisions(st, "derivative_part", &part(d0, 1)),
            vec![("flagged".into(), Some("under, not_lesion".into()), false)],
            "{name}"
        );
        assert_eq!(
            decisions(st, "derivative_part", &part(d0, 3)),
            vec![("flagged".into(), Some("over".into()), false)],
            "{name}"
        );
        assert_eq!(
            decisions(st, "derivative_part", &part(d0, 2)),
            vec![("accepted".into(), None, false)],
            "{name}"
        );
        assert_eq!(
            part_status(st, d0),
            vec![
                (1, Some("flagged".into()), Some("under, not_lesion".into())),
                (2, Some("accepted".into()), None),
                (3, Some("flagged".into()), Some("over".into())),
            ],
            "{name}"
        );
        assert_eq!(
            derivative::get(st, d0)
                .unwrap()
                .unwrap()
                .check_status
                .as_deref(),
            Some("flagged"),
            "{name}"
        );
        assert_eq!(
            part_status(st, d3)[1],
            (2, Some("flagged".into()), Some("under, not_lesion".into())),
            "{name}"
        );
        let now = campaign::items(st, c.id).unwrap();
        let zero = now.iter().find(|i| i.derivative_id == Some(d0)).unwrap();
        assert_eq!(zero.state, "resolved", "{name}");
        assert_eq!(
            zero.outcome["decisions"]["parts"]
                .as_object()
                .map(|m| m.len()),
            Some(3),
            "{name}: {}",
            zero.outcome
        );
        // the refused item: unresolved, nothing written for it, and it says
        // so
        let one = now.iter().find(|i| i.derivative_id == Some(d1)).unwrap();
        assert_eq!(one.state, "refused", "{name}");
        assert_eq!(one.refused_why.as_deref(), Some(why.as_str()), "{name}");
        assert!(
            decisions(st, "derivative", &d1.to_string()).is_empty(),
            "{name}"
        );
        assert!(
            decisions(st, "derivative_part", &part(d1, 2)).is_empty(),
            "{name}"
        );
        assert_eq!(
            derivative::get(st, d1).unwrap().unwrap().check_status,
            None,
            "{name}"
        );
        assert!(
            part_status(st, d1).iter().all(|(_, s, _)| s.is_none()),
            "{name}"
        );

        // part grain: two raters flag one lesion for different reasons
        let pq = question("part");
        let c = campaign::create(
            reg,
            &new("t33-p", &pq, vec![item(d2, Some(1), s0, s1)], 2, vec![]),
        )
        .unwrap();
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            "flagged",
            json!({"reasons": ["under"]}),
            None,
            30,
        )
        .unwrap();
        read(
            reg,
            c.id,
            "bo@lab",
            Role::Rater,
            "flagged",
            json!({"reasons": ["over"]}),
            None,
            32,
        )
        .unwrap();
        let closed = close_as_person(reg, c.id, 40);
        assert_eq!(closed.resolved, 1, "{name}: {closed:?}");
        let st = reg.store();
        assert_eq!(
            part_status(st, d2)[0],
            (1, Some("flagged".into()), Some("under, over".into())),
            "{name}"
        );
        assert_eq!(
            decisions(st, "derivative_part", &part(d2, 1)),
            vec![("flagged".into(), Some("under, over".into()), false)],
            "{name}"
        );
        let two = derivative::get(st, d2).unwrap().unwrap();
        assert_eq!(two.check_status.as_deref(), Some("flagged"), "{name}");
        assert_eq!(two.check_reasons, json!(["under", "over"]), "{name}");
    }
}

/// Close a one-rater check over `results`, each answered as given, by a
/// person. Answers the campaign.
fn closed_check(
    reg: &mut Registry,
    name: &str,
    q: &Value,
    items: Vec<CheckItem>,
    answers: &[(&str, Value)],
    start: u32,
) -> i64 {
    let c = campaign::create(reg, &new(name, q, items, 1, vec![])).unwrap();
    for (i, (value, form)) in answers.iter().enumerate() {
        read(
            reg,
            c.id,
            "anna@lab",
            Role::Rater,
            value,
            form.clone(),
            None,
            start + 2 * i as u32,
        )
        .unwrap();
    }
    campaign::close(
        reg,
        &Close {
            campaign: c.id,
            who: "cleo@lab",
            author_kind: "person",
            model: None,
            picks: None,
            sealed: Some(&none_sealed),
        },
        &at(start + 20),
    )
    .unwrap();
    c.id
}

fn decision_ids(store: &mut Store, scope: &str, reference: &str, live: bool) -> Vec<i64> {
    let d = store.dialect();
    let sql = format!(
        "SELECT id FROM {} WHERE scope = {} AND ref = {} AND axis = 'check' {} ORDER BY id",
        store.qualified("decision"),
        d.param(1, Type::Text),
        d.param(2, Type::Text),
        if live { "AND withdrawn_at IS NULL" } else { "" },
    );
    store
        .query(&sql, &[Param::from(scope), Param::from(reference)])
        .unwrap()
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect()
}

/// Review of E1, E2, item 1: a check's decision withdrawn at the decisions
/// door clears the status it wrote, on the result or the part, and leaves
/// its review item closed; an agent withdraws none.
#[test]
fn a_check_s_decision_withdrawn_at_the_decisions_door_clears_its_status() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, s1, _) = scans(reg);
        let d0 = mask(reg, s0, 1);
        let q = question("result");
        let c = closed_check(
            reg,
            "w1",
            &q,
            vec![item(d0, None, s0, s1)],
            &[(
                "flagged",
                json!({"parts": [{"label": 1, "reason": "under"}], "seen": [1, 2]}),
            )],
            0,
        );
        let st = reg.store();
        let result = decision_ids(st, "derivative", &d0.to_string(), true)[0];
        let part = decision_ids(st, "derivative_part", &format!("{d0}:lesions:1"), true)[0];
        assert!(
            nils_registry::review::withdraw_as(reg, result, "bot@lab", "agent").is_err(),
            "{name}"
        );
        assert_eq!(
            nils_registry::review::withdraw(reg, result, "dan@lab").unwrap(),
            0,
            "{name}"
        );
        let st = reg.store();
        let now = derivative::get(st, d0).unwrap().unwrap();
        assert_eq!(
            (now.check_status, now.check_decision_id, now.checked_at),
            (None, None, None),
            "{name}"
        );
        // the part's status is the part decision's own, and stays
        assert_eq!(
            part_status(st, d0)[0].1.as_deref(),
            Some("flagged"),
            "{name}"
        );
        let it = &campaign::items(st, c).unwrap()[0];
        let ri = nils_registry::review::item(st, it.review_item_id)
            .unwrap()
            .unwrap();
        assert_eq!(ri.status, "accepted", "{name}: no question reopened");
        nils_registry::review::withdraw(reg, part, "dan@lab").unwrap();
        let st = reg.store();
        assert_eq!(part_status(st, d0)[0].1, None, "{name}");
        assert!(
            nils_registry::review::withdraw(reg, part, "dan@lab").is_err(),
            "{name}"
        );
    }
}

/// Review of E2, item 5: a later check of a result, a close or a direct
/// one, withdraws every earlier decision on its parts and clears their
/// statuses before it writes its own; a part it does not judge is left
/// unchecked.
#[test]
fn a_later_check_takes_the_place_of_every_earlier_one_on_the_parts() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, s1, _) = scans(reg);
        let d0 = mask(reg, s0, 1);
        let q = question("result");
        closed_check(
            reg,
            "first",
            &q,
            vec![item(d0, None, s0, s1)],
            &[(
                "flagged",
                json!({"parts": [{"label": 1, "reason": "under"}], "seen": [1, 2]}),
            )],
            0,
        );
        let st = reg.store();
        assert_eq!(
            part_status(st, d0)
                .iter()
                .map(|p| p.1.clone())
                .collect::<Vec<_>>(),
            vec![Some("flagged".into()), Some("accepted".into()), None],
            "{name}"
        );
        // a second check sees only the third lesion
        closed_check(
            reg,
            "second",
            &q,
            vec![item(d0, None, s0, s1)],
            &[("accepted", json!({"seen": [3]}))],
            30,
        );
        let st = reg.store();
        assert_eq!(
            part_status(st, d0)
                .iter()
                .map(|p| p.1.clone())
                .collect::<Vec<_>>(),
            vec![None, None, Some("accepted".into())],
            "{name}"
        );
        assert!(
            decision_ids(st, "derivative_part", &format!("{d0}:lesions:1"), true).is_empty(),
            "{name}"
        );
        assert_eq!(
            decision_ids(st, "derivative_part", &format!("{d0}:lesions:1"), false).len(),
            1,
            "{name}"
        );
        // a direct check flags the second lesion: the third gives way
        check::check_directly(
            reg,
            &check::Direct {
                derivative: d0,
                value: "flagged",
                reasons: &[],
                parts: &[(2, "over".into())],
                set: None,
                why: None,
                who: "dan@lab",
                kind: "person",
            },
        )
        .unwrap();
        let st = reg.store();
        assert_eq!(
            part_status(st, d0)
                .iter()
                .map(|p| p.1.clone())
                .collect::<Vec<_>>(),
            vec![None, Some("flagged".into()), None],
            "{name}"
        );
        assert!(
            decision_ids(st, "derivative_part", &format!("{d0}:lesions:3"), true).is_empty(),
            "{name}"
        );
    }
}

/// Review of E2, item 7: a check's item the close leaves unresolved has
/// its review item closed as superseded, with no decision, so nothing is
/// left open that nothing could close; a later check of the result asks
/// it again.
#[test]
fn a_check_s_unresolved_items_ask_nothing_after_the_close() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, s1, _) = scans(reg);
        let (d0, d1) = (mask(reg, s0, 1), mask(reg, s0, 2));
        let q = question("result");
        // two results, one read
        let c = closed_check(
            reg,
            "half",
            &q,
            vec![item(d0, None, s0, s1), item(d1, None, s0, s1)],
            &[("accepted", json!({}))],
            0,
        );
        let st = reg.store();
        let its = campaign::items(st, c).unwrap();
        let left = its.iter().find(|i| i.derivative_id == Some(d1)).unwrap();
        assert_ne!(left.state, "resolved", "{name}");
        let ri = nils_registry::review::item(st, left.review_item_id)
            .unwrap()
            .unwrap();
        assert_eq!(ri.status, "superseded", "{name}");
        assert!(
            decision_ids(st, "derivative", &d1.to_string(), false).is_empty(),
            "{name}"
        );
        // a later check asks it again
        closed_check(
            reg,
            "again",
            &q,
            vec![item(d1, None, s0, s1)],
            &[("accepted", json!({}))],
            30,
        );
        let st = reg.store();
        assert_eq!(
            derivative::get(st, d1)
                .unwrap()
                .unwrap()
                .check_status
                .as_deref(),
            Some("accepted"),
            "{name}"
        );
    }
}

/// Review of E2, item 8: the hold is read where the writing is. A result an
/// open check holds goes into no second check, whoever previewed it free;
/// and on Postgres a direct check that began while a check holding the
/// result was being written waits for it, then is refused.
#[test]
fn a_result_is_held_by_one_open_check_read_under_the_lock() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, s1, _) = scans(reg);
        let (d0, d1) = (mask(reg, s0, 1), mask(reg, s0, 2));
        let q = question("result");
        campaign::create(
            reg,
            &new("first", &q, vec![item(d0, None, s0, s1)], 1, vec![]),
        )
        .unwrap();
        let e = campaign::create(
            reg,
            &new(
                "second",
                &q,
                vec![item(d1, None, s0, s1), item(d0, None, s0, s1)],
                1,
                vec![],
            ),
        )
        .unwrap_err();
        assert!(matches!(e, campaign::Error::Refused(_)), "{name}: {e}");
        assert!(
            campaign::by_name(reg.store(), "second").unwrap().is_none(),
            "{name}: nothing of it written"
        );
        if name != "postgres" {
            continue;
        }
        // a writer holding d1's row writes a check item for it; the direct
        // check that starts meanwhile waits, then sees the hold
        let dsn = postgres_dsn().unwrap();
        let mut other = Store::connect_postgres(&dsn, SCHEMA).unwrap();
        other.begin().unwrap();
        let held = check::held_locked(&mut other, &[d1]).unwrap();
        assert!(held.is_empty(), "{name}");
        let c = row(
            &mut other,
            "campaign",
            &[
                ("name", Param::from("racing")),
                ("status", Param::from("open")),
            ],
        );
        row(
            &mut other,
            "campaign_item",
            &[
                ("campaign_id", Param::Int(c)),
                ("derivative_id", Param::Int(d1)),
                ("state", Param::from("open")),
            ],
        );
        let committer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            other.commit().unwrap();
        });
        let e = check::check_directly(
            reg,
            &check::Direct {
                derivative: d1,
                value: "accepted",
                reasons: &[],
                parts: &[],
                set: None,
                why: None,
                who: "dan@lab",
                kind: "person",
            },
        )
        .unwrap_err();
        committer.join().unwrap();
        assert!(matches!(e, campaign::Error::Refused(_)), "{name}: {e}");
        assert_eq!(
            derivative::get(reg.store(), d1)
                .unwrap()
                .unwrap()
                .check_status,
            None,
            "{name}"
        );
    }
}

/// Review of E2, item 10: a check is compared on its verdict by the engine;
/// an external metric, which would close mixed verdicts as the first
/// rater's, is refused when it is made.
#[test]
fn a_check_takes_no_external_metric() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, s1, _) = scans(reg);
        let d0 = mask(reg, s0, 1);
        let q = question("result");
        let external = json!({"when": "disagree", "metric": "external", "threshold": 0.8});
        let mut n = new("ext", &q, vec![item(d0, None, s0, s1)], 2, vec![]);
        n.adjudication = &external;
        let e = campaign::create(reg, &n).unwrap_err();
        assert!(e.to_string().contains("adjudication.metric"), "{name}: {e}");
        let kappa = json!({"metric": "kappa"});
        n.adjudication = &kappa;
        campaign::create(reg, &n).unwrap();
    }
}

/// Review of E1, item 11: a page of parts far past the set is empty, and
/// its offset never overflows.
#[test]
fn a_page_of_parts_past_the_set_is_empty() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, _, _) = scans(reg);
        let d0 = mask(reg, s0, 1);
        for (page, per_page) in [
            (usize::MAX, 500),
            (1, usize::MAX),
            (usize::MAX / 2, 3),
            (7, 500),
        ] {
            let (count, parts) = derivative::parts(
                reg.store(),
                d0,
                "lesions",
                derivative::PartOrder::Position,
                page,
                per_page,
            )
            .unwrap();
            assert_eq!(
                (count, parts.len()),
                (3, 0),
                "{name}: page {page} of {per_page}"
            );
        }
        let (_, two) = derivative::parts(
            reg.store(),
            d0,
            "lesions",
            derivative::PartOrder::Position,
            1,
            2,
        )
        .unwrap();
        assert_eq!(
            two.iter().map(|p| p.label).collect::<Vec<_>>(),
            [3],
            "{name}"
        );
    }
}

/// The id of the check in force on a result.
fn decisions_of(reg: &mut Registry, d: i64) -> i64 {
    derivative::get(reg.store(), d)
        .unwrap()
        .unwrap()
        .check_decision_id
        .unwrap()
}

/// The status of a review item, by id.
fn review_status(reg: &mut Registry, id: i64) -> (String, Value) {
    let ri = nils_registry::review::item(reg.store(), id)
        .unwrap()
        .unwrap();
    (ri.status, ri.evidence)
}

/// A check whose two raters agree on two results, with the second's outcome
/// spoiled as the T33 pipe test spoiled it (`parts` is no list).
fn two_results_one_spoiled(
    reg: &mut Registry,
    name: &'static str,
    d0: i64,
    d1: i64,
    s0: i64,
    s1: i64,
    minute: u32,
) -> (i64, i64, i64) {
    let q = question("result");
    let c = campaign::create(
        reg,
        &new(
            name,
            &q,
            vec![item(d0, None, s0, s1), item(d1, None, s0, s1)],
            2,
            vec![],
        ),
    )
    .unwrap();
    for (i, who) in ["anna@lab", "bo@lab", "anna@lab", "bo@lab"]
        .iter()
        .enumerate()
    {
        let form = if i < 2 {
            json!({"seen": [1, 2]})
        } else {
            json!({"parts": [{"label": 2, "reason": "under"}], "seen": [1, 2]})
        };
        let value = if i < 2 { "accepted" } else { "flagged" };
        read(
            reg,
            c.id,
            who,
            Role::Rater,
            value,
            form,
            None,
            minute + 2 * i as u32,
        )
        .unwrap();
    }
    let items = campaign::items(reg.store(), c.id).unwrap();
    let spoiled = items
        .iter()
        .find(|i| i.derivative_id == Some(d1))
        .unwrap()
        .clone();
    assert_eq!(spoiled.state, "agreed");
    set_outcome_form(
        reg,
        &spoiled,
        json!({"reasons": [], "parts": "x", "points": [], "seen": [], "shown": []}),
    );
    (c.id, spoiled.id, spoiled.review_item_id)
}

/// After the T33 pipe test: a check's close that cannot read an
/// item's outcome writes nothing for it, and the item says so. It is kept
/// refused, with when and why, its question stays open in the queue, said
/// to be refused, and the campaign says a person may close it again. A
/// later close by an agent is refused; a person's, once the outcome reads,
/// writes the item as any close does and clears the refusal; with nothing
/// left to write, the campaign is closed for good.
#[test]
fn a_refused_check_item_stays_refused_until_a_later_close_writes_it() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, s1, _) = scans(reg);
        let (d0, d1) = (mask(reg, s0, 11), mask(reg, s0, 12));
        let (c, refused_item, question_id) =
            two_results_one_spoiled(reg, "t33-refused", d0, d1, s0, s1, 0);

        let closed = close_as_person(reg, c, 20);
        assert_eq!(
            (closed.resolved, closed.unresolved, closed.refused.len()),
            (1, 1, 1),
            "{name}: {closed:?}"
        );
        assert_eq!(closed.refused[0].0, refused_item, "{name}");
        let why = closed.refused[0].1.clone();
        assert!(
            why.contains("cannot be read") && why.contains("form.parts is a list"),
            "{name}: {why}"
        );
        // the item: refused, with when and why, and nothing written for it
        let it = campaign::item(reg.store(), refused_item).unwrap().unwrap();
        assert_eq!(it.state, "refused", "{name}");
        assert_eq!(it.refused_why.as_deref(), Some(why.as_str()), "{name}");
        assert_eq!(
            it.refused_at.as_deref().map(|t| &t[..16]),
            Some("2026-10-11T11:20"),
            "{name}"
        );
        assert_eq!(it.as_json()["refused"]["why"], json!(why), "{name}");
        assert!(
            decisions(reg.store(), "derivative", &d1.to_string()).is_empty(),
            "{name}"
        );
        // its question stays open, said to be refused
        let (status, evidence) = review_status(reg, question_id);
        assert_eq!(status, "open", "{name}");
        assert_eq!(evidence["campaign_state"], "refused", "{name}");
        // the campaign is closed, counts the item as refused, and may be
        // closed again
        let camp = campaign::get(reg.store(), c).unwrap().unwrap();
        assert_eq!(camp.status, "closed", "{name}");
        assert!(
            campaign::closes_again(reg.store(), &camp).unwrap(),
            "{name}"
        );
        let counts = campaign::counts(reg.store(), c).unwrap();
        assert_eq!(counts["items"]["refused"], 1, "{name}: {counts}");
        assert_eq!(counts["items"]["resolved"], 1, "{name}: {counts}");

        // a later close while the outcome still cannot be read refuses it
        // again, and nothing else changes
        let again = close_as_person(reg, c, 24);
        assert_eq!(
            (again.resolved, again.refused.len(), again.decisions.len()),
            (1, 1, 0),
            "{name}: {again:?}"
        );
        assert_eq!(review_status(reg, question_id).0, "open", "{name}");

        // an agent never closes it again: a check is a person's
        let e = campaign::close(
            reg,
            &Close {
                campaign: c,
                who: "bot@lab",
                author_kind: "agent",
                model: None,
                picks: None,
                sealed: Some(&none_sealed),
            },
            &at(26),
        )
        .unwrap_err();
        assert!(
            e.to_string().contains("a person closes it again"),
            "{name}: {e}"
        );

        // the outcome reads again (settled again, or the row mended): a
        // person's later close writes it as usual
        let it = campaign::item(reg.store(), refused_item).unwrap().unwrap();
        set_outcome_form(
            reg,
            &it,
            json!({"reasons": [], "parts": [{"label": 2, "reason": "under"}], "points": [], "seen": [1, 2], "shown": []}),
        );
        let later = close_as_person(reg, c, 30);
        assert_eq!(
            (later.resolved, later.unresolved, later.refused.len()),
            (2, 0, 0),
            "{name}: {later:?}"
        );
        assert_eq!(
            later.decisions,
            vec![decisions_of(reg, d1)],
            "{name}: the result's"
        );
        let it = campaign::item(reg.store(), refused_item).unwrap().unwrap();
        assert_eq!(it.state, "resolved", "{name}");
        assert_eq!((&it.refused_at, &it.refused_why), (&None, &None), "{name}");
        assert_eq!(it.as_json()["refused"], Value::Null, "{name}");
        let st = reg.store();
        assert_eq!(
            decisions(st, "derivative", &d1.to_string()),
            vec![("flagged".into(), None, false)],
            "{name}"
        );
        assert_eq!(
            part_status(st, d1)[..2],
            [
                (1, Some("accepted".into()), None),
                (2, Some("flagged".into()), Some("under".into()))
            ],
            "{name}"
        );
        assert_eq!(review_status(reg, question_id).0, "accepted", "{name}");
        let camp = campaign::get(reg.store(), c).unwrap().unwrap();
        assert_eq!(camp.status, "closed", "{name}");
        assert_eq!(
            camp.closed_at.as_deref().map(|t| &t[..16]),
            Some("2026-10-11T11:30"),
            "{name}"
        );
        assert!(
            !campaign::closes_again(reg.store(), &camp).unwrap(),
            "{name}"
        );
        // nothing is left to write: closed for good
        let e = campaign::close(
            reg,
            &Close {
                campaign: c,
                who: "cleo@lab",
                author_kind: "person",
                model: None,
                picks: None,
                sealed: Some(&none_sealed),
            },
            &at(40),
        )
        .unwrap_err();
        assert!(e.to_string().contains("is closed"), "{name}: {e}");
    }
}

/// A later close never writes an older judgement over a newer one: while
/// another open check holds the result, the item stays refused and its
/// question open; once that check has written the result, the later close
/// leaves its check standing, keeps the item refused saying so, and closes
/// its question. A direct check made since does the same.
#[test]
fn a_later_close_leaves_a_newer_check_of_the_result_standing() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, s1, _) = scans(reg);
        let (d0, d1, d2) = (mask(reg, s0, 21), mask(reg, s0, 22), mask(reg, s0, 23));
        let (c, refused_item, question_id) =
            two_results_one_spoiled(reg, "t33-older", d0, d1, s0, s1, 0);
        close_as_person(reg, c, 20);
        let mend = |reg: &mut Registry, item_id: i64| {
            let it = campaign::item(reg.store(), item_id).unwrap().unwrap();
            set_outcome_form(
                reg,
                &it,
                json!({"reasons": [], "parts": [], "points": [], "seen": [1], "shown": []}),
            );
        };
        mend(reg, refused_item);

        // a newer check holds the result: it decides it
        let q = question("result");
        let newer = campaign::create(
            reg,
            &new("t33-newer", &q, vec![item(d1, None, s0, s1)], 1, vec![]),
        )
        .unwrap();
        let held = close_as_person(reg, c, 30);
        assert_eq!(held.refused.len(), 1, "{name}: {held:?}");
        assert!(
            held.refused[0]
                .1
                .contains("held by the open check t33-newer"),
            "{name}: {held:?}"
        );
        assert_eq!(review_status(reg, question_id).0, "open", "{name}");
        assert!(
            decisions(reg.store(), "derivative", &d1.to_string()).is_empty(),
            "{name}"
        );

        // the newer check closes and writes the result
        read(
            reg,
            newer.id,
            "anna@lab",
            Role::Rater,
            "rejected",
            json!({}),
            None,
            32,
        )
        .unwrap();
        let wrote = close_as_person(reg, newer.id, 40);
        assert_eq!(wrote.resolved, 1, "{name}: {wrote:?}");
        // the older campaign's later close leaves that check standing
        let camp = campaign::get(reg.store(), c).unwrap().unwrap();
        assert!(
            campaign::closes_again(reg.store(), &camp).unwrap(),
            "{name}"
        );
        let later = close_as_person(reg, c, 50);
        assert_eq!(
            (later.decisions.len(), later.refused.len()),
            (0, 1),
            "{name}: {later:?}"
        );
        let why = &later.refused[0].1;
        assert!(
            why.contains(&format!("result {d1} was checked again"))
                && why.contains("that check stands"),
            "{name}: {why}"
        );
        assert_eq!(
            decisions(reg.store(), "derivative", &d1.to_string()),
            vec![("rejected".into(), None, false)],
            "{name}: the newer check stands"
        );
        assert_eq!(
            derivative::get(reg.store(), d1)
                .unwrap()
                .unwrap()
                .check_status
                .as_deref(),
            Some("rejected"),
            "{name}"
        );
        let it = campaign::item(reg.store(), refused_item).unwrap().unwrap();
        assert_eq!(it.state, "refused", "{name}");
        assert_eq!(it.refused_why.as_deref(), Some(why.as_str()), "{name}");
        // its question is closed by the newer check, and nothing is left to
        // write
        assert_eq!(review_status(reg, question_id).0, "superseded", "{name}");
        let camp = campaign::get(reg.store(), c).unwrap().unwrap();
        assert!(
            !campaign::closes_again(reg.store(), &camp).unwrap(),
            "{name}"
        );

        // a direct check made since the refusal stands the same way
        let (c2, refused2, question2) =
            two_results_one_spoiled(reg, "t33-direct", d0, d2, s0, s1, 0);
        // d0 was checked by the first campaign: this one writes it again
        let first = close_as_person(reg, c2, 20);
        assert_eq!(first.refused.len(), 1, "{name}: {first:?}");
        let direct = check::check_directly(
            reg,
            &check::Direct {
                derivative: d2,
                value: "accepted",
                reasons: &[],
                parts: &[],
                set: Some("lesions"),
                why: None,
                who: "dan@lab",
                kind: "person",
            },
        )
        .unwrap();
        mend(reg, refused2);
        let later = close_as_person(reg, c2, 59);
        assert_eq!(later.refused.len(), 1, "{name}: {later:?}");
        assert!(
            later.refused[0]
                .1
                .contains(&format!("(decision {})", direct.decision)),
            "{name}: {later:?}"
        );
        assert_eq!(
            derivative::get(reg.store(), d2)
                .unwrap()
                .unwrap()
                .check_decision_id,
            Some(direct.decision),
            "{name}"
        );
        assert_eq!(review_status(reg, question2).0, "superseded", "{name}");
    }
}

/// The engine's reading where one result was made from a stack of a sample
/// sealed now: the result whose sha256 the test names below.
fn sealed_one(
    reg: &mut Registry,
    results: &[i64],
) -> Result<std::collections::BTreeSet<i64>, String> {
    let mut out = std::collections::BTreeSet::new();
    for d in results {
        let row = derivative::get(reg.store(), *d)
            .map_err(|e| e.to_string())?
            .ok_or("no such result")?;
        if row.sha256 == format!("{:064x}", 2) {
            out.insert(*d);
        }
    }
    Ok(out)
}

/// Record 55, the rulings of 2026-10-12, item 6: a check never holds a
/// sealed scan's result. A check made before the seal holds two results,
/// one of them made from a stack sealed now, as the engine reads it. A
/// rater is never offered what the engine withholds from them, nor handed
/// back a lease on it; the close writes the other result and leaves the
/// sealed one's item as it was, counted in `left_out_sealed` and never
/// named, with no decision and no status on its result, whoever closes; a
/// close without the engine's reading writes no item of a check and says
/// why.
#[test]
fn a_check_s_close_writes_nothing_of_a_result_made_from_a_sealed_stack() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let (s0, s1, _) = scans(reg);
        let (open, sealed) = (mask(reg, s0, 1), mask(reg, s0, 2));
        let q = question("result");
        let c = campaign::create(
            reg,
            &new(
                "sealed-before",
                &q,
                vec![item(sealed, None, s0, s1), item(open, None, s0, s1)],
                1,
                vec![],
            ),
        )
        .unwrap();
        let items = campaign::items(reg.store(), c.id).unwrap();
        let (i_sealed, i_open) = (items[0].id, items[1].id);
        let withheld: std::collections::BTreeSet<i64> = [i_sealed].into();
        let claim = |reg: &mut Registry, withheld: &std::collections::BTreeSet<i64>, minute| {
            campaign::claim_withholding(
                reg,
                c.id,
                "anna@lab",
                Role::Rater,
                campaign::Order::Position,
                false,
                withheld,
                &at(minute),
            )
            .unwrap()
        };
        let answer = |reg: &mut Registry, assignment: i64, minute| {
            campaign::answer(
                reg,
                &Given {
                    assignment,
                    principal: "anna@lab",
                    author_kind: "person",
                    model: None,
                    value: Some("accepted"),
                    form: Some(&json!({"seen": [1]})),
                    derivative_id: None,
                    why: None,
                    unsure: false,
                },
                &at(minute),
            )
            .unwrap()
        };
        // first by position, the sealed one is passed over, and nothing is
        // said to come after
        let first = claim(reg, &withheld, 1).expect("the open one");
        assert_eq!(first.item.id, i_open, "{name}");
        assert!(first.ahead.is_empty(), "{name}: {:?}", first.ahead);
        answer(reg, first.assignment.id, 2);
        assert!(claim(reg, &withheld, 3).is_none(), "{name}");
        // a lease on it from before is not handed back, nor a second item
        let before = claim(reg, &Default::default(), 4).expect("the sealed one");
        assert_eq!(before.item.id, i_sealed, "{name}");
        assert!(claim(reg, &withheld, 5).is_none(), "{name}");
        assert!(
            claim(reg, &Default::default(), 6)
                .expect("handed back")
                .held,
            "{name}"
        );
        answer(reg, before.assignment.id, 7);

        // a close without the engine's reading writes no item of a check
        let close = |reg: &mut Registry, sealed: Option<campaign::SealedResults<'_>>, minute| {
            campaign::close(
                reg,
                &Close {
                    campaign: c.id,
                    who: "cleo@lab",
                    author_kind: "person",
                    model: None,
                    picks: None,
                    sealed,
                },
                &at(minute),
            )
            .unwrap()
        };
        let blind = close(reg, None, 10);
        assert_eq!(blind.resolved, 0, "{name}: {blind:?}");
        assert_eq!(blind.refused.len(), 2, "{name}: {blind:?}");
        assert!(
            blind.refused[0].1.contains("engine's reading"),
            "{name}: {blind:?}"
        );
        assert!(
            decision_ids(reg.store(), "derivative", &open.to_string(), true).is_empty(),
            "{name}"
        );
        // with it, a person's later close writes the open one and leaves the
        // sealed one as it was, counted and never named
        let closed = close(reg, Some(&sealed_one), 12);
        assert_eq!(closed.resolved, 1, "{name}: {closed:?}");
        assert_eq!(closed.left_out_sealed, 1, "{name}: {closed:?}");
        assert!(closed.refused.is_empty(), "{name}: {closed:?}");
        let st = reg.store();
        assert_eq!(
            derivative::get(st, open)
                .unwrap()
                .unwrap()
                .check_status
                .as_deref(),
            Some("accepted"),
            "{name}"
        );
        let row = derivative::get(st, sealed).unwrap().unwrap();
        assert!(
            row.check_status.is_none() && row.check_decision_id.is_none(),
            "{name}"
        );
        assert!(
            decision_ids(st, "derivative", &sealed.to_string(), false).is_empty(),
            "{name}"
        );
        assert!(
            decision_ids(st, "derivative_part", &format!("{sealed}:lesions:1"), false).is_empty(),
            "{name}"
        );
        assert_eq!(
            campaign::item(st, i_sealed).unwrap().unwrap().state,
            "refused",
            "{name}: left as it was"
        );
    }
}
