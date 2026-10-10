// SPDX-License-Identifier: AGPL-3.0-only

//! Classifying a digested registry: the verdict, the evidence that made it,
//! and the decision that outranks it.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_registry::home::{Home, InitOptions};
use nils_registry::store::{Insert, Param};
use nils_registry::{Backend, Registry, Row, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_run";

fn postgres_dsn() -> Option<String> {
    env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
}

struct Lab {
    name: &'static str,
    home: Home,
    _dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Lab {
    fn new(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
        let dir = TempDir::new("classify-home");
        let home = Home::new(dir.path());
        home.keys(None).add("k", b"nils-classify-run-key").unwrap();
        home.init(&InitOptions {
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
            home,
            _dir: dir,
            _guard: None,
        }
    }
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

fn labs() -> Vec<Lab> {
    let mut out = vec![Lab::new("sqlite", Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
            ))
            .expect("drop");
        let mut lab = Lab::new("postgres", Backend::Postgres, Some(dsn));
        lab._guard = Some(guard);
        out.push(lab);
    }
    out
}

fn rows(reg: &mut Registry, sql: &str) -> Vec<Row> {
    let mut text = sql.to_string();
    for t in [
        "classification_evidence",
        "classification_axis",
        "classification",
        "stack_fingerprint",
        "review_item",
        "review_member",
        "decision",
        "stack",
        "diagnostic",
        "instance",
        "series",
    ] {
        text = text.replace(&format!("{{{t}}}"), &reg.store().qualified(t));
    }
    reg.store().query(&text, &[]).unwrap()
}

fn one(reg: &mut Registry, sql: &str) -> i64 {
    rows(reg, sql)[0].int(0).unwrap()
}

fn packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

fn elem(tag: dicom_core::Tag, vr: VR, v: &str) -> synth::Elem {
    synth::text(tag, vr, v)
}

/// One unmistakable stack: a Siemens MPRAGE.
fn tree() -> TempDir {
    let dir = TempDir::new("classify");
    let mut e = synth::minimal_mr("A", "A.1", "A.1.1");
    e.push(elem(tags::PATIENT_ID, VR::LO, "P1"));
    e.extend([
        elem(tags::SERIES_DESCRIPTION, VR::LO, "sag T1 mprage"),
        elem(tags::SCANNING_SEQUENCE, VR::CS, "GR"),
        elem(tags::SEQUENCE_VARIANT, VR::CS, "SK\\SP\\MP"),
        elem(tags::SEQUENCE_NAME, VR::SH, "*tfl3d1_16"),
        elem(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
        elem(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND\\NORM"),
        elem(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
    ]);
    dir.file("s/1", &synth::part10(&MetaFields::mr("A.1.1"), &e, true));
    dir
}

fn prepare(lab: &Lab, dir: &TempDir) -> Registry {
    let mut reg = lab.home.open().unwrap();
    let mut s = nils_digest::Settings::new(dir.path());
    s.name = "t".into();
    s.workers = 2;
    s.walk_threads = 2;
    digest(&s, &mut reg).unwrap();
    nils_classify::run(
        &mut reg,
        &nils_classify::Settings::default(),
        &Cancel::new(),
    )
    .unwrap();
    reg
}

#[test]
fn a_verdict_is_written_with_the_evidence_that_made_it() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(report.written, 1, "{name}");
        assert_eq!(report.no_pack, 0, "{name}");
        assert!(report.evidence > 0, "{name}");

        let got: Vec<(String, String)> = rows(
            &mut reg,
            "SELECT axis, COALESCE(value, '') FROM {classification_axis} ORDER BY axis",
        )
        .iter()
        .map(|r| (r.text(0).unwrap().into(), r.text(1).unwrap().into()))
        .collect();
        let by = |a: &str| {
            got.iter()
                .find(|(x, _)| x == a)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(by("technique"), "MPRAGE", "{name}: {got:?}");
        assert_eq!(by("base"), "T1w", "{name}");
        assert_eq!(by("directory_type"), "anat", "{name}");

        // and the evidence says why, by rule set, rule and what matched
        let ev: Vec<(String, String, String)> = rows(
            &mut reg,
            "SELECT axis, rule_set, COALESCE(matched, '') FROM {classification_evidence} WHERE axis = 'technique'",
        )
        .iter()
        .map(|r| {
            (
                r.text(0).unwrap().into(),
                r.text(1).unwrap().into(),
                r.text(2).unwrap().into(),
            )
        })
        .collect();
        assert_eq!(ev.len(), 1, "{name}: {ev:?}");
        assert_eq!(ev[0].1, "technique", "{name}");
        assert!(
            !ev[0].2.is_empty(),
            "{name}: the evidence names what matched"
        );

        // the row says which pack judged it, which is what makes a
        // re-classification a diff rather than an overwrite
        assert_eq!(
            rows(&mut reg, "SELECT pack, pack_version FROM {classification}")[0]
                .text(1)
                .unwrap(),
            pack.version.to_string(),
            "{name}"
        );
    }
}

#[test]
fn a_decision_outranks_the_rule_and_survives_a_re_classification() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let stack = one(&mut reg, "SELECT stack_id FROM {classification}");

        // A person says it is a T2w, and disagrees with the rule.
        reg.store()
            .insert(
                &Insert::new(
                    nils_registry::schema::table("decision"),
                    &[
                        "scope",
                        "ref",
                        "axis",
                        "value",
                        "actor",
                        "author_kind",
                        "why",
                        "decided_at",
                    ],
                ),
                &[vec![
                    Param::from("stack"),
                    Param::from(stack.to_string()),
                    Param::from("base"),
                    Param::from("T2w"),
                    Param::from("a person"),
                    Param::from("person"),
                    Param::from("checked by eye"),
                    Param::from(nils_registry::time::now_iso()),
                ]],
            )
            .unwrap();

        let again =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(
            rows(
                &mut reg,
                "SELECT value, tier FROM {classification_axis} WHERE axis = 'base'"
            )[0]
            .text(0)
            .unwrap(),
            "T2w",
            "{name}: the decision wins"
        );
        assert_eq!(
            rows(
                &mut reg,
                "SELECT value, tier FROM {classification_axis} WHERE axis = 'base'"
            )[0]
            .text(1)
            .unwrap(),
            "decision",
            "{name}: and the row says so"
        );
        assert!(
            again.review_items > 0,
            "{name}: the rule still disagrees, so a person is told"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind = 'base:decision'"
            ),
            1,
            "{name}"
        );

        // The evidence still records what the rule said, so the disagreement
        // is legible rather than lost.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_evidence} WHERE axis = 'base' AND value = 'T1w'"
            ),
            1,
            "{name}"
        );
    }
}

#[test]
fn a_modality_the_pack_does_not_judge_is_said_so_and_not_guessed() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("classify-ct");
        let mut e = synth::minimal_ct("B", "B.1", "B.1.1");
        e.push(elem(tags::PATIENT_ID, VR::LO, "P2"));
        dir.file("s/1", &synth::part10(&MetaFields::ct("B.1.1"), &e, true));
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(report.no_pack, 1, "{name}");
        assert_eq!(report.written, 0, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {review_item}"),
            0,
            "{name}: no pack is an outcome, not a question for a person"
        );
    }
}

#[test]
fn a_decision_about_an_origin_governs_every_stack_of_it_until_one_is_looked_at() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        let stack = {
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
            one(&mut reg, "SELECT stack_id FROM {classification}")
        };
        let decide = |reg: &mut Registry, scope: &str, reference: &str, value: &str| {
            reg.store()
                .insert(
                    &Insert::new(
                        nils_registry::schema::table("decision"),
                        &[
                            "scope",
                            "ref",
                            "axis",
                            "value",
                            "actor",
                            "author_kind",
                            "decided_at",
                        ],
                    ),
                    &[vec![
                        Param::from(scope),
                        Param::from(reference),
                        Param::from("base"),
                        Param::from(value),
                        Param::from("a person"),
                        Param::from("person"),
                        Param::from(nils_registry::time::now_iso()),
                    ]],
                )
                .unwrap();
        };
        let base = |reg: &mut Registry| -> String {
            rows(
                reg,
                "SELECT value FROM {classification_axis} WHERE axis = 'base'",
            )[0]
            .text(0)
            .unwrap()
            .to_string()
        };

        // The whole scanner is called wrong, and every stack it made follows.
        decide(&mut reg, "origin", "manufacturer=synthetic", "PDw");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        assert_eq!(base(&mut reg), "PDw", "{name}: the origin decides");

        // Someone looks at one series of it and says otherwise, and the
        // narrower call wins where it applies.
        let series = one(&mut reg, "SELECT series_id FROM {stack} WHERE id = 1");
        decide(&mut reg, "series", &series.to_string(), "T2w");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        assert_eq!(base(&mut reg), "T2w", "{name}: the series is closer");

        // And this one stack is closer still.
        decide(&mut reg, "stack", &stack.to_string(), "FLAIR");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        assert_eq!(base(&mut reg), "FLAIR", "{name}: the stack is closest");
    }
}

/// A pass does not read a pass.
///
/// The reference is built from `classification_axis`, which holds whatever
/// decided a value. On any run after the first that includes what an earlier
/// run's pass wrote, so without this the vote counts its own guesses as
/// evidence for the next guess. Measured on the live corpus, that is the
/// difference between an archive that can be sorted again and one that cannot:
/// from two ingest histories, a second sort agrees on 14 of 9,014 answers with
/// the pass's answers in the reference, and on all 31,880 without them.
#[test]
fn the_reference_holds_what_a_rule_decided_and_not_what_a_pass_did() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    let dir = tree();
    for lab in labs() {
        let name = lab.name;
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();

        let pool = |reg: &mut Registry| {
            nils_classify::passes::run(
                reg.store(),
                &pack,
                &Default::default(),
                &Cancel::new(),
                nils_pack::pass::Phase::After,
                1,
            )
            .unwrap()
            .into_iter()
            .find(|r| r.kind == "nearest_neighbour_vote")
            .expect("the physics vote ran")
            .pool
        };
        assert_eq!(
            pool(&mut reg),
            1,
            "{name}: the one stack the rules decided is the whole reference"
        );

        // The same stack, with its base now attributed to the pass, which is
        // what the row looks like on the run after a fill.
        let evidence = reg.store().qualified("classification_evidence");
        let sql = format!("UPDATE {evidence} SET pass = 'physics_vote' WHERE axis = 'base'");
        reg.store().execute(&sql, &[]).unwrap();
        assert_eq!(
            pool(&mut reg),
            0,
            "{name}: a value a pass wrote is not evidence for the next vote"
        );
    }
}

/// Wave 3 §10.1. In the live v0 archive, 4,692 body parts are an image model's
/// predictions, committed by a person through its body-part QC straight into
/// the classifier's own column with nothing to mark them. They are
/// discoverable at all only because v0's keyword classifier disagrees,
/// answering nothing for 4,692 of that cohort's 4,699 stacks.
///
/// So the thing to check is not that the value is stored. It is that a value a
/// model produced cannot sit where a rule's answer belongs and read the same.
#[test]
fn a_model_s_answer_does_not_read_like_a_rule_s() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    let dir = tree();
    for lab in labs() {
        let name = lab.name;
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let stack = one(&mut reg, "SELECT stack_id FROM {classification}");

        let decide = |reg: &mut Registry,
                      who: &str,
                      kind: &str,
                      version: Option<&str>,
                      axis: &str,
                      value: &str| {
            reg.store()
                .insert(
                    &Insert::new(
                        nils_registry::schema::table("decision"),
                        &[
                            "scope",
                            "ref",
                            "axis",
                            "value",
                            "actor",
                            "author_kind",
                            "author_version",
                            "decided_at",
                        ],
                    ),
                    &[vec![
                        Param::from("stack"),
                        Param::from(stack.to_string()),
                        Param::from(axis),
                        Param::from(value),
                        Param::from(who),
                        Param::from(kind),
                        match version {
                            Some(v) => Param::from(v),
                            None => Param::Null,
                        },
                        Param::from(nils_registry::time::now_iso()),
                    ]],
                )
                .unwrap();
        };
        decide(
            &mut reg,
            "bodypart-net",
            "model",
            Some("2.1.0"),
            "body_part",
            "brain",
        );
        // And a person overriding an axis a rule did answer, which is the
        // other shape the same question comes in.
        decide(&mut reg, "a person", "person", None, "base", "T2w");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();

        // The value is what the model said, and the row that carries it names
        // the model and its version. No rule wrote it and none claims to.
        let r = &rows(
            &mut reg,
            "SELECT value, author, author_kind, matched, rule_set FROM {classification_evidence} \
             WHERE axis = 'body_part' AND author_kind IS NOT NULL",
        )[0];
        assert_eq!(r.text(0).unwrap(), "brain", "{name}");
        assert_eq!(r.text(1).unwrap(), "bodypart-net", "{name}");
        assert_eq!(r.text(2).unwrap(), "model", "{name}");
        assert_eq!(r.text(3).unwrap(), "2.1.0", "{name}: which model");
        assert_eq!(r.text(4).unwrap(), "decision", "{name}: not a rule set");

        // The rules said nothing about this axis for this stack, which is the
        // case v0's 4,692 are: an engine that could not record an answer here
        // would leave a person no place to put one but the rules' own column.
        let claimed = one(
            &mut reg,
            "SELECT COUNT(*) FROM {classification_evidence} \
             WHERE axis = 'body_part' AND author_kind IS NULL",
        );
        assert_eq!(claimed, 0, "{name}: no rule claims the body part");

        // Where a rule did answer, its answer survives beside the person's:
        // the disagreement is visible rather than overwritten.
        let by_a_rule = one(
            &mut reg,
            "SELECT COUNT(*) FROM {classification_evidence} \
             WHERE axis = 'base' AND author_kind IS NULL",
        );
        assert!(by_a_rule > 0, "{name}: the rule's own answer survives");
        assert_eq!(
            rows(
                &mut reg,
                "SELECT value FROM {classification_axis} WHERE axis = 'base'"
            )[0]
            .text(0)
            .unwrap(),
            "T2w",
            "{name}: and the person's is what the axis says"
        );

        // The whole point, stated as the query an auditor would run: every
        // value that did not come from a rule can be found.
        let authored = one(
            &mut reg,
            "SELECT COUNT(*) FROM {classification_evidence} WHERE author_kind = 'model'",
        );
        assert_eq!(authored, 1, "{name}");
    }
}

/// Wave 4a §10.2, on both backends: one question about a rule is one item
/// with n members; a decision on it is one row with the group's scope and
/// reaches every member on the next run; a staged decision is not in
/// force until committed, and a commit is refused when the registry moved
/// on; a withdrawal reopens what the decision closed; a person's decision
/// is not overridden by an agent's.
#[test]
fn the_review_spine_groups_questions_and_a_decision_reaches_the_group() {
    use nils_registry::review::{self, Apply, Author};
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        // Two stacks of one series description, so the same question is
        // asked twice and grouped once.
        let dir = TempDir::new("classify-spine");
        for (n, sop) in [("1", "A.1.1"), ("2", "A.2.1")] {
            let mut e = synth::minimal_mr("A", &format!("A.{n}"), sop);
            e.push(elem(tags::PATIENT_ID, VR::LO, "P1"));
            e.push(elem(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
            dir.file(
                &format!("s{n}/1"),
                &synth::part10(&MetaFields::mr(sop), &e, true),
            );
        }
        let mut reg = prepare(&lab, &dir);
        let settings = nils_classify::job::Settings {
            review_below: Some(1.0),
            ..Default::default()
        };
        let first =
            nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert_eq!(first.written, 2, "{name}");
        assert!(first.review_items >= 2, "{name}: {first:?}");
        assert!(
            first.review_groups < first.review_items,
            "{name}: grouped: {} question(s) for {} item(s)",
            first.review_groups,
            first.review_items
        );
        // Every open classifier question is a group now, with its members.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE status = 'open' AND scope = 'stack' AND kind LIKE '%:%'"
            ),
            0,
            "{name}: no per-stack rows are left"
        );
        let groups = rows(
            &mut reg,
            "SELECT id, kind, members FROM {review_item} WHERE status = 'open' AND scope = 'group' AND kind = 'base:low_confidence'",
        );
        assert_eq!(groups.len(), 1, "{name}: one question about base");
        let group = groups[0].int(0).unwrap();
        assert_eq!(groups[0].int(2).unwrap(), 2, "{name}: with two members");
        let members = review::members(reg.store(), group).unwrap();
        assert_eq!(members.len(), 2, "{name}");
        assert!(members.iter().all(|m| m.decided_at.is_none()), "{name}");

        // One decision for the group: one row, scope group, and both stacks
        // read T2w on the next run.
        let applied = review::apply(
            &mut reg,
            &Apply {
                item: group,
                member: None,
                scope: "stack",
                value: Some("T2w"),
                author: Author {
                    who: "anna@ward-3",
                    kind: "person",
                    version: None,
                    model: None,
                },
                stage: false,
                why: Some("checked both"),
                campaign: None,
            },
        )
        .unwrap();
        assert_eq!(applied.scope, "group", "{name}");
        assert_eq!(applied.members, 2, "{name}");
        assert_eq!(applied.closed, vec![group], "{name}");
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {decision} WHERE scope = 'group'"
            ),
            1,
            "{name}: one row"
        );
        nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        let bases = rows(
            &mut reg,
            "SELECT value, tier FROM {classification_axis} WHERE axis = 'base' ORDER BY stack_id",
        );
        assert_eq!(bases.len(), 2, "{name}");
        for b in &bases {
            assert_eq!(
                b.text(0).unwrap(),
                "T2w",
                "{name}: the group decision reaches each member"
            );
            assert_eq!(b.text(1).unwrap(), "decision", "{name}");
        }
        // The answered item stays accepted; the run asked its new questions
        // as new items (C15), among them the disagreement with the rule.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE scope = 'group' AND kind = 'base:decision' AND status = 'open'"
            ),
            1,
            "{name}"
        );

        // C15 across scopes: an agent's call about one member, narrower
        // than the person's about the group, does not win. It is written
        // (a different key), and the next run still reads the person's.
        let disagreement = rows(
            &mut reg,
            "SELECT id FROM {review_item} WHERE status = 'open' AND scope = 'group' AND kind = 'base:decision'",
        )[0]
        .int(0)
        .unwrap();
        let stack_1 = review::members(reg.store(), disagreement).unwrap()[0].stack_id;
        review::apply(
            &mut reg,
            &Apply {
                item: disagreement,
                member: Some(stack_1),
                scope: "stack",
                value: Some("PDw"),
                author: Author {
                    who: "bot@ward-3",
                    kind: "agent",
                    version: None,
                    model: None,
                },
                stage: false,
                why: None,
                campaign: None,
            },
        )
        .unwrap();
        nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert!(
            rows(
                &mut reg,
                "SELECT value FROM {classification_axis} WHERE axis = 'base'"
            )
            .iter()
            .all(|r| r.text(0).unwrap() == "T2w"),
            "{name}: the person's group decision outranks the agent's narrower one"
        );
        // C15 on one key: once a person decided the stack itself, an agent
        // is refused on that key.
        let open_disagreement = rows(
            &mut reg,
            "SELECT id FROM {review_item} WHERE status = 'open' AND scope = 'group' AND kind = 'base:decision'",
        )[0]
        .int(0)
        .unwrap();
        review::apply(
            &mut reg,
            &Apply {
                item: open_disagreement,
                member: Some(stack_1),
                scope: "stack",
                value: Some("T2w"),
                author: Author {
                    who: "anna@ward-3",
                    kind: "person",
                    version: None,
                    model: None,
                },
                stage: false,
                why: None,
                campaign: None,
            },
        )
        .unwrap();
        nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        let open_disagreement = rows(
            &mut reg,
            "SELECT id FROM {review_item} WHERE status = 'open' AND scope = 'group' AND kind = 'base:decision'",
        )[0]
        .int(0)
        .unwrap();
        let refused = review::apply(
            &mut reg,
            &Apply {
                item: open_disagreement,
                member: Some(stack_1),
                scope: "stack",
                value: Some("PDw"),
                author: Author {
                    who: "bot@ward-3",
                    kind: "agent",
                    version: None,
                    model: None,
                },
                stage: false,
                why: None,
                campaign: None,
            },
        );
        assert!(
            matches!(&refused, Err(review::Error::Refused(m)) if m.contains("C15")),
            "{name}: {refused:?}"
        );

        // A staged decision on another question is written but not in
        // force; the registry moves on, so the commit is refused
        // until --anyway; committed, it is in force.
        let technique = rows(
            &mut reg,
            "SELECT id FROM {review_item} WHERE status = 'open' AND scope = 'group' AND kind = 'technique:low_confidence'",
        );
        assert_eq!(technique.len(), 1, "{name}");
        let technique = technique[0].int(0).unwrap();
        let staged = review::apply(
            &mut reg,
            &Apply {
                item: technique,
                member: None,
                scope: "stack",
                value: Some("FLAIR"),
                author: Author {
                    who: "anna@ward-3",
                    kind: "person",
                    version: None,
                    model: None,
                },
                stage: true,
                why: None,
                campaign: None,
            },
        )
        .unwrap();
        assert!(staged.staged, "{name}");
        assert_eq!(
            rows(
                &mut reg,
                "SELECT status FROM {review_item} WHERE id = {technique_id}"
                    .replace("{technique_id}", &technique.to_string())
                    .as_str()
            )[0]
            .text(0)
            .unwrap(),
            "staged",
            "{name}"
        );
        nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert!(
            rows(
                &mut reg,
                "SELECT tier FROM {classification_axis} WHERE axis = 'technique'"
            )
            .iter()
            .all(|r| r.text(0).unwrap() != "decision"),
            "{name}: staged is not in force"
        );
        // The registry moves on: an act that changes a judgement advances
        // the epoch. Staging does not (record 42), and neither does a run.
        reg.next_epoch().unwrap();
        let drift = review::commit(&mut reg, Some(staged.decision), false, "anna@ward-3");
        assert!(
            matches!(&drift, Err(review::Error::Refused(m)) if m.contains("moved on")),
            "{name}: {drift:?}"
        );
        let committed =
            review::commit(&mut reg, Some(staged.decision), true, "anna@ward-3").unwrap();
        assert_eq!(committed.decisions, vec![staged.decision], "{name}");
        nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert!(
            rows(
                &mut reg,
                "SELECT value, tier FROM {classification_axis} WHERE axis = 'technique'"
            )
            .iter()
            .all(|r| r.text(0).unwrap() == "FLAIR" && r.text(1).unwrap() == "decision"),
            "{name}: committed is in force"
        );

        // Withdrawn: the rule's answer is back, and the question is open again.
        let reopened = review::withdraw(&mut reg, staged.decision, "anna@ward-3").unwrap();
        assert!(reopened >= 1, "{name}: {reopened}");
        nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert!(
            rows(
                &mut reg,
                "SELECT tier FROM {classification_axis} WHERE axis = 'technique'"
            )
            .iter()
            .all(|r| r.text(0).unwrap() != "decision"),
            "{name}: withdrawn is not in force"
        );
        assert!(
            review::withdraw(&mut reg, staged.decision, "anna@ward-3").is_err(),
            "{name}: twice is refused"
        );
    }
}

/// A re-classification supersedes what the last run asked about the stacks
/// it judges again, grouped or not, a window at a time, and asks again. A
/// question that is not the classifier's, and one a person answered, stay
/// as they are.
#[test]
fn a_re_classification_supersedes_the_open_questions_of_its_stacks_and_asks_again() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("classify-again");
        for (n, sop) in [("1", "A.1.1"), ("2", "A.2.1"), ("3", "A.3.1")] {
            let mut e = synth::minimal_mr("A", &format!("A.{n}"), sop);
            e.push(elem(tags::PATIENT_ID, VR::LO, "P1"));
            e.push(elem(tags::SERIES_DESCRIPTION, VR::LO, "t1 mprage"));
            dir.file(
                &format!("s{n}/1"),
                &synth::part10(&MetaFields::mr(sop), &e, true),
            );
        }
        let mut reg = prepare(&lab, &dir);
        // One stack a window, so a group's members are judged in different
        // windows.
        let settings = nils_classify::job::Settings {
            review_below: Some(1.0),
            window: 1,
            ..Default::default()
        };
        let first =
            nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert_eq!(first.written, 3, "{name}");
        let groups = |reg: &mut Registry, status: &str| -> Vec<i64> {
            rows(
                reg,
                &format!(
                    "SELECT id FROM {{review_item}} WHERE scope = 'group' AND status = '{status}' ORDER BY id"
                ),
            )
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect()
        };
        let asked = groups(&mut reg, "open");
        assert!(!asked.is_empty(), "{name}: {first:?}");
        let stacks: Vec<i64> = rows(&mut reg, "SELECT id FROM {stack} ORDER BY id")
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect();
        // What an older engine left open on the first stack: a classifier
        // question of its own, one that is not the classifier's, and one a
        // person accepted.
        for (kind, status) in [
            ("base:conflict", "open"),
            ("split", "open"),
            ("technique:conflict", "accepted"),
        ] {
            reg.store()
                .insert(
                    &Insert::new(
                        nils_registry::schema::table("review_item"),
                        &["kind", "scope", "ref", "evidence", "status", "created_at"],
                    ),
                    &[vec![
                        Param::from(kind),
                        Param::from("stack"),
                        Param::from(format!("{{\"stack_id\": {}}}", stacks[0])),
                        Param::from("{}"),
                        Param::from(status),
                        Param::from("2026-10-01T10:00:00Z"),
                    ]],
                )
                .unwrap();
        }
        let status_of = |reg: &mut Registry, kind: &str| -> String {
            rows(
                reg,
                &format!(
                    "SELECT status FROM {{review_item}} WHERE kind = '{kind}' AND scope = 'stack'"
                ),
            )[0]
            .text(0)
            .unwrap()
            .to_string()
        };

        let again =
            nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert_eq!(again.written, 3, "{name}");
        let superseded = groups(&mut reg, "superseded");
        for g in &asked {
            assert!(superseded.contains(g), "{name}: group {g} was asked again");
        }
        let open = groups(&mut reg, "open");
        assert_eq!(
            open.len(),
            asked.len(),
            "{name}: the same questions, asked again"
        );
        assert!(
            open.iter().all(|g| !asked.contains(g)),
            "{name}: as new items"
        );
        assert_eq!(status_of(&mut reg, "base:conflict"), "superseded", "{name}");
        assert_eq!(status_of(&mut reg, "split"), "open", "{name}");
        assert_eq!(
            status_of(&mut reg, "technique:conflict"),
            "accepted",
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE status = 'open' AND scope = 'stack' AND kind LIKE '%:%'"
            ),
            0,
            "{name}: no per-stack rows are left"
        );
    }
}

/// Wave 4c §6.6: what the evaluator noticed is counted per batch, and a
/// site term that matched nothing is named, whether it was added to a
/// bucket or to an axis value's list (pack contract 5).
#[test]
fn the_diagnostics_are_counted_per_batch_and_an_unused_overlay_term_is_named() {
    let overlay = nils_pack::Overlay::parse(
        "overlay",
        "\
overlay: site
version: 1.0.0
pack: mri
scope: {manufacturer: SYNTHETIC}
buckets:
  localizer_words: {add: [zzznever]}
lists:
  technique.TSE: {add: [zzzturbo]}
cases:
  - name: the site's own localizer word
    stack: {text_sequence_name: 'zzznever_3d'}
    flags: {is_localizer: true}
  - name: the site's own turbo word
    stack: {text_series_description: 'zzzturbo'}
    axes: {technique: TSE}
",
    )
    .expect("the overlay parses");
    let pack = nils_pack::load(&packs(), Some(&overlay)).expect("the MRI pack loads amended");
    assert_eq!(pack.overlay_terms, vec!["zzznever", "zzzturbo"]);
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        for round in 1..=2 {
            let report = nils_classify::classify::classify(
                &mut reg,
                &pack,
                &Default::default(),
                &Cancel::new(),
            )
            .unwrap();
            assert_eq!(report.written, 1, "{name}");
            assert_eq!(
                report.diagnostics.get("overlay_unused"),
                Some(&2),
                "{name} round {round}: {:?}",
                report.diagnostics
            );
            // one row per kind per batch, replaced on the second run
            let rows_now: Vec<(String, i64, String)> = rows(
                &mut reg,
                "SELECT kind, count, sample FROM {diagnostic} WHERE kind = 'overlay_unused' ORDER BY id",
            )
            .iter()
            .map(|r| {
                (
                    r.text(0).unwrap().into(),
                    r.int(1).unwrap(),
                    r.opt_text(2).unwrap().unwrap_or("").into(),
                )
            })
            .collect();
            assert_eq!(rows_now.len(), 1, "{name} round {round}: {rows_now:?}");
            assert_eq!(rows_now[0].1, 2, "{name}");
            assert!(rows_now[0].2.contains("zzznever"), "{name}: {rows_now:?}");
            assert!(
                rows_now[0].2.contains("zzzturbo"),
                "a list's term is counted like a bucket's: {name}: {rows_now:?}"
            );
            // every diagnostic row of the classifier's kinds is scoped to the batch
            let scopes = one(
                &mut reg,
                "SELECT COUNT(*) FROM {diagnostic} WHERE kind IN ('axis_conflict', 'axis_unresolved', 'keyword_shadowed', 'overlay_unused') AND scope <> 'batch'",
            );
            assert_eq!(scopes, 0, "{name}");
        }
        // and the signals over the batch read the same rows back
        let scope = nils_classify::scope::Scope::parse("batch:1").unwrap();
        let signals = nils_classify::signals::signals(reg.store(), &scope).unwrap();
        assert_eq!(
            signals["diagnostics"]["overlay_unused"], 2,
            "{name}: {signals}"
        );
        assert_eq!(
            signals["unused_overlay_terms"],
            serde_json::json!(["zzznever", "zzzturbo"]),
            "{name}: {signals}"
        );
        assert!(
            signals["axes"]["technique"]["tiers"].is_object(),
            "{name}: {signals}"
        );
        // record 26: the same signals by value, and the origins of the scope
        let technique = signals["by_value"]["technique"]
            .as_object()
            .unwrap_or_else(|| panic!("{name}: {signals}"));
        assert_eq!(
            technique.len(),
            1,
            "one stack, one value: {name}: {signals}"
        );
        let (_, value) = technique.iter().next().unwrap();
        assert_eq!(value["decided"], 1, "{name}: {signals}");
        assert_eq!(
            value["by_flag"].as_i64().unwrap()
                + value["by_word"].as_i64().unwrap()
                + value["by_physics"].as_i64().unwrap(),
            1,
            "decided by one kind of clause: {name}: {signals}"
        );
        assert_eq!(value["unsure"], 0, "{name}: {signals}");
        assert!(value["shadowed"].is_array(), "{name}: {signals}");
        assert_eq!(
            value["overrode_with"],
            serde_json::json!([]),
            "{name}: {signals}"
        );
        assert!(
            signals["origins"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["name"] == "SYNTHETIC"
                    && o["kind"] == "manufacturer"
                    && o["stacks"] == 1),
            "{name}: {signals}"
        );
        let origin = nils_classify::scope::Scope::parse("origin:SYNTHETIC").unwrap();
        let by_origin = nils_classify::signals::signals(reg.store(), &origin).unwrap();
        assert_eq!(
            by_origin["diagnostics"]["overlay_unused"], 2,
            "{name}: {by_origin}"
        );
        assert!(nils_classify::scope::Scope::parse("nonsense").is_err());
    }
}

/// A split that leaves one image in every stack: a series of six echoes of
/// one slice, each echo stating its echo time, so every stack holds one
/// image; beside it the same split over two slices, whose stacks hold two
/// images each. And the phase-contrast study as the archive writes one, whose
/// echo number counts its frames under an echo time of 0.0, the scanner
/// saying it has nothing to say (record 35, S4): since wave 7a the digest
/// makes that one stack.
fn flow_tree() -> TempDir {
    let dir = TempDir::new("classify-flow");
    let write = |series: &str, echo: u32, instance: &str, te: &str, file: &str| {
        let sop = format!("A.{series}.{echo}.{instance}");
        let mut e = synth::minimal_mr("A", &format!("A.{series}"), &sop);
        e.push(elem(tags::PATIENT_ID, VR::LO, "P1"));
        e.extend([
            elem(tags::SERIES_DESCRIPTION, VR::LO, "ax flow"),
            elem(tags::SCANNING_SEQUENCE, VR::CS, "GR"),
            elem(tags::SEQUENCE_NAME, VR::SH, "*pc2d1"),
            elem(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
            elem(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
            elem(tags::ECHO_TIME, VR::DS, te),
            elem(tags::REPETITION_TIME, VR::DS, "30.0"),
            elem(tags::ECHO_NUMBERS, VR::IS, &echo.to_string()),
        ]);
        dir.file(file, &synth::part10(&MetaFields::mr(&sop), &e, true));
    };
    for echo in 1..=6 {
        let te = format!("{}", 2 * echo);
        write("1", echo, "1", &te, &format!("one/{echo}"));
        write("2", echo, "1", &te, &format!("two/{echo}-1"));
        write("2", echo, "2", &te, &format!("two/{echo}-2"));
        write("3", echo, "1", "0.0", &format!("flow/{echo}"));
    }
    dir
}

/// Record 55 H3 (2026-10-09): the split note is information, not a
/// question. The six stacks the split left holding one image carry it on
/// their classification, with the split's reason; nobody is asked.
#[test]
fn a_split_that_leaves_one_image_in_every_stack_is_noted_and_not_asked() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = flow_tree();
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();

        // The split fired on the two series that state their echo times, and
        // the reason is the echo; the flow study is one stack, whose echo
        // time is a zero the file carries, not an absence.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {stack_fingerprint} WHERE split_reason = 'multi_echo'"
            ),
            12,
            "{name}"
        );
        assert_eq!(
            rows(
                &mut reg,
                "SELECT n_instances, stacks_in_series FROM {stack_fingerprint} WHERE echo_time = 0"
            )
            .iter()
            .map(|r| (r.int(0).unwrap(), r.int(1).unwrap()))
            .collect::<Vec<_>>(),
            [(6, 1)],
            "{name}"
        );

        // No question, about the split or anything else the sort could
        // decide alone.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind = 'split:one_image_per_stack'"
            ),
            0,
            "{name}"
        );
        // The note, on the six stacks of the series the split left holding
        // one image, on none of the six whose stacks hold two, and not on the
        // flow study, which is no longer split.
        assert_eq!(report.split_notes, 6, "{name}");
        let noted = rows(
            &mut reg,
            "SELECT c.stack_id, CAST(c.notes AS TEXT), f.n_instances FROM {classification} c \
             JOIN {stack_fingerprint} f ON f.stack_id = c.stack_id ORDER BY c.stack_id",
        );
        let mut with_note = 0;
        for r in &noted {
            let notes: serde_json::Value = r
                .opt_text(1)
                .unwrap()
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(serde_json::Value::Null);
            let single = r.int(2).unwrap() == 1;
            assert_eq!(notes["split"].is_object(), single, "{name}: {notes}");
            if single {
                with_note += 1;
                assert_eq!(
                    notes["split"]["kind"], "split:one_image_per_stack",
                    "{name}"
                );
                assert_eq!(notes["split"]["value"], "multi_echo", "{name}");
                assert_eq!(notes["split"]["stacks_in_series"], 6.0, "{name}");
                assert_eq!(notes["split"]["n_instances"], 1.0, "{name}");
            }
        }
        assert_eq!(with_note, 6, "{name}");

        // And a zero echo time decided nothing: the flow study is not called
        // an anatomical T1w on the strength of it.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_axis} a JOIN {stack_fingerprint} f \
                 ON f.stack_id = a.stack_id WHERE a.axis = 'base' AND f.echo_time = 0"
            ),
            0,
            "{name}: a zero echo time is not a short one"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_evidence} WHERE rule = 'physics:gre_t1w'"
            ),
            0,
            "{name}"
        );
    }
}

/// The ruling of 2026-10-09 for the fold: the stack a fold keeps is judged
/// anew. A registry holding the flow study as the split left it, one stack
/// per frame, and sorted so, folds on a re-read of one of its files; the
/// next sort judges the one stack that stays with all six frames, and notes
/// no split on it.
#[test]
fn a_stack_a_fold_keeps_is_judged_anew_by_the_next_sort() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = flow_tree();
        let mut reg = prepare(&lab, &dir);
        // the flow study as the split left it: a stack per frame
        let first = one(
            &mut reg,
            "SELECT s.id FROM {stack} s JOIN {series} se ON se.id = s.series_id \
             WHERE se.series_instance_uid = 'A.3'",
        );
        for echo in 2..=6 {
            rows(
                &mut reg,
                &format!(
                    "INSERT INTO {{stack}} (series_id, stack_index, stack_key, modality, \
                     orientation, image_type, echo_numbers, echo_time, repetition_time, \
                     orientation_confidence, n_instances, first_batch_id) \
                     SELECT series_id, {index}, '{echo:016x}', modality, orientation, image_type, \
                     '{echo}', echo_time, repetition_time, orientation_confidence, 1, first_batch_id \
                     FROM {{stack}} WHERE id = {first}",
                    index = 100 + echo
                ),
            );
            rows(
                &mut reg,
                &format!(
                    "UPDATE {{instance}} SET stack_id = \
                     (SELECT id FROM {{stack}} WHERE stack_key = '{echo:016x}') \
                     WHERE sop_instance_uid = 'A.3.{echo}.1'"
                ),
            );
        }
        rows(
            &mut reg,
            &format!("UPDATE {{stack}} SET n_instances = 1 WHERE id = {first}"),
        );
        rows(
            &mut reg,
            "UPDATE {series} SET n_stacks = 6 WHERE series_instance_uid = 'A.3'",
        );
        let sort = |reg: &mut Registry| {
            nils_classify::run(reg, &nils_classify::Settings::default(), &Cancel::new()).unwrap();
            nils_classify::classify::classify(reg, &pack, &Default::default(), &Cancel::new())
                .unwrap()
        };
        // sorted so, the six one-image stacks of the flow study carry the
        // split note beside the six of the first series
        assert_eq!(sort(&mut reg).split_notes, 12, "{name}");

        // a re-read of one file of each series folds the flow study
        let mut s = nils_digest::Settings::new(dir.path());
        s.name = "t".into();
        s.workers = 2;
        s.walk_threads = 2;
        s.reread_every = true;
        s.reread_one = true;
        let read = digest(&s, &mut reg).unwrap();
        assert_eq!(read.written.unwrap().echo_stacks_folded, 5, "{name}");
        // nothing the sort said of the stacks of the study is left
        assert_eq!(
            one(
                &mut reg,
                &format!("SELECT COUNT(*) FROM {{classification}} WHERE stack_id = {first}")
            ),
            0,
            "{name}"
        );

        // and the next sort judges the stack that stays anew, with all its
        // frames, and notes no split on it
        assert_eq!(sort(&mut reg).split_notes, 6, "{name}");
        let fingerprint = rows(
            &mut reg,
            &format!(
                "SELECT n_instances, stacks_in_series FROM {{stack_fingerprint}} \
                 WHERE stack_id = {first}"
            ),
        );
        assert_eq!(
            (
                fingerprint[0].int(0).unwrap(),
                fingerprint[0].int(1).unwrap()
            ),
            (6, 1),
            "{name}"
        );
        let notes = rows(
            &mut reg,
            &format!("SELECT CAST(notes AS TEXT) FROM {{classification}} WHERE stack_id = {first}"),
        );
        assert_eq!(notes.len(), 1, "{name}");
        let notes: serde_json::Value = notes[0]
            .opt_text(0)
            .unwrap()
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(serde_json::Value::Null);
        assert!(notes["split"].is_null(), "{name}: {notes}");
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification} c WHERE NOT EXISTS \
                 (SELECT 1 FROM {stack} s WHERE s.id = c.stack_id)"
            ),
            0,
            "{name}"
        );
    }
}

/// One stack of one series, built from the elements a case needs, so that a
/// threshold can be met exactly rather than described.
fn one_stack(description: &str, extra: &[(dicom_core::Tag, VR, &str)]) -> TempDir {
    let dir = TempDir::new("classify");
    let mut e = synth::minimal_mr("A", "A.1", "A.1.1");
    e.push(elem(tags::PATIENT_ID, VR::LO, "P1"));
    e.extend([
        elem(tags::SERIES_DESCRIPTION, VR::LO, description),
        elem(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
        elem(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
    ]);
    for (tag, vr, value) in extra {
        e.push(elem(*tag, *vr, value));
    }
    dir.file("s/1", &synth::part10(&MetaFields::mr("A.1.1"), &e, true));
    dir
}

/// The axis as it was written, with the confidence and the tier.
fn axis_of(reg: &mut Registry, axis: &str) -> (String, f64, String) {
    let r = rows(
        reg,
        &format!(
            "SELECT COALESCE(value, ''), confidence, tier FROM {{classification_axis}} WHERE axis = '{axis}'"
        ),
    );
    assert_eq!(r.len(), 1, "one stack, one row for {axis}");
    (
        r[0].text(0).unwrap().into(),
        r[0].double(1).unwrap(),
        r[0].text(2).unwrap().into(),
    )
}

/// Open questions of a kind, whether they are still per stack or have been
/// collapsed into the group a person reads.
fn asked(reg: &mut Registry, kind: &str) -> i64 {
    one(
        reg,
        &format!("SELECT COUNT(*) FROM {{review_item}} WHERE kind = '{kind}'"),
    )
}

/// Wave 2 §8.2: a threshold is read as strictly below, so an answer written
/// at exactly the confidence the threshold names is an answer. The MRI
/// pack's body-part number is the case that matters: the keyword tier writes
/// 0.65 and the threshold is 0.65, so the whole axis stands on its boundary.
#[test]
fn a_body_part_exactly_on_its_threshold_is_not_a_question_and_is_counted() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    assert_eq!(pack.review.below("body_part"), 0.65);
    for lab in labs() {
        let name = lab.name;
        let dir = one_stack(
            "sag t1 spine",
            &[
                (tags::BODY_PART_EXAMINED, VR::CS, "SPINE"),
                (tags::SCANNING_SEQUENCE, VR::CS, "SE"),
                (tags::REPETITION_TIME, VR::DS, "600"),
                (tags::ECHO_TIME, VR::DS, "12"),
            ],
        );
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(
            axis_of(&mut reg, "body_part"),
            ("spine".to_string(), 0.65, "keywords".to_string()),
            "{name}"
        );
        assert_eq!(
            asked(&mut reg, "body_part:low_confidence"),
            0,
            "{name}: 0.65 is not below 0.65"
        );
        // and the run says how many answers stand on their threshold
        assert_eq!(report.at_threshold.get("body_part"), Some(&1), "{name}");
        assert!(report.on_the_threshold() >= 1, "{name}");

        // The same stack, asked about one hundredth higher: now it is below,
        // and still no question. Record 55 H3: the body part is its image
        // model's, so a sort asks nothing about it even where a person names
        // a threshold; the weak answer is noted.
        let settings = nils_classify::job::Settings {
            review_below: Some(0.66),
            ..Default::default()
        };
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert_eq!(
            asked(&mut reg, "body_part:low_confidence"),
            0,
            "{name}: the body part is the model's"
        );
        assert_eq!(report.at_threshold.get("body_part"), None, "{name}");
        assert_eq!(report.below.get("body_part"), Some(&1), "{name}");
        let notes = notes_of(&mut reg);
        assert!(
            notes["below"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["axis"] == "body_part" && b["below"] == 0.66),
            "{name}: {notes}"
        );
    }
}

/// The one stack's classification notes (record 55 H3).
fn notes_of(reg: &mut Registry) -> serde_json::Value {
    let r = rows(reg, "SELECT CAST(notes AS TEXT) FROM {classification}");
    assert_eq!(r.len(), 1, "one stack");
    r[0].opt_text(0)
        .unwrap()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// The other half of the same boundary, on the axis the corpus run found 354
/// of: base from the physics tier, written at exactly 0.70 against a default
/// threshold of 0.70.
#[test]
fn a_base_from_physics_exactly_on_its_threshold_is_an_answer() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    assert_eq!(pack.review.below("base"), 0.70);
    for lab in labs() {
        let name = lab.name;
        // A gradient echo with a very short echo time and no keyword.
        let dir = one_stack(
            "ax gre",
            &[
                (tags::SCANNING_SEQUENCE, VR::CS, "GR"),
                (tags::REPETITION_TIME, VR::DS, "250"),
                (tags::ECHO_TIME, VR::DS, "4.6"),
                (tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
            ],
        );
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(
            axis_of(&mut reg, "base"),
            ("T1w".to_string(), 0.70, "physics".to_string()),
            "{name}"
        );
        assert_eq!(
            asked(&mut reg, "base:low_confidence"),
            0,
            "{name}: 0.70 is not below 0.70"
        );
        assert_eq!(report.at_threshold.get("base"), Some(&1), "{name}");
    }
}

/// And the rule on the same axis that writes one hundredth less was a
/// question. Record 55 H3 (2026-10-09): a rule's low confidence alone is
/// never one; the answer is noted below the pack's threshold, and asked only
/// where a person names a threshold for the run.
#[test]
fn a_base_one_hundredth_below_its_threshold_is_noted_and_not_asked() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        // A spin echo with a long repetition and a short echo time: PDw at
        // 0.65, which is the population the corpus run asked about.
        let dir = one_stack(
            "ax se",
            &[
                (tags::SCANNING_SEQUENCE, VR::CS, "SE"),
                (tags::REPETITION_TIME, VR::DS, "3000"),
                (tags::ECHO_TIME, VR::DS, "12"),
                (tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
            ],
        );
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(
            axis_of(&mut reg, "base"),
            ("PDw".to_string(), 0.65, "physics".to_string()),
            "{name}"
        );
        assert_eq!(
            asked(&mut reg, "base:low_confidence"),
            0,
            "{name}: 0.65 is below 0.70, and that alone is no question"
        );
        assert_eq!(report.at_threshold.get("base"), None, "{name}");
        assert_eq!(report.below.get("base"), Some(&1), "{name}");
        let notes = notes_of(&mut reg);
        let below: Vec<&serde_json::Value> = notes["below"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["axis"] == "base")
            .collect();
        assert_eq!(below.len(), 1, "{name}: {notes}");
        assert_eq!(below[0]["value"], "PDw", "{name}");
        assert_eq!(below[0]["confidence"], 0.65, "{name}");
        assert_eq!(below[0]["below"], 0.70, "{name}");
        assert_eq!(below[0]["tier"], "physics", "{name}");

        // a person's own threshold for the run still asks
        let settings = nils_classify::job::Settings {
            review_below: Some(0.70),
            ..Default::default()
        };
        nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind = 'base:low_confidence' AND status = 'open'"
            ),
            1,
            "{name}"
        );
    }
}

/// Every threshold of this kind in the engine, at the value itself. Each
/// line is the sentence the threshold is written with, so a comparison that
/// drifts from its words fails here.
#[test]
fn every_threshold_reads_the_value_on_it_as_its_own_words_do() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");

    // review.low_confidence, per axis: below, so exactly on it is not weak.
    for axis in ["body_part", "base", "technique"] {
        let below = pack.review.below(axis);
        assert!(!pack.review.asks_about(axis, below), "{axis} on {below}");
        assert!(pack.review.asks_about(axis, below - 0.01), "{axis}");
        assert!(!pack.review.asks_about(axis, below + 0.01), "{axis}");
        // and arithmetic that lands on the threshold the long way round is
        // still on it
        assert!(
            !pack
                .review
                .asks_about(axis, (below / 3.0) + (below / 3.0) + (below / 3.0)),
            "{axis}: a rounding error is not a doubt"
        );
    }

    // A pass's emit.review_item_below: the same sentence.
    let vote = pack
        .passes
        .iter()
        .find(|p| p.name == "physics_vote")
        .expect("the MRI pack declares physics_vote");
    let below = vote.emit.review_below;
    assert_eq!(below, 0.7);
    assert!(!nils_pack::weaker_than(below, below));
    assert!(nils_pack::at_threshold(below, below));
    assert!(nils_pack::weaker_than(below - 0.01, below));

    // A pick's borders: `within` takes the value itself, `below` does not.
    let model = pack
        .picks
        .iter()
        .find(|m| m.name == "main")
        .expect("the MRI pack declares the main pick");
    assert!(model.borders.runner_up_within <= model.borders.runner_up_within);
    let (_, floor) = model
        .borders
        .rare_within
        .clone()
        .expect("the main pick declares rare_within");
    assert!(!nils_pack::weaker_than(floor, floor), "exactly a tenth");
    assert!(nils_pack::weaker_than(floor - 0.01, floor));

    // And the digest's own: a plane at exactly the oblique confidence is
    // not oblique.
    let straight = nils_digest::stack::Orientation {
        class: nils_digest::stack::Class::Axial,
        confidence: nils_digest::stack::OBLIQUE_BELOW,
    };
    assert!(!straight.oblique());
    let tilted = nils_digest::stack::Orientation {
        class: nils_digest::stack::Class::Axial,
        confidence: nils_digest::stack::OBLIQUE_BELOW - 0.01,
    };
    assert!(tilted.oblique());
}

/// Wave 2 §8.2 and record 35 finding 6: evidence that disagreed reaches the
/// stack it belongs to. Candidate D's case, as the corpus holds it: a spine
/// whose text also names the brain. The spine rule is ordered first and
/// decides, the set stops there. Record 55 H3 (2026-10-09): the pack's order
/// did what it was written to do, so it is no question; who beat whom is
/// kept on the stack, and the tally keeps counting it.
#[test]
fn a_conflict_is_kept_on_the_stack_and_never_asked() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = one_stack(
            "sag t1 cervical cerebral",
            &[
                (tags::BODY_PART_EXAMINED, VR::CS, "SPINE"),
                (tags::SCANNING_SEQUENCE, VR::CS, "SE"),
                (tags::REPETITION_TIME, VR::DS, "600"),
                (tags::ECHO_TIME, VR::DS, "12"),
            ],
        );
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();

        // the answer is the one the order gives, unchanged
        assert_eq!(
            axis_of(&mut reg, "body_part"),
            ("spine".to_string(), 0.65, "keywords".to_string()),
            "{name}"
        );

        // no question
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind LIKE '%:conflict'"
            ),
            0,
            "{name}"
        );
        assert_eq!(report.overrides.get("rule_order"), Some(&1), "{name}");

        // and a person reading the stack sees that it was contested, by whom
        // and over whom, with what ranked them
        let notes = notes_of(&mut reg);
        let o: Vec<&serde_json::Value> = notes["overrides"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|o| o["axis"] == "body_part")
            .collect();
        assert_eq!(o.len(), 1, "{name}: {notes}");
        let o = o[0];
        assert_eq!(o["value"], "spine", "{name}: {o}");
        assert_eq!(o["other"], "brain", "{name}: {o}");
        assert_eq!(o["rank"], "rule_order", "{name}: {o}");
        assert_eq!(o["by"]["rule_set"], "body_part", "{name}: {o}");
        assert_eq!(o["by"]["rule"], "spine", "{name}: {o}");
        assert_eq!(o["over"]["rule"], "brain", "{name}: {o}");
        assert_eq!(o["by"]["tier"], "keywords", "{name}: {o}");
        assert_eq!(o["over"]["confidence"], 0.65, "{name}: {o}");
        assert!(
            o["by"]["rule_at"].as_u64() < o["over"]["rule_at"].as_u64(),
            "{name}: the spine rule is ranked first: {o}"
        );
        assert!(
            !o["over"]["matched"].as_str().unwrap_or_default().is_empty(),
            "{name}: the note names what the pre-empted rule cited: {o}"
        );

        // and the tally that already counted it keeps counting it
        assert!(
            one(
                &mut reg,
                "SELECT CAST(SUM(count) AS BIGINT) FROM {diagnostic} WHERE kind = 'axis_conflict' AND scope = 'batch'"
            ) >= 1,
            "{name}"
        );
    }
}

/// A stack the pack has ruled out is asked nothing, a conflict among the
/// rest included: the silence of §8.2 is what keeps a queue readable, and a
/// new kind of item must not walk around it.
#[test]
fn a_conflict_on_a_stack_the_pack_rules_out_is_not_a_question() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        // A secondary capture: the pack excludes it as not an image.
        let dir = one_stack(
            "sag t1 cervical cerebral screenshot",
            &[
                (tags::BODY_PART_EXAMINED, VR::CS, "SPINE"),
                (tags::IMAGE_TYPE, VR::CS, "DERIVED\\SECONDARY\\SCREEN SAVE"),
                (tags::SCANNING_SEQUENCE, VR::CS, "SE"),
            ],
        );
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(report.silent, 1, "{name}");
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind LIKE '%:conflict'"
            ),
            0,
            "{name}"
        );
    }
}

/// Record 35, the seam between S4 and S5: one clause carries both.
///
/// `physics:gre_t1w` is a window on a small echo time, so S4 gave it a
/// `gt: 0` guard, and it writes 0.70, which is exactly the threshold S5
/// made read as strictly below. The two meet on one clause: the guard
/// decides whether the clause fires at all, and only then does the boundary
/// decide whether the answer is a question. A zero echo time must therefore
/// produce no answer on the boundary rather than a confident one, and no
/// count against the threshold either.
#[test]
fn a_clause_with_a_zero_guard_and_a_threshold_answers_on_neither_when_the_guard_fails() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    assert_eq!(pack.review.below("base"), 0.70);
    for lab in labs() {
        let name = lab.name;
        // The same stack as the test above, with the one number the scanner
        // wrote as zero. Everything else that could make it a T1w is absent.
        let dir = one_stack(
            "ax gre",
            &[
                (tags::SCANNING_SEQUENCE, VR::CS, "GR"),
                (tags::REPETITION_TIME, VR::DS, "250"),
                (tags::ECHO_TIME, VR::DS, "0.0"),
                (tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
            ],
        );
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();

        // The guard held: the window never fired, so no evidence stands on
        // it and nothing was written at the confidence it would have used.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_evidence} WHERE rule = 'physics:gre_t1w'"
            ),
            0,
            "{name}: a zero echo time is not a very short one"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_axis} WHERE axis = 'base' AND confidence = 0.7"
            ),
            0,
            "{name}"
        );
        // And the boundary counted nothing, because there is no answer for
        // it to sit on: a clause whose guard failed is silence, not a
        // confident answer that happens to be on the threshold.
        assert_eq!(report.at_threshold.get("base"), None, "{name}");
    }
}

/// The words of one line of the report, with its padding taken out, so that
/// a test says what a person reads and not how wide the column is.
fn line(text: &str, label: &str) -> String {
    text.lines()
        .find(|l| l.trim_start().starts_with(label))
        .unwrap_or_else(|| panic!("no line beginning {label} in:\n{text}"))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Record 35, regression 3: the line held a count of items against a count
/// of stacks and called the ratio a share of the stacks, so a run that
/// raised 830 items on 546 of 836 stacks printed "99.3% of the stacks".
/// Each number now counts what its own sentence names.
#[test]
fn the_line_counts_stacks_as_stacks_and_items_as_items() {
    let mut report = nils_classify::Classified::new(7, 3, "mri@0.1.3".to_string());
    report.read = 836;
    report.written = 836;
    report.review_stacks = 546;
    report.review_items = 830;
    report.review_groups = 58;
    let printed = report.to_string();
    assert_eq!(
        line(&printed, "stacks to review"),
        "stacks to review 546 65.3% of the 836 classified",
        "{printed}"
    );
    assert_eq!(
        line(&printed, "review items"),
        "review items 830 on those stacks, as 58 question(s)",
        "{printed}"
    );
    // and never the items over the stacks, which is what 99.3 per cent was
    assert!(!printed.contains("99.3%"), "{printed}");
}

/// Record 35, regression 3, from the registry: one stack answering weakly on
/// several axes raises one item per axis, and is one stack in the line.
#[test]
fn a_stack_carrying_two_items_is_one_stack_in_the_line() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        // Every answer below certainty is a question, so the one stack of
        // this tree raises one for each axis the pack answered.
        let settings = nils_classify::job::Settings {
            review_below: Some(1.0),
            ..Default::default()
        };
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert_eq!(report.written, 1, "{name}");
        assert!(
            report.review_items >= 2,
            "{name}: {} item(s)",
            report.review_items
        );
        assert_eq!(
            report.review_stacks, 1,
            "{name}: one stack, whatever it asked"
        );
        // and the count is the registry's own: the distinct stacks the
        // members of this run's questions stand on
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(DISTINCT stack_id) FROM {review_member}"
            ),
            1,
            "{name}"
        );
        let printed = report.to_string();
        assert_eq!(
            line(&printed, "stacks to review"),
            "stacks to review 1 100.0% of the 1 classified",
            "{name}: {printed}"
        );
        assert_eq!(
            line(&printed, "review items"),
            format!(
                "review items {} on those stacks, as {} question(s)",
                report.review_items, report.review_groups
            ),
            "{name}: {printed}"
        );
    }
}

/// The flow study as the archive holds it: the series whose echo number
/// counts its frames under an echo time of zero, which the split broke into
/// one-image stacks until the digest learned to read it as one (wave 7a), and
/// beside it the same protocol's series whose echo time is a measurement and
/// which the rules therefore judge. Those judged stacks are what the vote
/// reads as neighbours.
fn flow_with_neighbours() -> TempDir {
    let dir = TempDir::new("classify-flow-pool");
    let image = |series: &str, echo: Option<u32>, instance: u32, te: &str, file: &str| {
        let sop = format!("A.{series}.{}.{instance}", echo.unwrap_or(0));
        let mut e = synth::minimal_mr("A", &format!("A.{series}"), &sop);
        e.push(elem(tags::PATIENT_ID, VR::LO, "P1"));
        e.extend([
            elem(tags::SERIES_DESCRIPTION, VR::LO, "ax flow"),
            elem(tags::SCANNING_SEQUENCE, VR::CS, "GR"),
            elem(tags::SEQUENCE_NAME, VR::SH, "*pc2d1"),
            elem(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
            elem(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
            elem(tags::ECHO_TIME, VR::DS, te),
            elem(tags::REPETITION_TIME, VR::DS, "30.0"),
            elem(tags::FLIP_ANGLE, VR::DS, "15"),
        ]);
        if let Some(n) = echo {
            e.push(elem(tags::ECHO_NUMBERS, VR::IS, &n.to_string()));
        }
        dir.file(file, &synth::part10(&MetaFields::mr(&sop), &e, true));
    };
    // The study's fragments: one image each, and no echo time.
    for echo in 1..=6 {
        image("1", Some(echo), 1, "0.0", &format!("split/{echo}"));
    }
    // The same protocol's whole series, with an echo time that was measured.
    for series in ["2", "3"] {
        for instance in 1..=4 {
            image(
                series,
                None,
                instance,
                "3.0",
                &format!("whole/{series}-{instance}"),
            );
        }
    }
    dir
}

/// Record 35, the re-run: S4 guarded the base windows against an echo time
/// of zero, and candidate J's fragments came out T1w all the same, because
/// the vote reads the same number through its key. A zero bins with the
/// short echo times of gradient-echo anatomy, and the neighbours the vote
/// found were the flow study's own whole series. A zero is a hole now, so
/// the study's frames, one stack since wave 7a, are named by nothing.
#[test]
fn a_zero_echo_time_does_not_vote_itself_a_base_from_its_neighbours() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = flow_with_neighbours();
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();

        // The six frames are one stack, and it has no echo time to speak of.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {stack_fingerprint} WHERE n_instances = 6 AND echo_time = 0"
            ),
            1,
            "{name}"
        );
        // Nothing wrote a base on it: not a rule, whose window the guard
        // closes, and not the pass, whose bin no longer holds it.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_axis} a JOIN {stack_fingerprint} f \
                 ON f.stack_id = a.stack_id WHERE a.axis = 'base' AND f.echo_time = 0"
            ),
            0,
            "{name}: a flow study is not an anatomical T1w"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_evidence} WHERE axis = 'base' AND pass IS NOT NULL"
            ),
            0,
            "{name}: and the vote answered none of them"
        );
        // What the study does say about it is unchanged: the technique is
        // the flow sequence. No split is left to note.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_axis} a JOIN {stack_fingerprint} f \
                 ON f.stack_id = a.stack_id \
                 WHERE a.axis = 'technique' AND a.value = 'PC' AND f.echo_time = 0"
            ),
            1,
            "{name}"
        );
        let split = rows(&mut reg, "SELECT CAST(notes AS TEXT) FROM {classification}")
            .iter()
            .filter_map(|r| r.opt_text(0).unwrap().map(str::to_string))
            .filter_map(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .filter(|n| n["split"]["kind"] == "split:one_image_per_stack")
            .count();
        assert_eq!(split, 0, "{name}");
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind = 'split:one_image_per_stack'"
            ),
            0,
            "{name}"
        );
        // And the whole series, whose echo time is a measurement, has no
        // base either: since MRI pack 0.21.0 no phase-contrast output has one
        // (record 48, conventions round 3, case 5), so the window that judged
        // it before never runs, and nothing the vote could fill is left.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_axis} WHERE axis = 'base'"
            ),
            0,
            "{name}: a phase contrast has no base"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_evidence} WHERE rule = 'physics:gre_t1w'"
            ),
            0,
            "{name}"
        );
    }
}

/// kineuro/nils#94: the text an unresolved axis was matched against. Stacks
/// whose description carries only a word the pack does not know leave axes
/// unresolved; per axis the signals fold their search texts into distinct
/// texts with the stacks each covers, the most common first, at most ten,
/// and show a text only when it covers five stacks of three subjects. A
/// text on one subject's stacks is withheld and counted, whatever its
/// number of stacks.
#[test]
fn the_text_an_unresolved_axis_was_matched_against_is_sampled_bounded_and_withheld() {
    use nils_classify::signals::{TEXT_MIN_STACKS, TEXT_MIN_SUBJECTS, TEXTS_MAX};
    assert_eq!((TEXT_MIN_STACKS, TEXT_MIN_SUBJECTS, TEXTS_MAX), (5, 3, 10));
    let dir = TempDir::new("classify-unresolved");
    // (description, subject): the site's word on three subjects, six stacks,
    // in two spellings; eleven more words on three subjects, five stacks
    // each; one word on one subject's six stacks; one on two subjects' six
    let mut planted: Vec<(String, &str)> = Vec::new();
    for (k, who) in ["P1", "P2", "P3", "P1", "P2", "P3"].iter().enumerate() {
        let word = if k % 2 == 0 { "zzzagent" } else { "ZZZAGENT" };
        planted.push((word.to_string(), who));
    }
    for w in 0..11 {
        for who in ["P1", "P2", "P3", "P1", "P2"] {
            planted.push((format!("zzzword{w:02}"), who));
        }
    }
    for _ in 0..6 {
        planted.push(("zzzlone".to_string(), "P4"));
    }
    for who in ["P4", "P5", "P4", "P5", "P4", "P5"] {
        planted.push(("zzzpair".to_string(), who));
    }
    for (i, (word, who)) in planted.iter().enumerate() {
        let study = format!("S{who}");
        let sop = format!("{study}.{i}.1");
        let mut e = synth::minimal_mr(&study, &format!("{study}.{i}"), &sop);
        e.push(elem(tags::PATIENT_ID, VR::LO, who));
        e.extend([
            elem(tags::SERIES_DESCRIPTION, VR::LO, word),
            elem(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
        ]);
        dir.file(
            &format!("{who}/{i}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    let total = planted.len() as i64;
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(DISTINCT subject_id) FROM {stack_fingerprint}"
            ),
            5,
            "{name}"
        );
        let scope = nils_classify::scope::Scope::parse("batch:1").unwrap();
        let signals = nils_classify::signals::signals(reg.store(), &scope).unwrap();
        let counted = signals["diagnostics"]["axis_unresolved"]
            .as_i64()
            .unwrap_or_else(|| panic!("{name}: no axis is unresolved: {signals}"));
        let sampled =
            nils_classify::signals::unresolved_texts(reg.store(), &pack, &scope, 2_000).unwrap();
        assert_eq!(sampled["read"], total, "{name}: {sampled}");
        assert_eq!(sampled["complete"], true, "{name}: {sampled}");
        let axes = sampled["axes"]
            .as_object()
            .unwrap_or_else(|| panic!("{name}: {sampled}"));
        assert!(!axes.is_empty(), "{name}: {sampled}");
        let mut stacks = 0;
        for (axis, doc) in axes {
            stacks += doc["stacks"].as_i64().unwrap();
            let texts = doc["texts"].as_array().unwrap();
            assert_eq!(doc["stacks"], total, "{name} {axis}: {doc}");
            assert_eq!(doc["distinct"], 14, "{name} {axis}: {doc}");
            assert_eq!(texts.len(), 10, "bounded: {name} {axis}: {doc}");
            // folded: the two spellings of the site word are one text, first
            assert!(
                texts[0]["text"].as_str().unwrap().contains("zzzagent"),
                "{name} {axis}: {doc}"
            );
            assert_eq!(texts[0]["stacks"], 6, "{name} {axis}: {doc}");
            assert!(
                texts[1..].iter().all(|t| t["stacks"] == 5),
                "{name} {axis}: {doc}"
            );
            // one subject's word and two subjects' word are withheld, counted
            let shown = doc.to_string();
            assert!(!shown.contains("zzzlone"), "{name} {axis}: {doc}");
            assert!(!shown.contains("zzzpair"), "{name} {axis}: {doc}");
            assert_eq!(doc["withheld"]["texts"], 2, "{name} {axis}: {doc}");
            assert_eq!(doc["withheld"]["stacks"], 12, "{name} {axis}: {doc}");
        }
        assert_eq!(
            stacks, counted,
            "the stacks the diagnostics count: {name}: {sampled} {signals}"
        );
        // a smaller sample says it did not read everything
        let two = nils_classify::signals::unresolved_texts(reg.store(), &pack, &scope, 2).unwrap();
        assert_eq!(two["read"], 2, "{name}: {two}");
        assert_eq!(two["complete"], false, "{name}: {two}");
    }
}

/// kineuro/nils#94, the part that remained: the text an axis the rules
/// resolved was matched against, per value. Stacks whose description
/// carries a word the pack knows resolve the base by that word: per axis
/// and value the signals fold their search texts into distinct texts with
/// the stacks each covers and the words the rules cited in it, under the
/// same bounds and showing threshold as the unresolved texts. A text on
/// one subject's stacks is withheld and counted. A stack the rules left
/// unresolved is in the unresolved sample and not in this one.
#[test]
fn the_text_a_resolved_axis_was_matched_against_is_sampled_by_value() {
    let dir = TempDir::new("classify-resolved");
    // (description, subject): one spelling of a T1w on three subjects, in
    // two cases; a T2w on three subjects; the same T1w words with one
    // subject's own word beside them; and one stack with no word at all
    let mut planted: Vec<(&str, &str)> = Vec::new();
    for who in ["P1", "P2", "P3", "P1", "P2", "P3"] {
        planted.push(("t1 mprage", who));
    }
    for who in ["P1", "P2", "P3", "P1", "P2"] {
        planted.push(("T2 TSE", who));
    }
    for _ in 0..6 {
        planted.push(("t1 mprage zzzlone", "P4"));
    }
    planted.push(("zzzunknown", "P5"));
    for (i, (word, who)) in planted.iter().enumerate() {
        let study = format!("S{who}");
        let sop = format!("{study}.{i}.1");
        let mut e = synth::minimal_mr(&study, &format!("{study}.{i}"), &sop);
        e.push(elem(tags::PATIENT_ID, VR::LO, who));
        e.extend([
            elem(tags::SERIES_DESCRIPTION, VR::LO, word),
            elem(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
        ]);
        dir.file(
            &format!("{who}/{i}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let scope = nils_classify::scope::Scope::parse("batch:1").unwrap();
        let both = nils_classify::signals::texts(reg.store(), &pack, &scope, 2_000).unwrap();
        let resolved = &both.resolved;
        assert_eq!(resolved["read"], planted.len(), "{name}: {resolved}");
        assert_eq!(resolved["complete"], true, "{name}: {resolved}");
        assert_eq!(resolved["text"], "search_text", "{name}: {resolved}");
        assert_eq!(
            resolved["shown_when"], both.unresolved["shown_when"],
            "{name}: {resolved}"
        );
        // the base: every stack but the one with no word, which is the
        // unresolved sample's
        let base = &resolved["axes"]["base"];
        assert_eq!(base["stacks"], planted.len() - 1, "{name}: {base}");
        assert_eq!(
            both.unresolved["axes"]["base"]["stacks"], 1,
            "{name}: {}",
            both.unresolved
        );
        // a value a word decided: its text, its stacks and the word
        assert_eq!(
            base["values"]["T2w"],
            serde_json::json!({
                "stacks": 5, "distinct": 1,
                "texts": [{"text": "t2w tse", "stacks": 5, "words": ["t2w"]}],
                "withheld": {"texts": 0, "stacks": 0},
            }),
            "{name}: {base}"
        );
        // folded by the normalised text, the one subject's word withheld
        // and counted, never shown
        let mprage = &resolved["axes"]["technique"]["values"]["MPRAGE"];
        assert_eq!(mprage["stacks"], 12, "{name}: {mprage}");
        assert_eq!(mprage["distinct"], 2, "{name}: {mprage}");
        assert_eq!(
            mprage["texts"],
            serde_json::json!([{"text": "t1w mprage", "stacks": 6, "words": ["mprage"]}]),
            "{name}: {mprage}"
        );
        assert_eq!(
            mprage["withheld"],
            serde_json::json!({"texts": 1, "stacks": 6}),
            "{name}: {mprage}"
        );
        assert!(
            !resolved.to_string().contains("zzzlone"),
            "{name}: {resolved}"
        );
        // every word shown is one the rules cited in that text
        for (axis, doc) in resolved["axes"].as_object().unwrap() {
            for (value, v) in doc["values"].as_object().unwrap() {
                for t in v["texts"].as_array().unwrap() {
                    let text = t["text"].as_str().unwrap();
                    for w in t["words"].as_array().unwrap() {
                        assert!(
                            text.contains(w.as_str().unwrap()),
                            "{name} {axis}={value}: {t}"
                        );
                    }
                }
            }
        }
        // the unresolved sample reads as it did, with no words
        assert!(
            !both.unresolved.to_string().contains("\"words\""),
            "{name}: {}",
            both.unresolved
        );
        assert_eq!(
            nils_classify::signals::unresolved_texts(reg.store(), &pack, &scope, 2_000).unwrap(),
            both.unresolved,
            "{name}"
        );
        assert_eq!(
            nils_classify::signals::resolved_texts(reg.store(), &pack, &scope, 2_000).unwrap(),
            both.resolved,
            "{name}"
        );
        // a smaller sample says it did not read everything
        let two = nils_classify::signals::texts(reg.store(), &pack, &scope, 2).unwrap();
        assert_eq!(two.resolved["read"], 2, "{name}: {}", two.resolved);
        assert_eq!(two.resolved["complete"], false, "{name}: {}", two.resolved);
    }
}

/// Record 48: the rules' own answer is held to the pack's exclusions and
/// implications, as a rater's is. A T2*-weighted turbo spin echo breaks
/// `t2star-not-spin-echo`: the values stay as the rules decided them, one
/// item names the constraint with its reason and sources, the two axes it
/// involves are written below every threshold, and the run counts it. The
/// same stack without the star breaks nothing.
#[test]
fn an_answer_that_breaks_the_pack_s_own_constraint_is_kept_doubted_and_asked() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = one_stack("ax t2* tse", &[(tags::SCANNING_SEQUENCE, VR::CS, "SE")]);
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        let (base, base_conf, _) = axis_of(&mut reg, "base");
        let (technique, technique_conf, _) = axis_of(&mut reg, "technique");
        assert_eq!(
            (base.as_str(), technique.as_str()),
            ("T2*w", "TSE"),
            "{name}"
        );
        assert_eq!(
            base_conf,
            nils_classify::classify::BROKEN_CONFIDENCE,
            "{name}"
        );
        assert_eq!(
            technique_conf,
            nils_classify::classify::BROKEN_CONFIDENCE,
            "{name}"
        );
        // an axis the constraint does not involve keeps its confidence
        let (_, contrast_conf, _) = axis_of(&mut reg, "provenance");
        assert!(
            contrast_conf > nils_classify::classify::BROKEN_CONFIDENCE,
            "{name}"
        );
        assert_eq!(asked(&mut reg, "classify.excluded"), 1, "{name}");
        let id = one(
            &mut reg,
            "SELECT MIN(id) FROM {review_item} WHERE kind = 'classify.excluded'",
        );
        let item = nils_registry::review::item(reg.store(), id)
            .unwrap()
            .expect("the item is there");
        let e = item.evidence;
        assert_eq!(e["constraint"], "t2star-not-spin-echo", "{name}: {e}");
        assert!(
            e["why"].as_str().unwrap().contains("gradient echo"),
            "{name}: {e}"
        );
        assert_eq!(
            e["sources"],
            serde_json::json!(["P8", "IM2"]),
            "{name}: {e}"
        );
        assert_eq!(e["decided"]["base"], "T2*w", "{name}: {e}");
        assert_eq!(
            report.broken.get("excluded:t2star-not-spin-echo"),
            Some(&1),
            "{name}"
        );
        assert_eq!(report.broken_stacks, 1, "{name}");
        assert!(report.to_string().contains("against the pack"), "{name}");
    }
    for lab in labs() {
        let name = lab.name;
        let dir = one_stack("ax t2 tse", &[(tags::SCANNING_SEQUENCE, VR::CS, "SE")]);
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(report.broken_stacks, 0, "{name}: {:?}", report.broken);
        assert_eq!(asked(&mut reg, "classify.excluded"), 0, "{name}");
    }
}

/// Seal every stack of the registry as one sample, as `nils labels seal`
/// would, or lift the seal when `sealed` is false.
fn seal_all(reg: &mut Registry, sealed: bool) {
    if !sealed {
        let sql = format!(
            "UPDATE {} SET unsealed_at = sealed_at, unsealed_by = 'test'",
            reg.store().qualified("sealed_stack")
        );
        reg.store().execute(&sql, &[]).unwrap();
        return;
    }
    let table = reg.store().qualified("sealed_stack");
    if one(reg, &format!("SELECT COUNT(*) FROM {table}")) > 0 {
        let sql = format!("UPDATE {table} SET unsealed_at = NULL, unsealed_by = NULL");
        reg.store().execute(&sql, &[]).unwrap();
        return;
    }
    let ids: Vec<i64> = rows(reg, "SELECT id FROM {stack} ORDER BY id")
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect();
    let now = nils_registry::time::now_iso();
    let seal: Vec<Vec<Param>> = ids
        .iter()
        .map(|id| {
            vec![
                Param::from("selection:sealed@1"),
                Param::Int(*id),
                Param::Int(0),
                Param::from("test"),
                Param::from(now.as_str()),
            ]
        })
        .collect();
    reg.store()
        .insert(
            &Insert::new(
                nils_registry::schema::table("sealed_stack"),
                &["sample", "stack_id", "subject_id", "sealed_by", "sealed_at"],
            ),
            &seal,
        )
        .unwrap();
}

/// Record 48, D1 of the move: sealed means sealed at the source. A
/// classification over stacks of a sample sealed now writes what the rules
/// found and raises no review item, not one that the doors then withhold,
/// and its rows say they raised none. Lifted, the same run asks again; and
/// `nils repair sealed-review` closes what an older engine raised, keeps
/// what a reading campaign holds, audits nothing it did not do, and finds
/// nothing the second time.
#[test]
fn a_classification_over_a_sealed_stack_raises_no_review_item() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        // record 56 section 2: a split is a note now, so the question the seal
        // keeps back is a base the rules left empty on a scan that is no scout
        let dir = some_stacks(&[
            ("ax mystery", &[(tags::SCANNING_SEQUENCE, VR::CS, "SE")]),
            ("ax mystery two", &[(tags::SCANNING_SEQUENCE, VR::CS, "SE")]),
        ]);
        let mut reg = prepare(&lab, &dir);
        seal_all(&mut reg, true);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        assert_eq!(report.review_items, 0, "{name}: the report raised none");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {review_item}"),
            0,
            "{name}: a sealed stack never becomes a review item"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification} WHERE review_items <> 0"
            ),
            0,
            "{name}: the classification rows say they raised none"
        );
        assert!(
            one(&mut reg, "SELECT COUNT(*) FROM {classification}") > 0,
            "{name}: what the rules found is still written"
        );

        // lifted, the same stacks ask the question the seal kept back
        seal_all(&mut reg, false);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let open = one(
            &mut reg,
            "SELECT COUNT(*) FROM {review_item} WHERE status = 'open'",
        );
        assert!(
            open > 0,
            "{name}: unsealed, the missing base is a question again"
        );

        // sealed again over what an older engine would have left open, with
        // one stack item a reading campaign holds
        seal_all(&mut reg, true);
        let stack = one(&mut reg, "SELECT MIN(id) FROM {stack}");
        let now = nils_registry::time::now_iso();
        let held = reg
            .store()
            .insert(
                &Insert::new(
                    nils_registry::schema::table("review_item"),
                    &["kind", "scope", "ref", "evidence", "status", "created_at"],
                )
                .returning(&["id"]),
                &[vec![
                    Param::from("campaign.axes"),
                    Param::from("stack"),
                    Param::from(serde_json::json!({"stack_id": stack}).to_string()),
                    Param::from("{}"),
                    Param::from("open"),
                    Param::from(now.as_str()),
                ]],
            )
            .unwrap()[0]
            .int(0)
            .unwrap();
        reg.store()
            .insert(
                &Insert::new(
                    nils_registry::schema::table("campaign_item"),
                    &[
                        "campaign_id",
                        "position",
                        "review_item_id",
                        "stack_id",
                        "key",
                        "state",
                        "round",
                    ],
                ),
                &[vec![
                    Param::Int(1),
                    Param::Int(1),
                    Param::Int(held),
                    Param::Int(stack),
                    Param::from("k1"),
                    Param::from("open"),
                    Param::Int(1),
                ]],
            )
            .unwrap();

        let dry = nils_registry::review::close_sealed_items(reg.store(), true).unwrap();
        assert_eq!(
            dry.closed.len() as i64,
            open,
            "{name}: every open item is sealed"
        );
        assert_eq!(dry.kept_for_campaigns, vec![held], "{name}");
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE status = 'open'"
            ),
            open + 1,
            "{name}: a dry run writes nothing"
        );
        let swept = nils_registry::review::close_sealed_items(reg.store(), false).unwrap();
        assert_eq!(swept, dry, "{name}");
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE status = 'open'"
            ),
            1,
            "{name}: only the campaign's item is still open"
        );
        assert_eq!(
            one(
                &mut reg,
                &format!(
                    "SELECT COUNT(*) FROM {{review_item}} WHERE id = {held} AND status = 'open'"
                )
            ),
            1,
            "{name}"
        );
        let again = nils_registry::review::close_sealed_items(reg.store(), false).unwrap();
        assert!(
            again.closed.is_empty(),
            "{name}: a second run finds nothing"
        );
    }
}

/// The elements a case adds to a series.
type Elements<'a> = &'a [(dicom_core::Tag, VR, &'a str)];

/// Several series of one study, each built from the elements its case needs.
fn some_stacks(series: &[(&str, Elements)]) -> TempDir {
    let dir = TempDir::new("classify-some");
    for (i, (description, extra)) in series.iter().enumerate() {
        let n = i + 1;
        let sop = format!("A.{n}.1");
        let mut e = synth::minimal_mr("A", &format!("A.{n}"), &sop);
        e.push(elem(tags::PATIENT_ID, VR::LO, "P1"));
        e.extend([
            elem(tags::SERIES_DESCRIPTION, VR::LO, description),
            elem(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
            elem(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
        ]);
        for (tag, vr, value) in *extra {
            e.push(elem(*tag, *vr, value));
        }
        dir.file(
            &format!("s{n}/1"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    dir
}

/// Record 55 H3 and record 56 section 2, Nima's rulings of 2026-10-09: the
/// pack decides, and a sort asks only where it is truly necessary. Over one
/// study: a spine whose name also says the brain, decided by the pack's
/// order (an override); a base the physics gives under the pack's threshold
/// (a low confidence); a scan nothing weights (its base, which matters, is
/// missing); a scout nothing weights either (a localizer has no base, and is
/// silent); a scan whose name says contrast was given; and no body part or
/// post-contrast a rule could name on most. The override and the weak answer
/// are kept on the stacks and asked about nowhere, the missing base of the
/// scan that is no scout is the one question, the scout's is not asked, and
/// the body part and the post-contrast, each its own operation's, are never
/// asked about, while what the rules state of them is kept.
#[test]
fn only_a_missing_answer_that_matters_is_asked_and_the_rest_is_kept() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    assert_eq!(
        nils_pack::matters::missing_asked(&pack),
        vec!["base".to_string()],
        "the axes where a missing answer is asked, as the engine works them out"
    );
    for lab in labs() {
        let name = lab.name;
        let dir = some_stacks(&[
            (
                "sag t1 cervical cerebral",
                &[
                    (tags::BODY_PART_EXAMINED, VR::CS, "SPINE"),
                    (tags::SCANNING_SEQUENCE, VR::CS, "SE"),
                    (tags::REPETITION_TIME, VR::DS, "600"),
                    (tags::ECHO_TIME, VR::DS, "12"),
                ],
            ),
            (
                "ax se",
                &[
                    (tags::SCANNING_SEQUENCE, VR::CS, "SE"),
                    (tags::REPETITION_TIME, VR::DS, "3000"),
                    (tags::ECHO_TIME, VR::DS, "12"),
                    (tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
                ],
            ),
            ("ax mystery", &[(tags::SCANNING_SEQUENCE, VR::CS, "SE")]),
            ("localizer", &[(tags::SCANNING_SEQUENCE, VR::CS, "SE")]),
            (
                "ax t1 post gd",
                &[
                    (tags::SCANNING_SEQUENCE, VR::CS, "SE"),
                    (tags::REPETITION_TIME, VR::DS, "600"),
                    (tags::ECHO_TIME, VR::DS, "12"),
                ],
            ),
        ]);
        let mut reg = prepare(&lab, &dir);
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
                .unwrap();
        let stack_of = |reg: &mut Registry, description: &str| -> i64 {
            one(
                reg,
                &format!(
                    "SELECT stack_id FROM {{stack_fingerprint}} WHERE text_series_description = '{description}'"
                ),
            )
        };
        let (spine, weak, mystery, scout, given) = (
            stack_of(&mut reg, "sag t1 cervical cerebral"),
            stack_of(&mut reg, "ax se"),
            stack_of(&mut reg, "ax mystery"),
            stack_of(&mut reg, "localizer"),
            stack_of(&mut reg, "ax t1 post gd"),
        );
        let notes = |reg: &mut Registry, stack: i64| -> serde_json::Value {
            let r = rows(
                reg,
                &format!(
                    "SELECT CAST(notes AS TEXT) FROM {{classification}} WHERE stack_id = {stack}"
                ),
            );
            r[0].opt_text(0)
                .unwrap()
                .and_then(|t| serde_json::from_str(t).ok())
                .unwrap_or(serde_json::Value::Null)
        };

        // nothing the pack decided is a question, and nothing about the body
        // part or the post-contrast is
        for kind in [
            "%:conflict",
            "%:low_confidence",
            "split:%",
            "body_part:%",
            "body_region:%",
            "post_contrast:%",
        ] {
            assert_eq!(
                one(
                    &mut reg,
                    &format!("SELECT COUNT(*) FROM {{review_item}} WHERE kind LIKE '{kind}'")
                ),
                0,
                "{name}: {kind}"
            );
        }

        // the override is kept on its stack, with what ranked it
        let n = notes(&mut reg, spine);
        assert!(
            n["overrides"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["axis"] == "body_part"
                    && o["by"]["rule"] == "spine"
                    && o["over"]["rule"] == "brain"
                    && o["rank"] == "rule_order"),
            "{name}: {n}"
        );
        // the weak answer is kept on its stack
        let n = notes(&mut reg, weak);
        assert!(
            n["below"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["axis"] == "base" && b["value"] == "PDw"),
            "{name}: {n}"
        );

        // the missing base is the question, on the one stack nothing weights
        let missing = rows(
            &mut reg,
            "SELECT i.id, i.scope FROM {review_item} i WHERE i.kind = 'base:missing' AND i.status = 'open'",
        );
        assert_eq!(missing.len(), 1, "{name}: one question");
        let members: Vec<i64> = rows(
            &mut reg,
            &format!(
                "SELECT stack_id FROM {{review_member}} WHERE item_id = {}",
                missing[0].int(0).unwrap()
            ),
        )
        .iter()
        .map(|r| r.int(0).unwrap())
        .collect();
        assert_eq!(members, vec![mystery], "{name}");
        assert_eq!(
            report.missing.get("base"),
            Some(&1),
            "{name}: {:?}",
            report.missing
        );
        let item = nils_registry::review::item(reg.store(), missing[0].int(0).unwrap())
            .unwrap()
            .expect("the item is there");
        assert_eq!(item.evidence["axis"], "base", "{name}: {}", item.evidence);
        // the stack's classification counts it, and notes the axes no rule
        // answered, the body part among them, which is never asked
        assert!(
            one(
                &mut reg,
                &format!("SELECT review_items FROM {{classification}} WHERE stack_id = {mystery}")
            ) >= 1,
            "{name}"
        );
        let n = notes(&mut reg, mystery);
        let unresolved: Vec<&str> = n["unresolved"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(unresolved.contains(&"base"), "{name}: {n}");
        assert!(unresolved.contains(&"body_part"), "{name}: {n}");
        assert!(unresolved.contains(&"post_contrast"), "{name}: {n}");
        assert!(
            report.unresolved.get("body_part").copied().unwrap_or(0) >= 2,
            "{name}: {:?}",
            report.unresolved
        );
        assert_eq!(report.missing.get("body_part"), None, "{name}");
        assert_eq!(report.missing.get("post_contrast"), None, "{name}");

        // the scout: a localizer, silent, its empty base noted and not asked
        let directory_type = rows(
            &mut reg,
            &format!(
                "SELECT value FROM {{classification_axis}} WHERE axis = 'directory_type' AND stack_id = {scout}"
            ),
        );
        assert_eq!(
            directory_type[0].opt_text(0).unwrap(),
            Some("localizer"),
            "{name}"
        );
        assert!(!members.contains(&scout), "{name}: the scout is not asked");
        let n = notes(&mut reg, scout);
        assert!(
            n["unresolved"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "base"),
            "{name}: {n}"
        );
        assert_eq!(
            report.silent, 1,
            "{name}: the scout is the one silent stack"
        );

        // the post-contrast the rules state is kept as they state it
        let stated = rows(
            &mut reg,
            &format!(
                "SELECT value FROM {{classification_axis}} WHERE axis = 'post_contrast' AND stack_id = {given}"
            ),
        );
        assert_eq!(stated[0].opt_text(0).unwrap(), Some("1"), "{name}: given");
    }
}

/// Review of 2026-10-10: a sort supersedes the questions it asks itself and
/// no others. A body-part model's grouped question about a stack, and a
/// question about an axis the pack leaves to an operation of its own, stay
/// open through a re-sort; the sort's own missing base is superseded and
/// asked again, once.
#[test]
fn a_sort_leaves_the_questions_of_other_operations_open() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    assert!(pack.review.by_model.iter().any(|a| a == "post_contrast"));
    for lab in labs() {
        let name = lab.name;
        let dir = some_stacks(&[("ax mystery", &[(tags::SCANNING_SEQUENCE, VR::CS, "SE")])]);
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let stack = one(&mut reg, "SELECT stack_id FROM {classification}");
        let item = |reg: &mut Registry, kind: &str, scope: &str| -> i64 {
            let reference = if scope == "stack" {
                serde_json::json!({"stack_id": stack}).to_string()
            } else {
                serde_json::json!({"run": 1}).to_string()
            };
            reg.store()
                .insert(
                    &Insert::new(
                        nils_registry::schema::table("review_item"),
                        &[
                            "kind",
                            "scope",
                            "ref",
                            "evidence",
                            "status",
                            "created_at",
                            "members",
                        ],
                    ),
                    &[vec![
                        Param::from(kind),
                        Param::from(scope),
                        Param::from(reference),
                        Param::from("{}"),
                        Param::from("open"),
                        Param::from(nils_registry::time::now_iso()),
                        Param::Int(1),
                    ]],
                )
                .unwrap();
            let id = one(reg, "SELECT MAX(id) FROM {review_item}");
            if scope == "group" {
                reg.store()
                    .insert(
                        &Insert::new(
                            nils_registry::schema::table("review_member"),
                            &["item_id", "stack_id"],
                        ),
                        &[vec![Param::Int(id), Param::Int(stack)]],
                    )
                    .unwrap();
            }
            id
        };
        let model = item(&mut reg, "body_part:model", "group");
        let step = item(&mut reg, "post_contrast:missing", "stack");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let status = |reg: &mut Registry, id: i64| -> String {
            rows(
                reg,
                &format!("SELECT status FROM {{review_item}} WHERE id = {id}"),
            )[0]
            .text(0)
            .unwrap()
            .to_string()
        };
        assert_eq!(
            status(&mut reg, model),
            "open",
            "{name}: the model's question"
        );
        assert_eq!(
            status(&mut reg, step),
            "open",
            "{name}: the step's question"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind = 'base:missing' AND status = 'open'"
            ),
            1,
            "{name}: the sort's own question, asked again once"
        );
        assert!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind = 'base:missing' AND status = 'superseded'"
            ) >= 1,
            "{name}: and the earlier one superseded"
        );
    }
}
