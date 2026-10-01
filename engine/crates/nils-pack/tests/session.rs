// SPDX-License-Identifier: AGPL-3.0-only

//! Record 53: the session pass as the MRI pack declares it, decided over
//! invented stacks (`nils_pack::session::decide`), the same code the engine
//! runs over the registry and a replay runs over packets; and pack contract
//! 8's three additions refused where a pack gets them wrong.

use std::path::{Path, PathBuf};

use nils_pack::session::{InForce, Sib, decide};
use nils_pack::stack::{Stack, Value};

fn mri() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri")
}

fn stack(fields: &[(&str, &str)]) -> Stack {
    let mut s = Stack::new();
    for (k, v) in fields {
        s.set(k, Value::Text(Some(v))).unwrap();
    }
    s
}

/// A GE susceptibility image, unprefixed, as the route's fallback leaves it.
fn silent(number: &str) -> Stack {
    stack(&[
        ("modality", "MR"),
        ("manufacturer", "GE MEDICAL SYSTEMS"),
        ("image_type", "ORIGINAL\\PRIMARY\\OTHER"),
        ("scanning_sequence", "GR"),
        ("mr_acquisition_type", "3D"),
        ("text_series_description", "Ax SWAN"),
        ("orientation", "Axial"),
        ("rows", "256"),
        ("columns", "256"),
        ("n_slices", "60"),
        ("series_number", number),
    ])
}

fn computed(number: &str, name: &str) -> Stack {
    let mut s = silent(number);
    s.set("text_series_description", Value::Text(Some(name)))
        .unwrap();
    s
}

/// The axes in force as the rules leave the silent stack: provenance from
/// the name, construct and base from the route's fallback.
fn in_force(pack: &nils_pack::Pack, construct_tier: &str) -> Vec<InForce> {
    pack.axes
        .iter()
        .map(|a| match a.name.as_str() {
            "provenance" => InForce {
                values: vec!["SWIRecon".into()],
                tier: "keywords".into(),
                confidence: 0.85,
            },
            "construct" => InForce {
                values: vec!["SWI".into()],
                tier: construct_tier.into(),
                confidence: 0.7,
            },
            "base" => InForce {
                values: vec!["SWI".into()],
                tier: "stated".into(),
                confidence: 0.7,
            },
            _ => InForce::default(),
        })
        .collect()
}

fn sib(id: i64, stack: Stack, same_series: bool, frame: Option<bool>) -> Sib {
    Sib {
        id,
        stack,
        private: Vec::new(),
        same_series,
        same_frame_of_reference: frame,
    }
}

#[test]
fn the_mri_session_pass_decides_only_from_a_related_computed_output() {
    let pack = nils_pack::load(&mri(), None).expect("the MRI pack loads");
    let pass = pack
        .passes
        .iter()
        .find(|p| p.session().is_some())
        .expect("a session pass");
    let session = pass.session().unwrap();
    let target = pass.target.as_ref();
    let me = silent("5");
    let axis = |n: &str| pack.axes.iter().position(|a| a.name == n).unwrap();

    // Beside GE's computed output of series 5, numbered 500: the magnitude.
    let a = decide(
        &pack,
        target,
        session,
        &me,
        &[],
        &in_force(&pack, "stated"),
        &[sib(7, computed("500", "SWI: Ax SWAN"), false, Some(true))],
    )
    .expect("the rule holds");
    assert_eq!(a.rule, "ge_swan_beside_its_computed_output");
    assert_eq!(a.cited, [7]);
    assert!(a.held.is_none());
    assert_eq!(
        a.writes,
        [
            (axis("provenance"), vec!["RawRecon".to_string()]),
            (axis("construct"), vec!["Magnitude".to_string()]),
            (axis("base"), vec!["T2*w".to_string()]),
        ]
    );
    assert!((a.confidence - 0.65).abs() < 1e-9);

    // Next to it too (within one).
    assert!(
        decide(
            &pack,
            target,
            session,
            &me,
            &[],
            &in_force(&pack, "stated"),
            &[sib(7, computed("6", "mIP: Ax SWAN"), false, Some(true))],
        )
        .is_some()
    );

    // Nothing holds: no sibling, a sibling of the stack's own series, an
    // unrelated series number, another frame of reference or an unknown
    // one, another geometry, an unprefixed sibling.
    for (why, sibs) in [
        ("alone", vec![]),
        (
            "own series",
            vec![sib(7, computed("5", "SWI: Ax SWAN"), true, Some(true))],
        ),
        (
            "unrelated number",
            vec![sib(7, computed("9", "SWI: Ax SWAN"), false, Some(true))],
        ),
        (
            "another frame",
            vec![sib(7, computed("500", "SWI: Ax SWAN"), false, Some(false))],
        ),
        (
            "an unknown frame",
            vec![sib(7, computed("500", "SWI: Ax SWAN"), false, None)],
        ),
        ("another geometry", {
            let mut s = computed("500", "SWI: Ax SWAN");
            s.set("rows", Value::Text(Some("512"))).unwrap();
            vec![sib(7, s, false, Some(true))]
        }),
        (
            "unprefixed",
            vec![sib(7, computed("500", "Ax SWAN"), false, Some(true))],
        ),
    ] {
        assert_eq!(
            decide(
                &pack,
                target,
                session,
                &me,
                &[],
                &in_force(&pack, "stated"),
                &sibs
            ),
            None,
            "{why}"
        );
    }

    // The header decided the construct: the rule holds and changes nothing,
    // and says why.
    let held = decide(
        &pack,
        target,
        session,
        &me,
        &[],
        &in_force(&pack, "exclusive"),
        &[sib(7, computed("500", "SWI: Ax SWAN"), false, Some(true))],
    )
    .expect("the rule holds");
    assert!(held.writes.is_empty());
    assert!(
        held.held
            .as_deref()
            .unwrap()
            .contains("construct is SWI by exclusive"),
        "{held:?}"
    );

    // Not a target: a prefixed stack itself, and a stack whose construct
    // the route did not leave at SWI.
    let prefixed = computed("500", "SWI: Ax SWAN");
    assert_eq!(
        decide(
            &pack,
            target,
            session,
            &prefixed,
            &[],
            &in_force(&pack, "stated"),
            &[sib(7, silent("5"), false, Some(true))],
        ),
        None
    );
    let mut no_swi = in_force(&pack, "stated");
    no_swi[axis("construct")].values = vec!["Magnitude".into()];
    assert_eq!(
        decide(
            &pack,
            target,
            session,
            &me,
            &[],
            &no_swi,
            &[sib(7, computed("500", "SWI: Ax SWAN"), false, Some(true))],
        ),
        None,
        "the route did not leave the construct at SWI"
    );

    // Nor a GE susceptibility image that is not named SWAN, beside the
    // same computed output: only a SWAN acquisition is read this way.
    for name in ["Ax QSM 3D", "Ax susceptibility"] {
        assert_eq!(
            decide(
                &pack,
                target,
                session,
                &computed("5", name),
                &[],
                &in_force(&pack, "stated"),
                &[sib(7, computed("500", "SWI: Ax SWAN"), false, Some(true))],
            ),
            None,
            "{name}"
        );
    }
}

/// The MRI pack, copied, with `edit` applied to one of its files.
fn edited(file: &str, edit: impl Fn(&str) -> String) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                copy(&p, &to.join(e.file_name()));
            } else {
                std::fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let to = std::env::temp_dir().join(format!(
        "nils-session-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&to);
    copy(&mri(), &to);
    let text = std::fs::read_to_string(to.join(file)).unwrap();
    let changed = edit(&text);
    assert_ne!(changed, text, "the edit of {file} changed nothing");
    std::fs::write(to.join(file), changed).unwrap();
    to
}

fn refused(dir: &Path) -> String {
    let e = match nils_pack::load(dir, None) {
        Ok(_) => panic!("the pack loaded"),
        Err(e) => e.to_string(),
    };
    let _ = std::fs::remove_dir_all(dir);
    e
}

#[test]
fn contract_8_is_refused_where_a_pack_gets_it_wrong() {
    // A sibling seen by what was decided of it.
    let e = refused(&edited("passes/session.yml", |t| {
        t.replace(
            "          - {text: manufacturer, prefix: ge}\n          - {text: search_text",
            "          - {axis: construct, is: SWI}\n          - {text: search_text",
        )
    }));
    assert!(
        e.contains("reads an axis; a sibling is seen by its header alone"),
        "{e}"
    );

    // A sibling read beyond the fields the pass names.
    let e = refused(&edited("passes/session.yml", |t| {
        t.replace(
            "          - {text: manufacturer, prefix: ge}\n          - {text: search_text",
            "          - {field: echo_time, gt: 10}\n          - {text: search_text",
        )
    }));
    assert!(
        e.contains("reads echo_time, which is not among the session's sibling_fields"),
        "{e}"
    );

    // A pass that writes an axis decided after the passes.
    let e = refused(&edited("passes/session.yml", |t| {
        t.replace(
            "      base: T2starw\n",
            "      base: T2starw\n      role: t1w\n",
        )
    }));
    assert!(e.contains("role is decided after the passes"), "{e}");

    // A derived text named as a sibling field: a packet carries fields.
    let e = refused(&edited("passes/session.yml", |t| {
        t.replace(
            "    - manufacturer\n",
            "    - manufacturer\n    - search_text\n",
        )
    }));
    assert!(e.contains("search_text is a text the pack derives"), "{e}");

    // A private element shown that is not ingested, one of an unknown kind,
    // and one at an identification block.
    let e = refused(&edited("private.yml", |t| {
        t.replace(
            "    - name: ge_private_image_type\n",
            "    - name: ge_scanner_study\n      why: t\n    - name: ge_private_image_type\n",
        )
    }));
    assert!(
        e.contains("ge_scanner_study is not an ingested element"),
        "{e}"
    );
    let e = refused(&edited("private.yml", |t| {
        t.replace(
            "    - name: ge_private_image_type\n",
            "    - name: siemens_sds_0021xx1c\n      why: t\n    - name: ge_private_image_type\n",
        )
    }));
    assert!(e.contains("siemens_sds_0021xx1c is of kind unknown"), "{e}");
    let e = refused(&edited("private.yml", |t| {
        t.replace(
            "  ingest:\n",
            "  ingest:\n    - creator: GEMS_IDEN_01\n      group: 0x0009\n      element: 0x02\n      name: ge_suite_id\n      kind: parameter\n      why: t\n",
        )
        .replace(
            "    - name: ge_private_image_type\n",
            "    - name: ge_suite_id\n      why: t\n    - name: ge_private_image_type\n",
        )
    }));
    assert!(e.contains("which this engine never shows"), "{e}");

    // Each of the three at contract 7, the other two taken out.
    for (keep, says) in [
        (
            "session",
            "session_context is pack contract 8's; this pack declares contract 7",
        ),
        (
            "fallback",
            "a list of values is pack contract 8's; this pack declares contract 7",
        ),
        (
            "shown",
            "shown is pack contract 8's; this pack declares contract 7",
        ),
    ] {
        let dir = edited("pack.yml", |t| t.replace("contract: 8\n", "contract: 7\n"));
        let edit = |name: &str, f: &dyn Fn(&str) -> String| {
            let p = dir.join(name);
            let text = std::fs::read_to_string(&p).unwrap();
            std::fs::write(&p, f(&text)).unwrap();
        };
        if keep != "session" {
            edit("pack.yml", &|t| t.replace("  - passes/session.yml\n", ""));
        }
        if keep != "fallback" {
            edit("picks/main.yml", &|t| {
                t.replace("is: [EPIMix, NeuroMix]}", "is: EPIMix}")
            });
        }
        if keep != "shown" {
            edit("private.yml", &|t| {
                let mut out = Vec::new();
                let mut skipping = false;
                for l in t.lines() {
                    if l == "  shown:" {
                        skipping = true;
                        continue;
                    }
                    if skipping && (l.starts_with("    ") || l.trim().is_empty()) {
                        continue;
                    }
                    skipping = false;
                    out.push(l);
                }
                out.join("\n")
            });
        }
        let e = refused(&dir);
        assert!(e.contains(says), "{keep}: {e}");
    }
}

/// The `quasi` mark 1.0.0-alpha.66 let a `private.shown` entry carry is
/// withdrawn (the ruling of 2026-10-01: a sequence name is shown at every
/// detail). A pack that still carries it, as MRI pack 0.20.0 and 0.20.1 do,
/// loads unchanged, and every element it lists stays shown.
#[test]
fn a_shown_entry_marked_quasi_still_loads_and_is_shown() {
    let dir = edited("private.yml", |t| {
        t.replace(
            "    - name: ge_pulse_sequence_name\n",
            "    - name: ge_pulse_sequence_name\n      quasi: true\n",
        )
    });
    let text = std::fs::read_to_string(dir.join("private.yml")).unwrap();
    assert!(
        text.contains("      quasi: true\n"),
        "the edit did not land"
    );
    let pack = nils_pack::load(&dir, None).unwrap();
    let plain = nils_pack::load(&mri(), None).unwrap();
    assert_eq!(pack.shown, plain.shown);
    assert!(
        pack.shown
            .iter()
            .any(|s| s.name == "ge_pulse_sequence_name")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
