// SPDX-License-Identifier: AGPL-3.0-only

//! The semantic normalizer (`docs/specs/wave2-fingerprint-and-classify.md`,
//! §6.4), which belongs to a pack and not to the fingerprint.
//!
//! Folding text is a fact about text and happens once, at digest. Deciding
//! that `ir` means inversion recovery, that `*` means star, that seventeen
//! words are boilerplate, is a claim about MRI, so it lives here and runs when
//! a pack is loaded. The practical difference: in v0, correcting the token map
//! means re-extracting the archive, because the rewritten text is what was
//! stored; here it is a pack version and a re-classification.
//!
//! Twelve ordered steps, transcribed from v0's `sort/semantic_normalizer.py`.
//! Three of them are load-bearing in ways that are easy to get wrong, and each
//! is called out where it happens.

use std::collections::BTreeMap;

pub struct Normalizer {
    /// The field the result is published as.
    pub into: String,
    /// The fields joined, in order, before anything else happens.
    pub from: Vec<usize>,
    /// Literal substrings removed before normalization, case-sensitively.
    pub raw_removals: Vec<String>,
    /// A character that becomes a word: `*` becomes `star`, so `T2*` is one
    /// token and not two.
    pub meaningful: Vec<(char, String)>,
    /// A character that becomes a space, splitting what it joined.
    pub to_space: Vec<char>,
    /// A character that becomes nothing.
    pub remove: Vec<char>,
    /// Tokens dropped after de-duplication: boilerplate that means nothing.
    pub token_removals: Vec<String>,
    /// Token to canonical form, applied unconditionally.
    pub token_replacements: BTreeMap<String, String>,
    /// Applied after those, reading the token set as it stands then.
    pub conditional: Vec<Conditional>,
}

pub struct Conditional {
    pub canonical: String,
    pub replace: String,
    pub when_has_any: Vec<String>,
    pub when_has_all: Vec<String>,
}

impl Normalizer {
    /// The normalized blob, or `None` when nothing survives.
    pub fn apply(&self, parts: &[&str]) -> Option<String> {
        // The join v0 makes before it normalizes.
        let mut text = String::new();
        for p in parts {
            if p.is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(p);
        }
        if text.is_empty() {
            return None;
        }

        // 0. literal removals, before anything is folded, and case-sensitive:
        // v0's one entry is an uppercase Swedish phrase, which is why the
        // fingerprint keeps case (§4.2).
        for r in &self.raw_removals {
            if text.contains(r.as_str()) {
                text = text.replace(r.as_str(), " ");
            }
        }
        if text.trim().is_empty() {
            return None;
        }

        // 1, 2, 3. characters that become a word, a space, or nothing.
        for (c, w) in &self.meaningful {
            if text.contains(*c) {
                text = text.replace(*c, w);
            }
        }
        for c in &self.to_space {
            if text.contains(*c) {
                text = text.replace(*c, " ");
            }
        }
        for c in &self.remove {
            if text.contains(*c) {
                text = text.replace(*c, "");
            }
        }

        // 4. lower case.
        let lowered = text.to_lowercase();

        // 4.5 and 5. `+` and `-` become tokens of their own, because they
        // carry contrast meaning, and everything else outside the alphabet
        // becomes a space.
        let mut cleaned = String::with_capacity(lowered.len() + 8);
        for c in lowered.chars() {
            match c {
                '+' | '-' => {
                    cleaned.push(' ');
                    cleaned.push(c);
                    cleaned.push(' ');
                }
                'a'..='z' | '0'..='9' | '_' => cleaned.push(c),
                c if c.is_whitespace() => cleaned.push(c),
                _ => cleaned.push(' '),
            }
        }

        // 6. tokenize on space and underscore.
        let mut tokens: Vec<String> = cleaned
            .split(|c: char| c.is_whitespace() || c == '_')
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();

        // 7. de-duplicate, keeping the FIRST occurrence and the order. A
        // description that repeats a word loses the repeat, and a two-word
        // keyword only matches if the survivors end up adjacent.
        let mut seen: Vec<String> = Vec::with_capacity(tokens.len());
        tokens.retain(|t| {
            if seen.contains(t) {
                false
            } else {
                seen.push(t.clone());
                true
            }
        });

        // 7.5 boilerplate out.
        if !self.token_removals.is_empty() {
            tokens.retain(|t| !self.token_removals.contains(t));
        }
        if tokens.is_empty() {
            return None;
        }

        // 8. unconditional replacements.
        for t in &mut tokens {
            if let Some(c) = self.token_replacements.get(t.as_str()) {
                *t = c.clone();
            }
        }

        // 9. conditional replacements, reading the token set as it stands
        // after step 8. v0's one rule looks for a token step 8 has already
        // replaced, so it can never fire; a pack writes the condition against
        // what is there (§6.4, and spikes/pack finding 5).
        for rule in &self.conditional {
            if !tokens.contains(&rule.replace) {
                continue;
            }
            let has = |w: &String| tokens.iter().any(|t| t == w);
            let fire = (!rule.when_has_any.is_empty() && rule.when_has_any.iter().any(has))
                || (!rule.when_has_all.is_empty() && rule.when_has_all.iter().all(has));
            if fire {
                for t in &mut tokens {
                    if *t == rule.replace {
                        *t = rule.canonical.clone();
                    }
                }
            }
        }

        // 10. join.
        if tokens.is_empty() {
            None
        } else {
            Some(tokens.join(" "))
        }
    }
}
