// SPDX-License-Identifier: AGPL-3.0-only

//! An enhanced multi-frame object read whole (record 37, S8).
//!
//! v0, and NILS until now, read the shared functional groups and the **first
//! item** of the per-frame sequence, and treated one file as one instance in
//! one stack. About one enhanced object in ten holds more than one stack
//! inside it: frames with different orientations, echoes or acquisitions. This
//! module reads every per-frame item and groups the frames by what a stack is
//! made of, so that the digest can make one stack per group.
//!
//! What a frame contributes is what an instance contributes: the stack-level
//! columns of the catalogue (§8 of `docs/specs/wave1-parse-and-digest.md`). A
//! column whose source never enters the per-frame groups cannot vary frame to
//! frame, so it is taken once from the file; the rest are resolved per frame,
//! with the shared groups as the fallback they already are.

use dicom_core::DicomValue;
use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;

use crate::catalogue::{Level, Source, Step, fields_of};
use crate::charset::Charset;
use crate::value::{Value, convert};

/// How many stacks one file may be split into. A file that states more
/// distinct stacks than this is read as one, as before, and says so with a
/// diagnostic: a registry is not the place to discover that a single object
/// claims a thousand identities.
pub const GROUPS_MAX: usize = 64;

/// One run of frames of a multi-frame object that carry the same stack-level
/// values, and so belong in one stack.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameGroup {
    /// One slot per stack-level catalogue row, in catalogue order.
    pub values: Vec<Option<Value>>,
    /// The frame numbers in this group, counting from one, as ascending
    /// inclusive ranges.
    pub ranges: Vec<(u32, u32)>,
    /// How many frames are in the group.
    pub count: u32,
}

impl FrameGroup {
    /// The value of a stack-level column, by name.
    pub fn value(&self, column: &str) -> Option<&Value> {
        fields_of(Level::Stack)
            .position(|(_, f)| f.column == column)
            .and_then(|i| self.values.get(i))
            .and_then(Option::as_ref)
    }

    /// The first frame of the group, counting from one.
    pub fn first_frame(&self) -> u32 {
        self.ranges.first().map(|(a, _)| *a).unwrap_or(0)
    }

    /// The frames as a list a person can read: `1-4,9,12-20`.
    pub fn list(&self) -> String {
        let mut out = String::new();
        for (i, (a, b)) in self.ranges.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            if a == b {
                out.push_str(&a.to_string());
            } else {
                out.push_str(&format!("{a}-{b}"));
            }
        }
        out
    }

    fn add(&mut self, frame: u32) {
        self.count += 1;
        match self.ranges.last_mut() {
            Some(last) if last.1 + 1 == frame => last.1 = frame,
            _ => self.ranges.push((frame, frame)),
        }
    }
}

/// What the frames of one file say about its stacks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frames {
    /// Items in the per-frame functional groups sequence; zero when the file
    /// has none.
    pub count: u32,
    /// One group per distinct set of stack-level values, in the order the
    /// frames first show them. Empty when the file has fewer than two frames,
    /// which is every classic instance.
    pub groups: Vec<FrameGroup>,
    /// The file held more distinct groups than [`GROUPS_MAX`]: `groups` is
    /// empty and the file is read as one stack, from its first frame.
    pub capped: bool,
}

impl Frames {
    /// True when the file's frames belong in more than one stack by their raw
    /// values. Two groups may still round to one signature, which the digest
    /// decides.
    pub fn split(&self) -> bool {
        self.groups.len() > 1
    }
}

/// The per-frame items of a data set, when it has any.
fn per_frame(dataset: &InMemDicomObject) -> Option<&[InMemDicomObject]> {
    match dataset
        .get(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)?
        .value()
    {
        DicomValue::Sequence(seq) => Some(seq.items()),
        _ => None,
    }
}

/// Whether a column's source can read a per-frame item at all. One that
/// cannot is the same for every frame, so it is read once.
fn per_frame_source(source: Source) -> bool {
    match source {
        Source::Chain(steps) => steps
            .iter()
            .any(|s| matches!(s, Step::Fg(_, _) | Step::Private(_))),
        _ => false,
    }
}

/// Group the frames of `dataset` by their stack-level values. `values` are the
/// file's own catalogue values, already extracted, which every column that
/// cannot vary per frame takes.
pub fn frame_groups(
    dataset: &InMemDicomObject,
    charset: &Charset,
    values: &[Option<Value>],
) -> Frames {
    let Some(items) = per_frame(dataset) else {
        return Frames::default();
    };
    let count = items.len() as u32;
    if items.len() < 2 {
        return Frames {
            count,
            ..Frames::default()
        };
    }
    // The stack-level columns, in catalogue order, with the file's value for
    // the ones no frame can change.
    let stack: Vec<(usize, Source, crate::Converter, bool)> = fields_of(Level::Stack)
        .map(|(i, f)| (i, f.source, f.converter, per_frame_source(f.source)))
        .collect();
    let fixed: Vec<Option<Value>> = stack
        .iter()
        .map(|(i, _, _, varies)| match varies {
            true => None,
            false => values.get(*i).cloned().flatten(),
        })
        .collect();
    let mut groups: Vec<FrameGroup> = Vec::new();
    let mut row: Vec<Option<Value>> = Vec::with_capacity(stack.len());
    for (frame, _) in items.iter().enumerate() {
        row.clear();
        for (slot, (_, source, converter, varies)) in stack.iter().enumerate() {
            if !varies {
                row.push(fixed[slot].clone());
                continue;
            }
            let Source::Chain(steps) = source else {
                row.push(None);
                continue;
            };
            let mut found = None;
            for step in *steps {
                if let Some(e) = crate::extract::resolve_at(dataset, step, frame) {
                    let c = convert(*converter, e, charset);
                    // the file-level rule (§6.2): the first step that has
                    // something to say wins, even when what it says is
                    // unreadable, which the file's own extraction reported
                    if c.value.is_some() || c.invalid.is_some() {
                        found = c.value;
                        break;
                    }
                }
            }
            row.push(found);
        }
        let number = frame as u32 + 1;
        match groups.iter_mut().find(|g| g.values == row) {
            Some(g) => g.add(number),
            None => {
                if groups.len() == GROUPS_MAX {
                    return Frames {
                        count,
                        groups: Vec::new(),
                        capped: true,
                    };
                }
                groups.push(FrameGroup {
                    values: row.clone(),
                    ranges: vec![(number, number)],
                    count: 1,
                });
            }
        }
    }
    Frames {
        count,
        groups,
        capped: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{self, Elem};
    use dicom_core::VR;

    fn extracted(meta: synth::MetaFields, elems: Vec<Elem>) -> crate::Extracted {
        let dir = synth::TempDir::new("frames");
        let path = dir.file("a.dcm", &synth::part10(&meta, &elems, true));
        crate::extract(&path).unwrap()
    }

    fn enhanced(shared: Vec<Elem>, per_frame: Vec<Vec<Elem>>) -> crate::Extracted {
        extracted(
            synth::enhanced_meta("1.2.3.4"),
            synth::enhanced_mr("1.2.3", "1.2.3.1", "1.2.3.4", shared, per_frame),
        )
    }

    /// An enhanced object whose frames are all alike is one group.
    #[test]
    fn frames_that_agree_are_one_group() {
        let x = enhanced(
            vec![synth::fg_orientation("1\\0\\0\\0\\1\\0")],
            (0..8).map(|_| Vec::new()).collect(),
        );
        assert_eq!(x.frames.count, 8);
        assert_eq!(x.frames.groups.len(), 1);
        assert!(!x.frames.split());
        let g = &x.frames.groups[0];
        assert_eq!(g.count, 8);
        assert_eq!(g.list(), "1-8");
        assert_eq!(
            g.value("image_orientation_patient").map(Value::to_string),
            Some("1\\0\\0\\0\\1\\0".to_string())
        );
    }

    /// Two orientations across the frames of one file are two groups, each
    /// with the frames that hold it.
    #[test]
    fn two_orientations_in_one_file_are_two_groups() {
        let per_frame: Vec<Vec<Elem>> = (0..6)
            .map(|i| {
                vec![synth::fg_orientation(match i < 3 {
                    true => "1\\0\\0\\0\\1\\0",
                    false => "0\\1\\0\\0\\0\\-1",
                })]
            })
            .collect();
        let x = enhanced(Vec::new(), per_frame);
        assert!(x.frames.split());
        assert_eq!(x.frames.groups.len(), 2);
        assert_eq!(x.frames.groups[0].list(), "1-3");
        assert_eq!(x.frames.groups[1].list(), "4-6");
        assert_eq!(x.frames.groups[1].first_frame(), 4);
    }

    /// Frames that alternate keep their numbers, and the shared groups are
    /// the fallback of a frame that says nothing.
    #[test]
    fn interleaved_echoes_keep_their_frame_numbers() {
        let per_frame: Vec<Vec<Elem>> = (0..6)
            .map(|i| {
                vec![synth::fg(
                    tags::MR_ECHO_SEQUENCE,
                    vec![synth::num(
                        tags::EFFECTIVE_ECHO_TIME,
                        VR::FD,
                        match i % 2 {
                            0 => 10.0,
                            _ => 80.0,
                        },
                    )],
                )]
            })
            .collect();
        let x = enhanced(vec![synth::fg_orientation("1\\0\\0\\0\\1\\0")], per_frame);
        assert_eq!(x.frames.groups.len(), 2);
        assert_eq!(x.frames.groups[0].list(), "1,3,5");
        assert_eq!(x.frames.groups[1].list(), "2,4,6");
        // the orientation comes from the shared groups for every frame
        for g in &x.frames.groups {
            assert_eq!(
                g.value("image_orientation_patient").map(Value::to_string),
                Some("1\\0\\0\\0\\1\\0".to_string())
            );
        }
    }

    /// A classic single-frame instance has no frames to group.
    #[test]
    fn a_classic_instance_has_no_frame_groups() {
        let x = extracted(
            synth::MetaFields::mr("1.2.3.4"),
            synth::minimal_mr("1.2.3", "1.2.3.1", "1.2.3.4"),
        );
        assert_eq!(x.frames, Frames::default());
        assert!(!x.frames.split());
    }

    /// Beyond the cap the file is read as one stack and says so.
    #[test]
    fn too_many_groups_are_capped() {
        let per_frame: Vec<Vec<Elem>> = (0..=GROUPS_MAX)
            .map(|i| {
                vec![synth::fg(
                    tags::MR_ECHO_SEQUENCE,
                    vec![synth::num(
                        tags::EFFECTIVE_ECHO_TIME,
                        VR::FD,
                        10.0 + i as f64,
                    )],
                )]
            })
            .collect();
        let x = enhanced(Vec::new(), per_frame);
        assert!(x.frames.capped);
        assert!(x.frames.groups.is_empty());
        assert_eq!(x.frames.count as usize, GROUPS_MAX + 1);
    }
}
