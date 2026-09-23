// SPDX-License-Identifier: AGPL-3.0-only

//! Record 41, S2: every rule's vote. Hearing every rule changes no verdict,
//! the votes hold the rule that decided each axis, and they hold the rules
//! an ordered set never reached.

use std::path::PathBuf;

use nils_pack::corpus::{self, Case};
use nils_pack::{Evaluated, Pack, Verdict, Vote};

fn mri() -> Pack {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    nils_pack::load(&dir, None).expect("the MRI pack loads")
}

fn cases() -> Vec<Case> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    corpus::read(&dir)
        .expect("the corpus reads")
        .into_iter()
        .map(|(_, c)| c)
        .collect()
}

fn evaluated<'a>(pack: &'a Pack, case: &'a Case) -> Evaluated<'a> {
    let mut private = vec![String::new(); pack.ingest.len()];
    for (k, v) in &case.private {
        let i = pack
            .ingest
            .iter()
            .position(|x| x.name == *k)
            .expect("an ingested field");
        private[i] = v.clone();
    }
    Evaluated::with_private(pack, &case.stack, private)
}

/// What the rules decided, as the seed of the disposition phase.
fn seed(pack: &Pack, verdict: &Verdict) -> Vec<Vec<String>> {
    let mut seed = vec![Vec::new(); pack.axes.len()];
    for a in &verdict.axes {
        seed[pack.axis_index(&a.axis).expect("an axis")] = a.values.clone();
    }
    seed
}

/// The verdict without its votes, which is what the other half must equal.
fn without_votes(v: &Verdict) -> Verdict {
    Verdict {
        votes: Vec::new(),
        ..v.clone()
    }
}

/// Every evidence row a rule wrote has its vote: the same rule, value and
/// tier on the same axis. A default is not a rule and casts none.
fn covers(verdict: &Verdict, name: &str) {
    for e in verdict.evidence.iter().filter(|e| e.tier != "default") {
        assert!(
            verdict.votes.iter().any(|v| v.axis == e.axis
                && v.value == e.value
                && v.rule_set == e.rule_set
                && v.rule == e.rule
                && v.tier == e.tier),
            "{name}: {} = {} by {}/{} ({}) has no vote",
            e.axis,
            e.value,
            e.rule_set,
            e.rule,
            e.tier
        );
    }
}

#[test]
fn hearing_every_rule_changes_no_verdict_and_holds_every_decider() {
    let pack = mri();
    let cases = cases();
    assert!(cases.len() >= 15, "{} cases", cases.len());
    let mut decided = 0;
    for case in &cases {
        let e = evaluated(&pack, case);
        let plain = e.classify();
        let heard = e.classify_with_votes();
        assert!(
            plain.votes.is_empty(),
            "{}: a plain verdict votes",
            case.name
        );
        assert_eq!(
            plain,
            without_votes(&heard),
            "{}: the verdict moved",
            case.name
        );
        covers(&heard, &case.name);
        decided += heard
            .evidence
            .iter()
            .filter(|e| e.tier != "default")
            .count();

        let seed = seed(&pack, &plain);
        let disposed = e.dispose(&seed);
        let heard_disposed = e.dispose_with_votes(&seed);
        assert_eq!(
            disposed,
            without_votes(&heard_disposed),
            "{}: the disposition moved",
            case.name
        );
        covers(&heard_disposed, &case.name);

        // And what the case's author wrote down is what the heard verdict
        // says, as the corpus check at load asks of the plain one.
        for (axis, want) in &case.axes {
            let got = match heard_disposed.axis(axis) {
                Some(a) => a.stored(),
                None => heard.stored(axis),
            };
            assert_eq!(&got, want, "{}: {axis}", case.name);
        }
    }
    assert!(decided > 100, "{decided} axes decided by a rule");
}

#[test]
fn every_vote_names_a_voter_the_pack_declares() {
    let pack = mri();
    let voters = nils_pack::voters(&pack);
    assert!(voters.len() > 200, "{} voters", voters.len());
    for case in cases() {
        for v in evaluated(&pack, &case).classify_with_votes().votes {
            assert!(
                voters.iter().any(|w| w.rule_set == v.rule_set
                    && w.rule == v.rule
                    && w.clause == v.clause
                    && w.axis == v.axis
                    && w.tier == v.tier),
                "{}: {v:?} is no voter of the pack",
                case.name
            );
        }
    }
}

fn has(votes: &[Vote], axis: &str, rule_set: &str, rule: &str, value: &str) -> bool {
    votes
        .iter()
        .any(|v| v.axis == axis && v.rule_set == rule_set && v.rule == rule && v.value == value)
}

/// The corpus's first case, a Siemens MPRAGE. The technique axis decides it
/// by its exclusive flag and stops there; the base contrast is decided by
/// the rule that reads the technique, and the rules behind it that would
/// have said T1w from the words and the physics are heard as well.
#[test]
fn an_mprage_is_heard_past_the_rule_that_decided_it() {
    let pack = mri();
    let case = cases()
        .into_iter()
        .find(|c| c.name.starts_with("a Siemens MPRAGE"))
        .expect("the MPRAGE case");
    let verdict = evaluated(&pack, &case).classify_with_votes();
    let votes = &verdict.votes;
    assert_eq!(verdict.stored("technique"), "MPRAGE");
    assert_eq!(verdict.stored("base"), "T1w");
    // The decider, cited by the evidence, is a vote.
    assert!(
        has(votes, "base", "base", "technique:MPRAGE", "T1w"),
        "{votes:#?}"
    );
    // A rule of the same ordered set, never reached, is a vote too: the
    // description says T1.
    assert!(
        has(votes, "base", "base", "keywords:T1w", "T1w"),
        "{votes:#?}"
    );
    // And the evidence cites one rule for base where the votes hold more.
    let cited = verdict.evidence.iter().filter(|e| e.axis == "base").count();
    let heard = votes.iter().filter(|v| v.axis == "base").count();
    assert!(heard > cited, "base: {heard} votes, {cited} cited");
    // No vote is cast by a rule whose set was not entered.
    for v in votes {
        let set = pack
            .rule_sets
            .iter()
            .find(|s| s.name == v.rule_set)
            .expect("a set");
        if set.enter_when.is_some() {
            assert!(verdict.entered.contains(&v.rule_set), "{v:?}");
        }
    }
}

/// What hearing every rule costs the evaluator alone, over the corpus's
/// stacks, which are chosen to reach every corner of the pack rather than
/// to be typical. Not a gate; run it by name with `--ignored --nocapture`.
#[test]
#[ignore]
fn what_the_votes_cost_the_evaluator() {
    let pack = mri();
    let cases = cases();
    let rounds = 200;
    let time = |votes: bool| {
        let started = std::time::Instant::now();
        let mut n = 0usize;
        for _ in 0..rounds {
            for case in &cases {
                let e = evaluated(&pack, case);
                let v = if votes {
                    e.classify_with_votes()
                } else {
                    e.classify()
                };
                n += v.votes.len();
            }
        }
        (started.elapsed().as_secs_f64(), n)
    };
    let (plain, _) = time(false);
    let (heard, votes) = time(true);
    let stacks = rounds * cases.len();
    eprintln!(
        "{stacks} evaluations: without votes {:.1} us each, with {:.1} us each ({:.2}x), {:.1} votes per stack",
        plain * 1e6 / stacks as f64,
        heard * 1e6 / stacks as f64,
        heard / plain,
        votes as f64 / stacks as f64
    );
}
