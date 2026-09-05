// SPDX-License-Identifier: AGPL-3.0-only

//! Where a stack goes when the tree is BIDS
//! (`docs/specs/wave3-anonymize-and-bids.md`, §9.3).
//!
//! Four routes, because a BIDS dataset is not the whole archive:
//!
//! | route | what goes there |
//! |---|---|
//! | the raw tree | everything with a valid datatype, suffix and entity set |
//! | `sourcedata/` | working scans and, by default, scouts, kept as DICOM |
//! | `derivatives/nils/` | what is derived and BIDS has no word for |
//! | nowhere | an acquisition BIDS cannot name, **reported, never silently dropped** |
//!
//! The line between the last two is the disposition and not the name: a
//! reformat BIDS cannot name is a derivative, and a magnetisation-transfer
//! weighted acquisition BIDS cannot name is a hole in the standard that a
//! release has to admit to rather than file under `derivatives`.
//!
//! **Two placements are a release's choice and not this module's**, because
//! both are defensible and which is right depends on who the dataset is for.
//! A release records which it took: a tree that does not say where it put its
//! localizers is a tree whose absence of localizers means nothing.

use super::name::Why;

/// Where a localizer goes. 116,318 stacks, 22 percent of the archive, and BIDS
/// has no word for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Localizers {
    /// `sourcedata/sub-*/ses-*/`, as DICOM. Valid BIDS, and a reader has to
    /// know to look there.
    #[default]
    SourceData,
    /// Its own directory beside `anat` and `dwi`. Needs a `.bidsignore` line,
    /// because it is not a BIDS datatype.
    Datatype,
    /// In `anat` with the others. Needs a `.bidsignore` line, because
    /// `localizer` is not a suffix BIDS has.
    Anat,
    /// Nowhere. Reported per session, and 22 percent of the archive is not in
    /// the tree.
    Drop,
}

/// Where a vendor's synthetic contrast goes. 2,543 stacks.
///
/// The BIDS qMRI appendix permits a vendor's pre-generated maps in raw `anat/`;
/// a purist puts every synthetic image in `derivatives/`. Neither is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Synthetic {
    /// In the raw tree, under the suffix its contrast gives it.
    #[default]
    Anat,
    /// In `derivatives/nils/`, with everything else that was computed.
    Derivatives,
}

impl Localizers {
    pub fn name(self) -> &'static str {
        match self {
            Localizers::SourceData => "sourcedata",
            Localizers::Datatype => "datatype",
            Localizers::Anat => "anat",
            Localizers::Drop => "drop",
        }
    }

    pub fn parse(text: &str) -> Option<Localizers> {
        match text {
            "sourcedata" => Some(Localizers::SourceData),
            "datatype" => Some(Localizers::Datatype),
            "anat" => Some(Localizers::Anat),
            "drop" => Some(Localizers::Drop),
            _ => None,
        }
    }
}

impl Synthetic {
    pub fn name(self) -> &'static str {
        match self {
            Synthetic::Anat => "anat",
            Synthetic::Derivatives => "derivatives",
        }
    }

    pub fn parse(text: &str) -> Option<Synthetic> {
        match text {
            "anat" => Some(Synthetic::Anat),
            "derivatives" => Some(Synthetic::Derivatives),
            _ => None,
        }
    }
}

/// The placements a release chose, recorded on the run and in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub localizers: Localizers,
    pub synthetic: Synthetic,
}

/// Where one stack goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The raw tree, under its BIDS name.
    Raw,
    /// `sourcedata/`, as DICOM under its descriptive name.
    SourceData,
    /// `derivatives/nils/`, as a dataset of its own.
    Derivatives,
    /// Its own directory beside the datatypes, under a name of ours, with a
    /// `.bidsignore` line to say the standard does not know it.
    Beside(&'static str),
    /// In the raw tree under a suffix the standard does not have, likewise
    /// ignored.
    Unofficial(&'static str),
    /// Nowhere, with the reason.
    Nowhere(Why),
}

impl Route {
    pub fn name(&self) -> &'static str {
        match self {
            Route::Raw => "raw",
            Route::SourceData => "sourcedata",
            Route::Derivatives => "derivatives",
            Route::Beside(_) => "beside",
            Route::Unofficial(_) => "unofficial",
            Route::Nowhere(_) => "nowhere",
        }
    }

    /// Whether the tree carries the data at all.
    pub fn is_written(&self) -> bool {
        !matches!(self, Route::Nowhere(_))
    }

    /// Whether it is written as DICOM rather than converted.
    ///
    /// `sourcedata` is DICOM by definition: it is the source. Everything else
    /// follows the release's output setting.
    pub fn is_source(&self) -> bool {
        matches!(self, Route::SourceData)
    }
}

/// The dispositions whose stacks were computed from other images.
///
/// The distinction that decides `derivatives` from nowhere: what a scanner or
/// a workstation made is a derivative whatever BIDS calls it, and what a
/// scanner acquired and BIDS has no word for is a gap in the standard.
fn is_derived(disposition: Option<&str>) -> bool {
    matches!(disposition, Some("scanner_derived") | Some("reformat"))
}

/// Where a stack goes, given what it is and what the release chose.
///
/// `named` is what §9.2 made of it: `Ok` when the standard admits a name.
pub fn route(
    disposition: Option<&str>,
    synthetic: bool,
    named: &Result<super::name::Name, Why>,
    options: Options,
) -> Route {
    // A scout first, because the release's choice about it outranks whether a
    // name happened to be buildable: a localizer that reads as a T1w is still
    // a localizer.
    if disposition == Some("scout") {
        return match options.localizers {
            Localizers::SourceData => Route::SourceData,
            Localizers::Datatype => Route::Beside("localizer"),
            Localizers::Anat => Route::Unofficial("anat"),
            Localizers::Drop => Route::Nowhere(Why::NoDatatype("localizer".into())),
        };
    }
    if disposition == Some("working_scan") {
        return Route::SourceData;
    }
    if synthetic && options.synthetic == Synthetic::Derivatives {
        return Route::Derivatives;
    }
    match named {
        Ok(_) => Route::Raw,
        Err(why) => match is_derived(disposition) {
            true => Route::Derivatives,
            false => Route::Nowhere(why.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bids::name::Name;

    fn named() -> Result<Name, Why> {
        Ok(Name {
            datatype: "anat",
            suffix: "T1w",
            entities: Vec::new(),
        })
    }

    fn unnamed() -> Result<Name, Why> {
        Err(Why::NoSuffix)
    }

    #[test]
    fn a_scout_goes_where_the_release_said_whatever_it_reads_as() {
        // A localizer that reads as a T1w is still a localizer, so the
        // release's choice outranks whether a name happened to be buildable.
        for (choice, expected) in [
            (Localizers::SourceData, Route::SourceData),
            (Localizers::Datatype, Route::Beside("localizer")),
            (Localizers::Anat, Route::Unofficial("anat")),
        ] {
            let o = Options {
                localizers: choice,
                ..Options::default()
            };
            assert_eq!(route(Some("scout"), false, &named(), o), expected);
        }
        let dropped = Options {
            localizers: Localizers::Drop,
            ..Options::default()
        };
        assert!(!route(Some("scout"), false, &named(), dropped).is_written());
    }

    #[test]
    fn a_working_scan_is_the_source_and_stays_dicom() {
        let r = route(Some("working_scan"), false, &named(), Options::default());
        assert_eq!(r, Route::SourceData);
        assert!(r.is_source());
    }

    #[test]
    fn the_line_between_derivatives_and_nowhere_is_the_disposition() {
        // A reformat BIDS cannot name is a derivative. An acquisition BIDS
        // cannot name is a hole in the standard, and a release has to admit to
        // it rather than file it under `derivatives`.
        assert_eq!(
            route(Some("reformat"), false, &unnamed(), Options::default()),
            Route::Derivatives
        );
        assert_eq!(
            route(
                Some("scanner_derived"),
                false,
                &unnamed(),
                Options::default()
            ),
            Route::Derivatives
        );
        assert_eq!(
            route(Some("acquisition"), false, &unnamed(), Options::default()),
            Route::Nowhere(Why::NoSuffix)
        );
    }

    #[test]
    fn a_scanner_derivative_the_standard_does_name_stays_in_the_raw_tree() {
        // An ADC map is `dwi/ADC` in raw BIDS, so being derived is not on its
        // own a reason to leave.
        assert_eq!(
            route(Some("scanner_derived"), false, &named(), Options::default()),
            Route::Raw
        );
    }

    #[test]
    fn a_synthetic_contrast_goes_where_the_release_said() {
        let purist = Options {
            synthetic: Synthetic::Derivatives,
            ..Options::default()
        };
        assert_eq!(
            route(Some("scanner_derived"), true, &named(), purist),
            Route::Derivatives
        );
        assert_eq!(
            route(Some("scanner_derived"), true, &named(), Options::default()),
            Route::Raw
        );
    }

    #[test]
    fn nowhere_carries_the_reason() {
        let r = route(
            Some("acquisition"),
            false,
            &Err(Why::NoTask),
            Options::default(),
        );
        assert_eq!(r, Route::Nowhere(Why::NoTask));
        match r {
            Route::Nowhere(why) => assert_eq!(why.kind(), "no_task"),
            _ => panic!("nowhere"),
        }
    }

    #[test]
    fn a_choice_is_a_word_the_release_records() {
        assert_eq!(Localizers::parse("datatype"), Some(Localizers::Datatype));
        assert_eq!(Localizers::parse("anywhere"), None);
        assert_eq!(
            Synthetic::parse("derivatives"),
            Some(Synthetic::Derivatives)
        );
        assert_eq!(Localizers::default().name(), "sourcedata");
        assert_eq!(Synthetic::default().name(), "anat");
    }
}
