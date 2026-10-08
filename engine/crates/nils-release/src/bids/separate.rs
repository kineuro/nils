// SPDX-License-Identifier: AGPL-3.0-only

//! A name for every scan (Wave 7a §8.1, record 55 C4): what tells apart the
//! stacks that built one BIDS name and are not one acquisition made again.
//!
//! Until Wave 7a such stacks were refused their BIDS name and routed to
//! `sourcedata/`, 434 of them on record 34's corpus. The rule now is that **a
//! release is never refused for a name conflict**. In order:
//!
//! 1. a true repeat stays a `run-` (the caller, with [`super::repeat`]);
//! 2. a difference among the classification axes is named by the axis's value;
//! 3. any other difference is named by the property and its value, from the
//!    measured fields [`super::repeat::differences`] compares;
//! 4. the last fallback is a plain number, never called a run.
//!
//! Each is one [`Mark`]: a token and the slot it goes in (record 55 C4). In a
//! BIDS name, in either naming mode, the token is a piece of `acq-` in its
//! slot: an axis in the axis's own place among the pack's groups, a property
//! after them (`3mm`, `TR2000`), the number last; strict BIDS admits no new
//! entity. In the descriptive layout the token is a slot of its own, `_3mm`,
//! before the plain `_<n>`.
//!
//! **No text makes a name.** The protocol name, the series description, the
//! sequence name and every other free text are compared by the repeat test
//! and never spelled here: a pair they alone separate falls to a number.

use std::collections::BTreeMap;

use nils_pack::bids::Mapping;

use super::repeat::Acquisition;

/// What decided a mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// A classification axis the pack decided.
    Axis,
    /// A measured property of the acquisition.
    Property,
    /// Nothing that can be spelled: the last fallback.
    Number,
}

/// One thing a name carries to tell it from the names beside it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Mark {
    pub by: By,
    /// The axis or the property, as the release record states it:
    /// `quality`, `SliceThickness`, `number`.
    pub property: String,
    /// The value, as the release record states it: `Distorted`, `3`, `2`.
    pub value: String,
    /// What a name spells: `Distorted`, `3mm`, `TR2000`, `2`.
    pub token: String,
    /// Where in `acq-` it goes ([`super::name::slot_of`]).
    #[serde(skip)]
    pub slot: usize,
}

impl Mark {
    /// The fallback: a plain number, counted from 1 in the order the group
    /// was made.
    pub fn number(n: i64) -> Mark {
        Mark {
            by: By::Number,
            property: "number".to_string(),
            value: n.to_string(),
            token: n.to_string(),
            slot: super::name::SLOT_NUMBER,
        }
    }
}

/// Axes that say what a release does with a stack, or what a person said
/// about it, and not what the scan is: they never separate two names.
const NOT_A_DIFFERENCE: &[&str] = &[
    "convertible",
    "directory_type",
    "disposition",
    "role",
    "task",
];

/// One stack of a colliding group: what the repeat test read of it, if it was
/// measured, and every axis value it states, as identities.
pub struct Member<'a> {
    pub acquisition: Option<&'a Acquisition>,
    pub said: Option<&'a BTreeMap<String, Vec<String>>>,
}

/// A measured property a name may carry, in the order they are tried: the
/// slice thickness first, because it is the difference a reader looks for
/// first, then the timings, then the rest of the geometry and the counts.
struct Property {
    /// The DICOM-like word, which the release record states.
    name: &'static str,
    /// Around the value in the BIDS mode's short token.
    prefix: &'static str,
    unit: &'static str,
    read: fn(&Acquisition) -> Option<String>,
}

const PROPERTIES: &[Property] = &[
    Property {
        name: "SliceThickness",
        prefix: "",
        unit: "mm",
        read: |a| a.slice_thickness.map(number),
    },
    Property {
        name: "RepetitionTime",
        prefix: "TR",
        unit: "",
        read: |a| a.repetition_time.map(number),
    },
    Property {
        name: "EchoTime",
        prefix: "TE",
        unit: "",
        read: |a| a.echo_time.map(number),
    },
    Property {
        name: "InversionTime",
        prefix: "TI",
        unit: "",
        read: |a| a.inversion_time.map(number),
    },
    Property {
        name: "FlipAngle",
        prefix: "FA",
        unit: "",
        read: |a| a.flip_angle.map(number),
    },
    Property {
        name: "BValue",
        prefix: "b",
        unit: "",
        read: |a| a.b_value.map(number),
    },
    Property {
        name: "SpacingBetweenSlices",
        prefix: "Sp",
        unit: "mm",
        read: |a| a.slice_spacing.map(number),
    },
    Property {
        name: "PixelSpacing",
        prefix: "Px",
        unit: "mm",
        read: |a| match (a.pixel_spacing_row, a.pixel_spacing_col) {
            (Some(r), Some(c)) if number(r) == number(c) => Some(number(r)),
            (Some(r), Some(c)) => Some(format!("{}x{}", number(r), number(c))),
            _ => None,
        },
    },
    Property {
        name: "Matrix",
        prefix: "",
        unit: "",
        read: |a| match (a.rows, a.columns) {
            (Some(r), Some(c)) => Some(format!("{r}x{c}")),
            _ => None,
        },
    },
    Property {
        name: "Slices",
        prefix: "",
        unit: "sl",
        read: |a| a.coverage.n_slices.map(|n| n.to_string()),
    },
    Property {
        name: "Images",
        prefix: "",
        unit: "img",
        read: |a| a.images.map(|n| n.to_string()),
    },
    Property {
        name: "AcquisitionType",
        prefix: "",
        unit: "",
        read: |a| a.acquisition_type.as_deref().map(word),
    },
    Property {
        name: "FieldStrength",
        prefix: "",
        unit: "T",
        read: |a| a.field_strength.map(number),
    },
    Property {
        name: "EchoTrainLength",
        prefix: "ETL",
        unit: "",
        read: |a| a.echo_train_length.map(|n| n.to_string()),
    },
    Property {
        name: "Averages",
        prefix: "Avg",
        unit: "",
        read: |a| a.averages.map(number),
    },
    Property {
        name: "PixelBandwidth",
        prefix: "BW",
        unit: "",
        read: |a| a.pixel_bandwidth.map(number),
    },
    Property {
        name: "Directions",
        prefix: "",
        unit: "dir",
        read: |a| a.directions.map(|n| n.to_string()),
    },
    Property {
        name: "EchoNumbers",
        prefix: "E",
        unit: "",
        read: |a| a.echo_numbers.as_deref().map(word),
    },
    Property {
        name: "TemporalPosition",
        prefix: "TP",
        unit: "",
        read: |a| a.temporal_position.map(|n| n.to_string()),
    },
    Property {
        name: "TemporalPositions",
        prefix: "NTP",
        unit: "",
        read: |a| a.temporal_positions.map(|n| n.to_string()),
    },
];

/// What tells the members of a group apart, one mark per member, or `None`
/// when nothing that can be spelled does: an axis first, then a measured
/// property. A member that states nothing on the deciding axis gets no mark
/// and keeps its name; every other member says its value.
///
/// One mark at a time, the first that separates any two members. The caller
/// regroups and asks again, so a group of three of which two are one
/// acquisition made again ends as `acq-…3mm_run-1`, `acq-…3mm_run-2` and
/// `acq-…5mm`.
pub fn separator(members: &[Member], map: &Mapping) -> Option<Vec<Option<Mark>>> {
    by_axis(members, map).or_else(|| by_property(members))
}

fn by_axis(members: &[Member], map: &Mapping) -> Option<Vec<Option<Mark>>> {
    // The pack's own order first, the order `acq-` is joined in, then any
    // other axis by name.
    let mut axes: Vec<&str> = Vec::new();
    for group in &map.acq {
        if !axes.contains(&group.from.as_str()) {
            axes.push(group.from.as_str());
        }
    }
    let mut rest: Vec<&str> = members
        .iter()
        .filter_map(|m| m.said)
        .flat_map(|s| s.keys().map(String::as_str))
        .filter(|a| !axes.contains(a))
        .collect();
    rest.sort_unstable();
    rest.dedup();
    axes.extend(rest);

    for axis in axes {
        if NOT_A_DIFFERENCE.contains(&axis) {
            continue;
        }
        let values: Vec<Option<&Vec<String>>> = members
            .iter()
            .map(|m| m.said.and_then(|s| s.get(axis)).filter(|v| !v.is_empty()))
            .collect();
        let tokens: Vec<Option<String>> = values
            .iter()
            .map(|v| {
                v.map(|values| {
                    values
                        .iter()
                        .map(|value| axis_token(map, axis, value))
                        .collect::<String>()
                })
                .filter(|t| !t.is_empty())
            })
            .collect();
        if all_same(&tokens) {
            continue;
        }
        return Some(
            values
                .iter()
                .zip(&tokens)
                .map(|(v, t)| {
                    let (Some(values), Some(token)) = (v, t) else {
                        return None;
                    };
                    Some(Mark {
                        by: By::Axis,
                        property: axis.to_string(),
                        value: values.join(","),
                        token: token.clone(),
                        slot: super::name::slot_of(map, axis),
                    })
                })
                .collect(),
        );
    }
    None
}

fn by_property(members: &[Member]) -> Option<Vec<Option<Mark>>> {
    for (at, p) in PROPERTIES.iter().enumerate() {
        let values: Vec<Option<String>> = members
            .iter()
            .map(|m| {
                m.acquisition
                    .and_then(|a| (p.read)(a))
                    .filter(|v| !v.is_empty())
            })
            .collect();
        // Measured on every member, or it says nothing about the one that
        // was not: an absence is not a measurement.
        if values.iter().any(Option::is_none) || all_same(&values) {
            continue;
        }
        return Some(
            values
                .into_iter()
                .map(|v| {
                    v.map(|value| Mark {
                        by: By::Property,
                        property: p.name.to_string(),
                        token: format!("{}{value}{}", p.prefix, p.unit),
                        slot: super::name::SLOT_PROPERTY + at,
                        value,
                    })
                })
                .collect(),
        );
    }
    None
}

fn all_same<T: PartialEq>(values: &[T]) -> bool {
    values.windows(2).all(|w| w[0] == w[1])
}

/// An axis value as a name token: the pack's own token where it declares
/// one, in either mode, and otherwise the value with a capital and with
/// everything a BIDS label refuses taken out.
fn axis_token(map: &Mapping, axis: &str, value: &str) -> String {
    map.acq
        .iter()
        .filter(|g| g.from == axis)
        .find_map(|g| g.tokens.get(value).cloned())
        .unwrap_or_else(|| camel(value))
}

/// `post_contrast` as `PostContrast`, `T2*w` as `T2w`: words joined with a
/// capital each, letters and digits only.
fn camel(text: &str) -> String {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// A short text value, letters and digits only: `3D`, `1\2` as `12`.
fn word(text: &str) -> String {
    text.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// A measured number as a label may spell it: at most two decimals, no
/// trailing zeros, the point as `p` and a minus as `m`. `2.5` is `2p5`,
/// `2000.0` is `2000`.
pub fn number(x: f64) -> String {
    let text = format!("{:.2}", x);
    let text = match text.contains('.') {
        true => text.trim_end_matches('0').trim_end_matches('.').to_string(),
        false => text,
    };
    let text = if text == "-0" { "0".to_string() } else { text };
    text.replace('.', "p").replace('-', "m")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_pack::bids::Tokens;

    fn mapping() -> Mapping {
        Mapping {
            acq: vec![Tokens {
                from: "quality".into(),
                modes: vec!["informative".into()],
                tokens: [("Distorted".to_string(), "Distorted".to_string())]
                    .into_iter()
                    .collect(),
            }],
            ..Mapping::default()
        }
    }

    fn said(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (k, v) in pairs {
            out.entry(k.to_string()).or_default().push(v.to_string());
        }
        out
    }

    #[test]
    fn numbers_are_spelled_in_letters_and_digits() {
        assert_eq!(number(3.0), "3");
        assert_eq!(number(2.5), "2p5");
        assert_eq!(number(2.46), "2p46");
        assert_eq!(number(2000.0), "2000");
        assert_eq!(number(0.8999), "0p9");
        assert_eq!(number(-1.5), "m1p5");
    }

    #[test]
    fn an_axis_comes_before_any_property() {
        let a = Acquisition {
            slice_thickness: Some(3.0),
            ..Acquisition::default()
        };
        let b = Acquisition {
            slice_thickness: Some(5.0),
            ..Acquisition::default()
        };
        let one = said(&[("base", "T1w")]);
        let two = said(&[("base", "T1w"), ("quality", "Distorted")]);
        let marks = separator(
            &[
                Member {
                    acquisition: Some(&a),
                    said: Some(&one),
                },
                Member {
                    acquisition: Some(&b),
                    said: Some(&two),
                },
            ],
            &mapping(),
        )
        .unwrap();
        assert_eq!(marks[0], None, "the one that states nothing keeps its name");
        let m = marks[1].as_ref().unwrap();
        assert_eq!(m.by, By::Axis);
        assert_eq!(m.token, "Distorted");
        assert_eq!(m.slot, 0, "in the quality group's own slot");
    }

    #[test]
    fn a_property_is_named_with_its_value() {
        let a = Acquisition {
            slice_thickness: Some(3.0),
            repetition_time: Some(2000.0),
            ..Acquisition::default()
        };
        let b = Acquisition {
            slice_thickness: Some(2.5),
            repetition_time: Some(2300.0),
            ..Acquisition::default()
        };
        let marks = separator(
            &[
                Member {
                    acquisition: Some(&a),
                    said: None,
                },
                Member {
                    acquisition: Some(&b),
                    said: None,
                },
            ],
            &mapping(),
        )
        .unwrap();
        let spelled: Vec<(&str, &str)> = marks
            .iter()
            .map(|m| {
                let m = m.as_ref().unwrap();
                (m.property.as_str(), m.token.as_str())
            })
            .collect();
        assert_eq!(
            spelled,
            [("SliceThickness", "3mm"), ("SliceThickness", "2p5mm")]
        );
    }

    #[test]
    fn a_property_one_member_did_not_record_separates_nothing() {
        let a = Acquisition {
            slice_thickness: Some(3.0),
            echo_time: Some(30.0),
            ..Acquisition::default()
        };
        let b = Acquisition {
            echo_time: Some(90.0),
            ..Acquisition::default()
        };
        let marks = separator(
            &[
                Member {
                    acquisition: Some(&a),
                    said: None,
                },
                Member {
                    acquisition: Some(&b),
                    said: None,
                },
            ],
            &mapping(),
        )
        .unwrap();
        assert_eq!(marks[0].as_ref().unwrap().token, "TE30");
        assert_eq!(marks[1].as_ref().unwrap().property, "EchoTime");
    }

    #[test]
    fn text_alone_separates_nothing() {
        let a = Acquisition {
            protocol: Some("t1 mprage".into()),
            description: Some("a".into()),
            ..Acquisition::default()
        };
        let b = Acquisition {
            protocol: Some("t1 mprage 2".into()),
            description: Some("b".into()),
            ..Acquisition::default()
        };
        let one = said(&[("disposition", "acquisition")]);
        let two = said(&[("disposition", "reformat")]);
        assert_eq!(
            separator(
                &[
                    Member {
                        acquisition: Some(&a),
                        said: Some(&one),
                    },
                    Member {
                        acquisition: Some(&b),
                        said: Some(&two),
                    },
                ],
                &mapping(),
            ),
            None
        );
    }
}
