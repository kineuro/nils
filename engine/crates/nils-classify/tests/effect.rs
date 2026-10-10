// SPDX-License-Identifier: AGPL-3.0-only

//! Record 56 §5.4: a rule change's effect on the sorting, measured on a
//! planted registry against the full re-sort it predicts. The report's
//! moves equal the diff of a classify under the patched pack, stack for
//! stack and axis for axis; its questions equal the questions that classify
//! leaves open; a stack of a sealed sample is never counted; and the
//! answers people settled say what the change fixes and breaks.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_pack::patch::{self, Patch};
use nils_registry::home::{Home, InitOptions};
use nils_registry::store::{Insert, Param};
use nils_registry::{Backend, Registry, Scheme, Store};
use serde_json::Value;

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_effect";

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
        let dir = TempDir::new("effect-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-classify-effect-key")
            .unwrap();
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

fn mri() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

fn text(tag: dicom_core::Tag, vr: VR, v: &str) -> synth::Elem {
    synth::text(tag, vr, v)
}

/// One series, one file, one stack.
fn series(dir: &TempDir, study: &str, n: u32, patient: &str, extra: &[synth::Elem], ct: bool) {
    let series = format!("{study}.{n}");
    let sop = format!("{series}.1");
    let mut e = if ct {
        synth::minimal_ct(study, &series, &sop)
    } else {
        synth::minimal_mr(study, &series, &sop)
    };
    e.push(text(tags::PATIENT_ID, VR::LO, patient));
    e.push(text(tags::SERIES_NUMBER, VR::IS, &n.to_string()));
    e.extend(extra.iter().cloned());
    let meta = if ct {
        MetaFields::ct(&sop)
    } else {
        MetaFields::mr(&sop)
    };
    dir.file(&format!("{study}/{n}"), &synth::part10(&meta, &e, true));
}

fn mprage(maker: &str, name: &str) -> Vec<synth::Elem> {
    vec![
        text(tags::SERIES_DESCRIPTION, VR::LO, name),
        text(tags::SCANNING_SEQUENCE, VR::CS, "GR"),
        text(tags::SEQUENCE_VARIANT, VR::CS, "SK\\SP\\MP"),
        text(tags::SEQUENCE_NAME, VR::SH, "*tfl3d1_16"),
        text(tags::MR_ACQUISITION_TYPE, VR::CS, "3D"),
        text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND\\NORM"),
        text(tags::MANUFACTURER, VR::LO, maker),
    ]
}

fn spin_echo(maker: &str, name: &str) -> Vec<synth::Elem> {
    vec![
        text(tags::SERIES_DESCRIPTION, VR::LO, name),
        text(tags::SCANNING_SEQUENCE, VR::CS, "SE"),
        text(tags::SEQUENCE_VARIANT, VR::CS, "NONE"),
        text(tags::MR_ACQUISITION_TYPE, VR::CS, "2D"),
        text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
        text(tags::MANUFACTURER, VR::LO, maker),
    ]
}

/// A registry of three subjects, each a study from a Siemens (anatomy and a
/// spin echo named a word the pack does not know) and a study from a maker
/// whose contrast series is named the way a partner site names it; the
/// first also a study from a maker the patch silences, and a CT study. The
/// maker is the study's, as the fingerprint reads it.
fn tree() -> TempDir {
    let dir = TempDir::new("effect-src");
    for (s, patient) in ["P1", "P2", "P3"].iter().enumerate() {
        let study = |n: u32| format!("1.2.9.{}.{n}", s + 1);
        series(
            &dir,
            &study(1),
            1,
            patient,
            &mprage("SIEMENS", "sag t1 mprage"),
            false,
        );
        series(
            &dir,
            &study(1),
            2,
            patient,
            &spin_echo("SIEMENS", "ax zqzq pd"),
            false,
        );
        series(
            &dir,
            &study(2),
            1,
            patient,
            &mprage("ACME MR", "ax t1 mdc"),
            false,
        );
        if s == 0 {
            series(
                &dir,
                &study(3),
                1,
                patient,
                &mprage("SILENT CO", "sag t1 mprage"),
                false,
            );
            series(
                &dir,
                &study(4),
                1,
                patient,
                &[text(tags::MANUFACTURER, VR::LO, "CTCO")],
                true,
            );
        }
    }
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

/// Every stored axis of every stack, values in order, as a re-sort leaves it.
fn stored(reg: &mut Registry) -> BTreeMap<i64, BTreeMap<String, String>> {
    let sql = format!(
        "SELECT stack_id, axis, value FROM {} ORDER BY id",
        reg.store().qualified("classification_axis")
    );
    let mut out: BTreeMap<i64, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for r in reg.store().query(&sql, &[]).unwrap() {
        let Some(v) = r.opt_text(2).unwrap().filter(|v| !v.is_empty()) else {
            continue;
        };
        out.entry(r.int(0).unwrap())
            .or_default()
            .entry(r.text(1).unwrap().to_string())
            .or_default()
            .push(v.to_string());
    }
    out.into_iter()
        .map(|(s, m)| {
            (
                s,
                m.into_iter()
                    .map(|(a, mut v)| {
                        v.sort();
                        (a, v.join(","))
                    })
                    .collect(),
            )
        })
        .collect()
}

/// The open questions about stacks, by kind, counting a grouped one's members.
fn open_questions(reg: &mut Registry) -> BTreeMap<String, i64> {
    let items = reg.store().qualified("review_item");
    let members = reg.store().qualified("review_member");
    let mut out = BTreeMap::new();
    for r in reg
        .store()
        .query(
            &format!(
                "SELECT i.kind, COUNT(m.stack_id) FROM {items} i JOIN {members} m ON m.item_id = i.id \
                 WHERE i.status = 'open' AND i.scope = 'group' GROUP BY i.kind"
            ),
            &[],
        )
        .unwrap()
    {
        *out.entry(r.text(0).unwrap().to_string()).or_insert(0) += r.int(1).unwrap();
    }
    for r in reg
        .store()
        .query(
            &format!(
                "SELECT kind, COUNT(*) FROM {items} WHERE status = 'open' AND scope = 'stack' GROUP BY kind"
            ),
            &[],
        )
        .unwrap()
    {
        *out.entry(r.text(0).unwrap().to_string()).or_insert(0) += r.int(1).unwrap();
    }
    out
}

fn sort_csv(v: &str) -> String {
    let mut parts: Vec<&str> = v.split(',').filter(|x| !x.is_empty()).collect();
    parts.sort_unstable();
    parts.join(",")
}

/// (axis, from, to) -> stacks, from a report.
fn report_moves(doc: &Value) -> BTreeMap<(String, String, String), i64> {
    let mut out = BTreeMap::new();
    for a in doc["axes"].as_array().unwrap() {
        for t in a["transitions"].as_array().unwrap() {
            *out.entry((
                a["axis"].as_str().unwrap().to_string(),
                sort_csv(t["from"].as_str().unwrap()),
                sort_csv(t["to"].as_str().unwrap()),
            ))
            .or_insert(0) += t["stacks"].as_i64().unwrap();
        }
    }
    out
}

fn diff(
    before: &BTreeMap<i64, BTreeMap<String, String>>,
    after: &BTreeMap<i64, BTreeMap<String, String>>,
    skip: &BTreeSet<i64>,
) -> BTreeMap<(String, String, String), i64> {
    let mut out = BTreeMap::new();
    let none = BTreeMap::new();
    let stacks: BTreeSet<&i64> = before.keys().chain(after.keys()).collect();
    for s in stacks {
        if skip.contains(s) {
            continue;
        }
        let (b, a) = (
            before.get(s).unwrap_or(&none),
            after.get(s).unwrap_or(&none),
        );
        let axes: BTreeSet<&String> = b.keys().chain(a.keys()).collect();
        for axis in axes {
            let x = b.get(axis).cloned().unwrap_or_default();
            let y = a.get(axis).cloned().unwrap_or_default();
            if x != y {
                *out.entry((axis.clone(), x, y)).or_insert(0) += 1;
            }
        }
    }
    out
}

fn names(
    _: &nils_pack::Pack,
    axes: &BTreeMap<String, String>,
    _: &nils_classify::effect::NameFacts,
) -> (String, Option<String>) {
    (
        format!(
            "{}_{}",
            axes.get("base").cloned().unwrap_or_default(),
            axes.get("technique").cloned().unwrap_or_default()
        ),
        None,
    )
}

const OPS: &str = "\
patch: 1
pack: mri
reason: the planted registry's check
evidence: this test
operations:
  - {op: add_words, axis: technique, value: TSE, words: [zqzq]}
  - {op: add_words, axis: post_contrast, value: given, words: [mdc]}
  - {op: by_model, axis: base}
  - {op: silence, axis: base, when: {tag: manufacturer, is: silent co}}
";

fn decide(reg: &mut Registry, stack: i64, axis: &str, value: &str) {
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
                Param::from(axis),
                Param::from(value),
                Param::from("a person"),
                Param::from("person"),
                Param::from("read by eye"),
                Param::from(nils_registry::time::now_iso()),
            ]],
        )
        .unwrap();
}

/// A person's decision that `axis` holds nothing on one stack.
fn decide_nothing(reg: &mut Registry, stack: i64, axis: &str) {
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
                Param::from(axis),
                Param::Null,
                Param::from("a person"),
                Param::from("person"),
                Param::from("read by eye"),
                Param::from(nils_registry::time::now_iso()),
            ]],
        )
        .unwrap();
}

fn stack_named(reg: &mut Registry, maker: &str, name: &str, nth: usize) -> i64 {
    let sql = format!(
        "SELECT stack_id FROM {} WHERE manufacturer = '{maker}' AND text_series_description LIKE '%{name}%' ORDER BY stack_id",
        reg.store().qualified("stack_fingerprint")
    );
    reg.store().query(&sql, &[]).unwrap()[nth].int(0).unwrap()
}

#[test]
fn the_report_s_moves_and_questions_equal_a_full_re_sort_and_a_sealed_stack_is_never_counted() {
    let pack = nils_pack::load(&mri(), None).expect("the MRI pack loads");
    let p = Patch::parse("ops", OPS).unwrap();
    let patched = patch::apply(&mri(), &p, &|_| true).unwrap();
    assert!(
        patched.cases.is_none(),
        "the planted patch keeps the pack's cases: {:?}",
        patched.cases.map(|e| e.to_string())
    );
    let out = TempDir::new("effect-pack");
    let written = out.path().join("mri");
    patched.docs.write(&written, Some("1.0.99")).unwrap();
    let after_pack = nils_pack::load(&written, None).expect("the patched pack loads");

    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        // a stack the change moves, sealed now: never read, never counted
        let zqzq: Vec<i64> = (0..3)
            .map(|i| stack_named(&mut reg, "SIEMENS", "zqzq", i))
            .collect();
        let sealed = zqzq[2];
        nils_registry::labels::seal(&mut reg, "certificate-x", None, &[sealed], "a test").unwrap();
        // the answers people settled: the spin echo is a TSE (the rules
        // say otherwise before the change), the silenced stack is a T1w
        decide(&mut reg, zqzq[0], "technique", "TSE");
        let silent = stack_named(&mut reg, "SILENT CO", "mprage", 0);
        decide(&mut reg, silent, "base", "T1w");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let before = stored(&mut reg);
        let asked_before = open_questions(&mut reg);

        let settings = nils_classify::effect::Settings {
            scope: None,
            examples: 10,
            workers: 2,
        };
        let doc = nils_classify::effect::run(reg.store(), &mri(), &pack, &p, &settings, &names)
            .unwrap_or_else(|e| panic!("{name}: {e}"));

        // the full re-sort under the patched pack
        nils_classify::classify::classify(
            &mut reg,
            &after_pack,
            &Default::default(),
            &Cancel::new(),
        )
        .unwrap();
        let after = stored(&mut reg);
        let asked_after = open_questions(&mut reg);

        let skip: BTreeSet<i64> = [sealed].into_iter().collect();
        let resort = diff(&before, &after, &skip);
        assert!(
            !resort.is_empty(),
            "{name}: the planted change moves stacks"
        );
        assert_eq!(
            report_moves(&doc),
            resort,
            "{name}: the report's moves are the re-sort's"
        );
        // the sealed stack moves in the re-sort, and is in no count
        let with_sealed = diff(&before, &after, &BTreeSet::new());
        assert_ne!(
            with_sealed, resort,
            "{name}: the sealed stack moved in the re-sort"
        );
        assert_eq!(doc["scope"]["sealed_left_out"], 1, "{name}");
        let text = doc.to_string();
        assert!(
            !doc["axes"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|a| a["transitions"].as_array().unwrap())
                .flat_map(|t| t["examples"].as_array().unwrap())
                .any(|e| e["stack"] == sealed),
            "{name}: a sealed stack is no example: {text}"
        );
        // the questions: what appears and goes is what the re-sort left open
        let mut net: BTreeMap<String, i64> = BTreeMap::new();
        for k in doc["review"]["by_kind"].as_array().unwrap() {
            net.insert(
                k["kind"].as_str().unwrap().to_string(),
                k["appear"].as_i64().unwrap() - k["disappear"].as_i64().unwrap(),
            );
        }
        let kinds: BTreeSet<&String> = asked_before.keys().chain(asked_after.keys()).collect();
        for k in kinds {
            let want = asked_after.get(k).copied().unwrap_or(0)
                - asked_before.get(k).copied().unwrap_or(0);
            assert_eq!(
                net.get(k).copied().unwrap_or(0),
                want,
                "{name}: {k}: {asked_before:?} -> {asked_after:?}, report {net:?}"
            );
        }
        // the decision holds the technique both ways: no move, said
        let technique = doc["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axis"] == "technique")
            .unwrap();
        assert!(
            technique["held_by_decision"].as_i64().unwrap() >= 1,
            "{name}: {technique}"
        );
        // the answers: one fixed, one broken
        let answers = &doc["answers"];
        assert_eq!(answers["sets"][0]["answers"], 2, "{name}: {answers}");
        assert_eq!(
            answers["by_axis"]["technique"]["wrong_to_right"], 1,
            "{name}: {answers}"
        );
        assert_eq!(
            answers["by_axis"]["base"]["right_to_unanswered"], 1,
            "{name}: {answers}"
        );
        assert_eq!(answers["fixes"], 1, "{name}");
        assert_eq!(answers["breaks"], 1, "{name}");
        // what it ships as, and what was replayed
        assert_eq!(doc["ships"]["as"], "rules release", "{name}");
        assert_eq!(doc["ships"]["version"], "1.0.3", "{name}");
        assert_eq!(doc["scope"]["replayed"], "pack", "{name}");
    }
}

#[test]
fn an_operation_scoped_to_a_scanner_moves_its_scanner_s_stacks_alone() {
    let pack = nils_pack::load(&mri(), None).expect("the MRI pack loads");
    let p = Patch::parse(
        "ops",
        "patch: 1\npack: mri\nreason: r\nevidence: e\noperations:\n  - {op: add_words, axis: post_contrast, value: given, words: [mdc], scope: 'scanner:manufacturer=ACME MR'}\n  - {op: add_words, axis: technique, value: TSE, words: [zqzq], scope: 'scanner:manufacturer=OTHER'}\n",
    )
    .unwrap();
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let settings = nils_classify::effect::Settings::default();
        let doc = nils_classify::effect::run(reg.store(), &mri(), &pack, &p, &settings, &names)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let moves = report_moves(&doc);
        // the contrast word moves ACME's three stacks; the TSE word is
        // scoped to a maker the registry does not hold, and moves nothing
        assert_eq!(
            moves.get(&("post_contrast".into(), String::new(), "1".into())),
            Some(&3),
            "{name}: {moves:?}"
        );
        assert!(
            !moves.keys().any(|(axis, _, _)| axis == "technique"),
            "{name}: {moves:?}"
        );
        assert_eq!(doc["ships"]["as"], "overlay", "{name}");
        let by_op = doc["scope"]["by_operation"].as_array().unwrap();
        assert_eq!(by_op[0]["stacks"], 3, "{name}");
        assert_eq!(by_op[1]["stacks"], 0, "{name}");
        // every row of the move is ACME's
        let pc = doc["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["axis"] == "post_contrast")
            .unwrap();
        assert_eq!(pc["transitions"][0]["makes"]["ACME MR"], 3, "{name}: {pc}");
    }
}

#[test]
fn a_pack_s_vote_never_fills_a_stack_of_another_modality() {
    let pack = nils_pack::load(&mri(), None).expect("the MRI pack loads");
    for lab in labs() {
        let name = lab.name;
        let dir = tree();
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let sql = format!(
            "SELECT COUNT(*) FROM {} a JOIN {} f ON f.stack_id = a.stack_id WHERE f.modality <> 'MR'",
            reg.store().qualified("classification_axis"),
            reg.store().qualified("stack_fingerprint")
        );
        assert_eq!(
            reg.store().query(&sql, &[]).unwrap()[0].int(0).unwrap(),
            0,
            "{name}: the MRI pack wrote nothing on a CT stack"
        );
    }
}

/// One MR series of `files` files, one stack: a stack of two images or
/// fewer is a fragment a pick sets aside.
fn images(dir: &TempDir, study: &str, n: u32, patient: &str, extra: &[synth::Elem], files: u32) {
    let series = format!("{study}.{n}");
    for f in 1..=files {
        let sop = format!("{series}.{f}");
        let mut e = synth::minimal_mr(study, &series, &sop);
        e.push(text(tags::PATIENT_ID, VR::LO, patient));
        e.push(text(tags::SERIES_NUMBER, VR::IS, &n.to_string()));
        e.extend(extra.iter().cloned());
        dir.file(
            &format!("{study}/{n}/{f}"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
}

/// A registry of dated studies, so that a pick has occasions: an ACME and a
/// Siemens MPRAGE on one day, the ACME one of more images and so the
/// winner; an ACME one alone; and a Siemens one written twice.
fn dated_tree() -> TempDir {
    let dir = TempDir::new("effect-dated");
    let on = |date: &str, mut e: Vec<synth::Elem>| {
        e.push(text(tags::STUDY_DATE, VR::DA, date));
        e
    };
    let study = |s: u32| format!("1.2.9.8.{s}");
    let mut acme = on("20240115", mprage("ACME MR", "ax t1 mprage"));
    acme.push(text(tags::ECHO_TIME, VR::DS, "3.5"));
    images(&dir, &study(0), 1, "P1", &acme, 8);
    let siemens = on("20240115", mprage("SIEMENS", "sag t1 mprage"));
    images(&dir, &study(1), 1, "P1", &siemens, 4);
    let alone = on("20240201", mprage("ACME MR", "ax t1 mprage"));
    images(&dir, &study(2), 1, "P2", &alone, 4);
    for n in [1, 2] {
        let twice = on("20240301", mprage("SIEMENS", "sag t1 mprage"));
        images(&dir, &study(3), n, "P3", &twice, 4);
    }
    dir
}

/// What a pick run writes, per occasion (role, subject, day): the stacks
/// it picked and whether it raised a border. The runs before it are
/// forgotten first, so what stands is this run's alone.
type Run = BTreeMap<(String, i64, String), (Vec<i64>, bool)>;

fn pick_run(reg: &mut Registry, pack: &nils_pack::Pack) -> Run {
    let pick = reg.store().qualified("pick");
    let pick_stack = reg.store().qualified("pick_stack");
    reg.store()
        .batch(&format!(
            "DELETE FROM {pick_stack} WHERE pick_id IN (SELECT id FROM {pick} WHERE author_kind = 'agent'); \
             DELETE FROM {pick} WHERE author_kind = 'agent'"
        ))
        .unwrap();
    nils_classify::picking::run(
        reg,
        pack,
        &nils_registry::session::Scheme::default(),
        None,
        "a test",
    )
    .unwrap();
    let day = reg.store().dialect().text_of(
        nils_registry::schema::table("pick")
            .column("session_day")
            .unwrap(),
    );
    let mut out: Run = BTreeMap::new();
    for r in reg
        .store()
        .query(
            &format!(
                "SELECT p.role, p.subject_id, {day}, p.borders, ps.stack_id FROM {pick} p \
                 JOIN {pick_stack} ps ON ps.pick_id = p.id WHERE p.author_kind = 'agent' \
                 ORDER BY ps.stack_id"
            ),
            &[],
        )
        .unwrap()
    {
        let e = out
            .entry((
                r.text(0).unwrap().to_string(),
                r.int(1).unwrap(),
                r.text(2).unwrap().to_string(),
            ))
            .or_insert_with(|| (Vec::new(), false));
        e.0.push(r.int(4).unwrap());
        e.1 = r.opt_text(3).unwrap().is_some_and(|b| !b.is_empty());
    }
    out
}

/// The counts the report gives, from two runs.
fn counted(before: &Run, after: &Run) -> BTreeMap<&'static str, i64> {
    let mut c: BTreeMap<&'static str, i64> = BTreeMap::new();
    let keys: BTreeSet<&(String, i64, String)> = before.keys().chain(after.keys()).collect();
    for k in keys {
        let (b, a) = (before.get(k), after.get(k));
        *c.entry("occasions").or_insert(0) += 1;
        let pick = match (b, a) {
            (Some(x), Some(y)) if x.0 == y.0 => "kept",
            (Some(_), Some(_)) => "changed",
            (Some(_), None) => "removed",
            (None, Some(_)) => "added",
            (None, None) => unreachable!(),
        };
        *c.entry(pick).or_insert(0) += 1;
        let border = match (b.is_some_and(|x| x.1), a.is_some_and(|y| y.1)) {
            (false, true) => "raised",
            (true, false) => "settled",
            (true, true) => "still",
            (false, false) => continue,
        };
        *c.entry(border).or_insert(0) += 1;
    }
    c
}

#[test]
fn a_change_to_the_pick_file_is_scored_by_the_patched_pick_and_equals_two_pick_runs() {
    // The 2026-10-10 study of the pick borders: a pick change rehearsed is
    // scored by the patched pack's pick, and what the report counts is what
    // a pick run by each pack writes.
    let pack = nils_pack::load(&mri(), None).expect("the MRI pack loads");
    let p = Patch::parse(
        "ops",
        "patch: 1\npack: mri\nreason: the planted registry's pick\nevidence: this test\noperations:\n\
         \x20 - {op: set_candidates, role: t1w, unless: [{tag: manufacturer, is: ACME MR}]}\n\
         \x20 - {op: set_runner_up_within, within: 0}\n\
         \x20 - {op: remove_border, border: rare}\n",
    )
    .unwrap();
    let patched = patch::apply(&mri(), &p, &|_| true).unwrap();
    assert_eq!(patched.pack.contract, 9);
    for lab in labs() {
        let name = lab.name;
        let dir = dated_tree();
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let before = pick_run(&mut reg, &pack);
        assert!(!before.is_empty(), "{name}: the planted registry has picks");
        let settings = nils_classify::effect::Settings {
            scope: None,
            examples: 50,
            workers: 2,
        };
        let doc = nils_classify::effect::run(reg.store(), &mri(), &pack, &p, &settings, &names)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let after = pick_run(&mut reg, &patched.pack);
        let want = counted(&before, &after);
        let picks = &doc["picks"];
        for k in ["occasions", "kept", "changed", "removed", "added"] {
            assert_eq!(
                picks[k].as_i64().unwrap(),
                want.get(k).copied().unwrap_or(0),
                "{name}: {k}: {picks}"
            );
        }
        for k in ["raised", "settled", "still"] {
            assert_eq!(
                picks["borders"][k].as_i64().unwrap(),
                want.get(k).copied().unwrap_or(0),
                "{name}: borders {k}: {picks}"
            );
        }
        // the change reaches the picks: ACME's T1w compete no more, so the
        // occasion it won goes to the Siemens one and the one it had alone
        // has no pick
        assert_eq!(want.get("changed"), Some(&1), "{name}: {want:?}");
        assert_eq!(want.get("removed"), Some(&1), "{name}: {want:?}");
        // and the Siemens one written twice is a retake both ways
        assert_eq!(want.get("still"), Some(&1), "{name}: {want:?}");
        assert_eq!(
            picks["moved"].as_i64().unwrap(),
            picks["changed"].as_i64().unwrap()
                + picks["removed"].as_i64().unwrap()
                + picks["added"].as_i64().unwrap(),
            "{name}"
        );
        // the roles add up to the model, and an example says what moved
        let model = &picks["models"][0];
        let roles: i64 = model["by_role"]
            .as_object()
            .unwrap()
            .values()
            .map(|r| r["occasions"].as_i64().unwrap())
            .sum();
        assert_eq!(roles, model["occasions"].as_i64().unwrap(), "{name}");
        assert!(
            model["examples"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["before"] != e["after"]
                    || e["borders"]["before"] != e["borders"]["after"]),
            "{name}: {model}"
        );
    }
}

/// Review of 2026-10-10: a person's decision that an axis holds nothing
/// answers it, in a sort and in the report's replay alike. Before, the row
/// such a decision writes carries no value and was read as no answer, so
/// every sort asked `<axis>:missing` about the stack again, and the report
/// counted it as asked on both sides.
#[test]
fn a_decision_of_nothing_answers_the_axis_in_a_sort_and_in_the_replay() {
    let pack = nils_pack::load(&mri(), None).expect("the MRI pack loads");
    let p = Patch::parse(
        "ops",
        "patch: 1\npack: mri\nreason: a check\nevidence: this test\noperations:\n  - {op: add_words, axis: technique, value: TSE, words: [zqzq]}\n",
    )
    .unwrap();
    let settings = nils_classify::effect::Settings {
        scope: None,
        examples: 10,
        workers: 2,
    };
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("effect-nothing");
        series(
            &dir,
            "1.2.9.7.1",
            1,
            "P7",
            &mprage("SIEMENS", "sag t1 mprage"),
            false,
        );
        // nothing weights it, so its base, which matters, is missing
        series(
            &dir,
            "1.2.9.7.1",
            2,
            "P7",
            &[
                text(tags::SERIES_DESCRIPTION, VR::LO, "ax mystery"),
                text(tags::SCANNING_SEQUENCE, VR::CS, "SE"),
                text(tags::MANUFACTURER, VR::LO, "SIEMENS"),
            ],
            false,
        );
        let mut reg = prepare(&lab, &dir);
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        let mystery = stack_named(&mut reg, "SIEMENS", "mystery", 0);
        let missing = |reg: &mut Registry| {
            open_questions(reg)
                .get("base:missing")
                .copied()
                .unwrap_or(0)
        };
        assert_eq!(missing(&mut reg), 1, "{name}: the missing base is asked");
        let asked = |reg: &mut Registry| -> i64 {
            let doc = nils_classify::effect::run(reg.store(), &mri(), &pack, &p, &settings, &names)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            doc["review"]["stacks_asked"]["before"].as_i64().unwrap()
        };
        let before_decision = asked(&mut reg);
        assert!(before_decision >= 1, "{name}");

        decide_nothing(&mut reg, mystery, "base");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        assert_eq!(missing(&mut reg), 0, "{name}: the decision answered it");
        nils_classify::classify::classify(&mut reg, &pack, &Default::default(), &Cancel::new())
            .unwrap();
        assert_eq!(
            missing(&mut reg),
            0,
            "{name}: and every later sort keeps it answered"
        );
        assert_eq!(
            asked(&mut reg),
            before_decision - 1,
            "{name}: the replay asks nothing about it either"
        );
    }
}
