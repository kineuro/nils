// SPDX-License-Identifier: AGPL-3.0-only

//! A pass over a corpus: the reference the pack declared, the bin, the
//! widening, the minimum and the tie.

use std::path::Path;

use nils_pack::pass::{Corpus, run_vote};
use nils_pack::{Pack, Stack};

fn mri() -> Pack {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    nils_pack::load(&dir, None).expect("the MRI pack loads")
}

/// One stack, by the fields a vote reads.
fn stack(tr: f64, te: f64, sequence: &str) -> Stack {
    let mut s = Stack::new();
    s.set("repetition_time", nils_pack::stack::Value::Num(Some(tr)))
        .unwrap();
    s.set("echo_time", nils_pack::stack::Value::Num(Some(te)))
        .unwrap();
    s.set("n_instances", nils_pack::stack::Value::Num(Some(20.0)))
        .unwrap();
    s.set(
        "scanning_sequence",
        nils_pack::stack::Value::Text(Some(sequence)),
    )
    .unwrap();
    s.set("modality", nils_pack::stack::Value::Text(Some("MR")))
        .unwrap();
    s
}

/// A corpus of stacks with the axes already decided, as a pass finds them.
fn corpus(pack: &Pack, rows: &[(Stack, &str, &str, &str)]) -> Corpus {
    let mut c = Corpus::new(pack);
    for (i, (s, base, technique, intent)) in rows.iter().enumerate() {
        c.push(
            i as i64 + 1,
            |f| s.as_text(f).into_owned(),
            |a| match pack.axes[a].name.as_str() {
                "base" => base.to_string(),
                "technique" => technique.to_string(),
                "directory_type" => intent.to_string(),
                _ => String::new(),
            },
        );
    }
    c
}

/// The bug this test exists for: a key dimension read as text from a stack
/// that keeps numbers as numbers came back empty, every stack fell into the
/// same bin, and the vote answered the whole corpus with whatever was
/// commonest. The bins have to separate stacks that differ.
#[test]
fn a_stack_votes_among_the_stacks_whose_physics_is_its_own() {
    let pack = mri();
    let pass = &pack.passes[0];
    let vote = pass.vote().expect("the physics vote");

    let mut rows: Vec<(Stack, &str, &str, &str)> = Vec::new();
    // Twenty T2w spin echoes at a long TR and TE,
    for _ in 0..20 {
        rows.push((stack(4000.0, 90.0, "SE"), "T2w", "TSE", "anat"));
    }
    // twenty T1w gradient echoes at a short one,
    for _ in 0..20 {
        rows.push((stack(10.0, 3.0, "GR"), "T1w", "FLASH", "anat"));
    }
    // and one stack of each shape with nothing decided about it.
    rows.push((stack(4000.0, 90.0, "SE"), "", "", "anat"));
    rows.push((stack(10.0, 3.0, "GR"), "", "", "anat"));

    let c = corpus(&pack, &rows);
    let (answers, pool, _) = run_vote(&pack, pass, vote, &c, false);
    assert_eq!(pool, 40, "the reference is what the filter admits");
    assert_eq!(answers.len(), 2, "and the targets are the two gaps");

    let said = |at: usize| -> Vec<String> {
        answers
            .iter()
            .find(|a| a.at == at)
            .map(|a| a.writes.iter().map(|(_, v)| v.clone()).collect())
            .unwrap_or_default()
    };
    assert_eq!(
        said(40),
        vec!["T2w", "TSE"],
        "the long TR votes with its own"
    );
    assert_eq!(said(41), vec!["T1w", "FLASH"], "and the short one with its");
    for a in &answers {
        assert_eq!(a.outcome.method, "exact_bin");
        assert_eq!(a.outcome.matches, 20, "all twenty agreed");
        assert_eq!(a.outcome.neighbours, 20, "and nobody else was in the bin");
    }
}

/// A vote that cannot tell two answers apart says so, where v0 would take
/// whichever the database returned first.
#[test]
fn an_even_split_decides_nothing() {
    let pack = mri();
    let pass = &pack.passes[0];
    let vote = pass.vote().expect("the physics vote");

    let mut rows: Vec<(Stack, &str, &str, &str)> = Vec::new();
    for _ in 0..10 {
        rows.push((stack(4000.0, 90.0, "SE"), "T2w", "TSE", "anat"));
    }
    for _ in 0..10 {
        rows.push((stack(4000.0, 90.0, "SE"), "PDw", "TSE", "anat"));
    }
    rows.push((stack(4000.0, 90.0, "SE"), "", "", "anat"));

    let c = corpus(&pack, &rows);
    let (answers, _, _) = run_vote(&pack, pass, vote, &c, false);
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].outcome.method, "tie");
    assert!(answers[0].writes.is_empty(), "and writes nothing");
}

/// Two neighbours is what the pack asks for; one is not enough.
#[test]
fn one_neighbour_is_not_a_vote() {
    let pack = mri();
    let pass = &pack.passes[0];
    let vote = pass.vote().expect("the physics vote");
    let rows: Vec<(Stack, &str, &str, &str)> = vec![
        (stack(4000.0, 90.0, "SE"), "T2w", "TSE", "anat"),
        (stack(4000.0, 90.0, "SE"), "", "", "anat"),
    ];
    let c = corpus(&pack, &rows);
    let (answers, _, _) = run_vote(&pack, pass, vote, &c, false);
    assert_eq!(answers[0].outcome.method, "insufficient_matches");
    assert!(answers[0].writes.is_empty());
}

/// The same corpus twice gives the same answers, which is the property v0
/// cannot have: its reference is whatever the database held at the time.
#[test]
fn the_same_reference_gives_the_same_answer_twice() {
    let pack = mri();
    let pass = &pack.passes[0];
    let vote = pass.vote().expect("the physics vote");
    let mut rows: Vec<(Stack, &str, &str, &str)> = Vec::new();
    for i in 0..30 {
        let te = 80.0 + f64::from(i % 3);
        rows.push((stack(4000.0, te, "SE"), "T2w", "TSE", "anat"));
    }
    rows.push((stack(4000.0, 90.0, "SE"), "", "", "anat"));
    let c = corpus(&pack, &rows);
    let (first, _, _) = run_vote(&pack, pass, vote, &c, false);
    let (again, _, _) = run_vote(&pack, pass, vote, &c, false);
    assert_eq!(first.len(), again.len());
    for (a, b) in first.iter().zip(&again) {
        assert_eq!(a.outcome.method, b.outcome.method);
        assert_eq!(a.writes, b.writes);
        assert_eq!(a.outcome.matches, b.outcome.matches);
    }
}
