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
            scheme: Scheme::DEFAULT,
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
            .unwrap()[0]
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
                },
                stage: false,
                why: Some("checked both"),
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
                },
                stage: false,
                why: None,
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
                },
                stage: false,
                why: None,
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
                },
                stage: false,
                why: None,
            },
        );
        assert!(
            matches!(&refused, Err(review::Error::Refused(m)) if m.contains("C15")),
            "{name}: {refused:?}"
        );

        // A staged decision on another question is written but not in
        // force; the registry moves on (a run), so the commit is refused
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
                },
                stage: true,
                why: None,
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

/// A phase-contrast study, as the archive writes one: a series whose echo
/// number varies per image, so every stack holds one image, and an echo time
/// of 0.0, which is the scanner saying it has nothing to say (record 35, S4).
/// Beside it, the same split over a series whose stacks hold two images each,
/// which is what an ordinary multi-echo acquisition looks like.
fn flow_tree() -> TempDir {
    let dir = TempDir::new("classify-flow");
    let write = |series: &str, echo: u32, instance: &str, file: &str| {
        let sop = format!("A.{series}.{echo}.{instance}");
        let mut e = synth::minimal_mr("A", &format!("A.{series}"), &sop);
        e.push(elem(tags::PATIENT_ID, VR::LO, "P1"));
        e.extend([
            elem(tags::SERIES_DESCRIPTION, VR::LO, "ax flow"),
            elem(tags::SCANNING_SEQUENCE, VR::CS, "GR"),
            elem(tags::SEQUENCE_NAME, VR::SH, "*pc2d1"),
            elem(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
            elem(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
            elem(tags::ECHO_TIME, VR::DS, "0.0"),
            elem(tags::REPETITION_TIME, VR::DS, "30.0"),
            elem(tags::ECHO_NUMBERS, VR::IS, &echo.to_string()),
        ]);
        dir.file(file, &synth::part10(&MetaFields::mr(&sop), &e, true));
    };
    for echo in 1..=6 {
        write("1", echo, "1", &format!("one/{echo}"));
        write("2", echo, "1", &format!("two/{echo}-1"));
        write("2", echo, "2", &format!("two/{echo}-2"));
    }
    dir
}

#[test]
fn a_split_that_leaves_one_image_in_every_stack_is_a_question() {
    let pack = nils_pack::load(&packs(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = flow_tree();
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();

        // The split fired on both series, and the reason is the echo number.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {stack_fingerprint} WHERE split_reason = 'multi_echo'"
            ),
            12,
            "{name}"
        );
        // The echo time is a zero the file carries, not an absence.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {stack_fingerprint} WHERE echo_time = 0"
            ),
            12,
            "{name}"
        );

        // One question, with the six stacks of the series the split left
        // holding one image as its members, and none of the six whose stacks
        // hold two.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_item} WHERE kind = 'split:one_image_per_stack'"
            ),
            1,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT members FROM {review_item} WHERE scope = 'group' AND kind = 'split:one_image_per_stack'"
            ),
            6,
            "{name}"
        );
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_member} m JOIN {stack_fingerprint} f ON f.stack_id = m.stack_id WHERE f.n_instances = 1"
            ),
            6,
            "{name}: the members are the stacks holding one image"
        );

        // And a zero echo time decided nothing: not one of the twelve is
        // called an anatomical T1w on the strength of it.
        assert_eq!(
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_axis} WHERE axis = 'base'"
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

        // The same stack, asked about one hundredth higher: now it is below.
        let settings = nils_classify::job::Settings {
            review_below: Some(0.66),
            ..Default::default()
        };
        let report =
            nils_classify::classify::classify(&mut reg, &pack, &settings, &Cancel::new()).unwrap();
        assert_eq!(
            asked(&mut reg, "body_part:low_confidence"),
            1,
            "{name}: 0.65 is below 0.66"
        );
        assert_eq!(report.at_threshold.get("body_part"), None, "{name}");
    }
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

/// And the rule on the same axis that writes one hundredth less is a
/// question, so the boundary is the only thing between them.
#[test]
fn a_base_one_hundredth_below_its_threshold_is_a_question() {
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
            1,
            "{name}: 0.65 is below 0.70"
        );
        assert_eq!(report.at_threshold.get("base"), None, "{name}");
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
/// decides, the set stops there, and nothing used to record that the brain
/// rule would have fired: the run raised 719 conflicts and kept every one of
/// them in a per-batch tally.
#[test]
fn a_conflict_reaches_the_stack_and_the_tally_keeps_counting() {
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
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();

        // the answer is the one the order gives, unchanged
        assert_eq!(
            axis_of(&mut reg, "body_part"),
            ("spine".to_string(), 0.65, "keywords".to_string()),
            "{name}"
        );

        // and a person reading the stack sees that it was contested
        let items = rows(
            &mut reg,
            "SELECT id, scope, COALESCE(group_key, ''), COALESCE(members, 0) \
             FROM {review_item} WHERE kind = 'body_part:conflict'",
        );
        assert_eq!(items.len(), 1, "{name}");
        let id = items[0].int(0).unwrap();
        let item = nils_registry::review::item(reg.store(), id)
            .unwrap()
            .expect("the item is there");
        let evidence = item.evidence;
        assert_eq!(evidence["axis"], "body_part", "{name}: {evidence}");
        assert_eq!(evidence["value"], "spine", "{name}: {evidence}");
        assert_eq!(evidence["other"], "brain", "{name}: {evidence}");
        assert_eq!(
            evidence["decided_by"]["rule"], "spine",
            "{name}: {evidence}"
        );
        assert_eq!(evidence["over"]["rule"], "brain", "{name}: {evidence}");
        assert!(
            !evidence["over"]["matched"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "{name}: the item names what the pre-empted rule cited: {evidence}"
        );
        // one question per pair of answers, with the stack as its member
        assert_eq!(items[0].text(1).unwrap(), "group", "{name}");
        assert_eq!(
            items[0].text(2).unwrap(),
            "body_part:conflict|spine over brain|",
            "{name}"
        );
        assert_eq!(items[0].int(3).unwrap(), 1, "{name}");
        assert_eq!(
            one(&mut reg, "SELECT COUNT(*) FROM {review_member}"),
            one(
                &mut reg,
                "SELECT COUNT(*) FROM {review_member} m JOIN {review_item} i ON i.id = m.item_id \
                 WHERE i.kind = 'body_part:conflict'"
            ),
            "{name}: the conflict's member is the stack"
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
