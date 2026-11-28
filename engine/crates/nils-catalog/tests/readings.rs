// SPDX-License-Identifier: AGPL-3.0-only

//! What a question authored as text names a value by, read and said.
//!
//! The draft door reads two things from the pack's vocabulary and says
//! each in its repairs: another name of a value as the word the pack
//! stores (`FLASH` as `SPGRE`), and a fine value the pack reads as its
//! wider one where it is written alone (`body_part = brain` as
//! `body_region = head`). The document stored is the one read. The fine
//! value stays expressible (`exact: true`, beside its own wider value,
//! negated, among alternatives of its axis, in a list of several), and a
//! document that arrives structured is taken as written.
//!
//! The counts are held against values planted on each backend's synthetic
//! registry: by a stack's place `i` among those a stack set sees,
//! `body_part` is `brain` on one in twenty, `spine` on one, nothing on
//! one and `brain-neck` on the rest, each with the region the rules fold
//! it into, and `technique` is the spoiled gradient echo on one in three,
//! half of them in the word an older pack stored.

mod common;

use common::{Lab, quasi, replant};
use nils_ask::affordance::{self, Applied, Drafted, Setting};
use nils_ask::exec::Bounds;
use nils_ask::moves::{Kind, MoveCall};
use nils_ask::validate::{Names, Scope};
use nils_ask::{document, parse, parse_read, prepare};
use nils_registry::session::Scheme;
use serde_json::{Value, json};

fn labs() -> Vec<Lab> {
    common::labs("nils_readings_test")
}

fn bounds() -> Bounds {
    Bounds {
        timeout_ms: 20_000,
        max_rows: 5_000,
        max_bytes: 4 * 1024 * 1024,
    }
}

/// How many stacks hold a brain, a brain and neck, and the spoiled
/// gradient echo in either spelling.
fn plant(l: &mut Lab) -> (i64, i64, i64) {
    let part = |i: usize| match i % 20 {
        0 => Some("brain"),
        18 => Some("spine"),
        19 => None,
        _ => Some("brain-neck"),
    };
    let parts = replant(l, "body_part", part);
    replant(l, "body_region", |i| {
        part(i).map(|p| if p == "spine" { "spine" } else { "head" })
    });
    let techniques = replant(l, "technique", |i| match i % 6 {
        0 => Some("SPGRE"),
        3 => Some("FLASH"),
        _ => Some("MPRAGE"),
    });
    (
        parts["brain"],
        parts["brain-neck"],
        techniques["SPGRE"] + techniques["FLASH"],
    )
}

fn draft(l: &mut Lab, text: &str) -> Drafted {
    let scope = quasi();
    let scheme = Scheme::default();
    let s = Setting {
        names: &l.catalog,
        scope: &scope,
        scheme: &scheme,
        principal: "author",
        bounds: bounds(),
        values_cap: 50,
    };
    affordance::draft(&mut l.registry, text, &s, None).unwrap_or_else(|e| panic!("{}: {e}", l.name))
}

/// A stack set of the clauses given, counted.
fn scans(clauses: Value) -> String {
    json!({
        "ast_version": 1, "name": "read",
        "sets": {"scans": {"grain": "stack", "where": clauses}},
        "out": {"set": "scans", "level": "count"}
    })
    .to_string()
}

fn is(axis: &str, value: &str) -> Value {
    json!(["=", {}, ["axis", {}, axis], value])
}

/// The rows a document answers at `level: count`.
fn count(l: &mut Lab, text: &str) -> i64 {
    let (_, a) = common::ask_of(l, text);
    a.rows[0].opt_int(0).unwrap().unwrap()
}

/// The stored document of a draft, as JSON.
fn stored(l: &mut Lab, d: &Drafted) -> Value {
    let id = d.document.unwrap_or_else(|| {
        panic!(
            "{}: not stored: {:?}",
            l.name,
            d.diagnosis
                .issues
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
        )
    });
    let doc = document::get(l.registry.store(), id).unwrap().unwrap();
    without_default(serde_json::to_value(&doc.ask).unwrap())
}

/// The stored document of a draft as it is, the clause that keeps the
/// kinds of scan a question counts by default with it.
fn stored_whole(l: &mut Lab, d: &Drafted) -> Value {
    let doc = document::get(l.registry.store(), d.document.unwrap())
        .unwrap()
        .unwrap();
    serde_json::to_value(&doc.ask).unwrap()
}

/// The clause a draft adds to a stack set that names no disposition.
fn default_clause() -> Value {
    json!([
        "in",
        {"unset": true},
        ["axis", {}, "disposition"],
        ["acquisition", "scanner_derived"]
    ])
}

/// A document without the default clause, wherever a draft added it: the
/// tests of the other readings hold a document to what they read.
fn without_default(mut doc: Value) -> Value {
    fn strip(set: &mut Value) {
        let Some(m) = set.as_object_mut() else { return };
        let empty = match m.get_mut("where").and_then(Value::as_array_mut) {
            Some(clauses) => {
                clauses.retain(|c| *c != default_clause());
                clauses.is_empty()
            }
            None => false,
        };
        if empty {
            m.remove("where");
        }
    }
    if let Some(sets) = doc.get_mut("sets").and_then(Value::as_object_mut) {
        sets.values_mut().for_each(strip);
    }
    if let Some(stages) = doc.get_mut("pipeline").and_then(Value::as_array_mut) {
        stages.iter_mut().for_each(strip);
    }
    doc
}

/// The readings of values among a draft's repairs, as they are served:
/// the words and the wider values. The kinds of scan added by default are
/// [`defaults`].
fn readings(d: &Drafted) -> Vec<Value> {
    d.repairs
        .iter()
        .filter(|r| r.read.as_ref().is_some_and(|x| x.kind != "default"))
        .map(|r| serde_json::to_value(r).unwrap())
        .collect()
}

/// The default readings among a draft's repairs.
fn defaults(d: &Drafted) -> Vec<Value> {
    d.repairs
        .iter()
        .filter(|r| r.read.as_ref().is_some_and(|x| x.kind == "default"))
        .map(|r| serde_json::to_value(r).unwrap())
        .collect()
}

#[test]
fn another_name_of_a_value_is_read_as_the_stored_word_and_said() {
    for mut l in labs() {
        let (_, _, spgre) = plant(&mut l);
        // authored as YAML, the value by the name an older pack stored
        let text = "\
ast_version: 1
name: spoiled
sets:
  t1:
    grain: stack
    where:
      - [\"=\", {}, [\"axis\", {}, \"technique\"], \"FLASH\"]
out: {set: t1, level: count}
";
        let d = draft(&mut l, text);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        assert_eq!(
            readings(&d),
            [json!({
                "path": "sets.t1.where[0]",
                "what": "On technique, FLASH is another name of SPGRE, the word the pack stores: \
                         it was read as SPGRE.",
                "read": {
                    "kind": "word", "axis": "technique", "wrote": "FLASH",
                    "as_axis": "technique", "as_value": "SPGRE"
                }
            })],
            "{}",
            l.name
        );
        // the diagnosis carries the same list
        assert_eq!(d.diagnosis.repairs, d.repairs, "{}", l.name);
        // what is stored is what was read, under the hash of that document
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["t1"]["where"],
            json!([is("technique", "SPGRE")]),
            "{}",
            l.name
        );
        let read = json!({
            "ast_version": 1, "name": "spoiled",
            "sets": {"t1": {"grain": "stack", "where": [is("technique", "SPGRE")]}},
            "out": {"set": "t1", "level": "count"}
        })
        .to_string();
        let whole = stored_whole(&mut l, &d).to_string();
        assert_eq!(
            without_default(serde_json::from_str(&whole).unwrap()).to_string(),
            {
                let v: Value = serde_json::from_str(&read).unwrap();
                v.to_string()
            }
        );
        let hash = prepare(parse(&whole).unwrap(), &l.catalog, &quasi())
            .unwrap()
            .hash;
        assert_eq!(d.hash.as_deref(), Some(hash.as_str()), "{}", l.name);
        // and it selects what the name selected: every stack of the value,
        // in either spelling a row holds
        let written = read.replace("SPGRE", "FLASH");
        assert_eq!(count(&mut l, &written), spgre, "{}", l.name);
        assert_eq!(count(&mut l, &read), spgre, "{}", l.name);
        // the stored word itself is no reading, and a structural repair
        // carries no `read`
        let d = draft(
            &mut l,
            &text
                .replace("\"FLASH\"", "\"SPGRE\"")
                .replace("\"=\", {}, ", "\"==\", "),
        );
        assert!(d.diagnosis.valid, "{}", l.name);
        assert_eq!(readings(&d), [] as [Value; 0], "{}", l.name);
        assert!(!d.repairs.is_empty(), "{}", l.name);
        // (the kinds of scan a question counts are a reading of their own)
        let structural: Vec<_> = d
            .repairs
            .iter()
            .filter(|r| r.read.as_ref().is_none_or(|x| x.kind != "default"))
            .collect();
        assert!(!structural.is_empty(), "{}", l.name);
        for r in structural {
            let v = serde_json::to_value(r).unwrap();
            assert!(v.get("read").is_none(), "{}: {v}", l.name);
        }
    }
}

#[test]
fn every_name_but_the_stored_word_is_read_wherever_a_where_names_it() {
    for mut l in labs() {
        plant(&mut l);
        // by identity, in a list beside the stored word, negated, nested,
        // and a number that is an earlier name
        let text = scans(json!([
            is("technique", "SP-GRE"),
            [
                "in",
                {},
                ["axis", {}, "technique"],
                ["FLASH", "SPGRE", "MPRAGE"]
            ],
            ["<>", {}, ["axis", {}, "technique"], "FLASH"],
            [
                "not",
                {},
                [
                    "or",
                    {},
                    is("technique", "TurboFLASH"),
                    is("technique", "MPRAGE")
                ]
            ],
            ["=", {}, ["axis", {}, "post_contrast"], 1]
        ]));
        let d = draft(&mut l, &text);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        let said: Vec<(String, String, String, String)> = readings(&d)
            .iter()
            .map(|r| {
                let t = |v: &Value| v.as_str().unwrap().to_string();
                assert_eq!(r["read"]["kind"], "word", "{}", l.name);
                assert_eq!(r["read"]["axis"], r["read"]["as_axis"], "{}", l.name);
                assert!(r["read"].get("exact").is_none(), "{}", l.name);
                (
                    t(&r["path"]),
                    t(&r["read"]["axis"]),
                    t(&r["read"]["wrote"]),
                    t(&r["read"]["as_value"]),
                )
            })
            .collect();
        let row = |p: &str, a: &str, w: &str, v: &str| {
            (p.to_string(), a.to_string(), w.to_string(), v.to_string())
        };
        assert_eq!(
            said,
            [
                row("sets.scans.where[0]", "technique", "SP-GRE", "SPGRE"),
                row("sets.scans.where[1]", "technique", "FLASH", "SPGRE"),
                row("sets.scans.where[2]", "technique", "FLASH", "SPGRE"),
                row(
                    "sets.scans.where[3][2][2]",
                    "technique",
                    "TurboFLASH",
                    "FSPGRE"
                ),
                row("sets.scans.where[4]", "post_contrast", "1", "given"),
            ],
            "{}",
            l.name
        );
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([
                is("technique", "SPGRE"),
                ["in", {}, ["axis", {}, "technique"], ["SPGRE", "MPRAGE"]],
                ["<>", {}, ["axis", {}, "technique"], "SPGRE"],
                [
                    "not",
                    {},
                    [
                        "or",
                        {},
                        is("technique", "FSPGRE"),
                        is("technique", "MPRAGE")
                    ]
                ],
                is("post_contrast", "given")
            ]),
            "{}",
            l.name
        );
        // a text that names no value is left to the validator, which
        // refuses it as it did
        let d = draft(&mut l, &scans(json!([is("technique", "no such word")])));
        assert!(!d.diagnosis.valid && d.document.is_none(), "{}", l.name);
        assert_eq!(readings(&d), [] as [Value; 0], "{}", l.name);
        assert_eq!(
            d.diagnosis.issues[0].message, "no such word is not a value of axis technique",
            "{}: {:?}",
            l.name, d.diagnosis.issues
        );
    }
}

/// A word the pack lists for a value (`terms`) is no name of it: a
/// document may not write one. A question authored as text has it read as
/// the value where one value of the axis alone has the word, letter case
/// and surrounding space aside, and so is a name in another letter case.
#[test]
fn a_word_the_pack_lists_for_one_value_is_read_as_that_value_and_said() {
    for mut l in labs() {
        let (_, _, spgre) = plant(&mut l);
        let mprage = common::stacks_seen(&mut l).len() as i64 - spgre;
        assert!(mprage > 0 && spgre > 0, "{}", l.name);
        // a vendor's word for the sequence
        let d = draft(&mut l, &scans(json!([is("technique", "BRAVO")])));
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        assert_eq!(
            readings(&d),
            [json!({
                "path": "sets.scans.where[0]",
                "what": "On technique, the pack lists BRAVO as a word for MPRAGE: it was read \
                         as MPRAGE, the word the pack stores.",
                "read": {
                    "kind": "word", "axis": "technique", "wrote": "BRAVO",
                    "as_axis": "technique", "as_value": "MPRAGE"
                }
            })],
            "{}",
            l.name
        );
        assert_eq!(d.diagnosis.repairs, d.repairs, "{}", l.name);
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([is("technique", "MPRAGE")]),
            "{}",
            l.name
        );
        assert_eq!(count(&mut l, &doc.to_string()), mprage, "{}", l.name);
        // in any letter case, with space around it, in a list and negated;
        // and a name in another letter case
        let text = scans(json!([
            [
                "in",
                {},
                ["axis", {}, "technique"],
                ["bravo", " spgr ", "MPRAGE"]
            ],
            ["<>", {}, ["axis", {}, "technique"], "3d tfe"],
            is("technique", "mprage"),
            ["not", {}, is("technique", "flash")]
        ]));
        let d = draft(&mut l, &text);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        let said: Vec<(String, String, String, String)> = readings(&d)
            .iter()
            .map(|r| {
                let t = |v: &Value| v.as_str().unwrap().to_string();
                assert_eq!(r["read"]["kind"], "word", "{}", l.name);
                assert_eq!(r["read"]["as_axis"], "technique", "{}", l.name);
                (
                    t(&r["path"]),
                    t(&r["read"]["wrote"]),
                    t(&r["read"]["as_value"]),
                    t(&r["what"]),
                )
            })
            .collect();
        let term = |w: &str, v: &str| {
            format!(
                "On technique, the pack lists {w} as a word for {v}: it was read as {v}, the \
                 word the pack stores."
            )
        };
        let folded = |w: &str, v: &str| {
            format!(
                "On technique, {w} is a name of {v} but for letter case or surrounding space: \
                 it was read as {v}, the word the pack stores."
            )
        };
        let row = |p: &str, w: &str, v: &str, what: String| {
            (p.to_string(), w.to_string(), v.to_string(), what)
        };
        assert_eq!(
            said,
            [
                row(
                    "sets.scans.where[0]",
                    "bravo",
                    "MPRAGE",
                    term("bravo", "MPRAGE")
                ),
                row(
                    "sets.scans.where[0]",
                    " spgr ",
                    "SPGRE",
                    term(" spgr ", "SPGRE")
                ),
                row(
                    "sets.scans.where[1]",
                    "3d tfe",
                    "MPRAGE",
                    term("3d tfe", "MPRAGE")
                ),
                row(
                    "sets.scans.where[2]",
                    "mprage",
                    "MPRAGE",
                    folded("mprage", "MPRAGE")
                ),
                row(
                    "sets.scans.where[3][2]",
                    "flash",
                    "SPGRE",
                    folded("flash", "SPGRE")
                ),
            ],
            "{}",
            l.name
        );
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([
                ["in", {}, ["axis", {}, "technique"], ["MPRAGE", "SPGRE"]],
                ["<>", {}, ["axis", {}, "technique"], "MPRAGE"],
                is("technique", "MPRAGE"),
                ["not", {}, is("technique", "SPGRE")]
            ]),
            "{}",
            l.name
        );
        // an exact name is that value, whatever another value lists: MIP
        // is a value, and the minimum projection lists mIP
        let exact = scans(json!([["has", {}, ["axis", {}, "construct"], "MIP"]]));
        let d = draft(&mut l, &exact);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        assert_eq!(readings(&d), [] as [Value; 0], "{}", l.name);
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([["has", {}, ["axis", {}, "construct"], "MIP"]]),
            "{}",
            l.name
        );
    }
}

/// A word two values of an axis have is read as neither: the document is
/// refused as it was, and the refusal names the values the word may mean.
/// A document that arrives structured is refused for a word one value
/// alone lists as well, and told the value.
#[test]
fn a_word_two_values_share_is_left_and_refused_and_a_structured_document_reads_none() {
    for mut l in labs() {
        plant(&mut l);
        // two values list the word
        let d = draft(
            &mut l,
            &scans(json!([[
                "has",
                {},
                ["axis", {}, "construct"],
                "isotropic DWI"
            ]])),
        );
        assert!(!d.diagnosis.valid && d.document.is_none(), "{}", l.name);
        assert_eq!(readings(&d), [] as [Value; 0], "{}", l.name);
        assert_eq!(d.diagnosis.issues.len(), 1, "{}", l.name);
        assert_eq!(
            d.diagnosis.issues[0].path, "sets.scans.where[0]",
            "{}",
            l.name
        );
        assert_eq!(
            d.diagnosis.issues[0].message,
            "isotropic DWI is not a value of axis construct; the pack has it as a word for \
             more than one value: Trace or isoDWI",
            "{}",
            l.name
        );
        assert!(
            d.diagnosis.issues[0]
                .next
                .starts_with("write the one meant: Trace or isoDWI; or GET /api/ask/catalog"),
            "{}: {}",
            l.name,
            d.diagnosis.issues[0].next
        );
        // one value has it as a name in another case and another lists it
        let d = draft(
            &mut l,
            &scans(json!([["has", {}, ["axis", {}, "construct"], "mip"]])),
        );
        assert!(!d.diagnosis.valid, "{}", l.name);
        assert_eq!(readings(&d), [] as [Value; 0], "{}", l.name);
        assert_eq!(
            d.diagnosis.issues[0].message,
            "mip is not a value of axis construct; the pack has it as a word for more than one \
             value: MIP or MinIP",
            "{}",
            l.name
        );
        // the documents door takes a document as it is written: a word one
        // value alone lists is refused there, as before, and told the value
        let scope = quasi();
        let scheme = Scheme::default();
        let s = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "desk",
            bounds: bounds(),
            values_cap: 50,
        };
        for (word, said) in [
            (
                "BRAVO",
                "BRAVO is not a value of axis technique; the pack lists it as a word for MPRAGE",
            ),
            (
                "mprage",
                "mprage is not a value of axis technique; the pack writes it MPRAGE",
            ),
        ] {
            let text = scans(json!([is("technique", word)]));
            let ask = parse(&text).unwrap();
            let refused = match affordance::post(&mut l.registry, &ask, &s, None) {
                Err(affordance::AffordanceError::Ask(nils_ask::Error::Invalid(issues))) => issues,
                other => panic!(
                    "{}: {word} was not refused: {:?}",
                    l.name,
                    other.map(|d| d.id)
                ),
            };
            assert_eq!(refused.len(), 1, "{}: {refused:?}", l.name);
            assert_eq!(refused[0].code.name(), "unknown_value", "{}", l.name);
            assert_eq!(refused[0].message, said, "{}", l.name);
            // and strict validation of the same document says the same
            let e = prepare(ask, &l.catalog, &scope).unwrap_err();
            assert!(e.to_string().contains(said), "{}: {e}", l.name);
        }
    }
}

/// A word the pack lists for a value that is read wider when written alone
/// is read wider as the value is: one reading, which says what was
/// written. With `exact: true` the word is still read as the value's
/// stored word, and only the wider reading is left out; and a clause that
/// keeps the wider value by a word of its own is the narrowing on purpose
/// it would be in the stored words.
#[test]
fn a_listed_word_of_a_fine_value_is_read_wider_in_one_reading() {
    for mut l in labs() {
        let (brain, brain_neck, _) = plant(&mut l);
        let exact = "[\"=\", {exact: true}, [\"axis\", {}, \"body_part\"], \"brain\"]";
        let d = draft(&mut l, &scans(json!([is("body_part", "Neuro")])));
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        assert_eq!(
            readings(&d),
            [json!({
                "path": "sets.scans.where[0]",
                "what": format!(
                    "body_part = Neuro, a word the pack lists for brain, was read as \
                     body_region = head, which covers brain and brain-neck: the pack reads \
                     brain written alone as the wider head. To keep brain alone, write {exact}."
                ),
                "read": {
                    "kind": "wider", "axis": "body_part", "wrote": "Neuro",
                    "as_axis": "body_region", "as_value": "head", "exact": exact
                }
            })],
            "{}",
            l.name
        );
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([is("body_region", "head")]),
            "{}",
            l.name
        );
        assert_eq!(
            count(&mut l, &doc.to_string()),
            brain + brain_neck,
            "{}",
            l.name
        );
        // meant as written: the word is read, the value kept alone
        let kept = "ast_version: 1\nname: kept\nsets:\n  scans:\n    grain: stack\n    where:\n      \
                    - [\"=\", {exact: true}, [\"axis\", {}, \"body_part\"], \"neuro\"]\n\
                    out: {set: scans, level: count}\n";
        let d = draft(&mut l, kept);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        let r = readings(&d);
        assert_eq!(r.len(), 1, "{}: {r:?}", l.name);
        assert_eq!(
            r[0]["read"],
            json!({
                "kind": "word", "axis": "body_part", "wrote": "neuro",
                "as_axis": "body_part", "as_value": "brain"
            }),
            "{}",
            l.name
        );
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([["=", {"exact": true}, ["axis", {}, "body_part"], "brain"]]),
            "{}",
            l.name
        );
        assert_eq!(count(&mut l, &doc.to_string()), brain, "{}", l.name);
        // the wider value kept by a word the pack lists for it, beside the
        // fine one by a word of its own: two words read, nothing wider
        let d = draft(
            &mut l,
            &scans(json!([
                is("body_region", "skull"),
                is("body_part", "neuro")
            ])),
        );
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        let kinds: Vec<(String, String, String)> = readings(&d)
            .iter()
            .map(|r| {
                let t = |v: &Value| v.as_str().unwrap().to_string();
                (
                    t(&r["read"]["kind"]),
                    t(&r["read"]["wrote"]),
                    t(&r["read"]["as_value"]),
                )
            })
            .collect();
        let row = |k: &str, w: &str, v: &str| (k.to_string(), w.to_string(), v.to_string());
        assert_eq!(
            kinds,
            [row("word", "skull", "head"), row("word", "neuro", "brain")],
            "{}",
            l.name
        );
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([is("body_region", "head"), is("body_part", "brain")]),
            "{}",
            l.name
        );
        // and in an `or` beside another value of the axis named by a word
        // of its own, both are meant as written
        let d = draft(
            &mut l,
            &scans(json!([[
                "or",
                {},
                is("body_part", "neuro"),
                is("body_part", "head and neck")
            ]])),
        );
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([[
                "or",
                {},
                is("body_part", "brain"),
                is("body_part", "brain-neck")
            ]]),
            "{}",
            l.name
        );
        assert!(
            readings(&d).iter().all(|r| r["read"]["kind"] == "word"),
            "{}",
            l.name
        );
    }
}

#[test]
fn a_fine_value_written_alone_is_read_as_its_wider_value_and_said() {
    for mut l in labs() {
        let (brain, brain_neck, _) = plant(&mut l);
        let text = "\
ast_version: 1
name: brain scans
sets:
  eligible:
    grain: stack
    where:
      - [\"=\", {}, [\"axis\", {}, \"technique\"], \"MPRAGE\"]
      - [\"=\", {}, [\"axis\", {}, \"body_part\"], \"brain\"]
out: {set: eligible, level: count}
";
        let d = draft(&mut l, text);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        let exact = "[\"=\", {exact: true}, [\"axis\", {}, \"body_part\"], \"brain\"]";
        assert_eq!(
            readings(&d),
            [json!({
                "path": "sets.eligible.where[1]",
                "what": format!(
                    "body_part = brain was read as body_region = head, which covers brain and \
                     brain-neck: the pack reads brain written alone as the wider head. To keep \
                     brain alone, write {exact}."
                ),
                "read": {
                    "kind": "wider", "axis": "body_part", "wrote": "brain",
                    "as_axis": "body_region", "as_value": "head", "exact": exact
                }
            })],
            "{}",
            l.name
        );
        assert_eq!(d.diagnosis.repairs, d.repairs, "{}", l.name);
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["eligible"]["where"],
            json!([is("technique", "MPRAGE"), is("body_region", "head")]),
            "{}",
            l.name
        );
        // the document read warns of no narrow value: nothing is narrow
        assert!(
            d.diagnosis
                .warnings
                .iter()
                .all(|w| w.code.name() != "narrow_value"),
            "{}: {:?}",
            l.name,
            d.diagnosis.warnings
        );
        // alone in a set, it keeps every head: both fine values
        let d = draft(&mut l, &scans(json!([is("body_part", "brain")])));
        let doc = stored(&mut l, &d);
        assert_eq!(
            count(&mut l, &doc.to_string()),
            brain + brain_neck,
            "{}",
            l.name
        );
        // the clause the reading names keeps the fine value alone: drafted
        // as it is written there, it is taken as written and counts brains
        let kept = text.replace(
            "[\"=\", {}, [\"axis\", {}, \"body_part\"], \"brain\"]",
            exact,
        );
        assert_ne!(kept, text);
        let d = draft(&mut l, &kept);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        assert_eq!(readings(&d), [] as [Value; 0], "{}", l.name);
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["eligible"]["where"][1],
            json!(["=", {"exact": true}, ["axis", {}, "body_part"], "brain"]),
            "{}",
            l.name
        );
        let alone = scans(json!([["=", {"exact": true}, ["axis", {}, "body_part"], "brain"]]));
        let d = draft(&mut l, &alone);
        assert_eq!(readings(&d), [] as [Value; 0], "{}", l.name);
        assert_eq!(count(&mut l, &alone), brain, "{}", l.name);
        assert!(
            d.diagnosis
                .warnings
                .iter()
                .all(|w| w.code.name() != "narrow_value"),
            "{}: {:?}",
            l.name,
            d.diagnosis.warnings
        );
        // `in` with the one value, `has`, and another name of the value
        // are the same clause, each with the exact form of its own op
        for (clause, wrote, exact) in [
            (
                json!(["in", {}, ["axis", {}, "body_part"], ["brain"]]),
                "brain",
                "[\"in\", {exact: true}, [\"axis\", {}, \"body_part\"], [\"brain\"]]",
            ),
            (
                json!(["has", {}, ["axis", {}, "body_part"], "brain"]),
                "brain",
                "[\"has\", {exact: true}, [\"axis\", {}, \"body_part\"], \"brain\"]",
            ),
        ] {
            let d = draft(&mut l, &scans(json!([clause])));
            let r = readings(&d);
            assert_eq!(r.len(), 1, "{}: {clause}", l.name);
            assert_eq!(r[0]["read"]["kind"], "wider", "{}", l.name);
            assert_eq!(r[0]["read"]["wrote"], wrote, "{}", l.name);
            assert_eq!(r[0]["read"]["exact"], exact, "{}", l.name);
            let doc = stored(&mut l, &d);
            assert_eq!(
                doc["sets"]["scans"]["where"],
                json!([is("body_region", "head")]),
                "{}",
                l.name
            );
            // and the exact form drafts as written
            let kept = format!(
                "ast_version: 1\nname: kept\nsets:\n  scans:\n    grain: stack\n    where:\n      \
                 - {exact}\nout: {{set: scans, level: count}}\n"
            );
            let d = draft(&mut l, &kept);
            assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
            assert_eq!(readings(&d), [] as [Value; 0], "{}: {kept}", l.name);
        }
    }
}

#[test]
fn a_fine_value_is_taken_as_written_where_the_document_says_it_is_meant() {
    for mut l in labs() {
        plant(&mut l);
        let brain = is("body_part", "brain");
        let as_written = |l: &mut Lab, sets: Value, out: &str, why: &str| {
            let text = json!({
                "ast_version": 1, "name": "written", "sets": sets.clone(),
                "out": {"set": out, "level": "count"}
            })
            .to_string();
            let d = draft(l, &text);
            assert!(
                d.diagnosis.valid,
                "{}: {why}: {:?}",
                l.name, d.diagnosis.issues
            );
            assert_eq!(readings(&d), [] as [Value; 0], "{}: {why}", l.name);
            let doc = stored(l, &d);
            assert_eq!(doc["sets"], sets, "{}: {why}", l.name);
        };
        let one = |clauses: Value| json!({"scans": {"grain": "stack", "where": clauses}});
        // the set keeps the wider value itself, before or after
        as_written(
            &mut l,
            one(json!([is("body_region", "head"), brain])),
            "scans",
            "the wider value first",
        );
        as_written(
            &mut l,
            one(json!([brain, is("body_region", "head")])),
            "scans",
            "the wider value after",
        );
        // or the set it is drawn from does
        as_written(
            &mut l,
            json!({
                "heads": {"grain": "stack", "where": [is("body_region", "head")]},
                "scans": {"grain": "stack", "from": "heads", "where": [brain]}
            }),
            "scans",
            "the wider value in the set it is drawn from",
        );
        // negated, in each way a document negates
        as_written(
            &mut l,
            one(json!([["<>", {}, ["axis", {}, "body_part"], "brain"]])),
            "scans",
            "not equal",
        );
        as_written(
            &mut l,
            one(json!([[
                "not_in",
                {},
                ["axis", {}, "body_part"],
                ["brain"]
            ]])),
            "scans",
            "not among",
        );
        as_written(
            &mut l,
            one(json!([["not", {}, brain]])),
            "scans",
            "under a not",
        );
        // among alternatives of its own axis, and in a list of several
        as_written(
            &mut l,
            one(json!([["or", {}, brain, is("body_part", "spine")]])),
            "scans",
            "an or with another value of the axis",
        );
        as_written(
            &mut l,
            one(json!([[
                "in",
                {},
                ["axis", {}, "body_part"],
                ["brain", "spine"]
            ]])),
            "scans",
            "a list of several values",
        );
        // the values the pack declares no reading for
        for part in ["brain-neck", "neck", "spine", "chest", "other"] {
            as_written(&mut l, one(json!([is("body_part", part)])), "scans", part);
        }
        // the wider axis itself
        as_written(
            &mut l,
            one(json!([is("body_region", "head")])),
            "scans",
            "the wider value",
        );
    }
}

#[test]
fn a_fine_value_is_read_inside_an_and_an_or_and_a_pipeline_stage() {
    for mut l in labs() {
        plant(&mut l);
        // a part of an `and`, and of an `or` whose other part is of
        // another axis
        let text = scans(json!([
            [
                "and",
                {},
                is("body_part", "brain"),
                is("technique", "MPRAGE")
            ],
            ["or", {}, is("body_part", "brain"), is("technique", "FLASH")]
        ]));
        let d = draft(&mut l, &text);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        let paths: Vec<(String, String)> = readings(&d)
            .iter()
            .map(|r| {
                (
                    r["path"].as_str().unwrap().to_string(),
                    r["read"]["kind"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            paths,
            [
                ("sets.scans.where[0][2]".to_string(), "wider".to_string()),
                ("sets.scans.where[1][2]".to_string(), "wider".to_string()),
                ("sets.scans.where[1][3]".to_string(), "word".to_string()),
            ],
            "{}",
            l.name
        );
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([
                [
                    "and",
                    {},
                    is("body_region", "head"),
                    is("technique", "MPRAGE")
                ],
                [
                    "or",
                    {},
                    is("body_region", "head"),
                    is("technique", "SPGRE")
                ]
            ]),
            "{}",
            l.name
        );
        // a stage of a pipeline, told at the stage, and a later stage read
        // with the stage it is drawn from: the first stage keeps the wider
        // value, so the second narrows on purpose
        let text = json!({
            "ast_version": 1, "name": "stages",
            "pipeline": [
                {"grain": "stack", "where": [is("body_part", "brain")]},
                {"name": "sharp", "grain": "stack", "where": [is("technique", "FLASH")]}
            ],
            "out": {"set": "sharp", "level": "count"}
        })
        .to_string();
        let d = draft(&mut l, &text);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        let paths: Vec<String> = readings(&d)
            .iter()
            .map(|r| r["path"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            paths,
            ["pipeline[0].where[0]", "pipeline[1].where[0]"],
            "{}",
            l.name
        );
        let doc = stored(&mut l, &d);
        assert_eq!(
            doc["pipeline"][0]["where"],
            json!([is("body_region", "head")]),
            "{}",
            l.name
        );
        assert_eq!(
            doc["pipeline"][1]["where"],
            json!([is("technique", "SPGRE")]),
            "{}",
            l.name
        );
        let text = json!({
            "ast_version": 1, "name": "stages",
            "pipeline": [
                {"grain": "stack", "where": [is("body_region", "head")]},
                {"name": "brains", "grain": "stack", "where": [is("body_part", "brain")]}
            ],
            "out": {"set": "brains", "level": "count"}
        })
        .to_string();
        let d = draft(&mut l, &text);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        assert_eq!(readings(&d), [] as [Value; 0], "{}", l.name);
    }
}

#[test]
fn a_document_that_arrives_structured_is_taken_as_written() {
    for mut l in labs() {
        let (brain, _, _) = plant(&mut l);
        let text = scans(json!([is("body_part", "brain"), is("technique", "FLASH")]));
        let written: Value = serde_json::from_str(&text).unwrap();
        // the documents door: stored as written, under the hash of what
        // was written
        let scope = quasi();
        let scheme = Scheme::default();
        let s = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "desk",
            bounds: bounds(),
            values_cap: 50,
        };
        let ask = parse(&text).unwrap();
        let doc = affordance::post(&mut l.registry, &ask, &s, None).unwrap();
        assert_eq!(
            serde_json::to_value(&doc.ask).unwrap(),
            written,
            "{}",
            l.name
        );
        // a draft of the same text is read, and is another document
        let d = draft(&mut l, &text);
        assert_eq!(readings(&d).len(), 2, "{}", l.name);
        assert_ne!(d.hash.as_deref(), Some(doc.hash.as_str()), "{}", l.name);
        assert_ne!(stored(&mut l, &d), written, "{}", l.name);
        // and the fine value alone keeps the brains it says
        assert_eq!(
            count(&mut l, &scans(json!([is("body_part", "brain")]))),
            brain,
            "{}",
            l.name
        );
        // the reading by itself, as every door that repairs calls it, and
        // a second reading of what was read reads nothing more
        let (read, repairs) = parse_read(&text, &l.catalog).unwrap();
        assert_eq!(repairs.len(), 3, "{}", l.name);
        let again = serde_json::to_string(&read).unwrap();
        let (twice, repairs) = parse_read(&again, &l.catalog).unwrap();
        assert_eq!(repairs, [], "{}", l.name);
        assert_eq!(twice, read, "{}", l.name);
    }
}

#[test]
fn a_reading_is_said_of_a_document_that_is_refused_too() {
    for mut l in labs() {
        plant(&mut l);
        let text = json!({
            "ast_version": 1, "name": "refused",
            "sets": {
                "scans": {"grain": "stack", "where": [
                    is("body_part", "brain"),
                    ["=", {}, ["field", {}, "no_such_field"], 1]
                ]}
            },
            "out": {"set": "scans", "level": "count"}
        })
        .to_string();
        let d = draft(&mut l, &text);
        assert!(!d.diagnosis.valid && d.document.is_none(), "{}", l.name);
        assert_eq!(readings(&d).len(), 1, "{}", l.name);
        assert_eq!(d.diagnosis.repairs, d.repairs, "{}", l.name);
    }
}

#[test]
fn the_catalog_serves_what_a_value_is_read_as() {
    let labs = labs();
    let l = &labs[0];
    let doc = l.catalog.document(&Scope::default());
    let axis = |name: &str| -> Value {
        doc["axes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap_or_else(|| panic!("no axis {name}"))
            .clone()
    };
    let value = |axis: &Value, id: &str| -> Value {
        axis["values"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == id)
            .unwrap_or_else(|| panic!("no value {id}"))
            .clone()
    };
    let part = axis("body_part");
    assert_eq!(
        value(&part, "brain")["read_as"],
        json!({"axis": "body_region", "value": "head"})
    );
    for id in ["brain-neck", "neck", "spine", "chest", "other"] {
        assert!(value(&part, id).get("read_as").is_none(), "{id}");
    }
    let region = axis("body_region");
    assert_eq!(value(&region, "head")["read_for"], json!(["brain"]));
    for id in ["spine", "chest", "other"] {
        assert!(value(&region, id).get("read_for").is_none(), "{id}");
    }
    // the ask reads the same declaration by name
    let kin = l.catalog.axis_kin("body_part");
    assert_eq!(
        kin.wider[0].read_for,
        [("head".to_string(), vec!["brain".to_string()])]
    );
    // additive: a catalog written before the field is read as it was
    let before: nils_catalog::AxisRecord = serde_json::from_value(json!({
        "name": "body_part", "multi": false, "stores": "id",
        "values": [{"id": "brain", "label": "brain"}]
    }))
    .unwrap();
    assert_eq!(before.values[0].read_as, None);
    assert!(before.values[0].read_for.is_empty());
}

/// The kinds of scan a question counts unless it says. The pack's
/// disposition says which (`ask_default`), and a stack set of a question
/// authored as text that names no disposition gets the clause that keeps
/// them, said as a reading of kind `default`.
#[test]
fn the_kinds_of_scan_a_question_counts_are_added_where_it_names_none_and_said() {
    for mut l in labs() {
        plant(&mut l);
        // some stacks are reformats, one in twenty a working scan, and one
        // in ten nothing has sorted yet: it holds no disposition at all
        replant(&mut l, "disposition", |i| match i % 20 {
            0 => Some("working_scan"),
            5 | 10 | 15 => Some("reformat"),
            7 => Some("scanner_derived"),
            2 | 13 => None,
            _ => Some("acquisition"),
        });
        let d = draft(&mut l, &scans(json!([is("technique", "MPRAGE")])));
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        assert_eq!(readings(&d), [] as [Value; 0], "{}", l.name);
        assert_eq!(
            defaults(&d),
            [json!({
                "path": "sets.scans.where[1]",
                "what": "No clause of scans names a disposition, so disposition among \
                         [acquisition, scanner_derived] (or none yet) was added: a question \
                         counts those unless it says, and a stack that holds no disposition \
                         yet with them. Reformat, working_scan, scout, display_composite and \
                         excluded stacks are left out. To count another kind, write the \
                         disposition clause the words ask for in scans, for example [\"in\", \
                         {unset: true}, [\"axis\", {}, \"disposition\"], [\"acquisition\", \
                         \"scanner_derived\", \"reformat\"]].",
                "read": {
                    "kind": "default", "axis": "disposition", "wrote": "",
                    "as_axis": "disposition", "as_value": "acquisition, scanner_derived",
                    "as_values": ["acquisition", "scanner_derived"]
                }
            })],
            "{}",
            l.name
        );
        assert_eq!(d.diagnosis.repairs, d.repairs, "{}", l.name);
        let doc = stored_whole(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([is("technique", "MPRAGE"), default_clause()]),
            "{}",
            l.name
        );
        // it counts the acquisitions and the scanner-derived images, and
        // neither the reformats nor the working scans
        let of = |clauses: Value| scans(clauses);
        let all = count(&mut l, &of(json!([is("technique", "MPRAGE")])));
        let reformats = count(
            &mut l,
            &of(json!([
                is("technique", "MPRAGE"),
                is("disposition", "reformat")
            ])),
        );
        let working = count(
            &mut l,
            &of(json!([
                is("technique", "MPRAGE"),
                is("disposition", "working_scan")
            ])),
        );
        assert!(reformats > 0 && working > 0, "{}", l.name);
        assert_eq!(
            count(&mut l, &doc.to_string()),
            all - reformats - working,
            "{}",
            l.name
        );
        // the stacks nothing has sorted yet are among them: the same
        // clause without `unset` leaves them out, as a clause a person
        // writes on the two kinds does
        let sorted = count(
            &mut l,
            &of(json!([
                is("technique", "MPRAGE"),
                [
                    "in",
                    {},
                    ["axis", {}, "disposition"],
                    ["acquisition", "scanner_derived"]
                ]
            ])),
        );
        let unsorted = all - reformats - working - sorted;
        assert!(
            unsorted > 0,
            "{}: {all} {reformats} {working} {sorted}",
            l.name
        );
        // `=` takes the option as `in` does
        assert_eq!(
            count(
                &mut l,
                &of(json!([
                    is("technique", "MPRAGE"),
                    ["=", {"unset": true}, ["axis", {}, "disposition"], "reformat"]
                ])),
            ),
            reformats + unsorted,
            "{}",
            l.name
        );
        // the clause the reading names counts the reformats too, and is
        // taken as written
        let more = json!([
            "in",
            {"unset": true},
            ["axis", {}, "disposition"],
            ["acquisition", "scanner_derived", "reformat"]
        ]);
        let d = draft(&mut l, &of(json!([is("technique", "MPRAGE"), more])));
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        assert_eq!(defaults(&d), [] as [Value; 0], "{}", l.name);
        let doc = stored_whole(&mut l, &d);
        assert_eq!(count(&mut l, &doc.to_string()), all - working, "{}", l.name);
        // a set with no clause at all gets it as its first
        let d = draft(&mut l, &scans(json!([])));
        assert_eq!(defaults(&d)[0]["path"], "sets.scans.where[0]", "{}", l.name);
        let doc = stored_whole(&mut l, &d);
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([default_clause()]),
            "{}",
            l.name
        );
        // a second draft of what was read adds nothing more
        let d = draft(&mut l, &doc.to_string());
        assert_eq!(defaults(&d), [] as [Value; 0], "{}", l.name);
        assert_eq!(stored_whole(&mut l, &d), doc, "{}", l.name);
    }
}

/// Any clause on the disposition, in the set or in a set it is drawn from,
/// turns the default off for it; a chain of sets gets one clause, at the
/// set the others are drawn from; a set at another grain is untouched.
#[test]
fn the_default_is_one_clause_a_chain_and_is_turned_off_by_a_clause_on_the_axis() {
    for mut l in labs() {
        plant(&mut l);
        let drafted = |l: &mut Lab, sets: Value, out: &str| {
            let text = json!({
                "ast_version": 1, "name": "kinds", "sets": sets,
                "out": {"set": out, "level": "count"}
            })
            .to_string();
            let d = draft(l, &text);
            assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
            let paths: Vec<String> = defaults(&d)
                .iter()
                .map(|r| r["path"].as_str().unwrap().to_string())
                .collect();
            (paths, stored_whole(l, &d))
        };
        let mprage = is("technique", "MPRAGE");
        let none: [&str; 0] = [];
        // a clause on the axis in the set, however it names it
        for clause in [
            is("disposition", "acquisition"),
            json!(["<>", {}, ["axis", {}, "disposition"], "reformat"]),
            json!(["in", {}, ["axis", {}, "disposition"], ["reformat", "scout"]]),
            json!(["not", {}, is("disposition", "scout")]),
            json!([
                "or",
                {},
                is("disposition", "reformat"),
                is("technique", "SPGRE")
            ]),
        ] {
            let sets = json!({"scans": {"grain": "stack", "where": [mprage, clause]}});
            let (paths, doc) = drafted(&mut l, sets.clone(), "scans");
            assert_eq!(paths, none, "{}: {clause}", l.name);
            assert_eq!(doc["sets"], sets, "{}: {clause}", l.name);
        }
        // a chain: one clause, at the set the others are drawn from
        let (paths, doc) = drafted(
            &mut l,
            json!({
                "base": {"grain": "stack", "where": [is("body_region", "head")]},
                "t1": {"grain": "stack", "from": "base", "where": [mprage]},
                "sharp": {"grain": "stack", "from": "t1", "where": [is("technique", "MPRAGE")]},
                "other": {"grain": "stack", "from": "base", "where": [is("technique", "SPGRE")]}
            }),
            "sharp",
        );
        assert_eq!(paths, ["sets.base.where[1]"], "{}", l.name);
        assert_eq!(
            doc["sets"]["base"]["where"],
            json!([is("body_region", "head"), default_clause()]),
            "{}",
            l.name
        );
        for drawn in ["t1", "sharp", "other"] {
            assert_eq!(
                doc["sets"][drawn]["where"].as_array().unwrap().len(),
                1,
                "{}: {drawn}",
                l.name
            );
        }
        // the set the others are drawn from names the axis: off for all
        let (paths, _) = drafted(
            &mut l,
            json!({
                "base": {"grain": "stack", "where": [is("disposition", "reformat")]},
                "t1": {"grain": "stack", "from": "base", "where": [mprage]}
            }),
            "t1",
        );
        assert_eq!(paths, none, "{}", l.name);
        // one drawn set names the axis: the shared set is left as written,
        // since a clause there would cut the set that asked for another
        // kind, and the other drawn set gets the clause itself
        let (paths, doc) = drafted(
            &mut l,
            json!({
                "base": {"grain": "stack", "where": [is("body_region", "head")]},
                "plain": {"grain": "stack", "from": "base", "where": [mprage]},
                "views": {"grain": "stack", "from": "base", "where": [is("disposition", "reformat")]}
            }),
            "plain",
        );
        assert_eq!(paths, ["sets.plain.where[1]"], "{}", l.name);
        assert_eq!(
            doc["sets"]["base"]["where"],
            json!([is("body_region", "head")]),
            "{}",
            l.name
        );
        assert_eq!(
            doc["sets"]["views"]["where"],
            json!([is("disposition", "reformat")]),
            "{}",
            l.name
        );
        // stack sets a session set reads through `has`, and one that is of
        // it: each starts a chain of its own; the session set is untouched
        let (paths, doc) = drafted(
            &mut l,
            json!({
                "t1": {"grain": "stack", "where": [mprage]},
                "spoiled": {"grain": "stack", "where": [is("technique", "SPGRE")]},
                "visits": {"grain": "session", "has": [
                    {"set": "t1", "min": 1},
                    {"set": "spoiled", "min": 1}
                ]},
                "theirs": {"grain": "stack", "of": "visits", "where": [mprage]}
            }),
            "theirs",
        );
        assert_eq!(
            paths,
            [
                "sets.spoiled.where[1]",
                "sets.t1.where[1]",
                "sets.theirs.where[1]"
            ],
            "{}",
            l.name
        );
        assert!(doc["sets"]["visits"].get("where").is_none(), "{}", l.name);
        // the result of an algebra is other sets' rows: they get it, it
        // does not
        let (paths, doc) = drafted(
            &mut l,
            json!({
                "a": {"grain": "stack", "where": [mprage]},
                "b": {"grain": "stack", "where": [is("technique", "SPGRE")]},
                "either": {"grain": "stack", "algebra": {"op": "union", "sets": ["a", "b"]}}
            }),
            "either",
        );
        assert_eq!(paths, ["sets.a.where[1]", "sets.b.where[1]"], "{}", l.name);
        assert!(doc["sets"]["either"].get("where").is_none(), "{}", l.name);
        // the stages of a pipeline are a chain: the first stage has it
        let text = json!({
            "ast_version": 1, "name": "stages",
            "pipeline": [
                {"grain": "stack", "where": [is("body_region", "head")]},
                {"name": "sharp", "grain": "stack", "where": [mprage]}
            ],
            "out": {"set": "sharp", "level": "count"}
        })
        .to_string();
        let d = draft(&mut l, &text);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        assert_eq!(defaults(&d).len(), 1, "{}", l.name);
        assert_eq!(
            defaults(&d)[0]["path"],
            "pipeline[0].where[1]",
            "{}",
            l.name
        );
        // a document that arrives structured is taken as written
        let scope = quasi();
        let scheme = Scheme::default();
        let s = Setting {
            names: &l.catalog,
            scope: &scope,
            scheme: &scheme,
            principal: "desk",
            bounds: bounds(),
            values_cap: 50,
        };
        let text = scans(json!([is("technique", "MPRAGE")]));
        let doc = affordance::post(&mut l.registry, &parse(&text).unwrap(), &s, None).unwrap();
        assert_eq!(
            serde_json::to_value(&doc.ask).unwrap(),
            serde_json::from_str::<Value>(&text).unwrap(),
            "{}",
            l.name
        );
    }
}

/// The default clause is a clause like any written one to the diagnosis:
/// where it and the set's own value hold together on no stack, the
/// warning names the two, and says to write the disposition clause the
/// words ask for.
#[test]
fn a_set_the_default_empties_is_told_to_write_the_disposition_it_means() {
    for mut l in labs() {
        plant(&mut l);
        // a value only working scans hold
        let only = replant(&mut l, "contrast_mix", |i| {
            Some(if i % 20 == 0 {
                "T2like_T1"
            } else {
                "PD_T2star"
            })
        });
        let kinds = replant(&mut l, "disposition", |i| {
            Some(if i % 20 == 0 {
                "working_scan"
            } else {
                "acquisition"
            })
        });
        let d = draft(&mut l, &scans(json!([is("contrast_mix", "T2like_T1")])));
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        assert_eq!(defaults(&d).len(), 1, "{}", l.name);
        assert_eq!(
            d.diagnosis.zero_rows.as_deref(),
            Some(
                "no stack of scans survives where disposition among [acquisition, \
                 scanner_derived] (or none yet); everything after it is empty"
            ),
            "{}",
            l.name
        );
        let warned: Vec<_> = d
            .diagnosis
            .warnings
            .iter()
            .filter(|w| w.code.name() == "emptied_by")
            .collect();
        assert_eq!(warned.len(), 1, "{}: {:?}", l.name, d.diagnosis.warnings);
        assert_eq!(warned[0].path, "sets.scans.where[0]", "{}", l.name);
        assert_eq!(only["T2like_T1"], kinds["working_scan"], "{}", l.name);
        // what the set holds without its own clause: the default kinds
        let acquisitions = count(&mut l, &scans(json!([default_clause()])));
        assert!(acquisitions >= kinds["acquisition"], "{}", l.name);
        assert_eq!(
            warned[0].message,
            format!(
                "scans keeps no stack: contrast_mix = T2like_T1 and disposition among \
                 [acquisition, scanner_derived] (or none yet) hold together on no stack. \
                 Without contrast_mix = T2like_T1, scans holds {acquisitions} stacks."
            ),
            "{}",
            l.name
        );
        assert_eq!(
            warned[0].next,
            "a question counts acquisition and scanner_derived of disposition unless it says: \
             where the words ask for another kind on scans, write the disposition clause they \
             ask for in scans, for example [\"in\", {unset: true}, [\"axis\", {}, \
             \"disposition\"], [\"acquisition\", \"scanner_derived\", \"reformat\"]]; where they \
             do not, no stack holds both and the answer stands",
            "{}",
            l.name
        );
        // drawn from a shared set that carries the default, the warning is
        // at the default's own clause there, with the same step
        let text = json!({
            "ast_version": 1, "name": "shared",
            "sets": {
                "base": {"grain": "stack", "where": [is("post_contrast", "not_given")]},
                "item": {"grain": "stack", "from": "base", "where": [is("contrast_mix", "T2like_T1")]}
            },
            "out": {"set": "item", "level": "count"}
        })
        .to_string();
        replant(&mut l, "post_contrast", |_| Some("not_given"));
        let d = draft(&mut l, &text);
        assert!(d.diagnosis.valid, "{}: {:?}", l.name, d.diagnosis.issues);
        let warned: Vec<_> = d
            .diagnosis
            .warnings
            .iter()
            .filter(|w| w.code.name() == "emptied_by")
            .collect();
        assert_eq!(warned.len(), 1, "{}: {:?}", l.name, d.diagnosis.warnings);
        assert_eq!(warned[0].path, "sets.base.where[1]", "{}", l.name);
        assert!(
            warned[0]
                .next
                .starts_with("a question counts acquisition and scanner_derived of disposition"),
            "{}: {}",
            l.name,
            warned[0].next
        );
    }
}

/// A move writes for a person what a draft writes from their words: a
/// stack set a move adds gets the default clause, a set given a clause
/// that names its own kinds loses it, and a clause that takes a kind away
/// leaves it. The sets touched are answered with their sentences.
#[test]
fn a_move_that_adds_a_stack_set_writes_the_default_as_a_draft_does() {
    for mut l in labs() {
        plant(&mut l);
        let scope = quasi();
        let scheme = Scheme::default();
        // one move on a stored document, by its kind and its arguments
        let moved = |l: &mut Lab, document: i64, set: &str, kind: Kind, with: &[(&str, Value)]| {
            let s = Setting {
                names: &l.catalog,
                scope: &scope,
                scheme: &scheme,
                principal: "desk",
                bounds: bounds(),
                values_cap: 50,
            };
            let doc = document::get(l.registry.store(), document)
                .unwrap()
                .unwrap();
            let epoch = l.registry.meta().epoch;
            let opts = affordance::options(epoch, &doc.ask, Some(set), &s).unwrap();
            // the move of the kind whose holes take what is given: an axis
            // has a move of its own
            let m = opts
                .moves
                .iter()
                .find(|m| {
                    m.kind == kind
                        && with.iter().all(|(k, v)| {
                            m.holes.iter().any(|h| {
                                h.name == *k && (h.fillers.is_empty() || h.fillers.contains(v))
                            })
                        })
                })
                .unwrap_or_else(|| panic!("{}: no {kind:?} move on {set}", l.name));
            let call = MoveCall {
                move_id: m.id,
                args: with
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
            };
            let applied: Applied = affordance::apply(
                &mut l.registry,
                document,
                epoch,
                &opts.token,
                set,
                std::slice::from_ref(&call),
                &s,
            )
            .unwrap_or_else(|e| panic!("{}: {kind:?} on {set}: {e}", l.name));
            let stored = document::get(l.registry.store(), applied.document)
                .unwrap()
                .unwrap();
            (applied, serde_json::to_value(&stored.ask).unwrap())
        };
        // a document as a card opens it: people, and a stack set written
        // by hand of every kind, which is taken as written
        let text = json!({
            "ast_version": 1, "name": "card",
            "sets": {
                "people": {"grain": "subject"},
                "raw": {"grain": "stack", "where": [is("technique", "MPRAGE")]}
            },
            "out": {"set": "people", "level": "count"}
        })
        .to_string();
        let card = {
            let s = Setting {
                names: &l.catalog,
                scope: &scope,
                scheme: &scheme,
                principal: "desk",
                bounds: bounds(),
                values_cap: 50,
            };
            affordance::post(&mut l.registry, &parse(&text).unwrap(), &s, None).unwrap()
        };
        // a new stack set: the default clause is its first, and its
        // sentence says it
        let (applied, doc) = moved(
            &mut l,
            card.id,
            "people",
            Kind::AddSet,
            &[("name", json!("scans")), ("grain", json!("stack"))],
        );
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([default_clause()]),
            "{}",
            l.name
        );
        assert_eq!(
            doc["sets"]["raw"]["where"],
            json!([is("technique", "MPRAGE")]),
            "{}",
            l.name
        );
        let said = applied
            .changed
            .iter()
            .find(|(set, _)| set == "scans")
            .map(|(_, sentence)| sentence.clone())
            .unwrap_or_else(|| panic!("{}: {:?}", l.name, applied.changed));
        assert!(
            said.contains("disposition among [acquisition, scanner_derived] (or none yet)"),
            "{}: {said}",
            l.name
        );
        // a clause on another axis leaves it
        let (applied, doc) = moved(
            &mut l,
            applied.document,
            "scans",
            Kind::AddAxisWhere,
            &[("axis", json!("technique")), ("value", json!("MPRAGE"))],
        );
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([default_clause(), is("technique", "MPRAGE")]),
            "{}",
            l.name
        );
        // a set drawn from one that carries the clause gets none of its
        // own; one drawn from the set written by hand, which names no
        // disposition, gets it itself, and that set is left as written
        let (applied, doc) = moved(
            &mut l,
            applied.document,
            "scans",
            Kind::AddSet,
            &[
                ("name", json!("sharp")),
                ("grain", json!("stack")),
                ("source", json!("from:scans")),
            ],
        );
        assert!(
            doc["sets"]["sharp"].get("where").is_none(),
            "{}: {doc}",
            l.name
        );
        let (applied, doc) = moved(
            &mut l,
            applied.document,
            "raw",
            Kind::AddSet,
            &[
                ("name", json!("drawn")),
                ("grain", json!("stack")),
                ("source", json!("from:raw")),
            ],
        );
        assert_eq!(
            doc["sets"]["drawn"]["where"],
            json!([default_clause()]),
            "{}",
            l.name
        );
        assert_eq!(
            doc["sets"]["raw"]["where"],
            json!([is("technique", "MPRAGE")]),
            "{}",
            l.name
        );
        // a set at another grain gets nothing
        let (applied, doc) = moved(
            &mut l,
            applied.document,
            "people",
            Kind::AddSet,
            &[("name", json!("visits")), ("grain", json!("session"))],
        );
        assert!(doc["sets"]["visits"].get("where").is_none(), "{}", l.name);
        // a kind taken away leaves the default where it is
        let (applied, doc) = moved(
            &mut l,
            applied.document,
            "scans",
            Kind::ExcludeScenario,
            &[("disposition", json!("scout"))],
        );
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([
                default_clause(),
                is("technique", "MPRAGE"),
                ["<>", {}, ["axis", {}, "disposition"], "scout"]
            ]),
            "{}",
            l.name
        );
        // the person says which kind is meant: the default steps aside
        let (_, doc) = moved(
            &mut l,
            applied.document,
            "scans",
            Kind::AddAxisWhere,
            &[("axis", json!("disposition")), ("value", json!("reformat"))],
        );
        assert_eq!(
            doc["sets"]["scans"]["where"],
            json!([
                is("technique", "MPRAGE"),
                ["<>", {}, ["axis", {}, "disposition"], "scout"],
                is("disposition", "reformat")
            ]),
            "{}",
            l.name
        );
    }
}
