// SPDX-License-Identifier: AGPL-3.0-only

//! The keyword clauses' words, indexed once per pack.
//!
//! A keyword clause holds when one of its words is a substring of a text,
//! both lowercased, and cites the first of its words in list order that is
//! (v0's rule). Asked word by word, a stack's text is searched once for every
//! word of every clause, several hundred times per stack, and that was most
//! of what classifying one cost. Here the words of every clause that reads
//! one text are one Aho-Corasick automaton, a stack's text is read through
//! it once, and a clause then looks its words up by number.
//!
//! The answer is the same by construction: an overlapping search reports
//! every occurrence of every word, so a word is marked exactly when the text
//! contains it, and the clause still takes its own words in its own order.

use std::collections::HashMap;

use aho_corasick::AhoCorasick;

use crate::rules::{Clause, RuleSet};

/// One automaton per text a keyword clause searches.
#[derive(Clone, Default)]
pub struct Index {
    texts: Vec<Text>,
}

#[derive(Clone)]
struct Text {
    /// The field the clauses search, as they name it.
    field: usize,
    /// The distinct lowercased words, numbered as the clauses' `ids` are.
    words: usize,
    automaton: AhoCorasick,
}

impl Index {
    /// Number every keyword clause's words, lowercased, by the text it reads,
    /// and build one automaton per text. Each clause's `ids` is filled here,
    /// so this runs after the last change to any list (the overlay's).
    pub fn build(rule_sets: &mut [RuleSet]) -> Index {
        // per field: its words in first-seen order, and their numbers
        let mut fields: Vec<(usize, Vec<String>, HashMap<String, usize>)> = Vec::new();
        for set in rule_sets.iter_mut() {
            for rule in &mut set.rules {
                for c in &mut rule.clauses {
                    let Clause::Keywords {
                        field, list, ids, ..
                    } = c
                    else {
                        continue;
                    };
                    let at = match fields.iter().position(|(f, _, _)| f == field) {
                        Some(at) => at,
                        None => {
                            fields.push((*field, Vec::new(), HashMap::new()));
                            fields.len() - 1
                        }
                    };
                    let (_, words, number) = &mut fields[at];
                    *ids = list
                        .iter()
                        .map(|w| {
                            let w = w.to_lowercase();
                            *number.entry(w.clone()).or_insert_with(|| {
                                words.push(w);
                                words.len() - 1
                            })
                        })
                        .collect();
                }
            }
        }
        let mut texts = Vec::with_capacity(fields.len());
        for (field, words, _) in fields {
            match AhoCorasick::new(&words) {
                Ok(automaton) => texts.push(Text {
                    field,
                    words: words.len(),
                    automaton,
                }),
                // An automaton too large to build: its clauses keep no
                // numbers and are searched word by word, as before.
                Err(_) => {
                    for set in rule_sets.iter_mut() {
                        for rule in &mut set.rules {
                            for c in &mut rule.clauses {
                                if let Clause::Keywords { field: f, ids, .. } = c
                                    && *f == field
                                {
                                    ids.clear();
                                }
                            }
                        }
                    }
                }
            }
        }
        Index { texts }
    }

    /// Where the automaton of a field sits, if the field has one.
    pub fn slot(&self, field: usize) -> Option<usize> {
        self.texts.iter().position(|t| t.field == field)
    }

    /// How many texts have an automaton: the slots run below this.
    pub fn slots(&self) -> usize {
        self.texts.len()
    }

    /// Which of a slot's words a lowercased text contains, by number.
    pub fn scan(&self, slot: usize, lowered: &str) -> Vec<bool> {
        let t = &self.texts[slot];
        let mut hit = vec![false; t.words];
        for m in t.automaton.find_overlapping_iter(lowered) {
            hit[m.pattern().as_usize()] = true;
        }
        hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(words: &[&str]) -> (Index, Vec<usize>) {
        let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        let automaton = AhoCorasick::new(&words).unwrap();
        (
            Index {
                texts: vec![Text {
                    field: 7,
                    words: words.len(),
                    automaton,
                }],
            },
            (0..words.len()).collect(),
        )
    }

    #[test]
    fn every_word_a_text_contains_is_marked_overlapping_or_nested() {
        let words = ["t2", "t2 flair", "flair", "air", "dwi", "", "ä"];
        let (index, _) = text(&words);
        for hay in ["ax t2 flair", "dwi", "", "t2*", "fläir", "tä"] {
            let hit = index.scan(0, hay);
            for (i, w) in words.iter().enumerate() {
                assert_eq!(hit[i], hay.contains(w), "{w:?} in {hay:?}");
            }
        }
    }
}
