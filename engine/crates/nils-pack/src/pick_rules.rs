// SPDX-License-Identifier: AGPL-3.0-only

//! A dataset's or a cohort's own main-scan rules (record 55, decision 6 of
//! 2026-10-10; the main-scans study, sections 1 to 3).
//!
//! The main scans of a scope are chosen in two steps. First, per role, the
//! **kind** of scan each visit takes: the dimension, the technique and the
//! contrast of a candidate, chosen across a subject's visits by one of three
//! ways of keeping them alike. Then, within the kind, one candidate per
//! visit: the pack's score ([`pick::score`]) with the rules' own weights, a
//! factor for a derived series, and a stated order for a near tie. Nothing
//! is asked of a person: a near tie is settled by the order, and a retake by
//! the later full repeat.
//!
//! Like [`pick`], this knows nothing about a registry. The rules are a
//! document (stored as JSON, shown as YAML), checked when they are read;
//! version 1 is derived from the pack's pick model and never stored.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::pick::{self, Candidate, Model, Reference};

/// The names a candidate carries for the rules, beside those the pick model
/// reads: the kind, what the rules allow, the tie keys and the derived
/// factor read them.
pub const READS: &[&str] = &[
    "acquisition_type_filled",
    "mr_acquisition_type",
    "technique",
    "post_contrast",
    "body_part",
    "slice_thickness",
    "orientation",
    "n_instances",
    "series_number",
    "image_type",
    "provenance",
    "construct",
];

/// The kind of a candidate with no technique (section 2).
pub const UNKNOWN: &str = "Unknown";

/// What a kind with contrast given ends in (section 2).
pub const CONTRAST: &str = " +C";

macro_rules! words {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $variant:ident = $word:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub enum $name { $($(#[$vm])* $variant),+ }

        impl $name {
            /// Every value, in the order the document lists them.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The word the document writes.
            pub fn name(self) -> &'static str {
                match self { $($name::$variant => $word),+ }
            }

            pub fn parse(word: &str) -> Option<$name> {
                $name::ALL.iter().copied().find(|v| v.name() == word)
            }

            fn choices() -> String {
                $name::ALL.iter().map(|v| v.name()).collect::<Vec<_>>().join(" | ")
            }
        }
    };
}

words! {
    /// How a subject's visits are kept alike (section 3, step 2).
    KeepAlike {
        /// The subject keeps the kind that covers most of its visits.
        WithinEachSubject = "within_each_subject",
        /// Each visit takes the first kind in the order that it has.
        AcrossTheData = "across_the_data",
        /// The first kind that covers every visit, else each visit's first.
        Balanced = "balanced",
    }
}

words! {
    /// Which candidates the contrast allows.
    Contrast {
        Either = "either",
        Without = "without",
        With = "with",
    }
}

words! {
    /// Which candidates the dimension allows.
    Dimension {
        Any = "any",
        ThreeD = "3d",
    }
}

words! {
    /// Which candidates the body part allows: the brain (brain, brain-neck or
    /// not stated, the 2026-10-10 borders study's R1), or any.
    BodyPart {
        Brain = "brain",
        Any = "any",
    }
}

words! {
    /// One step of the order a near tie within a kind goes by (section 3).
    TieKey {
        /// Axial, then coronal, then sagittal.
        AxialCoronalSagittal = "axial_coronal_sagittal",
        /// The thinner slices.
        ThinnerSlices = "thinner_slices",
        /// The scanner's default reconstruction before ND, ORIG or a derived
        /// series (R6).
        DefaultReconstruction = "default_reconstruction",
        /// Of two full stacks of one kind with the same parameters taken at
        /// different times, the later (R7).
        LaterFullRepeat = "later_full_repeat",
        /// The lower series number.
        EarlierSeries = "earlier_series",
    }
}

/// Why a document is not rules: the field, by its path, and what is wrong.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for Refusal {}

fn refuse(field: impl Into<String>, message: impl Into<String>) -> Refusal {
    Refusal {
        field: field.into(),
        message: message.into(),
    }
}

/// One role's rules.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleRules {
    pub keep_alike: KeepAlike,
    /// Kind keys, best first. A kind the data holds that this does not name
    /// comes after them, used: a new kind appears rather than vanishes.
    pub kinds_in_order: Vec<String>,
    /// Kinds switched off.
    pub not_used: Vec<String>,
    pub contrast: Contrast,
    pub dimension: Dimension,
    pub body_part: BodyPart,
    /// None means any.
    pub slice_thickness_at_most_mm: Option<f64>,
}

/// The five weights of the score within a kind, by the components they
/// weigh (`slices`, `fov`, `mod`, `orient`, `complete`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    pub slice_count: f64,
    pub field_of_view: f64,
    pub modifiers: f64,
    pub orientation: f64,
    pub completeness: f64,
}

impl Weights {
    /// Each weight with its name in the document and the pack's component.
    fn named(&self) -> [(&'static str, &'static str, f64); 5] {
        [
            ("slice_count", "slices", self.slice_count),
            ("field_of_view", "fov", self.field_of_view),
            ("modifiers", "mod", self.modifiers),
            ("orientation", "orient", self.orientation),
            ("completeness", "complete", self.completeness),
        ]
    }

    fn set(&mut self, name: &str, value: f64) {
        match name {
            "slice_count" => self.slice_count = value,
            "field_of_view" => self.field_of_view = value,
            "modifiers" => self.modifiers = value,
            "orientation" => self.orientation = value,
            "completeness" => self.completeness = value,
            _ => {}
        }
    }
}

/// How the one scan of a visit is chosen among candidates of one kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Within {
    pub weights: Weights,
    /// The factor a derived series' score is multiplied by.
    pub derived_series_scores: f64,
    /// Candidates within this percentage of the best go by the tie order.
    pub near_tie_within_percent: f64,
    pub near_tie_goes_to: Vec<TieKey>,
}

/// A scope's rules: one per role of the pack's pick, and the within-kind
/// part they share.
#[derive(Debug, Clone, PartialEq)]
pub struct Rules {
    /// In the pick's order of roles.
    pub roles: Vec<(String, RoleRules)>,
    pub within: Within,
}

const ROLE_KEYS: &[&str] = &[
    "keep_alike",
    "kinds_in_order",
    "not_used",
    "contrast",
    "dimension",
    "body_part",
    "slice_thickness_at_most_mm",
];
const WITHIN_KEYS: &[&str] = &[
    "weights",
    "derived_series_scores",
    "near_tie_within_percent",
    "near_tie_goes_to",
];

/// Thickness bounds a limit is checked against, in millimetres.
pub const THICKNESS_MM: (f64, f64) = (0.5, 10.0);
/// Near-tie bounds, in percent.
pub const NEAR_TIE_PERCENT: (f64, f64) = (0.0, 25.0);

impl Rules {
    /// Version 1, the pack's defaults (section 1): derived from the pick
    /// model, never stored. The kinds' order is 3D before 2D, then without
    /// contrast before with (the borders study's R3, option A), then the
    /// role's technique tiers, unknown last; balanced; either contrast; any
    /// dimension; the brain; no thickness limit; the five weights of the
    /// components `slices`, `fov`, `mod`, `orient` and `complete`; the
    /// derived factor of the pick's penalty; the near tie of
    /// `borders.runner_up_within`; R8's order with R7.
    pub fn defaults(model: &Model) -> Rules {
        let weight = |name: &str| {
            model
                .components
                .iter()
                .find(|c| c.name == name)
                .map_or(0.0, |c| c.weight)
        };
        let derived = model
            .penalty
            .as_ref()
            .and_then(|p| p.by_value.values().copied().reduce(f64::min))
            .unwrap_or(1.0);
        let roles = model
            .roles
            .iter()
            .map(|role| {
                (
                    role.clone(),
                    RoleRules {
                        keep_alike: KeepAlike::Balanced,
                        kinds_in_order: default_kinds(model, role),
                        not_used: Vec::new(),
                        contrast: Contrast::Either,
                        dimension: Dimension::Any,
                        body_part: BodyPart::Brain,
                        slice_thickness_at_most_mm: None,
                    },
                )
            })
            .collect();
        Rules {
            roles,
            within: Within {
                weights: Weights {
                    slice_count: weight("slices"),
                    field_of_view: weight("fov"),
                    modifiers: weight("mod"),
                    orientation: weight("orient"),
                    completeness: weight("complete"),
                },
                derived_series_scores: derived,
                near_tie_within_percent: round6(model.borders.runner_up_within * 100.0),
                near_tie_goes_to: TieKey::ALL.to_vec(),
            },
        }
    }

    /// The rules of one role.
    pub fn role(&self, role: &str) -> Option<&RoleRules> {
        self.roles.iter().find(|(r, _)| r == role).map(|(_, x)| x)
    }

    /// Read and check a document (section 1, "checked on save"). What it
    /// leaves out is the default's; every role and tie key must be known,
    /// weights from 0 to 1, the derived factor from 0 to 1, the near tie
    /// from 0 to 25 and a thickness from 0.5 to 10 mm. A refusal names the
    /// field.
    pub fn from_json(doc: &Value, defaults: &Rules) -> Result<Rules, Refusal> {
        let top = object(doc, "rules")?;
        known(top, "", &["roles", "same_kind_in_one_visit"])?;
        let mut out = defaults.clone();
        if let Some(roles) = top.get("roles") {
            let roles = object(roles, "roles")?;
            for (role, given) in roles {
                let field = format!("roles.{role}");
                let Some(slot) = out.roles.iter_mut().find(|(r, _)| r == role) else {
                    return Err(refuse(
                        field,
                        format!(
                            "not a role of the pack's pick; the roles are {}",
                            defaults
                                .roles
                                .iter()
                                .map(|(r, _)| r.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                };
                slot.1 = role_from_json(given, &field, &slot.1)?;
            }
        }
        if let Some(w) = top.get("same_kind_in_one_visit") {
            out.within = within_from_json(w, &out.within)?;
        }
        Ok(out)
    }

    /// The document as JSON, every field written, in a fixed order: what is
    /// stored and what the digest is of.
    pub fn to_json(&self) -> Value {
        let mut roles = Map::new();
        for (role, r) in &self.roles {
            roles.insert(
                role.clone(),
                json!({
                    "keep_alike": r.keep_alike.name(),
                    "kinds_in_order": r.kinds_in_order,
                    "not_used": r.not_used,
                    "contrast": r.contrast.name(),
                    "dimension": r.dimension.name(),
                    "body_part": r.body_part.name(),
                    "slice_thickness_at_most_mm": r.slice_thickness_at_most_mm,
                }),
            );
        }
        let mut weights = Map::new();
        for (name, _, w) in self.within.weights.named() {
            weights.insert(name.to_string(), json!(w));
        }
        json!({
            "roles": roles,
            "same_kind_in_one_visit": {
                "weights": weights,
                "derived_series_scores": self.within.derived_series_scores,
                "near_tie_within_percent": self.within.near_tie_within_percent,
                "near_tie_goes_to": self
                    .within
                    .near_tie_goes_to
                    .iter()
                    .map(|k| k.name())
                    .collect::<Vec<_>>(),
            },
        })
    }

    /// The document as YAML, with the comments of section 1 beside the
    /// fields: what a person reads as text.
    pub fn to_yaml(&self) -> String {
        let mut out = String::from("roles:\n");
        let line = |out: &mut String, text: String, comment: &str| {
            if comment.is_empty() {
                out.push_str(&text);
            } else {
                out.push_str(&format!("{text:<37} # {comment}"));
            }
            out.push('\n');
        };
        for (role, r) in &self.roles {
            out.push_str(&format!("  {}:\n", yaml_text(role)));
            line(
                &mut out,
                format!("    keep_alike: {}", r.keep_alike.name()),
                &KeepAlike::choices(),
            );
            line(
                &mut out,
                format!(
                    "    kinds_in_order: {}",
                    yaml_list(&r.kinds_in_order, "      ")
                ),
                "kind keys, best first; a kind not named comes after them",
            );
            line(
                &mut out,
                format!("    not_used: {}", yaml_list(&r.not_used, "      ")),
                "kinds switched off",
            );
            line(
                &mut out,
                format!("    contrast: {}", r.contrast.name()),
                &Contrast::choices(),
            );
            line(
                &mut out,
                format!("    dimension: {}", r.dimension.name()),
                &Dimension::choices(),
            );
            line(
                &mut out,
                format!("    body_part: {}", r.body_part.name()),
                "brain (brain, brain-neck or not stated) | any",
            );
            line(
                &mut out,
                format!(
                    "    slice_thickness_at_most_mm: {}",
                    r.slice_thickness_at_most_mm
                        .map_or("null".to_string(), number)
                ),
                "null means any",
            );
        }
        let w = &self.within;
        out.push_str("same_kind_in_one_visit:\n");
        line(
            &mut out,
            format!(
                "  weights: {{{}}}",
                w.weights
                    .named()
                    .iter()
                    .map(|(name, _, v)| format!("{name}: {v:.2}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "",
        );
        line(
            &mut out,
            format!("  derived_series_scores: {:.2}", w.derived_series_scores),
            "a derived series' score is multiplied by this",
        );
        line(
            &mut out,
            format!(
                "  near_tie_within_percent: {}",
                number(w.near_tie_within_percent)
            ),
            "within this of the best, the order below decides",
        );
        line(
            &mut out,
            format!(
                "  near_tie_goes_to: [{}]",
                w.near_tie_goes_to
                    .iter()
                    .map(|k| k.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "",
        );
        out
    }
}

fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

/// A number as the document writes it: whole numbers without decimals.
fn number(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{x:.0}")
    } else {
        let s = format!("{x:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// A text as a YAML plain scalar where it can be one, quoted otherwise.
fn yaml_text(text: &str) -> String {
    let plain = !text.is_empty()
        && text.trim() == text
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " +._-".contains(c))
        && !text.starts_with(['-', '.', '+'])
        && !matches!(
            text.to_ascii_lowercase().as_str(),
            "null" | "true" | "false" | "yes" | "no" | "on" | "off" | "~"
        )
        && text.parse::<f64>().is_err();
    if plain {
        text.to_string()
    } else {
        serde_json::to_string(text).unwrap_or_default()
    }
}

/// A list in flow style, wrapped so that no line runs far past a hundred
/// characters.
fn yaml_list(items: &[String], indent: &str) -> String {
    let mut out = String::from("[");
    let mut width = 0;
    for (i, item) in items.iter().enumerate() {
        let text = yaml_text(item);
        if i > 0 {
            out.push(',');
            if width + text.len() > 80 {
                out.push('\n');
                out.push_str(indent);
                width = 0;
            } else {
                out.push(' ');
            }
        }
        width += text.len() + 2;
        out.push_str(&text);
    }
    out.push(']');
    out
}

fn object<'a>(v: &'a Value, field: &str) -> Result<&'a Map<String, Value>, Refusal> {
    v.as_object()
        .ok_or_else(|| refuse(field, "an object of named fields"))
}

fn known(m: &Map<String, Value>, prefix: &str, keys: &[&str]) -> Result<(), Refusal> {
    match m.keys().find(|k| !keys.contains(&k.as_str())) {
        None => Ok(()),
        Some(k) => Err(refuse(
            format!("{prefix}{k}"),
            format!("not a field here; the fields are {}", keys.join(", ")),
        )),
    }
}

fn number_in(v: &Value, field: &str, (lo, hi): (f64, f64)) -> Result<f64, Refusal> {
    let n = v
        .as_f64()
        .ok_or_else(|| refuse(field, format!("a number from {lo} to {hi}")))?;
    if !n.is_finite() || n < lo - crate::THRESHOLD_TOLERANCE || n > hi + crate::THRESHOLD_TOLERANCE
    {
        return Err(refuse(field, format!("{n} is not from {lo} to {hi}")));
    }
    Ok(n)
}

fn word<T>(
    v: &Value,
    field: &str,
    parse: fn(&str) -> Option<T>,
    choices: String,
) -> Result<T, Refusal> {
    v.as_str()
        .and_then(parse)
        .ok_or_else(|| refuse(field, format!("one of {choices}")))
}

/// The most kinds a list names, and the longest key.
pub const KINDS_MOST: usize = 500;
pub const KIND_KEY_MOST: usize = 80;

fn kind_list(v: &Value, field: &str) -> Result<Vec<String>, Refusal> {
    let list = v
        .as_array()
        .ok_or_else(|| refuse(field, "a list of kind keys"))?;
    if list.len() > KINDS_MOST {
        return Err(refuse(field, format!("at most {KINDS_MOST} kinds")));
    }
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for item in list {
        let key = item
            .as_str()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| refuse(field, "a list of kind keys, each a text"))?;
        if key.chars().count() > KIND_KEY_MOST {
            return Err(refuse(
                field,
                format!("a kind key of at most {KIND_KEY_MOST} characters"),
            ));
        }
        if !seen.insert(key) {
            return Err(refuse(field, format!("{key} is named twice")));
        }
        out.push(key.to_string());
    }
    Ok(out)
}

fn role_from_json(v: &Value, field: &str, default: &RoleRules) -> Result<RoleRules, Refusal> {
    let m = object(v, field)?;
    let prefix = format!("{field}.");
    known(m, &prefix, ROLE_KEYS)?;
    let at = |k: &str| format!("{prefix}{k}");
    let mut r = default.clone();
    if let Some(x) = m.get("keep_alike") {
        r.keep_alike = word(x, &at("keep_alike"), KeepAlike::parse, KeepAlike::choices())?;
    }
    if let Some(x) = m.get("kinds_in_order") {
        r.kinds_in_order = kind_list(x, &at("kinds_in_order"))?;
    }
    if let Some(x) = m.get("not_used") {
        r.not_used = kind_list(x, &at("not_used"))?;
    }
    if let Some(x) = m.get("contrast") {
        r.contrast = word(x, &at("contrast"), Contrast::parse, Contrast::choices())?;
    }
    if let Some(x) = m.get("dimension") {
        r.dimension = word(x, &at("dimension"), Dimension::parse, Dimension::choices())?;
    }
    if let Some(x) = m.get("body_part") {
        r.body_part = word(x, &at("body_part"), BodyPart::parse, BodyPart::choices())?;
    }
    if let Some(x) = m.get("slice_thickness_at_most_mm") {
        r.slice_thickness_at_most_mm = match x {
            Value::Null => None,
            n => Some(number_in(
                n,
                &at("slice_thickness_at_most_mm"),
                THICKNESS_MM,
            )?),
        };
    }
    Ok(r)
}

fn within_from_json(v: &Value, default: &Within) -> Result<Within, Refusal> {
    const FIELD: &str = "same_kind_in_one_visit";
    let m = object(v, FIELD)?;
    known(m, &format!("{FIELD}."), WITHIN_KEYS)?;
    let mut w = default.clone();
    if let Some(x) = m.get("weights") {
        let field = format!("{FIELD}.weights");
        let given = object(x, &field)?;
        let names: Vec<&str> = w.weights.named().iter().map(|(n, _, _)| *n).collect();
        known(given, &format!("{field}."), &names)?;
        for (name, value) in given {
            let n = number_in(value, &format!("{field}.{name}"), (0.0, 1.0))?;
            w.weights.set(name, n);
        }
    }
    if let Some(x) = m.get("derived_series_scores") {
        w.derived_series_scores =
            number_in(x, &format!("{FIELD}.derived_series_scores"), (0.0, 1.0))?;
    }
    if let Some(x) = m.get("near_tie_within_percent") {
        w.near_tie_within_percent = number_in(
            x,
            &format!("{FIELD}.near_tie_within_percent"),
            NEAR_TIE_PERCENT,
        )?;
    }
    if let Some(x) = m.get("near_tie_goes_to") {
        let field = format!("{FIELD}.near_tie_goes_to");
        let list = x
            .as_array()
            .ok_or_else(|| refuse(&field, format!("a list of {}", TieKey::choices())))?;
        let mut keys = Vec::with_capacity(list.len());
        for item in list {
            let k = word(item, &field, TieKey::parse, TieKey::choices())?;
            if keys.contains(&k) {
                return Err(refuse(field, format!("{} is named twice", k.name())));
            }
            keys.push(k);
        }
        w.near_tie_goes_to = keys;
    }
    Ok(w)
}

// ------------------------------------------------------------------- kinds

fn holds(csv: &str, token: &str) -> bool {
    csv.split(',').any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// The dimension a candidate states: `acquisition_type_filled`, else
/// `mr_acquisition_type`.
pub fn dimension_of(c: &Candidate) -> &str {
    ["acquisition_type_filled", "mr_acquisition_type"]
        .iter()
        .map(|n| c.get(n).trim())
        .find(|v| !v.is_empty())
        .unwrap_or("")
}

/// Whether a candidate's contrast was given: the post-contrast axis `1`.
pub fn contrast_given(c: &Candidate) -> bool {
    holds(c.get("post_contrast"), "1")
}

/// A candidate's kind (section 2): its dimension, its technique and its
/// contrast, one text that is the key and the label (`3D MPRAGE`,
/// `2D TSE +C`, `Unknown`).
pub fn kind_of(c: &Candidate) -> String {
    let technique = match c.get("technique").trim() {
        "" => UNKNOWN,
        t => t,
    };
    let mut out = match dimension_of(c) {
        "" => technique.to_string(),
        d => format!("{d} {technique}"),
    };
    if contrast_given(c) {
        out.push_str(CONTRAST);
    }
    out
}

/// A kind's parts, read back from its key: its dimension, whether contrast
/// was given, and its technique.
fn parts_of(kind: &str) -> (&str, bool, &str) {
    let (rest, contrast) = match kind.strip_suffix(CONTRAST) {
        Some(r) => (r, true),
        None => (kind, false),
    };
    match rest.split_once(' ') {
        Some((d, t))
            if d.len() >= 2
                && d.ends_with('D')
                && d[..d.len() - 1].chars().all(|c| c.is_ascii_digit()) =>
        {
            (d, contrast, t)
        }
        _ => ("", contrast, rest),
    }
}

/// The role's techniques by the pick's tier table, best first, the unknown
/// one last.
fn tiers(model: &Model, role: &str) -> Vec<String> {
    let table = model.components.iter().find_map(|c| match &c.kind {
        pick::Kind::Tier { per_role, .. } => per_role.get(role),
        _ => None,
    });
    let mut named: Vec<(&String, f64)> = table
        .map(|t| {
            t.iter()
                .filter(|(k, _)| *k != UNKNOWN)
                .map(|(k, v)| (k, *v))
                .collect()
        })
        .unwrap_or_default();
    named.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    named.into_iter().map(|(k, _)| k.clone()).collect()
}

/// Where a kind stands by the pack's order: its dimension (3D, 2D, another,
/// none), its contrast (without before with), its technique's tier (a
/// technique the table does not name after those it does, the unknown one
/// last), then its key.
fn default_rank(tiers: &[String], kind: &str) -> (u8, u8, usize, String) {
    let (d, contrast, technique) = parts_of(kind);
    let dim = match d {
        "3D" => 0,
        "2D" => 1,
        "" => 3,
        _ => 2,
    };
    let tier = if technique == UNKNOWN {
        tiers.len() + 1
    } else {
        tiers
            .iter()
            .position(|t| t == technique)
            .unwrap_or(tiers.len())
    };
    (dim, u8::from(contrast), tier, kind.to_string())
}

/// Version 1's kinds for a role: every dimension, contrast and technique of
/// the role's tier table, in the pack's order.
fn default_kinds(model: &Model, role: &str) -> Vec<String> {
    let mut techniques = tiers(model, role);
    techniques.push(UNKNOWN.to_string());
    let mut out = Vec::new();
    for dim in ["3D", "2D", ""] {
        for contrast in [false, true] {
            for t in &techniques {
                let mut k = if dim.is_empty() {
                    t.clone()
                } else {
                    format!("{dim} {t}")
                };
                if contrast {
                    k.push_str(CONTRAST);
                }
                out.push(k);
            }
        }
    }
    out
}

/// The order of a role's kinds under the rules: those the list names in its
/// order, then any other in the pack's order.
#[derive(Debug, Clone)]
pub struct KindOrder {
    named: BTreeMap<String, usize>,
    tiers: Vec<String>,
}

impl KindOrder {
    pub fn new(model: &Model, role: &str, rules: &RoleRules) -> KindOrder {
        KindOrder {
            named: rules
                .kinds_in_order
                .iter()
                .enumerate()
                .map(|(i, k)| (k.clone(), i))
                .collect(),
            tiers: tiers(model, role),
        }
    }

    /// Where a kind stands: the lower, the better.
    pub fn rank(&self, kind: &str) -> (usize, (u8, u8, usize, String)) {
        match self.named.get(kind) {
            Some(i) => (*i, (0, 0, 0, String::new())),
            None => (self.named.len(), default_rank(&self.tiers, kind)),
        }
    }

    /// Whether the list does not name it: a new kind, which the desk marks.
    pub fn is_new(&self, kind: &str) -> bool {
        !self.named.contains_key(kind)
    }

    /// These kinds, best first.
    pub fn sort(&self, kinds: &mut [String]) {
        kinds.sort_by_cached_key(|k| self.rank(k));
    }
}

// ------------------------------------------------------- what the rules allow

/// Whether a kind is used at all by a role's rules: not switched off, and
/// within the contrast and the dimension they allow.
pub fn kind_allowed(r: &RoleRules, kind: &str) -> bool {
    if r.not_used.iter().any(|k| k == kind) {
        return false;
    }
    let (dim, contrast, _) = parts_of(kind);
    let contrast_ok = match r.contrast {
        Contrast::Either => true,
        Contrast::Without => !contrast,
        Contrast::With => contrast,
    };
    let dim_ok = match r.dimension {
        Dimension::Any => true,
        Dimension::ThreeD => dim == "3D",
    };
    contrast_ok && dim_ok
}

/// Whether a candidate of a kind is allowed by a role's rules: its kind, and
/// its body part and slice thickness. A thickness nothing recorded is not
/// over a limit, because an absence is not a measurement.
pub fn allowed(r: &RoleRules, c: &Candidate, kind: &str) -> bool {
    if !kind_allowed(r, kind) {
        return false;
    }
    if r.body_part == BodyPart::Brain {
        let part = c.get("body_part").trim();
        if !part.is_empty() && !holds(part, "brain") && !holds(part, "brain-neck") {
            return false;
        }
    }
    if let Some(limit) = r.slice_thickness_at_most_mm
        && let Ok(t) = c.get("slice_thickness").trim().parse::<f64>()
        && t > limit + crate::THRESHOLD_TOLERANCE
    {
        return false;
    }
    true
}

// ------------------------------------------------------------- the choosing

/// What the rules chose for one visit of one subject and role.
#[derive(Debug, Clone, Default)]
pub struct Chosen {
    /// The kinds of every candidate of the visit, before the rules' allowed.
    pub held: Vec<String>,
    /// The kinds of its allowed candidates, best first by the rules' order.
    pub allowed: Vec<String>,
    /// The kind it takes; none when no candidate is allowed.
    pub kind: Option<String>,
    /// The candidate chosen within the kind, when it was scored.
    pub winner: Option<Candidate>,
    /// Its score: the pack's components under the rules' weights, with the
    /// derived factor as the penalty.
    pub scored: Option<pick::Scored>,
    pub runner_up_score: f64,
    /// How much of the winner's score separates it from the next, as a
    /// fraction; below nought where the tie order chose a lower score.
    pub margin: f64,
    /// The candidates of the kind and their scores, as they were ranked.
    pub considered: Vec<(Vec<i64>, f64)>,
    /// The tie key that decided between the first two, where one did.
    pub decided: Option<TieKey>,
}

/// One candidate as the rules see it: its kind, and for a candidate that
/// was one take of several, which candidate it came from and its moment.
#[derive(Debug, Clone)]
struct Take {
    c: Candidate,
    kind: String,
    /// The candidate it was taken apart from, by index, for the later full
    /// repeat.
    group: usize,
    moment: Option<String>,
}

/// The rules of a scope, ready to choose with.
#[derive(Debug, Clone)]
pub struct Chooser<'a> {
    pub rules: &'a Rules,
    pub model: &'a Model,
    /// The pick model with the rules' weights, `dim`, `tech` and `share` at
    /// nought and no penalty: the score within a kind.
    scoring: Model,
    /// The fraction of its largest slice count a stack must hold to be a
    /// full one, from the pick's retake border (v0's 0.5).
    partial_below: f64,
}

impl<'a> Chooser<'a> {
    pub fn new(rules: &'a Rules, model: &'a Model) -> Chooser<'a> {
        let mut scoring = model.clone();
        let named = rules.within.weights.named();
        for comp in &mut scoring.components {
            if let Some((_, _, w)) = named.iter().find(|(_, c, _)| *c == comp.name) {
                comp.weight = *w;
            } else if matches!(comp.name.as_str(), "dim" | "tech" | "share") {
                // equal within a kind
                comp.weight = 0.0;
            }
        }
        scoring.penalty = None;
        Chooser {
            rules,
            model,
            scoring,
            partial_below: model
                .borders
                .retake
                .as_ref()
                .map_or(0.5, |r| r.partial_below),
        }
    }

    /// The order of a role's kinds.
    pub fn order(&self, role: &str) -> Option<KindOrder> {
        self.rules
            .role(role)
            .map(|r| KindOrder::new(self.model, role, r))
    }

    /// Choose for one subject and role: `visits` are its visits in order,
    /// each the candidates the pack's role rules made of it (R5 and R6
    /// already applied). Each visit's kind is chosen by the rules' way of
    /// keeping alike; then, with a reference, the candidate within it. A
    /// visit without an allowed candidate takes nothing.
    pub fn choose(
        &self,
        role: &str,
        visits: &[Vec<Candidate>],
        reference: Option<&Reference>,
    ) -> Vec<Chosen> {
        let (Some(r), Some(order)) = (self.rules.role(role), self.order(role)) else {
            return visits.iter().map(|_| Chosen::default()).collect();
        };
        let mut out: Vec<Chosen> = Vec::with_capacity(visits.len());
        let mut options: Vec<Vec<Take>> = Vec::with_capacity(visits.len());
        for candidates in visits {
            let mut held: Vec<String> = Vec::new();
            let mut mine: Vec<Take> = Vec::new();
            for (i, c) in candidates.iter().enumerate() {
                let kind = kind_of(c);
                if !held.contains(&kind) {
                    held.push(kind.clone());
                }
                if !allowed(r, c, &kind) {
                    continue;
                }
                for (part, moment) in takes_apart(c) {
                    mine.push(Take {
                        c: part,
                        kind: kind.clone(),
                        group: i,
                        moment,
                    });
                }
            }
            order.sort(&mut held);
            let mut kinds: Vec<String> = Vec::new();
            for o in &mine {
                if !kinds.contains(&o.kind) {
                    kinds.push(o.kind.clone());
                }
            }
            order.sort(&mut kinds);
            out.push(Chosen {
                held,
                allowed: kinds,
                ..Chosen::default()
            });
            options.push(mine);
        }
        let kinds: Vec<Vec<String>> = out.iter().map(|c| c.allowed.clone()).collect();
        let taken = keep_alike(r.keep_alike, &kinds, |k| order.rank(k));
        for ((chosen, kind), mine) in out.iter_mut().zip(taken).zip(&options) {
            chosen.kind = kind;
            let (Some(kind), Some(reference)) = (&chosen.kind, reference) else {
                continue;
            };
            let of_kind: Vec<&Take> = mine.iter().filter(|o| &o.kind == kind).collect();
            self.within(role, &of_kind, reference, chosen);
        }
        out
    }

    /// A candidate's score within its kind.
    pub fn score(&self, role: &str, c: &Candidate, reference: &Reference) -> pick::Scored {
        let mut s = pick::score(&self.scoring, role, c, reference);
        if self.derived(c) {
            s.penalty = self.rules.within.derived_series_scores;
            s.score = (s.score * s.penalty).clamp(0.0, 1.0);
        }
        s
    }

    /// Whether a candidate is a derived series: its images say DERIVED, or
    /// its provenance is one the pick penalises or a derived one, or its
    /// construct is a reformat or a projection.
    pub fn derived(&self, c: &Candidate) -> bool {
        let image_derived = |v: &BTreeMap<String, String>| {
            v.get("image_type")
                .and_then(|t| t.split(['\\', ',']).next())
                .is_some_and(|t| t.trim().eq_ignore_ascii_case("DERIVED"))
        };
        let stacks: Vec<&BTreeMap<String, String>> = if c.each.is_empty() {
            vec![&c.values]
        } else {
            c.each.iter().collect()
        };
        if stacks.iter().all(|v| image_derived(v)) {
            return true;
        }
        let provenance = c.get("provenance");
        let penalised = self
            .model
            .penalty
            .as_ref()
            .is_some_and(|p| p.by_value.keys().any(|k| holds(c.get(&p.of), k)));
        penalised
            || provenance.split(',').any(|p| p.trim().ends_with("Derived"))
            || ["MPR", "MIP", "MinIP"]
                .iter()
                .any(|t| holds(c.get("construct"), t))
    }

    /// The candidate within one kind: by score, and within the near tie of
    /// the best by the tie order, the first key that tells two apart
    /// deciding; where none does, the higher score, then the lower stacks.
    fn within(&self, role: &str, of_kind: &[&Take], reference: &Reference, out: &mut Chosen) {
        if of_kind.is_empty() {
            return;
        }
        let mut scored: Vec<(usize, pick::Scored)> = of_kind
            .iter()
            .enumerate()
            .map(|(i, o)| (i, self.score(role, &o.c, reference)))
            .collect();
        let by_score = |a: &(usize, pick::Scored), b: &(usize, pick::Scored)| {
            b.1.score
                .total_cmp(&a.1.score)
                .then_with(|| of_kind[a.0].c.stacks.cmp(&of_kind[b.0].c.stacks))
        };
        scored.sort_by(by_score);
        let best = scored[0].1.score;
        let within = self.rules.within.near_tie_within_percent / 100.0;
        let near = scored
            .iter()
            .take_while(|(_, s)| {
                let gap = if best > 0.0 {
                    (best - s.score) / best
                } else {
                    0.0
                };
                gap <= within || crate::pack::at_threshold(gap, within)
            })
            .count();
        let keys = &self.rules.within.near_tie_goes_to;
        let by_keys = |a: &Take, b: &Take| {
            keys.iter()
                .map(|k| self.compare(*k, a, b))
                .find(|o| o.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        };
        scored[..near]
            .sort_by(|a, b| by_keys(of_kind[a.0], of_kind[b.0]).then_with(|| by_score(a, b)));
        if near >= 2 {
            let (a, b) = (of_kind[scored[0].0], of_kind[scored[1].0]);
            out.decided = keys
                .iter()
                .copied()
                .find(|k| self.compare(*k, a, b).is_ne());
        }
        out.considered = scored
            .iter()
            .map(|(i, s)| (of_kind[*i].c.stacks.clone(), s.score))
            .collect();
        let (first, s) = scored[0].clone();
        out.runner_up_score = scored.get(1).map_or(0.0, |(_, s)| s.score);
        out.margin = if s.score > 0.0 {
            (s.score - out.runner_up_score) / s.score
        } else {
            0.0
        };
        out.winner = Some(of_kind[first].c.clone());
        out.scored = Some(s);
    }

    /// How one tie key orders two candidates of one kind.
    fn compare(&self, key: TieKey, a: &Take, b: &Take) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        // a number first, the lower first, none after every number
        let low = |x: Option<f64>, y: Option<f64>| match (x, y) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        let num = |c: &Candidate, name: &str| c.get(name).trim().parse::<f64>().ok();
        match key {
            TieKey::AxialCoronalSagittal => {
                let plane =
                    |c: &Candidate| match c.get("orientation").trim().to_ascii_lowercase().as_str()
                    {
                        "axial" => 0,
                        "coronal" => 1,
                        "sagittal" => 2,
                        _ => 3,
                    };
                plane(&a.c).cmp(&plane(&b.c))
            }
            TieKey::ThinnerSlices => {
                low(num(&a.c, "slice_thickness"), num(&b.c, "slice_thickness"))
            }
            TieKey::DefaultReconstruction => {
                let other = |c: &Candidate| {
                    u8::from(
                        holds(c.get("construct"), "ND")
                            || holds(c.get("construct"), "ORIG")
                            || c.get("image_type")
                                .split(['\\', ','])
                                .any(|t| t.trim().eq_ignore_ascii_case("ND"))
                            || self.derived(c),
                    )
                };
                other(&a.c).cmp(&other(&b.c))
            }
            TieKey::LaterFullRepeat => {
                let full =
                    |x: &Take, y: &Take| match (num(&x.c, "n_instances"), num(&y.c, "n_instances"))
                    {
                        (Some(p), Some(q)) => {
                            let (lo, hi) = (p.min(q), p.max(q));
                            hi <= 0.0 || lo >= self.partial_below * hi
                        }
                        _ => true,
                    };
                match (&a.moment, &b.moment) {
                    (Some(x), Some(y)) if a.group == b.group && x != y && full(a, b) => y.cmp(x),
                    _ => Ordering::Equal,
                }
            }
            TieKey::EarlierSeries => {
                let first = |c: &Candidate| {
                    let each: Vec<f64> = c
                        .each
                        .iter()
                        .filter_map(|v| v.get("series_number")?.trim().parse().ok())
                        .collect();
                    each.into_iter()
                        .reduce(f64::min)
                        .or_else(|| num(c, "series_number"))
                };
                low(first(&a.c), first(&b.c))
            }
        }
    }
}

/// A candidate holding full takes acquired at different moments, taken
/// apart into one candidate per moment, so that a real repeat (R7) is two
/// candidates and not a question; any other as it is. Only where every
/// stack's moment and values are known.
fn takes_apart(c: &Candidate) -> Vec<(Candidate, Option<String>)> {
    let n = c.stacks.len();
    let moments: Option<Vec<&str>> = c.acquired.iter().map(Option::as_deref).collect();
    let whole = || vec![(c.clone(), None)];
    let Some(moments) = moments else {
        return whole();
    };
    if n < 2 || moments.len() != n || c.each.len() != n {
        return whole();
    }
    let mut by_moment: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, m) in moments.iter().enumerate() {
        by_moment.entry(m).or_default().push(i);
    }
    if by_moment.len() < 2 {
        return vec![(c.clone(), moments.first().map(|m| m.to_string()))];
    }
    by_moment
        .into_iter()
        .map(|(moment, idx)| {
            let each: Vec<BTreeMap<String, String>> =
                idx.iter().map(|i| c.each[*i].clone()).collect();
            (
                Candidate {
                    stacks: idx.iter().map(|i| c.stacks[*i]).collect(),
                    values: merged(&each),
                    each,
                    family: c.family.clone(),
                    acquired: idx.iter().map(|i| c.acquired[*i].clone()).collect(),
                },
                Some(moment.to_string()),
            )
        })
        .collect()
}

/// The values of several stacks as one candidate's: the first text, each
/// number at its largest, as the run merges a candidate's stacks.
fn merged(each: &[BTreeMap<String, String>]) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for v in each {
        for (name, value) in v {
            match out.get(name) {
                None => {
                    out.insert(name.clone(), value.clone());
                }
                Some(have) => {
                    if let (Ok(n), Ok(m)) = (value.parse::<f64>(), have.parse::<f64>())
                        && n > m
                    {
                        out.insert(name.clone(), value.clone());
                    }
                }
            }
        }
    }
    out
}

/// Section 3, step 2: the kind each visit takes, from the kinds each allows,
/// by the way of keeping alike, `rank` saying where a kind stands in the
/// rules' order (the lower, the better). A visit allowing nothing takes
/// nothing and is no visit of the subject's for the counting.
pub fn keep_alike<R: Ord>(
    way: KeepAlike,
    visits: &[Vec<String>],
    rank: impl Fn(&str) -> R,
) -> Vec<Option<String>> {
    let first = |v: &Vec<String>| v.iter().min_by_key(|k| rank(k)).cloned();
    let counted: Vec<&Vec<String>> = visits.iter().filter(|v| !v.is_empty()).collect();
    match way {
        KeepAlike::AcrossTheData => visits.iter().map(first).collect(),
        KeepAlike::WithinEachSubject => {
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for v in &counted {
                for k in v.iter() {
                    *counts.entry(k.as_str()).or_insert(0) += 1;
                }
            }
            // the most visits, and between as many the first in the order
            let kept = counts
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then_with(|| rank(b.0).cmp(&rank(a.0))))
                .map(|(k, _)| k.to_string());
            visits
                .iter()
                .map(|v| match &kept {
                    Some(k) if v.contains(k) => Some(k.clone()),
                    _ => first(v),
                })
                .collect()
        }
        KeepAlike::Balanced => {
            let all: BTreeSet<&String> = counted.iter().flat_map(|v| v.iter()).collect();
            let covering = all
                .into_iter()
                .filter(|k| counted.iter().all(|v| v.contains(k)))
                .min_by_key(|k| rank(k))
                .cloned();
            visits
                .iter()
                .map(|v| match &covering {
                    Some(k) if !v.is_empty() => Some(k.clone()),
                    _ => first(v),
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pick::{Borders, Component, Kind, Penalty, Retake};

    fn table(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    /// A model shaped as `picks/main.yml`, two roles.
    fn model() -> Model {
        let present = |name: &str, weight: f64| Component {
            name: name.into(),
            weight,
            kind: Kind::Present {
                of: vec!["image_orientation_patient".into()],
                base: 0.4,
                each: 0.6,
            },
        };
        Model {
            name: "main".into(),
            roles: vec!["t1w".into(), "flair".into()],
            components: vec![
                Component {
                    name: "dim".into(),
                    weight: 0.18,
                    kind: Kind::Choice {
                        of: "mr_acquisition_type".into(),
                        scores: table(&[("3D", 1.0), ("2D", 0.85)]),
                        missing: 0.4,
                        crowded_by: None,
                    },
                },
                Component {
                    name: "tech".into(),
                    weight: 0.15,
                    kind: Kind::Tier {
                        of: "technique".into(),
                        per_role: [
                            (
                                "t1w".to_string(),
                                table(&[
                                    ("MPRAGE", 1.0),
                                    ("SPACE", 0.85),
                                    ("SE", 0.5),
                                    ("Unknown", 0.3),
                                ]),
                            ),
                            ("flair".to_string(), table(&[("SPACE", 1.0), ("TSE", 0.9)])),
                        ]
                        .into_iter()
                        .collect(),
                        missing: 0.3,
                        bonuses: Vec::new(),
                    },
                },
                Component {
                    name: "slices".into(),
                    weight: 0.20,
                    kind: Kind::Percentile {
                        of: "n_instances".into(),
                        population: "slices".into(),
                        split_by: None,
                        missing: 0.4,
                        unknown: 0.6,
                    },
                },
                Component {
                    name: "mod".into(),
                    weight: 0.12,
                    kind: Kind::Tokens {
                        of: "modifier".into(),
                        base: 0.5,
                        per_role: BTreeMap::new(),
                    },
                },
                present("fov", 0.10),
                present("orient", 0.05),
                present("complete", 0.05),
            ],
            penalty: Some(Penalty {
                of: "provenance".into(),
                by_value: table(&[("EPIMix", 0.5), ("NeuroMix", 0.5)]),
            }),
            borders: Borders {
                runner_up_within: 0.05,
                retake: Some(Retake {
                    of: "n_instances".into(),
                    partial_below: 0.5,
                    partial_min_slices: 60.0,
                }),
                ..Borders::default()
            },
            same_acquisition: vec!["technique".into()],
            families: Vec::new(),
            candidates: BTreeMap::new(),
            near_tie: Vec::new(),
        }
    }

    fn candidate(stacks: &[i64], pairs: &[(&str, &str)]) -> Candidate {
        let values: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        Candidate {
            stacks: stacks.to_vec(),
            each: vec![values.clone(); stacks.len()],
            values,
            ..Candidate::default()
        }
    }

    fn kinds(list: &[&str]) -> Vec<String> {
        list.iter().map(|k| (*k).to_string()).collect()
    }

    #[test]
    fn version_one_is_the_pack_s_defaults() {
        let r = Rules::defaults(&model());
        assert_eq!(
            r.roles.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["t1w", "flair"]
        );
        let w = &r.within;
        assert_eq!(
            (
                w.weights.slice_count,
                w.weights.field_of_view,
                w.weights.modifiers
            ),
            (0.20, 0.10, 0.12)
        );
        assert_eq!(
            (w.weights.orientation, w.weights.completeness),
            (0.05, 0.05)
        );
        assert_eq!(w.derived_series_scores, 0.5);
        assert_eq!(w.near_tie_within_percent, 5.0);
        assert_eq!(w.near_tie_goes_to, TieKey::ALL);
        let t1 = r.role("t1w").unwrap();
        assert_eq!(t1.keep_alike, KeepAlike::Balanced);
        assert_eq!(
            (
                t1.contrast,
                t1.dimension,
                t1.body_part,
                t1.slice_thickness_at_most_mm
            ),
            (Contrast::Either, Dimension::Any, BodyPart::Brain, None)
        );
        // 3D before 2D, then without contrast before with, then the tiers,
        // the unknown technique last
        let pos = |k: &str| t1.kinds_in_order.iter().position(|x| x == k).unwrap();
        assert_eq!(
            t1.kinds_in_order[..4],
            kinds(&["3D MPRAGE", "3D SPACE", "3D SE", "3D Unknown"])
        );
        assert!(pos("3D Unknown") < pos("3D MPRAGE +C"));
        assert!(pos("3D SE +C") < pos("2D MPRAGE"));
        assert!(pos("2D Unknown +C") < pos("MPRAGE"));
        assert_eq!(t1.kinds_in_order.last().unwrap(), "Unknown +C");
        // a kind the list does not name comes after those it does, in the
        // pack's order, and is new
        let order = KindOrder::new(&model(), "t1w", t1);
        let mut held = kinds(&["2D EPI", "Unknown", "3D GRE", "2D SE"]);
        order.sort(&mut held);
        assert_eq!(held, kinds(&["2D SE", "Unknown", "3D GRE", "2D EPI"]));
        assert!(order.is_new("3D GRE") && !order.is_new("2D SE"));
    }

    #[test]
    fn a_kind_is_its_dimension_technique_and_contrast() {
        let c = |pairs: &[(&str, &str)]| kind_of(&candidate(&[1], pairs));
        assert_eq!(
            c(&[("mr_acquisition_type", "3D"), ("technique", "MPRAGE")]),
            "3D MPRAGE"
        );
        assert_eq!(
            c(&[
                ("acquisition_type_filled", "2D"),
                ("mr_acquisition_type", "3D"),
                ("technique", "TSE"),
                ("post_contrast", "1"),
            ]),
            "2D TSE +C"
        );
        assert_eq!(c(&[("post_contrast", "0")]), "Unknown");
        assert_eq!(parts_of("2D TSE +C"), ("2D", true, "TSE"));
        assert_eq!(parts_of("Unknown"), ("", false, "Unknown"));
    }

    #[test]
    fn a_refusal_names_the_field_and_what_is_left_out_is_the_default_s() {
        let d = Rules::defaults(&model());
        let refused = |doc: Value| Rules::from_json(&doc, &d).unwrap_err().field;
        assert_eq!(
            refused(json!({"same_kind_in_one_visit": {"weights": {"slice_count": 1.5}}})),
            "same_kind_in_one_visit.weights.slice_count"
        );
        assert_eq!(refused(json!({"roles": {"dwi": {}}})), "roles.dwi");
        assert_eq!(
            refused(json!({"same_kind_in_one_visit": {"near_tie_goes_to": ["coin_toss"]}})),
            "same_kind_in_one_visit.near_tie_goes_to"
        );
        assert_eq!(
            refused(json!({"same_kind_in_one_visit": {"near_tie_within_percent": 30}})),
            "same_kind_in_one_visit.near_tie_within_percent"
        );
        assert_eq!(
            refused(json!({"roles": {"t1w": {"slice_thickness_at_most_mm": 0.2}}})),
            "roles.t1w.slice_thickness_at_most_mm"
        );
        assert_eq!(
            refused(json!({"roles": {"t1w": {"keep_alike": "loosely"}}})),
            "roles.t1w.keep_alike"
        );
        assert_eq!(
            refused(json!({"roles": {"t1w": {"kinds_in_order": ["3D SE", "3D SE"]}}})),
            "roles.t1w.kinds_in_order"
        );
        assert_eq!(refused(json!({"colour": "blue"})), "colour");
        assert_eq!(refused(json!([])), "rules");
        // a partial document keeps the rest of the defaults, and a whole one
        // reads back as itself
        let r = Rules::from_json(
            &json!({"roles": {"flair": {"keep_alike": "across_the_data", "slice_thickness_at_most_mm": 3}}}),
            &d,
        )
        .unwrap();
        assert_eq!(
            r.role("flair").unwrap().keep_alike,
            KeepAlike::AcrossTheData
        );
        assert_eq!(
            r.role("flair").unwrap().slice_thickness_at_most_mm,
            Some(3.0)
        );
        assert_eq!(r.role("t1w"), d.role("t1w"));
        assert_eq!(Rules::from_json(&r.to_json(), &d).unwrap(), r);
        // the text is YAML with the comments, and reads back as the same
        let text = r.to_yaml();
        assert!(text.contains("keep_alike: across_the_data"), "{text}");
        assert!(
            text.contains("# within_each_subject | across_the_data | balanced"),
            "{text}"
        );
        assert!(text.contains("slice_thickness_at_most_mm: 3 "), "{text}");
        assert!(text.contains("# null means any"), "{text}");
        let back: Value = serde_saphyr::from_str(&text).unwrap();
        assert_eq!(Rules::from_json(&back, &d).unwrap(), r);
    }

    #[test]
    fn the_three_ways_keep_a_subject_s_visits_alike() {
        let v = |lists: &[&[&str]]| lists.iter().map(|l| kinds(l)).collect::<Vec<_>>();
        let order = ["A", "B", "C"];
        let got = |way, visits: &[Vec<String>]| -> Vec<Option<String>> {
            keep_alike(way, visits, |k| order.iter().position(|x| *x == k))
        };
        let some = |l: &[&str]| l.iter().map(|k| Some((*k).to_string())).collect::<Vec<_>>();
        use KeepAlike::*;
        // a subject with one visit gets the same answer from all three
        let one = v(&[&["A", "B"]]);
        for way in KeepAlike::ALL {
            assert_eq!(got(*way, &one), some(&["A"]), "{way:?}");
        }
        // B covers every visit: within and balanced keep it, across the
        // data takes each visit's first
        let covered = v(&[&["A", "B"], &["B"], &["A", "B"]]);
        assert_eq!(got(AcrossTheData, &covered), some(&["A", "B", "A"]));
        assert_eq!(got(WithinEachSubject, &covered), some(&["B", "B", "B"]));
        assert_eq!(got(Balanced, &covered), some(&["B", "B", "B"]));
        // no kind covers every visit: balanced takes each visit's first,
        // within keeps the kind of most visits where a visit has it
        let none = v(&[&["A", "B"], &["B"], &["B"], &["A"]]);
        assert_eq!(got(AcrossTheData, &none), some(&["A", "B", "B", "A"]));
        assert_eq!(got(Balanced, &none), some(&["A", "B", "B", "A"]));
        assert_eq!(got(WithinEachSubject, &none), some(&["B", "B", "B", "A"]));
        // a tie between kinds goes by the order
        let tied = v(&[&["A", "B"], &["A"], &["B"]]);
        assert_eq!(got(WithinEachSubject, &tied), some(&["A", "A", "B"]));
        // a visit that allows nothing takes nothing, and does not stop a
        // kind from covering the rest
        let empty = v(&[&["A", "B"], &[], &["B"]]);
        assert_eq!(
            got(Balanced, &empty),
            vec![Some("B".to_string()), None, Some("B".to_string())]
        );
    }

    #[test]
    fn the_rules_allow_by_kind_body_part_contrast_dimension_and_thickness() {
        let m = model();
        let mut rules = Rules::defaults(&m);
        let t1 = candidate(
            &[1],
            &[
                ("mr_acquisition_type", "3D"),
                ("technique", "MPRAGE"),
                ("slice_thickness", "1"),
            ],
        );
        let spine = candidate(
            &[2],
            &[
                ("mr_acquisition_type", "2D"),
                ("technique", "SE"),
                ("body_part", "spine"),
            ],
        );
        let post = candidate(
            &[3],
            &[
                ("mr_acquisition_type", "3D"),
                ("technique", "SPACE"),
                ("post_contrast", "1"),
                ("body_part", "brain-neck"),
                ("slice_thickness", "5"),
            ],
        );
        let visits = vec![vec![t1.clone(), spine.clone(), post.clone()]];
        let r = |rules: &Rules| {
            Chooser::new(rules, &m)
                .choose("t1w", &visits, None)
                .remove(0)
        };
        let got = r(&rules);
        assert_eq!(got.held, kinds(&["3D MPRAGE", "3D SPACE +C", "2D SE"]));
        assert_eq!(
            got.allowed,
            kinds(&["3D MPRAGE", "3D SPACE +C"]),
            "spine is no brain"
        );
        assert_eq!(got.kind.as_deref(), Some("3D MPRAGE"));
        // any body part, the spine too; without contrast, no post scan
        let t = &mut rules.roles[0].1;
        t.body_part = BodyPart::Any;
        t.contrast = Contrast::Without;
        assert_eq!(r(&rules).allowed, kinds(&["3D MPRAGE", "2D SE"]));
        // only 3D, a kind switched off, and a thickness limit: a thickness
        // nothing recorded is not over it
        let t = &mut rules.roles[0].1;
        t.contrast = Contrast::Either;
        t.dimension = Dimension::ThreeD;
        t.not_used = kinds(&["3D MPRAGE"]);
        t.slice_thickness_at_most_mm = Some(3.0);
        let got = r(&rules);
        assert!(got.allowed.is_empty(), "{:?}", got.allowed);
        assert_eq!(got.kind, None);
        let t = &mut rules.roles[0].1;
        t.not_used.clear();
        t.dimension = Dimension::Any;
        let thick_unknown = candidate(&[4], &[("mr_acquisition_type", "2D"), ("technique", "TSE")]);
        let got = Chooser::new(&rules, &m)
            .choose("t1w", &[vec![post, thick_unknown]], None)
            .remove(0);
        assert_eq!(
            got.allowed,
            kinds(&["2D TSE"]),
            "a new kind is used, at the end"
        );
    }

    #[test]
    fn a_near_tie_within_a_kind_goes_by_the_tie_keys() {
        let m = model();
        let mut rules = Rules::defaults(&m);
        let r = Reference::default();
        let base = [
            ("mr_acquisition_type", "3D"),
            ("technique", "MPRAGE"),
            ("n_instances", "176"),
        ];
        let with = |stacks: &[i64], more: &[(&str, &str)]| {
            let mut pairs: Vec<(&str, &str)> = base.to_vec();
            pairs.extend_from_slice(more);
            candidate(stacks, &pairs)
        };
        let pick = |rules: &Rules, cands: Vec<Candidate>| {
            Chooser::new(rules, &m)
                .choose("t1w", &[cands], Some(&r))
                .remove(0)
        };
        // axial, then coronal, then sagittal
        let got = pick(
            &rules,
            vec![
                with(&[1], &[("orientation", "Sagittal")]),
                with(&[2], &[("orientation", "Axial")]),
            ],
        );
        assert_eq!(got.winner.unwrap().stacks, [2]);
        assert_eq!(got.decided, Some(TieKey::AxialCoronalSagittal));
        // the thinner slices, where the planes are equal
        let got = pick(
            &rules,
            vec![
                with(&[1], &[("slice_thickness", "1.2")]),
                with(&[2], &[("slice_thickness", "1.0")]),
            ],
        );
        assert_eq!(got.winner.unwrap().stacks, [2]);
        assert_eq!(got.decided, Some(TieKey::ThinnerSlices));
        // the scanner's default reconstruction before ND
        let got = pick(
            &rules,
            vec![
                with(&[1], &[("image_type", "ORIGINAL\\PRIMARY\\M\\ND\\NORM")]),
                with(&[2], &[("image_type", "ORIGINAL\\PRIMARY\\M\\NORM")]),
            ],
        );
        assert_eq!(got.winner.unwrap().stacks, [2]);
        assert_eq!(got.decided, Some(TieKey::DefaultReconstruction));
        // a real repeat: one candidate of two full takes at different
        // moments is two, and the later stands
        let mut repeat = with(&[5, 6], &[]);
        repeat.acquired = vec![
            Some("20240101 101500".into()),
            Some("20240101 100900".into()),
        ];
        let got = pick(&rules, vec![repeat]);
        assert_eq!(got.winner.unwrap().stacks, [5]);
        assert_eq!(got.decided, Some(TieKey::LaterFullRepeat));
        assert_eq!(got.considered.len(), 2);
        // the earlier series, last
        let got = pick(
            &rules,
            vec![
                with(&[1], &[("series_number", "9")]),
                with(&[2], &[("series_number", "4")]),
            ],
        );
        assert_eq!(got.winner.unwrap().stacks, [2]);
        assert_eq!(got.decided, Some(TieKey::EarlierSeries));
        // the order of the keys is the rules': thinner before the plane
        rules.within.near_tie_goes_to = vec![TieKey::ThinnerSlices, TieKey::AxialCoronalSagittal];
        let got = pick(
            &rules,
            vec![
                with(&[1], &[("orientation", "Axial"), ("slice_thickness", "2")]),
                with(
                    &[2],
                    &[("orientation", "Sagittal"), ("slice_thickness", "1")],
                ),
            ],
        );
        assert_eq!(got.winner.unwrap().stacks, [2]);
        // outside the near tie the score decides, and a derived series
        // scores the derived factor of an original one
        rules.within.near_tie_within_percent = 0.0;
        let derived = with(
            &[1],
            &[
                ("image_type", "DERIVED\\PRIMARY\\MPR"),
                ("orientation", "Axial"),
            ],
        );
        let original = with(&[2], &[("orientation", "Sagittal")]);
        let chooser = Chooser::new(&rules, &m);
        let (d, o) = (
            chooser.score("t1w", &derived, &r),
            chooser.score("t1w", &original, &r),
        );
        assert!((d.score - 0.5 * o.score).abs() < 1e-9, "{d:?} {o:?}");
        assert_eq!(d.penalty, 0.5);
        let got = pick(&rules, vec![derived, original]);
        assert_eq!(got.winner.unwrap().stacks, [2]);
        assert_eq!(got.decided, None);
        assert!(got.margin > 0.4, "{}", got.margin);
        // dim, tech and share weigh nothing within a kind
        let scoring = &chooser.scoring;
        for c in &scoring.components {
            match c.name.as_str() {
                "dim" | "tech" => assert_eq!(c.weight, 0.0),
                "slices" => assert_eq!(c.weight, 0.20),
                _ => {}
            }
        }
        assert!(scoring.penalty.is_none());
    }
}
