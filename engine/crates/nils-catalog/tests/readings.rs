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
use nils_ask::affordance::{self, Drafted, Setting};
use nils_ask::exec::Bounds;
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
    serde_json::to_value(&doc.ask).unwrap()
}

/// The readings among a draft's repairs, as they are served.
fn readings(d: &Drafted) -> Vec<Value> {
    d.repairs
        .iter()
        .filter(|r| r.read.is_some())
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
        let hash = prepare(parse(&read).unwrap(), &l.catalog, &quasi())
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
        for r in &d.repairs {
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
        assert_eq!(repairs.len(), 2, "{}", l.name);
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
