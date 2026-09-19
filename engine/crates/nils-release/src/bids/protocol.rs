// SPDX-License-Identifier: AGPL-3.0-only

//! What the protocol text separates, when nothing else does (record 37, S4).
//!
//! The 2026-09-19 survey of the archive went through 437,069 stacks and asked,
//! for every name that carries more than one of them, what actually differs.
//! For **1,093 names over 2,290 stacks the answer is the protocol or sequence
//! text and nothing else at all**: the coverage agrees, the coil agrees, the
//! physics agree, every axis the pack decided agrees, and the only thing that
//! is not the same is a line a radiographer typed at a console. Those stacks
//! are two different things, and until this module existed the release said
//! they were one thing done twice.
//!
//! So the text is read, and it is read **last**. A name that rests on a
//! measurement is worth more than a name that rests on free text, and the
//! order is the whole of the argument:
//!
//! 1. the axes the pack decided, and the identities S5 added to them;
//! 2. the coverage (S1), the receive coil (S3) and the physics;
//! 3. and only where every one of those agrees, this.
//!
//! [`separates`] enforces that order itself rather than trusting its caller:
//! it is handed everything else NILS holds about each stack ([`Otherwise`])
//! and refuses to answer at all unless those agree exactly. So the text can
//! never overtake a fact, whatever a caller does.
//!
//! ## What never leaves
//!
//! Protocol text is free text a person typed. It is inconsistent, sometimes
//! damaged, and it can carry a name: the archive holds series whose
//! description is a clinical note and protocol names with a person in them.
//! **None of it reaches a filename, a report or a review item.** What reaches
//! them is [`mark`]: six hex characters of an unkeyed digest, the same device
//! `nils_pseudonymize::layout::study_hash` uses for a study UID, enough to
//! tell two texts apart and nothing a reader can turn back into either. The
//! text itself is compared in memory and dropped.
//!
//! ## The contract with S2
//!
//! S2 replaces the `run-` counter with a test of whether two stacks really are
//! one acquisition measured twice, and that test ignores the counter a scanner
//! welds onto a re-run step: `T1 MPRAGE` and `T1 MPRAGE 2` are one protocol.
//! If this slice separated on that counter the two would contradict each
//! other, one calling a pair a repeat and the other naming them apart. So both
//! compare text through [`step`], which takes a whole trailing numeric token
//! off and nothing else, and where two texts differ only by such a counter
//! **this module reports no separation** and the pair falls through to
//! whatever the repeat rule says about it.

use std::collections::{BTreeMap, HashMap};

use blake2::Digest;
use blake2::digest::consts::U32;
use nils_registry::schema::table;
use nils_registry::store::{Error as StoreError, Store};

/// Which of the three texts answered.
///
/// They are asked in this order, and **the first that differs decides the
/// whole group**. They cannot disagree about whether two stacks differ, since
/// one of them differing is enough; they can propose different partitions of
/// the same group, and letting one field decide means the partition has one
/// explanation rather than being a mixture of two fields' opinions that
/// nobody could read back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    /// `ProtocolName` (0018,1030): the step the operator chose at the console.
    /// First because it is what the survey measured, the field that separates
    /// 11,086 colliding names, and the one S2's repeat test already compares,
    /// so one field answers both questions.
    ProtocolName,
    /// `SeriesDescription` (0008,103E): what the scanner called the series it
    /// produced. Second because it describes the output rather than the step,
    /// it carries the reconstruction words the scanner appends, and of the
    /// three it is the likeliest to hold a free comment somebody typed.
    SeriesDescription,
    /// `SequenceName` (0018,0024): the vendor's name for the pulse sequence.
    /// Last because it is the coarsest of the three, many different protocol
    /// steps share one, and v0 found it absent on 37 per cent of series.
    SequenceName,
}

impl Field {
    /// In the order they are asked.
    pub const EVERY: [Field; 3] = [
        Field::ProtocolName,
        Field::SeriesDescription,
        Field::SequenceName,
    ];

    /// The DICOM keyword, which is the word a person can look up. A keyword
    /// names an element and never a patient, so it is safe in a report.
    pub fn keyword(self) -> &'static str {
        match self {
            Field::ProtocolName => "ProtocolName",
            Field::SeriesDescription => "SeriesDescription",
            Field::SequenceName => "SequenceName",
        }
    }
}

/// The three texts of one stack, **as the fingerprint already folded them**.
///
/// These are the `text_*_ci` columns: NFKC, whitespace collapsed to single
/// spaces, the ends trimmed, Unicode lower case. That folding is reused rather
/// than replaced, because every part of it is a difference in spelling and not
/// a difference in acquisition: `T1  MPRAGE` and `t1 mprage` are one protocol
/// by any reading, and comparing the raw strings would separate two stacks
/// over a console's capitalisation.
///
/// It stops exactly there. The pack's own normalisers, which drop tokens and
/// rewrite MRI vocabulary so a keyword rule can fire, are **not** applied:
/// that job is to make two different texts match, and this job is the
/// opposite. Nothing is transliterated and nothing is repaired either, which
/// matters for the spellings the archive damaged. Where a character set the
/// scanner could not write has left a literal `?` in place of an accented
/// letter, the `?` is compared as the character it is. Two stacks damaged the
/// same way therefore read as one text; two damaged differently read as two,
/// and are named apart. That error is in the safe direction: the cost is a
/// name more than was needed, said to be weak, rather than a false claim that
/// two acquisitions are one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Text {
    pub protocol: Option<String>,
    pub description: Option<String>,
    pub sequence: Option<String>,
}

impl Text {
    fn of(&self, field: Field) -> Option<&str> {
        match field {
            Field::ProtocolName => self.protocol.as_deref(),
            Field::SeriesDescription => self.description.as_deref(),
            Field::SequenceName => self.sequence.as_deref(),
        }
    }
}

/// Everything else NILS holds about a stack: every axis the pack decided, and
/// every measured column of the fingerprint that says what the image is.
///
/// Compared **exactly**, with no tolerance anywhere, because this is a guard
/// and not a judgement. S2's repeat test is the judgement, and it carries the
/// declared tolerances; this only has to know whether anything other than the
/// text could be the difference, and the strict answer errs towards saying
/// yes, which costs a separation that is never made rather than one made for
/// the wrong reason.
///
/// > When S2 has landed this is its answer rather than its own list: a group
/// > reaches the text exactly when `repeat::one_acquisition` returns the one
/// > difference "the protocol", and these three column lists go. The contract
/// > is the same either way, which is why the lists are the study's own.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Otherwise {
    /// Axis name to value, as the classifier stored them.
    pub axes: BTreeMap<String, String>,
    pub words: Vec<Option<String>>,
    pub numbers: Vec<Option<f64>>,
    pub counts: Vec<Option<i64>>,
}

/// The folded texts, in [`Field::EVERY`]'s order.
const TEXTS: &[&str] = &[
    "text_protocol_name_ci",
    "text_series_description_ci",
    "text_sequence_name_ci",
];

/// The facts that are words. `text_contrast_ci` is among them because it is
/// built from the contrast administration fields rather than typed by anyone,
/// and the survey found those separate 1,840 stacks on their own.
const WORDS: &[&str] = &[
    "orientation",
    "mr_acquisition_type",
    "image_type",
    "image_role",
    "scanning_sequence",
    "sequence_variant",
    "scan_options",
    "echo_numbers",
    "pixel_bandwidth",
    "acquisition_matrix",
    "receive_coil_name",
    "coverage_source",
    "split_reason",
    "pixel_spacing",
    "dwi_b_values",
    "dwi_pe_direction",
    "manufacturer",
    "manufacturer_model_name",
    "text_contrast_ci",
];

/// The facts that are measurements.
const NUMBERS: &[&str] = &[
    "echo_time",
    "repetition_time",
    "inversion_time",
    "flip_angle",
    "number_of_averages",
    "magnetic_field_strength",
    "slice_thickness",
    "spacing_between_slices",
    "fov_x",
    "fov_y",
    "slice_span_mm",
    "dwi_b_value",
    "diffusion_b_value",
];

/// The facts that are counts.
const COUNTS: &[&str] = &[
    "echo_train_length",
    "rows",
    "columns",
    "n_slices",
    "dwi_directions",
];

/// What one stack brings to the question.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stack {
    pub otherwise: Otherwise,
    pub text: Text,
}

/// The answer: which text told them apart, and the mark each stack takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Separation {
    pub field: Field,
    /// One mark per member, in the order the members were given. Two members
    /// whose text is the same take the same mark and go on sharing a name,
    /// which is right: they are the repeat inside the group, and the repeat
    /// rule is what has something to say about them.
    pub marks: Vec<String>,
}

/// How many stacks are asked about at once, so that an archive's worth of
/// collisions is not one statement.
const BATCH: usize = 2_000;

/// Read the texts and everything else NILS holds, for the stacks that want
/// one name.
///
/// **Only for those stacks.** Most of an archive collides with nothing, and a
/// name that is unique needs none of this; asking about every stack would
/// read forty columns of half a million rows to answer a question about
/// twenty per cent of them.
///
/// The fingerprint is the only source, so the answer is the registry's and
/// never a file's.
pub fn read(
    store: &mut Store,
    axes: &HashMap<i64, BTreeMap<String, String>>,
    wanted: &[i64],
) -> Result<HashMap<i64, Stack>, StoreError> {
    let t = table("stack_fingerprint");
    let column = |c: &str| {
        t.column(c)
            .unwrap_or_else(|| panic!("stack_fingerprint.{c} is not a column"))
            .name
    };
    let mut columns: Vec<&str> = vec![column("stack_id")];
    for c in TEXTS.iter().chain(WORDS).chain(NUMBERS).chain(COUNTS) {
        columns.push(column(c));
    }
    let mut out: HashMap<i64, Stack> = HashMap::new();
    for chunk in wanted.chunks(BATCH) {
        let sql = format!(
            "SELECT {} FROM {} WHERE stack_id IN ({})",
            columns.join(", "),
            store.qualified("stack_fingerprint"),
            chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        for r in store.query(&sql, &[])? {
            let stack = r.int(0)?;
            let mut at = 1;
            let mut texts: Vec<Option<String>> = Vec::with_capacity(TEXTS.len());
            for _ in TEXTS {
                texts.push(r.opt_text(at)?.map(str::to_string));
                at += 1;
            }
            let mut words: Vec<Option<String>> = Vec::with_capacity(WORDS.len());
            for _ in WORDS {
                words.push(r.opt_text(at)?.map(str::to_string));
                at += 1;
            }
            let mut numbers: Vec<Option<f64>> = Vec::with_capacity(NUMBERS.len());
            for _ in NUMBERS {
                numbers.push(r.opt_double(at)?);
                at += 1;
            }
            let mut counts: Vec<Option<i64>> = Vec::with_capacity(COUNTS.len());
            for _ in COUNTS {
                counts.push(r.opt_int(at)?);
                at += 1;
            }
            out.insert(
                stack,
                Stack {
                    otherwise: Otherwise {
                        axes: axes.get(&stack).cloned().unwrap_or_default(),
                        words,
                        numbers,
                        counts,
                    },
                    text: Text {
                        protocol: texts[0].clone(),
                        description: texts[1].clone(),
                        sequence: texts[2].clone(),
                    },
                },
            );
        }
    }
    Ok(out)
}

/// Whether the protocol text separates a group of stacks that want one name,
/// and how.
///
/// `None` is the usual answer, and it means the text is not what is going on:
/// either something else differs, in which case the text is not the last
/// resort and something stronger should say so, or the text agrees, or the
/// only thing between the two spellings is a scanner's own counter.
pub fn separates(group: &[&Stack]) -> Option<Separation> {
    if group.len() < 2 {
        return None;
    }
    // The order, enforced here rather than asked of a caller. Anything else
    // NILS holds outranks the text, so if any of it differs this is not the
    // last resort and the answer is no answer.
    let first = &group[0].otherwise;
    if group.iter().any(|s| &s.otherwise != first) {
        return None;
    }
    for field in Field::EVERY {
        // A field only answers where every member recorded it. An absence is
        // not a difference: nothing can be said about a value nobody wrote
        // down, and the fields are thin (v0 found the protocol name missing on
        // 11 per cent of series and the sequence name on 37).
        let Some(steps) = group
            .iter()
            .map(|s| s.text.of(field).map(step))
            .collect::<Option<Vec<String>>>()
        else {
            continue;
        };
        if steps.iter().all(|s| *s == steps[0]) {
            continue;
        }
        let marks: Vec<String> = steps.iter().map(|s| mark(s)).collect();
        // A digest is short, so check rather than assume: two texts that
        // differ have to take marks that differ, or the mark would say they
        // are one text. On a collision the next field is asked instead.
        let mut clash = false;
        for (i, a) in steps.iter().enumerate() {
            for (j, b) in steps.iter().enumerate().skip(i + 1) {
                if (a == b) != (marks[i] == marks[j]) {
                    clash = true;
                }
            }
        }
        if clash {
            continue;
        }
        return Some(Separation { field, marks });
    }
    None
}

/// One text with the scanner's own re-run counter taken off it.
///
/// A step run a second time comes back as `t1 mprage 2`, and the counter is
/// the scanner's bookkeeping rather than a different protocol. Only a **whole
/// trailing token** goes: a digit welded to a letter, as in `t1` or `p2`, is
/// part of the name, and dropping every digit the way the study's own query
/// did would read `t1 mprage` and `t2 mprage` as one protocol.
///
/// This is the shared half of the contract with S2. Both slices have to fold
/// a counter away the same way or they contradict each other, so there is one
/// function and this is it.
pub fn step(text: &str) -> String {
    // Idempotent on the folded columns, and correct on a raw string, so a
    // caller that hands over an unfolded text gets the same answer.
    let mut out = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    loop {
        let body = out.trim_end_matches(|c: char| c.is_ascii_digit());
        if body.len() == out.len() {
            break;
        }
        let head = body.trim_end_matches([' ', '_', '-', '.']);
        if head.len() == body.len() || head.is_empty() {
            break;
        }
        out = head.to_string();
    }
    out
}

/// What stands for one protocol text in a filename: `Text` and six hex
/// characters of the text's unkeyed BLAKE2b-256.
///
/// Two properties, and the name needs both. It **separates**, because two
/// texts that differ take different marks. And it **says what it is**: a
/// reader who meets `Text9f3ac1` in an `acq-` label can see that the last
/// thing standing between this file and its neighbour is free text and not a
/// measurement, which is the honest thing for the name to admit.
///
/// It is a digest and not the text because the text is free text and can name
/// a person. It is a digest and not a counter because a counter would change
/// the day a third stack arrived, and because a counter is the very thing
/// record 37 is removing from `run-`.
pub fn mark(step: &str) -> String {
    let digest = blake2::Blake2b::<U32>::digest(step.as_bytes());
    format!("Text{}", hex::encode(&digest[..3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One stack, with facts a real one carries. Nothing here is read from an
    /// archive: the shapes are the studies', the values are made up.
    fn stack(protocol: &str) -> Stack {
        Stack {
            otherwise: Otherwise {
                axes: [
                    ("base".to_string(), "T1w".to_string()),
                    ("technique".to_string(), "MPRAGE".to_string()),
                ]
                .into(),
                words: vec![Some("axial".to_string())],
                numbers: vec![Some(3.0)],
                counts: vec![Some(176)],
            },
            text: Text {
                protocol: Some(protocol.to_string()),
                ..Text::default()
            },
        }
    }

    #[test]
    fn two_stacks_that_differ_only_in_their_protocol_text_are_told_apart() {
        // The archive's 1,093 names: everything NILS holds agrees and the
        // text does not.
        let a = stack("t1 mprage sag");
        let b = stack("t1 mprage sag iso");
        let s = separates(&[&a, &b]).expect("the text separates them");
        assert_eq!(s.field, Field::ProtocolName);
        assert_ne!(s.marks[0], s.marks[1]);
    }

    #[test]
    fn the_text_is_never_reached_while_a_measurement_differs() {
        // S4 is the last resort. A pair that differs in its coverage differs
        // in something a measurement can say, and this must not answer for it
        // even though its text differs too.
        let a = stack("t1 mprage sag");
        let mut b = stack("t1 mprage sag iso");
        b.otherwise.counts = vec![Some(120)];
        assert_eq!(separates(&[&a, &b]), None);
        // The same for an axis the pack decided, which S5's identities join.
        let mut c = stack("t1 mprage sag iso");
        c.otherwise
            .axes
            .insert("construct".to_string(), "MEAN".to_string());
        assert_eq!(separates(&[&a, &c]), None);
    }

    #[test]
    fn the_counter_a_scanner_welds_on_a_rerun_is_not_a_separation() {
        // The contract with S2: S2 calls this pair one acquisition measured
        // twice, so S4 must not name them apart, or the two slices would say
        // opposite things about one pair.
        let a = stack("t1 mprage");
        let b = stack("t1 mprage 2");
        assert_eq!(separates(&[&a, &b]), None);
        assert_eq!(step("t1 mprage 2"), step("t1 mprage"));
        assert_eq!(step("t1 mprage_2"), "t1 mprage");
        assert_eq!(step("t1 mprage-2"), "t1 mprage");
        // And a digit inside a word is part of the word, not a counter.
        assert_ne!(step("t1 mprage"), step("t2 mprage"));
        assert_eq!(step("t1 mprage p2 iso"), "t1 mprage p2 iso");
    }

    #[test]
    fn a_text_only_one_of_them_recorded_is_not_a_difference() {
        // v0 found the protocol name absent on 11 per cent of series. An
        // absence is not a measurement, so the field is skipped and the next
        // is asked.
        let a = stack("t1 mprage");
        let mut b = stack("t1 mprage");
        b.text.protocol = None;
        assert_eq!(separates(&[&a, &b]), None);
        let mut a = a;
        a.text.description = Some("t1 mprage sag".to_string());
        b.text.description = Some("t1 mprage tra".to_string());
        let s = separates(&[&a, &b]).expect("the description answers instead");
        assert_eq!(s.field, Field::SeriesDescription);
    }

    #[test]
    fn the_fields_are_asked_in_order_and_one_of_them_decides() {
        // Two fields both differ, and they partition the group differently.
        // The first that differs decides the whole group, so the partition
        // has one explanation.
        let mut a = stack("t1 mprage sag");
        let mut b = stack("t1 mprage tra");
        let mut c = stack("t1 mprage tra");
        a.text.description = Some("one".to_string());
        b.text.description = Some("one".to_string());
        c.text.description = Some("two".to_string());
        a.text.sequence = Some("tfl3d1".to_string());
        b.text.sequence = Some("tfl3d1".to_string());
        c.text.sequence = Some("tfl3d1".to_string());
        let s = separates(&[&a, &b, &c]).expect("the protocol name answers");
        assert_eq!(s.field, Field::ProtocolName);
        // And the protocol name's partition is the one written: b and c share
        // a text, so they share a mark and go on sharing a name.
        assert_ne!(s.marks[0], s.marks[1]);
        assert_eq!(s.marks[1], s.marks[2]);
    }

    #[test]
    fn the_sequence_name_answers_when_the_two_above_it_agree() {
        let mut a = stack("t1 mprage");
        let mut b = stack("t1 mprage");
        a.text.description = Some("t1 mprage".to_string());
        b.text.description = Some("t1 mprage".to_string());
        a.text.sequence = Some("tfl3d1".to_string());
        b.text.sequence = Some("tfl3d1_ns".to_string());
        let s = separates(&[&a, &b]).expect("the sequence name answers");
        assert_eq!(s.field, Field::SequenceName);
        assert_eq!(s.field.keyword(), "SequenceName");
    }

    #[test]
    fn a_damaged_spelling_is_compared_as_the_characters_it_holds() {
        // The archive carries a literal question mark where a character set
        // the scanner could not write left an accented letter. Two stacks
        // damaged the same way are one text; two damaged differently are two,
        // and an extra name said to be weak is the safe direction.
        let a = stack("hj?rna t1");
        let b = stack("hj?rna t1");
        assert_eq!(separates(&[&a, &b]), None);
        let c = stack("hjarna t1");
        assert!(separates(&[&a, &c]).is_some());
    }

    #[test]
    fn folding_is_reused_so_a_spelling_is_not_a_difference() {
        // Case and whitespace are how a console wrote the same protocol, and
        // the fingerprint's folded columns already settle both.
        assert_eq!(step("  T1   MPRAGE  "), "t1 mprage");
        let a = stack("t1 mprage");
        let b = stack("T1  MPRAGE");
        assert_eq!(separates(&[&a, &b]), None);
    }

    #[test]
    fn no_protocol_text_ever_reaches_the_mark() {
        // The one rule that cannot bend: what goes into a filename is a
        // digest, so a protocol name that carries somebody's name carries it
        // nowhere. The mark says what it is and is the same length whatever
        // it stands for.
        let text = "dr somebody special t1";
        let m = mark(&step(text));
        assert_eq!(m.len(), "Text".len() + 6);
        assert!(m.starts_with("Text"));
        assert!(m.chars().all(|c| c.is_ascii_alphanumeric()));
        for word in ["dr", "somebody", "special", "t1"] {
            assert!(!m.to_lowercase().contains(word), "{m}");
        }
        // And it is a function of the text alone, so two runs and two
        // machines write one name.
        assert_eq!(m, mark(&step("  DR SOMEBODY   SPECIAL T1 ")));
    }

    #[test]
    fn one_stack_is_not_a_collision() {
        let a = stack("t1 mprage");
        assert_eq!(separates(&[&a]), None);
    }
}
