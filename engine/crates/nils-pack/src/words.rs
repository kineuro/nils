// SPDX-License-Identifier: AGPL-3.0-only

//! What the pack's vocabulary knows of a series' own names, and the maker a
//! person calls a scanner by: two of the damaged-header flag's signs
//! (record 56 §3; record 55, ruling 31 (c)).
//!
//! The rules read a series by its names first, so a dataset whose names the
//! pack cannot read is one where the rules fall short: another language,
//! another site's abbreviations, an export that rewrote the names. A
//! series' description and protocol name are folded into words as the pack
//! folds them for its rules (the derived text the pack names, §6.4, with
//! that text's own reading of which maker's protocol name it leaves out),
//! and each word is looked up in everything the pack matches text by: the
//! words of its keyword clauses and of every text condition of its rules,
//! flags, parsers and passes, after any overlay.
//!
//! - **A word** is what the fold leaves with a letter in it and two
//!   characters or more. `512` and `p` are parameters or noise, not words.
//! - **A word is known** when one of the pack's words of three characters or
//!   more lies inside it, as a keyword is matched (`flair` in `t2flair`), or
//!   when it is one of the pack's two-character words exactly (`ax` is never
//!   the inside of `max`).
//! - **A series is unread** when it has words and the pack knows none of
//!   them ([`Read::unread`]): its names tell the rules nothing, and they fall
//!   back to the physics, or to nothing.

use std::collections::BTreeSet;

use aho_corasick::AhoCorasick;

use crate::expr::Expr;
use crate::normalize::Normalizer;
use crate::rules::Clause;
use crate::stack::field_index;
use crate::{Pack, reads};

/// The pack's words, and the fold that turns a series' names into words.
pub struct Vocabulary {
    /// The derived text whose fold is used, by name; none where the pack
    /// derives no text from the series description, and names are folded
    /// plainly (lower case, split on anything but a letter or a digit).
    pub text: Option<String>,
    fold: Option<Normalizer>,
    /// Where the series description and the protocol name sit in the fold's
    /// `from`, so its reading of a maker's field is the fold's own.
    at: [Option<usize>; 2],
    long: Option<AhoCorasick>,
    short: BTreeSet<String>,
    /// How many distinct words the pack knows.
    pub size: usize,
}

/// What the pack knows of one series' names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Read {
    /// The series' distinct words.
    pub words: usize,
    /// Of them, the ones the pack knows.
    pub known: usize,
}

impl Read {
    /// The series has words and the pack knows none of them.
    pub fn unread(&self) -> bool {
        self.words > 0 && self.known == 0
    }
}

impl Vocabulary {
    /// The pack's vocabulary, folding names as its derived text `text` does;
    /// with none named, as the first text it derives from the series
    /// description. A name that is no derived text of the pack is refused.
    pub fn of(pack: &Pack, text: Option<&str>) -> Result<Vocabulary, String> {
        let description = field_index("text_series_description");
        let protocol = field_index("text_protocol_name");
        let fold =
            match text {
                Some(name) => Some(pack.derived.iter().find(|d| d.into == name).ok_or_else(
                    || format!("the pack {} derives no text named {name}", pack.id()),
                )?),
                None => pack
                    .derived
                    .iter()
                    .find(|d| description.is_some_and(|f| d.from.contains(&f))),
            };
        let at = match fold {
            Some(f) => [
                description.and_then(|d| f.from.iter().position(|x| *x == d)),
                protocol.and_then(|p| f.from.iter().position(|x| *x == p)),
            ],
            None => [None, None],
        };
        let known = pack_words(pack);
        let size = known.len();
        let (long, short): (Vec<String>, Vec<String>) =
            known.into_iter().partition(|w| w.chars().count() >= 3);
        let long = if long.is_empty() {
            None
        } else {
            Some(AhoCorasick::new(&long).map_err(|e| e.to_string())?)
        };
        Ok(Vocabulary {
            text: fold.map(|f| f.into.clone()),
            fold: fold.cloned(),
            at,
            long,
            short: short.into_iter().collect(),
            size,
        })
    }

    /// A series' words, folded as the pack folds its names: the description
    /// and the protocol name, each as the fold reads it for this maker (the
    /// MRI pack's technique text leaves GE's protocol name out, since it
    /// names the exam and not the series), distinct, in order.
    pub fn words(&self, description: &str, protocol: &str, manufacturer: &str) -> Vec<String> {
        let mut parts: Vec<&str> = Vec::with_capacity(2);
        for (k, text) in [description, protocol].into_iter().enumerate() {
            let reads = match (&self.fold, self.at[k]) {
                (Some(f), Some(at)) => f.reads(at, manufacturer),
                // a fold that does not read the field folds it all the same:
                // it is the series' own name
                _ => true,
            };
            if reads && !text.trim().is_empty() {
                parts.push(text);
            }
        }
        let folded = match &self.fold {
            Some(f) => f.apply(&parts).unwrap_or_default(),
            None => plain(&parts.join(" ")),
        };
        let mut out: Vec<String> = Vec::new();
        for w in folded.split_whitespace() {
            if is_word(w) && !out.iter().any(|o| o == w) {
                out.push(w.to_string());
            }
        }
        out
    }

    /// Whether the pack knows a word, as [`Vocabulary::words`] gives it.
    pub fn knows(&self, word: &str) -> bool {
        self.short.contains(word) || self.long.as_ref().is_some_and(|a| a.is_match(word))
    }

    /// What the pack knows of one series' names.
    pub fn read(&self, description: &str, protocol: &str, manufacturer: &str) -> Read {
        let words = self.words(description, protocol, manufacturer);
        Read {
            words: words.len(),
            known: words.iter().filter(|w| self.knows(w)).count(),
        }
    }
}

/// Lower case, and anything but a letter or a digit a space: the fold of a
/// pack that derives no text from the names.
fn plain(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect()
}

/// A word: a letter in it, and two characters or more.
fn is_word(w: &str) -> bool {
    w.chars().count() >= 2 && w.chars().any(|c| c.is_ascii_lowercase())
}

/// Every word the pack matches text by, lower case: the keyword clauses'
/// lists and the literals of every text condition, in rule sets (their entry
/// conditions, derived values, gates and clauses), flags, parsers and
/// passes. A literal of several words gives each of them.
pub fn pack_words(pack: &Pack) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for set in &pack.rule_sets {
        if let Some(e) = &set.enter_when {
            literals(e, &mut out);
        }
        for d in &set.derives {
            for c in &d.cases {
                if let Some(e) = &c.when {
                    literals(e, &mut out);
                }
            }
        }
        for r in &set.rules {
            if let Some(e) = &r.requires {
                literals(e, &mut out);
            }
            for c in &r.clauses {
                match c {
                    Clause::Keywords { list, .. } => {
                        for w in list {
                            pieces(w, &mut out);
                        }
                    }
                    Clause::When { expr, .. } => literals(expr, &mut out),
                    _ => {}
                }
            }
        }
    }
    for e in &pack.flags {
        literals(e, &mut out);
    }
    for p in &pack.parsers {
        for e in &p.preds {
            literals(e, &mut out);
        }
    }
    for pass in &pack.passes {
        if let Some(e) = &pass.target {
            literals(e, &mut out);
        }
        if let Some(s) = pass.session() {
            for r in &s.rules {
                if let Some(e) = &r.when {
                    literals(e, &mut out);
                }
                literals(&r.sibling.when, &mut out);
            }
        }
    }
    out
}

/// The literals a condition matches text by.
fn literals(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Token(s) | Expr::Substring(s) | Expr::Equals(s) | Expr::Prefix { s, .. } => {
            pieces(s, out)
        }
        Expr::AnyToken(v) | Expr::AllTokens(v) | Expr::AnySubstring(v) => {
            for s in v {
                pieces(s, out);
            }
        }
        Expr::Text { inner, .. } | Expr::InParser { inner, .. } | Expr::Not(inner) => {
            literals(inner, out)
        }
        Expr::Any(es) | Expr::All(es) => {
            for x in es {
                literals(x, out);
            }
        }
        _ => {}
    }
}

/// A literal's words: lower case, split where the fold splits (white space
/// and the underscore), punctuation trimmed from the ends.
fn pieces(literal: &str, out: &mut BTreeSet<String>) {
    for p in literal
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == '_')
    {
        let p = p.trim_matches(|c: char| !c.is_alphanumeric());
        if is_word(p) {
            out.insert(p.to_string());
        }
    }
}

/// A maker as a person names it: the scanner's own spelling of the big
/// makers brought to one word, any other as written; none where the file
/// names none.
pub fn maker_of(raw: &str) -> Option<String> {
    let written = raw.trim();
    if written.is_empty() {
        return None;
    }
    let l = written.to_lowercase();
    let named = if l.contains("siemens") {
        "Siemens"
    } else if l.contains("philips") {
        "Philips"
    } else if l == "ge" || l.starts_with("ge ") || l.contains("general electric") {
        "GE"
    } else if l.contains("toshiba") || l.contains("canon") {
        "Canon"
    } else if l.contains("hitachi") || l.contains("fujifilm") {
        "Hitachi"
    } else {
        return Some(written.to_string());
    };
    Some(named.to_string())
}

/// The derived texts of a pack that fold the series description, by name:
/// what `review.damaged_headers.words` may name.
pub fn name_texts(pack: &Pack) -> Vec<String> {
    let description = field_index("text_series_description");
    pack.derived
        .iter()
        .filter(|d| description.is_some_and(|f| d.from.contains(&f)))
        .map(|d| d.into.clone())
        .collect()
}

/// For a test or a report: the field a derived text's position names.
pub fn fold_field(pack: &Pack, text: &str, k: usize) -> Option<String> {
    let d = pack.derived.iter().find(|d| d.into == text)?;
    reads::field_name(pack, *d.from.get(k)?).map(|(n, _)| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mri() -> Pack {
        crate::load(&crate::shipped_mri(), None).expect("the MRI pack loads")
    }

    #[test]
    fn a_word_has_a_letter_and_two_characters() {
        assert!(is_word("t1w"));
        assert!(is_word("ax"));
        assert!(!is_word("512"));
        assert!(!is_word("p"));
        assert!(!is_word("-"));
    }

    #[test]
    fn the_names_are_folded_as_the_rules_fold_them() {
        let pack = mri();
        let v = Vocabulary::of(&pack, Some("technique_text")).unwrap();
        // the fold's replacements: t2 is t2w, se is spin-echo; numbers go
        let w = v.words("T2_TSE_SAG_512", "", "SIEMENS");
        assert_eq!(w, ["t2w", "tse", "sag"]);
        // GE's protocol name names the exam, and the technique text leaves it out
        assert_eq!(
            v.words("Ax T2 FLAIR", "HJARNA RUTIN", "GE MEDICAL SYSTEMS"),
            ["ax", "t2w", "flair"]
        );
        assert_eq!(
            v.words("Ax T2 FLAIR", "HJARNA RUTIN", "SIEMENS"),
            ["ax", "t2w", "flair", "hjarna", "rutin"]
        );
        // a name that is nothing but numbers or boilerplate has no words
        assert!(v.words("", "", "SIEMENS").is_empty());
        assert!(v.words("1 2 3", "brain", "SIEMENS").is_empty());
    }

    #[test]
    fn the_pack_knows_its_own_words_and_not_another_language() {
        let pack = mri();
        let v = Vocabulary::of(&pack, Some("technique_text")).unwrap();
        assert!(v.size > 300, "{}", v.size);
        for w in [
            "flair",
            "t2w",
            "mprage",
            "dwi",
            "localizer",
            "t2flair",
            "swi",
        ] {
            assert!(v.knows(w), "{w}");
        }
        // the pack's words in other languages are its own (the anatomy
        // keywords' hjarna, the contrast phrases' mdc)
        assert!(v.knows("hjarna") && v.knows("mdc"));
        for w in ["zzqx", "rutin", "qqvv"] {
            assert!(!v.knows(w), "{w}");
        }
        let read = v.read("t1_mprage_sag", "", "SIEMENS");
        assert_eq!((read.words, read.known), (3, 3));
        assert!(!read.unread());
        let read = v.read("zzqx qqvv", "", "SIEMENS");
        assert_eq!((read.words, read.known), (2, 0));
        assert!(read.unread());
        assert!(
            !Read::default().unread(),
            "no words is no name, not an unknown one"
        );
    }

    #[test]
    fn a_text_the_pack_does_not_derive_is_refused() {
        let pack = mri();
        let e = Vocabulary::of(&pack, Some("no_such_text")).err().unwrap();
        assert!(e.contains("derives no text named no_such_text"), "{e}");
        assert!(name_texts(&pack).contains(&"technique_text".to_string()));
        // with none named, the first text the pack derives from the description
        assert_eq!(
            Vocabulary::of(&pack, None).unwrap().text.as_deref(),
            Some("search_text")
        );
    }

    #[test]
    fn a_maker_is_named_as_a_person_names_it() {
        assert_eq!(maker_of("SIEMENS").as_deref(), Some("Siemens"));
        assert_eq!(maker_of("Siemens Healthineers").as_deref(), Some("Siemens"));
        assert_eq!(maker_of("GE MEDICAL SYSTEMS").as_deref(), Some("GE"));
        assert_eq!(
            maker_of("Philips Medical Systems").as_deref(),
            Some("Philips")
        );
        assert_eq!(maker_of("TOSHIBA_MEC").as_deref(), Some("Canon"));
        assert_eq!(maker_of("FUJIFILM Healthcare").as_deref(), Some("Hitachi"));
        assert_eq!(maker_of("Bruker").as_deref(), Some("Bruker"));
        assert_eq!(maker_of("  "), None);
    }
}
