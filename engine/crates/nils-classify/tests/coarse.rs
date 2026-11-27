// SPDX-License-Identifier: AGPL-3.0-only

//! Two axes that say one thing at two grains agree in a stack's rows.
//!
//! A pack declares one axis the coarse form of another (`coarse_of`), and
//! the sort holds a stack's two rows to it wherever a decision stands on
//! one of them: a rule's value of the finer axis that a committed coarse
//! value does not cover is set aside, a committed value of the finer axis
//! decides the coarse one, and two decisions that do not agree are left
//! and counted. Held here over a made-up pack of two axes, four values
//! read off the image type and their size, so that nothing rests on the
//! MRI pack's body axes, and once over the MRI pack itself.

mod common;

use std::collections::BTreeMap;
use std::env;
use std::sync::{Mutex, MutexGuard};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_digest::{Cancel, digest};
use nils_registry::home::{Home, InitOptions};
use nils_registry::store::{Insert, Param};
use nils_registry::{Backend, Registry, Row, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_classify_coarse";

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
        let dir = TempDir::new("coarse-home");
        let home = Home::new(dir.path());
        home.keys(None)
            .add("k", b"nils-classify-coarse-key")
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

fn drop_schema(dsn: &str) {
    let mut store = Store::connect_postgres(dsn, SCHEMA).expect("connect");
    store
        .batch(&format!(
            "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
        ))
        .expect("drop");
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(dsn) = postgres_dsn().filter(|_| self._guard.is_some()) {
            drop_schema(&dsn);
        }
    }
}

fn labs() -> Vec<Lab> {
    let mut out = vec![Lab::new("sqlite", Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        drop_schema(&dsn);
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
        "review_member",
        "review_item",
        "stack_axis",
        "decision",
    ] {
        text = text.replace(&format!("{{{t}}}"), &reg.store().qualified(t));
    }
    reg.store().query(&text, &[]).unwrap()
}

// ---------------------------------------------------------------------------
// The made-up pack: an axis read off the image type, and its size. The
// finer axis is named `base` for one reason: a release reads an axis of
// that name, so a stack with no value on it is asked about
// (`nils_pack::matters`), and a value set aside must not become such a
// question.

const MANIFEST: &str = "\
pack: t
version: 1.0.0
contract: 1
modality: MR
parsers: [parsers.yml]
flags: [flags.yml]
axes: [axes/base.yml, axes/size.yml]
rules: [rules/base.yml, rules/size.yml]
order: [base, size]
";

const PARSERS: &str = "\
parsers:
  image_type:
    field: image_type
    case: upper
    tokenize: {split: '[\\\\/\\s]+'}
    predicates:
      is_original: {token: ORIGINAL}
      is_derived: {token: DERIVED}
";

const FLAGS: &str = "\
flags:
  is_original: image_type.is_original
  is_derived: image_type.is_derived
";

const CASES: &str = "\
cases:
  - name: an original image is original
    stack: {image_type: 'ORIGINAL\\PRIMARY'}
    flags: {is_original: true}
";

/// Four values; no size covers `d`.
const KIND_AXIS: &str = "\
axis: base
kind: single
values: {a: {}, b: {aliases: [beta]}, c: {}, d: {}}
";

const KIND_RULES: &str = "\
rule_set: base
decides: [base]
tiers: {stated: 0.9}
order: [original, derived]
rules:
  original:
    clauses: [{flag: is_original, tier: stated}]
    set: {base: a}
  derived:
    clauses: [{flag: is_derived, tier: stated}]
    set: {base: c}
";

const SIZE_AXIS: &str = "\
axis: size
kind: single
coarse_of: base
values:
  small: {covers: [a, b]}
  large: {covers: [c]}
";

/// The same axis in a pack that does not say what it is the coarse form of.
const SIZE_AXIS_PLAIN: &str = "\
axis: size
kind: single
values:
  small: {}
  large: {}
";

const SIZE_RULES: &str = "\
rule_set: size
decides: [size]
tiers: {stated: 0.8}
order: [small, large]
rules:
  small:
    clauses: [{when: {any: [{axis: base, is: a}, {axis: base, is: b}]}, cite: small, source: base}]
    set: {size: small}
  large:
    clauses: [{when: {axis: base, is: c}, cite: large, source: base}]
    set: {size: large}
";

fn made_up(size_axis: &str) -> (TempDir, nils_pack::Pack) {
    let d = TempDir::new("coarse-pack");
    for (name, text) in [
        ("pack.yml", MANIFEST),
        ("parsers.yml", PARSERS),
        ("flags.yml", FLAGS),
        ("corpus/cases.yml", CASES),
        ("axes/base.yml", KIND_AXIS),
        ("axes/size.yml", size_axis),
        ("rules/base.yml", KIND_RULES),
        ("rules/size.yml", SIZE_RULES),
    ] {
        d.file(name, text.as_bytes());
    }
    let pack = nils_pack::load(d.path(), None).expect("the made-up pack loads");
    (d, pack)
}

/// One series a case, named by its description, with the image type the
/// finer axis is read off.
fn tree(series: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new("coarse");
    for (i, (description, image_type)) in series.iter().enumerate() {
        let n = i + 1;
        let sop = format!("A.{n}.1");
        let mut e = synth::minimal_mr("A", &format!("A.{n}"), &sop);
        e.extend([
            synth::text(tags::PATIENT_ID, VR::LO, "P1"),
            synth::text(tags::SERIES_DESCRIPTION, VR::LO, description),
            synth::text(tags::IMAGE_TYPE, VR::CS, image_type),
            synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
        ]);
        dir.file(
            &format!("s{n}/1"),
            &synth::part10(&MetaFields::mr(&sop), &e, true),
        );
    }
    dir
}

fn digested(lab: &Lab, dir: &TempDir) -> Registry {
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

fn sort(reg: &mut Registry, pack: &nils_pack::Pack) -> nils_classify::report::Classified {
    nils_classify::classify::classify(reg, pack, &Default::default(), &Cancel::new()).unwrap()
}

fn stack_of(reg: &mut Registry, description: &str) -> i64 {
    rows(
        reg,
        &format!(
            "SELECT stack_id FROM {{stack_fingerprint}} WHERE text_series_description = '{description}'"
        ),
    )[0]
    .int(0)
    .unwrap()
}

/// A decision on one axis of one stack, in force as it is written; no
/// value for a decision that the axis holds nothing.
fn decide(reg: &mut Registry, stack: i64, axis: &str, value: Option<&str>, kind: &str) {
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
                Param::from("stack"),
                Param::from(stack.to_string()),
                Param::from(axis),
                value.map_or(Param::Null, Param::from),
                Param::from("a reader"),
                Param::from(kind),
                Param::from(nils_registry::time::now_iso()),
            ]],
        )
        .unwrap();
}

/// The rows of one axis of one stack: the value ("" for none), the
/// confidence and the tier. Empty where the stack holds no value on it.
fn held(reg: &mut Registry, stack: i64, axis: &str) -> Vec<(String, f64, String)> {
    rows(
        reg,
        &format!(
            "SELECT COALESCE(value, ''), confidence, tier FROM {{classification_axis}} \
             WHERE stack_id = {stack} AND axis = '{axis}' ORDER BY value"
        ),
    )
    .iter()
    .map(|r| {
        (
            r.text(0).unwrap().to_string(),
            r.double(1).unwrap(),
            r.text(2).unwrap().to_string(),
        )
    })
    .collect()
}

fn row(value: &str, confidence: f64, tier: &str) -> Vec<(String, f64, String)> {
    vec![(value.to_string(), confidence, tier.to_string())]
}

/// The evidence of one axis of one stack, as `tier rule_set/rule value
/// (source: matched)`, in a fixed order.
fn evidence(reg: &mut Registry, stack: i64, axis: &str) -> Vec<String> {
    rows(
        reg,
        &format!(
            "SELECT tier, rule_set, rule, value, source, COALESCE(matched, ''), confidence, \
             COALESCE(author_kind, '') FROM {{classification_evidence}} \
             WHERE stack_id = {stack} AND axis = '{axis}' ORDER BY tier, rule_set, rule, value"
        ),
    )
    .iter()
    .map(|r| {
        format!(
            "{} {}/{} [{}] ({}: {}) {} {}",
            r.text(0).unwrap(),
            r.text(1).unwrap(),
            r.text(2).unwrap(),
            r.text(3).unwrap(),
            r.text(4).unwrap(),
            r.text(5).unwrap(),
            r.double(6).unwrap(),
            r.text(7).unwrap(),
        )
        .trim_end()
        .to_string()
    })
    .collect()
}

/// Everything a sort wrote, as text: every axis row, every evidence row,
/// every stack's notes and the open questions by kind. Two sorts that
/// wrote the same leave the same.
fn written(reg: &mut Registry) -> Vec<String> {
    let mut out = Vec::new();
    for r in rows(
        reg,
        "SELECT stack_id, axis, COALESCE(value, ''), confidence, tier FROM {classification_axis} \
         ORDER BY stack_id, axis, value",
    ) {
        out.push(format!(
            "axis {} {} [{}] {} {}",
            r.int(0).unwrap(),
            r.text(1).unwrap(),
            r.text(2).unwrap(),
            r.double(3).unwrap(),
            r.text(4).unwrap()
        ));
    }
    for r in rows(
        reg,
        "SELECT stack_id, axis, value, tier, confidence, rule_set, rule, source, COALESCE(matched, ''), \
         COALESCE(author_kind, '') FROM {classification_evidence} \
         ORDER BY stack_id, axis, tier, rule_set, rule, value",
    ) {
        out.push(format!(
            "evidence {} {} [{}] {} {} {}/{} ({}: {}) {}",
            r.int(0).unwrap(),
            r.text(1).unwrap(),
            r.text(2).unwrap(),
            r.text(3).unwrap(),
            r.double(4).unwrap(),
            r.text(5).unwrap(),
            r.text(6).unwrap(),
            r.text(7).unwrap(),
            r.text(8).unwrap(),
            r.text(9).unwrap()
        ));
    }
    for r in rows(
        reg,
        "SELECT stack_id, COALESCE(CAST(notes AS TEXT), ''), review_items FROM {classification} ORDER BY stack_id",
    ) {
        out.push(format!(
            "class {} {} {}",
            r.int(0).unwrap(),
            r.text(1).unwrap(),
            r.int(2).unwrap()
        ));
    }
    for r in rows(
        reg,
        "SELECT kind, COUNT(*) FROM {review_item} WHERE status = 'open' GROUP BY kind ORDER BY kind",
    ) {
        out.push(format!(
            "asked {} {}",
            r.text(0).unwrap(),
            r.int(1).unwrap()
        ));
    }
    out
}

/// The stacks the open questions of a kind stand on, whether a question
/// is still about one stack or was collapsed into the group a person reads.
fn asked(reg: &mut Registry, kind: &str) -> Vec<i64> {
    let mut on: Vec<i64> = rows(
        reg,
        &format!(
            "SELECT m.stack_id FROM {{review_member}} m JOIN {{review_item}} i ON i.id = m.item_id \
             WHERE i.kind = '{kind}' AND i.status = 'open'"
        ),
    )
    .iter()
    .map(|r| r.int(0).unwrap())
    .collect();
    on.sort_unstable();
    on
}

fn notes(reg: &mut Registry, stack: i64) -> serde_json::Value {
    let r = rows(
        reg,
        &format!("SELECT CAST(notes AS TEXT) FROM {{classification}} WHERE stack_id = {stack}"),
    );
    r[0].opt_text(0)
        .unwrap()
        .map(|t| serde_json::from_str(t).unwrap())
        .unwrap_or(serde_json::Value::Null)
}

fn axes_agree(reg: &mut Registry, after: &str) {
    let standing = nils_registry::stack_axis::verify(reg.store()).unwrap();
    assert!(standing.agrees(), "after {after}: {standing:?}");
}

/// The pack's coarse forms as the registry counts them.
fn forms(pack: &nils_pack::Pack) -> Vec<nils_registry::coarse::Form> {
    nils_pack::coarse::forms(pack)
        .iter()
        .map(|f| nils_registry::coarse::Form {
            coarse: f.coarse.name.clone(),
            fine: f.fine.name.clone(),
            apart: f.apart(),
        })
        .collect()
}

/// The cases: a description, the image type the rules read, and the
/// decisions put on the stack (axis, value, who).
type Case = (
    &'static str,
    &'static str,
    &'static [(&'static str, Option<&'static str>, &'static str)],
);

const ORIGINAL: &str = "ORIGINAL\\PRIMARY";
const DERIVED: &str = "DERIVED\\PRIMARY";
const NEITHER: &str = "OTHER\\PRIMARY";

const CASES_OF: &[Case] = &[
    // the rules say a, which is small; large was decided and covers c alone
    ("set aside", ORIGINAL, &[("size", Some("large"), "model")]),
    // small was decided, and covers the rules' a
    ("covered", ORIGINAL, &[("size", Some("small"), "model")]),
    // c was decided where the rules say a: the size follows c
    ("folded", ORIGINAL, &[("base", Some("c"), "person")]),
    // the rules say nothing; b was decided, by the name it had before
    (
        "folded from silence",
        NEITHER,
        &[("base", Some("beta"), "model")],
    ),
    // d was decided, and no size covers d
    (
        "folded to nothing",
        ORIGINAL,
        &[("base", Some("d"), "person")],
    ),
    // both decided, and large does not cover a: a person's a over a
    // model's large
    (
        "person over model",
        DERIVED,
        &[
            ("base", Some("a"), "person"),
            ("size", Some("large"), "model"),
        ],
    ),
    // a person's small over a model's c, which small does not cover
    (
        "model under person",
        ORIGINAL,
        &[
            ("base", Some("c"), "model"),
            ("size", Some("small"), "person"),
        ],
    ),
    // two people, and two models, who do not agree
    (
        "apart people",
        DERIVED,
        &[
            ("base", Some("a"), "person"),
            ("size", Some("large"), "person"),
        ],
    ),
    (
        "apart models",
        DERIVED,
        &[
            ("base", Some("a"), "model"),
            ("size", Some("large"), "model"),
        ],
    ),
    // both decided, and large covers c
    (
        "agreed",
        ORIGINAL,
        &[
            ("base", Some("c"), "person"),
            ("size", Some("large"), "person"),
        ],
    ),
    // nothing decided
    ("rules alone", DERIVED, &[]),
    ("silent", NEITHER, &[]),
    // a decision that the axis holds nothing moves nothing on the other
    ("base decided none", ORIGINAL, &[("base", None, "person")]),
    ("size decided none", ORIGINAL, &[("size", None, "person")]),
];

/// Digest the cases, sort them once by the rules alone, put the decisions
/// in and sort again. Answers the registry, each case's stack and what
/// the first sort wrote.
fn sorted(
    lab: &Lab,
    pack: &nils_pack::Pack,
) -> (
    Registry,
    BTreeMap<&'static str, i64>,
    Vec<String>,
    nils_classify::report::Classified,
) {
    let series: Vec<(&str, &str)> = CASES_OF.iter().map(|(d, t, _)| (*d, *t)).collect();
    let dir = tree(&series);
    let mut reg = digested(lab, &dir);
    sort(&mut reg, pack);
    let before = written(&mut reg);
    let mut stacks = BTreeMap::new();
    for (description, _, decisions) in CASES_OF {
        let stack = stack_of(&mut reg, description);
        stacks.insert(*description, stack);
        for (axis, value, who) in *decisions {
            decide(&mut reg, stack, axis, *value, who);
        }
    }
    let report = sort(&mut reg, pack);
    (reg, stacks, before, report)
}

/// The rules' rows of the made-up pack: an original is a and small, a
/// derived image c and large.
fn rules_a() -> Vec<(String, f64, String)> {
    row("a", 0.9, "stated")
}

#[test]
fn a_committed_answer_holds_the_other_grain_to_it() {
    let (_pack_dir, pack) = made_up(SIZE_AXIS);
    assert!(
        nils_pack::matters::missing_asked(&pack).contains(&"base".to_string()),
        "a missing value of the finer axis is a question in this pack, so one set aside could become one"
    );
    for lab in labs() {
        let name = lab.name;
        let (mut reg, stacks, _, report) = sorted(&lab, &pack);
        let s = |case: &str| stacks[case];

        // 1. a committed coarse value that does not cover the rules' value
        // of the finer axis: the finer axis holds no value, and the
        // evidence keeps the rules' answer and says what set it aside
        assert_eq!(held(&mut reg, s("set aside"), "base"), [], "{name}");
        assert_eq!(
            held(&mut reg, s("set aside"), "size"),
            row("large", 0.8, "decision"),
            "{name}"
        );
        assert_eq!(
            evidence(&mut reg, s("set aside"), "base"),
            [
                "set_aside coarse_of/size [] (size: large was decided and does not cover a) 1 model",
                "stated base/original [a] (flags: is_original) 0.9",
            ],
            "{name}: the mark names who decided the value that set it aside"
        );
        let n = notes(&mut reg, s("set aside"));
        assert_eq!(
            n["set_aside"],
            serde_json::json!([{
                "axis": "base", "value": "a", "tier": "stated", "confidence": 0.9,
                "by": {"axis": "size", "value": "large", "who": "a reader", "kind": "model"},
            }]),
            "{name}: {n}"
        );
        // and one it covers stays
        assert_eq!(held(&mut reg, s("covered"), "base"), rules_a(), "{name}");
        assert_eq!(
            held(&mut reg, s("covered"), "size"),
            row("small", 0.8, "decision"),
            "{name}"
        );
        assert!(
            notes(&mut reg, s("covered"))["set_aside"].is_null(),
            "{name}"
        );

        // 2. a committed value of the finer axis decides the coarse one
        assert_eq!(
            held(&mut reg, s("folded"), "base"),
            row("c", 0.9, "decision"),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, s("folded"), "size"),
            row("large", 1.0, "folded"),
            "{name}: not the small the rules' a folds to"
        );
        assert_eq!(
            evidence(&mut reg, s("folded"), "size"),
            [
                "folded coarse_of/base [large] (base: c was decided) 1 person",
                "stated size/small [small] (base: small) 0.8",
            ],
            "{name}: the rules' own answer is kept beside it, and the fold names who decided"
        );
        // where the rules said nothing of either, and by another name
        assert_eq!(
            held(&mut reg, s("folded from silence"), "base"),
            row("b", 1.0, "decision"),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, s("folded from silence"), "size"),
            row("small", 1.0, "folded"),
            "{name}"
        );
        // and a decided value no coarse value covers leaves none
        assert_eq!(
            held(&mut reg, s("folded to nothing"), "base"),
            row("d", 0.9, "decision"),
            "{name}"
        );
        assert_eq!(held(&mut reg, s("folded to nothing"), "size"), [], "{name}");
        assert_eq!(
            evidence(&mut reg, s("folded to nothing"), "size"),
            [
                "folded coarse_of/base [] (base: d was decided, and no value covers it) 1 person",
                "stated size/small [small] (base: small) 0.8",
            ],
            "{name}"
        );

        // 3. both decided, and they do not agree. A person's answer holds
        // over a model's across the two axes: the person's a decides the
        // size, and the model's large gives way to the small that covers a
        assert_eq!(
            held(&mut reg, s("person over model"), "base"),
            row("a", 0.9, "decision"),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, s("person over model"), "size"),
            row("small", 1.0, "folded"),
            "{name}: not the model's large beside the person's a"
        );
        assert_eq!(
            evidence(&mut reg, s("person over model"), "size"),
            [
                "folded coarse_of/base [small] (base: a was decided, over large, which a reader (model) decided) 1 person",
                "stated size/large [large] (base: large) 0.8",
            ],
            "{name}: the evidence names both, and the model's answer has no row of its own"
        );
        // and a person's small sets the model's c aside as a rule's value
        assert_eq!(
            held(&mut reg, s("model under person"), "base"),
            [],
            "{name}"
        );
        assert_eq!(
            held(&mut reg, s("model under person"), "size"),
            row("small", 0.8, "decision"),
            "{name}"
        );
        assert_eq!(
            evidence(&mut reg, s("model under person"), "base"),
            [
                "set_aside coarse_of/size [] (size: small was decided and does not cover c, which a reader (model) decided) 1 person",
                "stated base/original [a] (flags: is_original) 0.9",
            ],
            "{name}"
        );
        assert_eq!(
            notes(&mut reg, s("model under person"))["set_aside"],
            serde_json::json!([{
                "axis": "base", "value": "c", "tier": "decision", "confidence": 1.0,
                "by": {"axis": "size", "value": "small", "who": "a reader", "kind": "person"},
                "decided_by": {"who": "a reader", "kind": "model"},
            }]),
            "{name}"
        );
        // two decisions of one standing stay as decided, and are counted
        for case in ["apart people", "apart models"] {
            assert_eq!(
                held(&mut reg, s(case), "base"),
                row("a", 0.9, "decision"),
                "{name}: {case}"
            );
            assert_eq!(
                held(&mut reg, s(case), "size"),
                row("large", 0.8, "decision"),
                "{name}: {case}"
            );
        }
        assert_eq!(
            held(&mut reg, s("agreed"), "base"),
            row("c", 0.9, "decision"),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, s("agreed"), "size"),
            row("large", 0.8, "decision"),
            "{name}"
        );
        assert_eq!(
            nils_registry::coarse::count_apart(reg.store(), &forms(&pack)).unwrap(),
            BTreeMap::from([("size".to_string(), 2)]),
            "{name}: the two stacks whose decisions of one standing do not agree"
        );

        // a decision that an axis holds nothing moves nothing on the other
        assert_eq!(
            held(&mut reg, s("base decided none"), "base"),
            row("", 0.9, "decision"),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, s("base decided none"), "size"),
            row("small", 0.8, "stated"),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, s("size decided none"), "base"),
            rules_a(),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, s("size decided none"), "size"),
            row("", 0.8, "decision"),
            "{name}"
        );

        // what the sort says of it
        assert_eq!(
            report.coarse,
            BTreeMap::from([
                ("base:set_aside".to_string(), 2),
                ("size:apart".to_string(), 2),
                ("size:folded".to_string(), 4),
            ]),
            "{name}"
        );
        assert_eq!(report.by_tier.get("size:folded"), Some(&3), "{name}");

        // no question comes of any of it: what is asked is a decision held
        // against the rules, as before, and the stack no rule answered
        assert_eq!(
            asked(&mut reg, "base:missing"),
            [s("silent")],
            "{name}: the stack no rule answered, and never the value set aside"
        );
        let kinds: Vec<String> = rows(
            &mut reg,
            "SELECT DISTINCT kind FROM {review_item} WHERE status = 'open' ORDER BY kind",
        )
        .iter()
        .map(|r| r.text(0).unwrap().to_string())
        .collect();
        assert_eq!(
            kinds,
            ["base:decision", "base:missing", "size:decision"],
            "{name}"
        );
        axes_agree(&mut reg, &format!("{name}: a sort under decisions"));
    }
}

#[test]
fn a_stack_with_no_decision_sorts_as_before_and_a_second_sort_changes_nothing() {
    let (_pack_dir, pack) = made_up(SIZE_AXIS);
    for lab in labs() {
        let name = lab.name;
        let (mut reg, stacks, before, _) = sorted(&lab, &pack);
        let after = written(&mut reg);
        // 4. the stacks nothing was decided on hold what the rules alone
        // gave them, row for row
        for case in ["rules alone", "silent"] {
            let of = |all: &[String]| -> Vec<String> {
                let stack = stacks[case];
                all.iter()
                    .filter(|l| {
                        let mut words = l.split(' ');
                        words.next() != Some("asked")
                            && words.next() == Some(stack.to_string().as_str())
                    })
                    .cloned()
                    .collect()
            };
            assert!(!of(&before).is_empty(), "{name}: {case} was sorted");
            assert_eq!(of(&before), of(&after), "{name}: {case}");
        }
        assert_eq!(
            held(&mut reg, stacks["rules alone"], "size"),
            row("large", 0.8, "stated"),
            "{name}"
        );
        // a second sort over the same decisions writes the same
        let report = sort(&mut reg, &pack);
        assert_eq!(written(&mut reg), after, "{name}: the second sort");
        assert_eq!(report.coarse.get("size:folded"), Some(&4), "{name}");
        let third = sort(&mut reg, &pack);
        assert_eq!(written(&mut reg), after, "{name}: and the third");
        assert_eq!(third.coarse, report.coarse, "{name}");
        axes_agree(&mut reg, &format!("{name}: a sort run again"));
    }
}

#[test]
fn a_pack_that_declares_no_coarse_form_sorts_as_it_did() {
    let (_pack_dir, pack) = made_up(SIZE_AXIS_PLAIN);
    assert!(nils_pack::coarse::forms(&pack).is_empty());
    for lab in labs() {
        let name = lab.name;
        let (mut reg, stacks, _, report) = sorted(&lab, &pack);
        let s = |case: &str| stacks[case];
        // the rules' value stays beside a decided size that says otherwise
        assert_eq!(held(&mut reg, s("set aside"), "base"), rules_a(), "{name}");
        assert_eq!(
            held(&mut reg, s("set aside"), "size"),
            row("large", 0.8, "decision"),
            "{name}"
        );
        // and the size stays at what the rules' value folds to
        assert_eq!(
            held(&mut reg, s("folded"), "size"),
            row("small", 0.8, "stated"),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, s("folded from silence"), "size"),
            [],
            "{name}"
        );
        assert_eq!(
            held(&mut reg, s("folded to nothing"), "size"),
            row("small", 0.8, "stated"),
            "{name}"
        );
        assert!(report.coarse.is_empty(), "{name}: {:?}", report.coarse);
        assert_eq!(
            rows(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_evidence} WHERE rule_set = 'coarse_of'"
            )[0]
            .int(0)
            .unwrap(),
            0,
            "{name}"
        );
        assert!(
            nils_registry::coarse::count_apart(reg.store(), &forms(&pack))
                .unwrap()
                .is_empty(),
            "{name}"
        );
    }
}

/// The MRI pack's two body axes, which the rule was written for: a body
/// part read from a word in the header never stands against a committed
/// region, a committed body part decides the region, and the sort asks
/// nothing about either, since both are an image model's to answer.
#[test]
fn the_mri_pack_s_body_part_and_region_agree_under_a_decision() {
    let pack = nils_pack::load(&common::shipped_mri(), None).expect("the MRI pack loads");
    assert_eq!(nils_pack::coarse::forms(&pack).len(), 1);
    let cases = [
        "sag t1 spine one",
        "sag t1 spine two",
        "sag t1 spine three",
        "sag t1 spine four",
    ];
    for lab in labs() {
        let name = lab.name;
        let dir = TempDir::new("coarse-mri");
        for (i, description) in cases.iter().enumerate() {
            let n = i + 1;
            let sop = format!("A.{n}.1");
            let mut e = synth::minimal_mr("A", &format!("A.{n}"), &sop);
            e.extend([
                synth::text(tags::PATIENT_ID, VR::LO, "P1"),
                synth::text(tags::SERIES_DESCRIPTION, VR::LO, description),
                synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
                synth::text(tags::SCANNING_SEQUENCE, VR::CS, "SE"),
                synth::text(tags::REPETITION_TIME, VR::DS, "600"),
                synth::text(tags::ECHO_TIME, VR::DS, "12"),
                synth::text(tags::BODY_PART_EXAMINED, VR::CS, "SPINE"),
            ]);
            dir.file(
                &format!("s{n}/1"),
                &synth::part10(&MetaFields::mr(&sop), &e, true),
            );
        }
        let mut reg = digested(&lab, &dir);
        sort(&mut reg, &pack);
        let stacks: Vec<i64> = cases
            .iter()
            .map(|description| stack_of(&mut reg, description))
            .collect();
        for s in &stacks {
            assert_eq!(
                held(&mut reg, *s, "body_part"),
                row("spine", 0.65, "keywords"),
                "{name}: the rules read a spine off the header's words"
            );
            assert_eq!(
                held(&mut reg, *s, "body_region"),
                row("spine", 0.65, "stated"),
                "{name}"
            );
        }
        let before = written(&mut reg);
        // a committed chest, a committed spine, a committed brain as the
        // body part; the fourth is left to the rules
        decide(&mut reg, stacks[0], "body_region", Some("chest"), "model");
        decide(&mut reg, stacks[1], "body_region", Some("spine"), "model");
        decide(&mut reg, stacks[2], "body_part", Some("brain"), "model");
        let report = sort(&mut reg, &pack);

        assert_eq!(held(&mut reg, stacks[0], "body_part"), [], "{name}");
        assert_eq!(
            held(&mut reg, stacks[0], "body_region"),
            row("chest", 0.65, "decision"),
            "{name}"
        );
        let kept = evidence(&mut reg, stacks[0], "body_part");
        assert!(
            kept.iter()
                .any(|e| e.starts_with("keywords body_part/") && e.contains("[spine]")),
            "{name}: the rules' answer stays in the evidence: {kept:?}"
        );
        assert!(
            kept.contains(
                &"set_aside coarse_of/body_region [] (body_region: chest was decided and does not cover spine) 1 model"
                    .to_string()
            ),
            "{name}: {kept:?}"
        );
        assert_eq!(
            notes(&mut reg, stacks[0])["set_aside"][0]["value"],
            "spine",
            "{name}"
        );
        // the read model holds no body part for it either
        let axes = rows(
            &mut reg,
            &format!(
                "SELECT COUNT(*) FROM {{stack_axis}} WHERE stack_id = {} \
                 AND CAST(axes AS TEXT) LIKE '%body_part%'",
                stacks[0]
            ),
        )[0]
        .int(0)
        .unwrap();
        assert_eq!(axes, 0, "{name}");

        assert_eq!(
            held(&mut reg, stacks[1], "body_part"),
            row("spine", 0.65, "keywords"),
            "{name}: a spine the committed spine covers stays"
        );
        assert_eq!(
            held(&mut reg, stacks[2], "body_part"),
            row("brain", 0.65, "decision"),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, stacks[2], "body_region"),
            row("head", 1.0, "folded"),
            "{name}: the head of the committed brain, not the spine of the header's word"
        );
        assert_eq!(report.coarse.get("body_part:set_aside"), Some(&1), "{name}");
        assert_eq!(report.coarse.get("body_region:folded"), Some(&1), "{name}");
        assert_eq!(report.coarse.get("body_region:apart"), None, "{name}");

        // the stack nothing was decided on is as it was
        let of = |all: &[String]| -> Vec<String> {
            all.iter()
                .filter(|l| {
                    let mut words = l.split(' ');
                    words.next() != Some("asked")
                        && words.next() == Some(stacks[3].to_string().as_str())
                })
                .cloned()
                .collect()
        };
        let after = written(&mut reg);
        assert_eq!(of(&before), of(&after), "{name}");
        // and nothing is asked about either axis
        for kind in [
            "body_part:missing",
            "body_part:decision",
            "body_part:low_confidence",
            "body_region:missing",
            "body_region:decision",
            "body_region:low_confidence",
        ] {
            assert_eq!(asked(&mut reg, kind), Vec::<i64>::new(), "{name}: {kind}");
        }
        axes_agree(&mut reg, &format!("{name}: the body axes under decisions"));
        sort(&mut reg, &pack);
        assert_eq!(written(&mut reg), after, "{name}: a second sort");
    }
}

// ---------------------------------------------------------------------------
// What the table promises beyond one committed decision on one stack.

/// A decision at any scope, in force as written, or staged and not
/// committed. Answers its id.
fn decide_at(
    reg: &mut Registry,
    scope: &str,
    reference: &str,
    axis: &str,
    value: &str,
    kind: &str,
    staged: bool,
) -> i64 {
    let now = nils_registry::time::now_iso();
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
                    "staged_at",
                ],
            )
            .returning(&["id"]),
            &[vec![
                Param::from(scope),
                Param::from(reference),
                Param::from(axis),
                Param::from(value),
                Param::from("a reader"),
                Param::from(kind),
                Param::from(now.as_str()),
                if staged {
                    Param::from(now.as_str())
                } else {
                    Param::Null
                },
            ]],
        )
        .unwrap()[0]
        .int(0)
        .unwrap()
}

fn withdraw(reg: &mut Registry, decision: i64) {
    let sql = format!(
        "UPDATE {} SET withdrawn_at = '2026-01-02T00:00:00Z' WHERE id = {decision}",
        reg.store().qualified("decision")
    );
    reg.store().execute(&sql, &[]).unwrap();
}

fn one_int(reg: &mut Registry, sql: &str) -> i64 {
    rows(reg, sql)[0].int(0).unwrap()
}

/// A withdrawn decision gives the rows back: the value set aside is the
/// rules' value again and the folded axis holds what the rules fold, at
/// the next sort, with no mark left.
#[test]
fn a_withdrawn_decision_gives_back_what_it_set_aside_and_folded() {
    let (_pack_dir, pack) = made_up(SIZE_AXIS);
    for lab in labs() {
        let name = lab.name;
        let dir = tree(&[("one", ORIGINAL), ("two", ORIGINAL)]);
        let mut reg = digested(&lab, &dir);
        sort(&mut reg, &pack);
        let before = written(&mut reg);
        let (one, two) = (stack_of(&mut reg, "one"), stack_of(&mut reg, "two"));
        let aside = decide_at(
            &mut reg,
            "stack",
            &one.to_string(),
            "size",
            "large",
            "person",
            false,
        );
        let fold = decide_at(
            &mut reg,
            "stack",
            &two.to_string(),
            "base",
            "c",
            "person",
            false,
        );
        sort(&mut reg, &pack);
        assert_eq!(held(&mut reg, one, "base"), [], "{name}");
        assert_eq!(
            held(&mut reg, two, "size"),
            row("large", 1.0, "folded"),
            "{name}"
        );

        withdraw(&mut reg, aside);
        withdraw(&mut reg, fold);
        let report = sort(&mut reg, &pack);
        assert_eq!(held(&mut reg, one, "base"), rules_a(), "{name}");
        assert_eq!(
            held(&mut reg, one, "size"),
            row("small", 0.8, "stated"),
            "{name}"
        );
        assert_eq!(held(&mut reg, two, "base"), rules_a(), "{name}");
        assert_eq!(
            held(&mut reg, two, "size"),
            row("small", 0.8, "stated"),
            "{name}"
        );
        assert!(report.coarse.is_empty(), "{name}: {:?}", report.coarse);
        assert!(notes(&mut reg, one)["set_aside"].is_null(), "{name}");
        assert_eq!(
            one_int(
                &mut reg,
                "SELECT COUNT(*) FROM {classification_evidence} WHERE rule_set = 'coarse_of'"
            ),
            0,
            "{name}"
        );
        // the open questions aside, the registry reads as before any decision
        let rows_of = |all: &[String]| -> Vec<String> {
            all.iter()
                .filter(|l| !l.starts_with("asked") && !l.starts_with("class"))
                .cloned()
                .collect()
        };
        assert_eq!(rows_of(&written(&mut reg)), rows_of(&before), "{name}");
    }
}

/// A decision reaches the stacks of its scope, whatever the scope: a
/// series, a subject, a machine. And a staged answer nobody committed is
/// no decision, and moves nothing.
#[test]
fn a_decision_at_a_wider_scope_holds_and_a_staged_one_does_not() {
    let (_pack_dir, pack) = made_up(SIZE_AXIS);
    for lab in labs() {
        let name = lab.name;
        let dir = tree(&[("one", ORIGINAL), ("two", ORIGINAL)]);
        let mut reg = digested(&lab, &dir);
        sort(&mut reg, &pack);
        let before = written(&mut reg);
        let (one, two) = (stack_of(&mut reg, "one"), stack_of(&mut reg, "two"));

        // staged and not committed: nothing moves
        let staged = decide_at(
            &mut reg,
            "stack",
            &one.to_string(),
            "size",
            "large",
            "person",
            true,
        );
        let report = sort(&mut reg, &pack);
        assert_eq!(
            written(&mut reg),
            before,
            "{name}: a staged answer moved a row"
        );
        assert!(report.coarse.is_empty(), "{name}: {:?}", report.coarse);
        withdraw(&mut reg, staged);

        // the series of one stack: that stack alone
        let sql = format!(
            "SELECT series_id FROM {} WHERE id = {one}",
            reg.store().qualified("stack")
        );
        let series = one_int(&mut reg, &sql);
        let d = decide_at(
            &mut reg,
            "series",
            &series.to_string(),
            "size",
            "large",
            "person",
            false,
        );
        sort(&mut reg, &pack);
        assert_eq!(held(&mut reg, one, "base"), [], "{name}: series");
        assert_eq!(held(&mut reg, two, "base"), rules_a(), "{name}: series");
        withdraw(&mut reg, d);

        // the subject: both stacks
        let sql = format!(
            "SELECT subject_id FROM {} WHERE id = {series}",
            reg.store().qualified("series")
        );
        let subject = one_int(&mut reg, &sql);
        let d = decide_at(
            &mut reg,
            "subject",
            &subject.to_string(),
            "base",
            "c",
            "person",
            false,
        );
        let report = sort(&mut reg, &pack);
        for s in [one, two] {
            assert_eq!(
                held(&mut reg, s, "base"),
                row("c", 0.9, "decision"),
                "{name}: subject"
            );
            assert_eq!(
                held(&mut reg, s, "size"),
                row("large", 1.0, "folded"),
                "{name}: subject"
            );
        }
        assert_eq!(report.coarse.get("size:folded"), Some(&2), "{name}");
        withdraw(&mut reg, d);

        // the machine: both stacks
        let d = decide_at(
            &mut reg,
            "origin",
            "manufacturer=synthetic",
            "size",
            "large",
            "person",
            false,
        );
        let report = sort(&mut reg, &pack);
        for s in [one, two] {
            assert_eq!(held(&mut reg, s, "base"), [], "{name}: origin");
            assert_eq!(
                held(&mut reg, s, "size"),
                row("large", 0.8, "decision"),
                "{name}: origin"
            );
        }
        assert_eq!(report.coarse.get("base:set_aside"), Some(&2), "{name}");
        withdraw(&mut reg, d);
        sort(&mut reg, &pack);
        let rows_of = |all: &[String]| -> Vec<String> {
            all.iter()
                .filter(|l| !l.starts_with("asked") && !l.starts_with("class"))
                .cloned()
                .collect()
        };
        assert_eq!(rows_of(&written(&mut reg)), rows_of(&before), "{name}");
    }
}

/// A pack of the made-up kind with its files changed or added.
fn made_up_with(
    files: &[(&str, &str)],
    rename: &dyn Fn(&str) -> String,
) -> (TempDir, nils_pack::Pack) {
    let d = TempDir::new("coarse-pack");
    let mut all: BTreeMap<&str, &str> = BTreeMap::from([
        ("pack.yml", MANIFEST),
        ("parsers.yml", PARSERS),
        ("flags.yml", FLAGS),
        ("corpus/cases.yml", CASES),
        ("axes/base.yml", KIND_AXIS),
        ("axes/size.yml", SIZE_AXIS),
        ("rules/base.yml", KIND_RULES),
        ("rules/size.yml", SIZE_RULES),
    ]);
    for (name, text) in files {
        all.insert(name, text);
    }
    for (name, text) in all {
        d.file(&rename(name), rename(text).as_bytes());
    }
    let pack = nils_pack::load(d.path(), None).expect("the made-up pack loads");
    (d, pack)
}

const BULK_AXIS: &str = "\
axis: bulk
kind: single
coarse_of: size
values:
  light: {covers: [small]}
  heavy: {covers: [large]}
";

const BULK_RULES: &str = "\
rule_set: bulk
decides: [bulk]
tiers: {stated: 0.7}
order: [light, heavy]
rules:
  light:
    clauses: [{when: {axis: size, is: small}, cite: light, source: size}]
    set: {bulk: light}
  heavy:
    clauses: [{when: {axis: size, is: large}, cite: heavy, source: size}]
    set: {bulk: heavy}
";

/// Three grains, base under size under bulk. The fold runs up: a decided
/// base decides the size, and the size folded from it decides the bulk.
/// Setting aside does not run down: a decided bulk sets the rules' size
/// aside and leaves the base as the rules decided it. And a decided bulk
/// is held against a size folded from a decided base as against a
/// decision of that base's standing.
#[test]
fn three_grains_fold_up_and_are_held_by_standing() {
    let manifest = MANIFEST
        .replace("axes/size.yml]", "axes/size.yml, axes/bulk.yml]")
        .replace("rules/size.yml]", "rules/size.yml, rules/bulk.yml]")
        .replace("order: [base, size]", "order: [base, size, bulk]");
    let (_pack_dir, pack) = made_up_with(
        &[
            ("pack.yml", &manifest),
            ("axes/bulk.yml", BULK_AXIS),
            ("rules/bulk.yml", BULK_RULES),
        ],
        &|t| t.to_string(),
    );
    assert_eq!(nils_pack::coarse::forms(&pack).len(), 2);
    for lab in labs() {
        let name = lab.name;
        let dir = tree(&[
            ("up", ORIGINAL),
            ("down", ORIGINAL),
            ("over", ORIGINAL),
            ("apart", ORIGINAL),
        ]);
        let mut reg = digested(&lab, &dir);
        sort(&mut reg, &pack);
        let s = |reg: &mut Registry, d: &str| stack_of(reg, d);
        let (up, down, over, apart) = (
            s(&mut reg, "up"),
            s(&mut reg, "down"),
            s(&mut reg, "over"),
            s(&mut reg, "apart"),
        );
        assert_eq!(
            held(&mut reg, up, "bulk"),
            row("light", 0.7, "stated"),
            "{name}"
        );
        // the rules say a, small, light
        decide(&mut reg, up, "base", Some("c"), "person");
        decide(&mut reg, down, "bulk", Some("heavy"), "person");
        decide(&mut reg, over, "base", Some("a"), "model");
        decide(&mut reg, over, "bulk", Some("heavy"), "person");
        decide(&mut reg, apart, "base", Some("a"), "person");
        decide(&mut reg, apart, "bulk", Some("heavy"), "person");
        let report = sort(&mut reg, &pack);

        // up: c, so large, so heavy
        assert_eq!(
            held(&mut reg, up, "size"),
            row("large", 1.0, "folded"),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, up, "bulk"),
            row("heavy", 1.0, "folded"),
            "{name}"
        );
        assert_eq!(
            evidence(&mut reg, up, "bulk")[0],
            "folded coarse_of/size [heavy] (size: large was decided) 1 person",
            "{name}"
        );
        // down: heavy does not cover the rules' small, and the base stays
        assert_eq!(
            held(&mut reg, down, "bulk"),
            row("heavy", 0.7, "decision"),
            "{name}"
        );
        assert_eq!(held(&mut reg, down, "size"), [], "{name}");
        assert_eq!(
            held(&mut reg, down, "base"),
            rules_a(),
            "{name}: not run down"
        );
        // over: a person's heavy over the small folded from a model's a
        assert_eq!(
            held(&mut reg, over, "base"),
            row("a", 0.9, "decision"),
            "{name}"
        );
        assert_eq!(held(&mut reg, over, "size"), [], "{name}");
        assert_eq!(
            held(&mut reg, over, "bulk"),
            row("heavy", 0.7, "decision"),
            "{name}"
        );
        assert_eq!(
            evidence(&mut reg, over, "size")[0],
            "set_aside coarse_of/bulk [] (bulk: heavy was decided and does not cover small, which a reader (model) decided) 1 person",
            "{name}"
        );
        // apart: two people; the size folded from one, the bulk the other's
        assert_eq!(
            held(&mut reg, apart, "size"),
            row("small", 1.0, "folded"),
            "{name}"
        );
        assert_eq!(
            held(&mut reg, apart, "bulk"),
            row("heavy", 0.7, "decision"),
            "{name}"
        );
        assert_eq!(report.coarse.get("bulk:apart"), Some(&1), "{name}");
        assert_eq!(
            nils_registry::coarse::count_apart(reg.store(), &forms(&pack)).unwrap(),
            BTreeMap::from([("bulk".to_string(), 1), ("size".to_string(), 0)]),
            "{name}: a decided bulk beside a size folded from a decided base"
        );
        let after = written(&mut reg);
        sort(&mut reg, &pack);
        assert_eq!(written(&mut reg), after, "{name}: a second sort");
        axes_agree(&mut reg, &format!("{name}: three grains"));
    }
}

const SHADE_AXIS: &str = "\
axis: shade
kind: single
coarse_of: base
values:
  dark: {covers: [a, c]}
  pale: {covers: [b]}
";

const SHADE_RULES: &str = "\
rule_set: shade
decides: [shade]
tiers: {stated: 0.7}
order: [dark, pale]
rules:
  dark:
    clauses: [{when: {any: [{axis: base, is: a}, {axis: base, is: c}]}, cite: dark, source: base}]
    set: {shade: dark}
  pale:
    clauses: [{when: {axis: base, is: b}, cite: pale, source: base}]
    set: {shade: pale}
";

/// Two coarse axes over one finer axis. A decision on the first sets the
/// rules' finer value aside, and the second does not keep what the rules
/// folded from that value: it is set aside with it. A second coarse axis
/// that has an answer of its own keeps it.
#[test]
fn a_second_coarse_axis_does_not_keep_what_was_folded_from_a_value_set_aside() {
    let manifest = MANIFEST
        .replace("axes/size.yml]", "axes/size.yml, axes/shade.yml]")
        .replace("rules/size.yml]", "rules/size.yml, rules/shade.yml]")
        .replace("order: [base, size]", "order: [base, size, shade]");
    let (_pack_dir, pack) = made_up_with(
        &[
            ("pack.yml", &manifest),
            ("axes/shade.yml", SHADE_AXIS),
            ("rules/shade.yml", SHADE_RULES),
        ],
        &|t| t.to_string(),
    );
    assert_eq!(nils_pack::coarse::forms(&pack).len(), 2);
    for lab in labs() {
        let name = lab.name;
        let dir = tree(&[
            ("first", ORIGINAL),
            ("second", ORIGINAL),
            ("both", ORIGINAL),
        ]);
        let mut reg = digested(&lab, &dir);
        sort(&mut reg, &pack);
        let (first, second, both) = (
            stack_of(&mut reg, "first"),
            stack_of(&mut reg, "second"),
            stack_of(&mut reg, "both"),
        );
        assert_eq!(
            held(&mut reg, first, "shade"),
            row("dark", 0.7, "stated"),
            "{name}"
        );
        // the rules say a, small, dark
        decide(&mut reg, first, "size", Some("large"), "person");
        decide(&mut reg, second, "shade", Some("pale"), "person");
        decide(&mut reg, both, "size", Some("large"), "person");
        decide(&mut reg, both, "shade", Some("dark"), "person");
        let report = sort(&mut reg, &pack);

        assert_eq!(held(&mut reg, first, "base"), [], "{name}");
        assert_eq!(
            held(&mut reg, first, "shade"),
            [],
            "{name}: folded from the a set aside"
        );
        assert_eq!(
            evidence(&mut reg, first, "shade"),
            [
                "set_aside coarse_of/size [] (size: large was decided and set aside the base that dark was folded from) 1 person",
                "stated shade/dark [dark] (base: dark) 0.7",
            ],
            "{name}"
        );
        // and the other way round, whichever form is read first
        assert_eq!(held(&mut reg, second, "base"), [], "{name}");
        assert_eq!(held(&mut reg, second, "size"), [], "{name}");
        assert_eq!(
            held(&mut reg, second, "shade"),
            row("pale", 0.7, "decision"),
            "{name}"
        );
        // an answer of its own stays
        assert_eq!(held(&mut reg, both, "base"), [], "{name}");
        assert_eq!(
            held(&mut reg, both, "shade"),
            row("dark", 0.7, "decision"),
            "{name}"
        );
        assert_eq!(report.coarse.get("base:set_aside"), Some(&3), "{name}");
        assert_eq!(report.coarse.get("shade:set_aside"), Some(&1), "{name}");
        assert_eq!(report.coarse.get("size:set_aside"), Some(&1), "{name}");
        let after = written(&mut reg);
        sort(&mut reg, &pack);
        assert_eq!(written(&mut reg), after, "{name}: a second sort");
        axes_agree(&mut reg, &format!("{name}: two coarse axes"));
    }
}

/// A decided value that no coarse value covers, on a stack the rules said
/// nothing of: the coarse axis holds no value because a decision says so,
/// and it is not asked about as an axis nobody answered. The pack is the
/// made-up one with its names changed, so that the coarse axis is the one
/// a release reads and a missing value of it is a question.
#[test]
fn a_decided_value_nothing_covers_raises_no_missing_question_on_the_coarse_axis() {
    let rename = |t: &str| {
        t.replace("size", "@")
            .replace("base", "kind")
            .replace('@', "base")
    };
    let (_pack_dir, pack) = made_up_with(&[], &rename);
    assert!(
        nils_pack::matters::missing_asked(&pack).contains(&"base".to_string()),
        "a missing value of the coarse axis is a question in this pack"
    );
    for lab in labs() {
        let name = lab.name;
        let dir = tree(&[
            ("decided", NEITHER),
            ("silent", NEITHER),
            ("set aside", ORIGINAL),
        ]);
        let mut reg = digested(&lab, &dir);
        sort(&mut reg, &pack);
        let (decided, silent, aside) = (
            stack_of(&mut reg, "decided"),
            stack_of(&mut reg, "silent"),
            stack_of(&mut reg, "set aside"),
        );
        assert_eq!(
            asked(&mut reg, "base:missing"),
            {
                let mut both = vec![decided, silent];
                both.sort_unstable();
                both
            },
            "{name}: before any decision the rules said nothing of either"
        );
        decide(&mut reg, decided, "kind", Some("d"), "person");
        // and a decision on the finer axis that the coarse decision of a
        // person overrules, where the rules were silent on neither
        decide(&mut reg, aside, "kind", Some("c"), "model");
        decide(&mut reg, aside, "base", Some("small"), "person");
        sort(&mut reg, &pack);
        assert_eq!(
            held(&mut reg, decided, "kind"),
            row("d", 1.0, "decision"),
            "{name}"
        );
        assert_eq!(held(&mut reg, decided, "base"), [], "{name}");
        assert_eq!(
            evidence(&mut reg, decided, "base"),
            ["folded coarse_of/kind [] (kind: d was decided, and no value covers it) 1 person"],
            "{name}"
        );
        assert_eq!(held(&mut reg, aside, "kind"), [], "{name}");
        assert_eq!(
            asked(&mut reg, "base:missing"),
            [silent],
            "{name}: the stack nobody answered, and not the one a decision answered"
        );
        assert_eq!(asked(&mut reg, "kind:missing"), Vec::<i64>::new(), "{name}");
    }
}
