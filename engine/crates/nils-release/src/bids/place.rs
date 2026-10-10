// SPDX-License-Identifier: AGPL-3.0-only

//! Where a stack goes when the tree is BIDS
//! (`docs/specs/wave3-anonymize-and-bids.md`, §9.3).
//!
//! Four routes, because a BIDS dataset is not the whole archive:
//!
//! | route | what goes there |
//! |---|---|
//! | the raw tree | everything with a valid datatype, suffix and entity set |
//! | `sourcedata/dicom/` | working scans and, by default, scouts, kept as DICOM |
//! | `derivatives/nils/` | what is derived and BIDS has no word for |
//! | `anat/<folder>/` | a folder of its own the pack names: SyMRI's images |
//! | nowhere | an acquisition BIDS cannot name, **reported, never silently dropped** |
//!
//! Beside the four, a release writes a DICOM export (record 55 C4,
//! 2026-10-09, after v0's `bids-dcm` tree): each converted stack's
//! de-identified slices under `sourcedata/dicom/`, in a folder named after
//! its NIfTI file without the extension, at the session and datatype path
//! the NIfTI has. Every DICOM a release writes is under `sourcedata/dicom/`,
//! the scouts and working scans too, so that `sourcedata/` can hold other
//! kinds of source beside it later. `sourcedata/` is where BIDS keeps data
//! before conversion, and the validator does not read it.
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
    /// `sourcedata/dicom/sub-*/ses-*/`, as DICOM. Valid BIDS, and a reader
    /// has to know to look there.
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
    /// With the rest of its acquisition, under `anat/` in the folder of its
    /// own the pack names (SyMRI: `anat/SyMRI/`, its acquisition, maps and
    /// synthetic contrasts together, as v0 kept them). A synthetic contrast
    /// no folder claims stays in raw `anat/`. The default since record 55
    /// C4 (2026-10-09): SyMRI is anatomical, and its pipeline reads DICOM.
    #[default]
    Folder,
    /// In the raw tree, under the suffix its contrast gives it.
    Anat,
    /// In `derivatives/nils/`, with everything else that was computed.
    Derivatives,
}

/// Which stacks the release's DICOM export carries (record 55 C4,
/// 2026-10-09). v0 wrote every stack of a BIDS export as DICOM, and that is
/// the default here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dicom {
    /// Every stack the release converted.
    #[default]
    All,
    /// Only the stacks in a folder of their own (SyMRI).
    Folders,
    /// None.
    None,
}

impl Dicom {
    pub fn name(self) -> &'static str {
        match self {
            Dicom::All => "all",
            Dicom::Folders => "folders",
            Dicom::None => "none",
        }
    }

    pub fn parse(text: &str) -> Option<Dicom> {
        match text {
            "all" => Some(Dicom::All),
            "folders" => Some(Dicom::Folders),
            "none" => Some(Dicom::None),
            _ => None,
        }
    }

    /// Whether a stack on this route gets its slices in the export. A stack
    /// in `sourcedata/` is DICOM already, and one nowhere is not written.
    pub fn carries(self, route: &str) -> bool {
        match self {
            Dicom::All => matches!(
                route,
                "raw" | "folder" | "derivatives" | "beside" | "unofficial"
            ),
            Dicom::Folders => route == "folder",
            Dicom::None => false,
        }
    }
}

/// Where the DICOM export puts a converted stack's slices: under
/// `sourcedata/dicom/`, at the path its NIfTI has (a derivative's without
/// `derivatives/nils/`), in a folder named after the file.
pub fn dicom_dir(dir: &str, stem: &str) -> String {
    let dir = dir.strip_prefix("derivatives/nils/").unwrap_or(dir);
    format!("sourcedata/dicom/{dir}/{stem}")
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
            Synthetic::Folder => "folder",
            Synthetic::Anat => "anat",
            Synthetic::Derivatives => "derivatives",
        }
    }

    pub fn parse(text: &str) -> Option<Synthetic> {
        match text {
            "folder" => Some(Synthetic::Folder),
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
    pub dicom: Dicom,
}

/// Where one stack goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The raw tree, under its BIDS name.
    Raw,
    /// `sourcedata/dicom/`, as DICOM under its descriptive name.
    SourceData,
    /// `derivatives/nils/`, as a dataset of its own.
    Derivatives,
    /// Under `anat/`, in the folder of its own the pack names, with a
    /// `.bidsignore` line, because a datatype directory holds no folders in
    /// the standard.
    Folder(String),
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
            Route::Folder(_) => "folder",
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
/// `folder` is the folder of its own the pack gives it, if any.
pub fn route(
    disposition: Option<&str>,
    synthetic: bool,
    derived: bool,
    folder: Option<&str>,
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
    // Record 55 C4, after the naming research: raw BIDS admits none of the
    // projections, reformats and perfusion maps the pack lists (`derived`),
    // and no reformat at all, so they are derivatives with `desc-`.
    if derived || disposition == Some("reformat") {
        return Route::Derivatives;
    }
    // Record 55 C4 (2026-10-09): SyMRI is anatomical and stays together, in
    // its own folder under `anat/`, named or not: the folder is outside the
    // standard, so a name the standard refuses is no reason to leave it out.
    if let (Some(folder), Synthetic::Folder) = (folder, options.synthetic) {
        return Route::Folder(folder.to_string());
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
            refused: Vec::new(),
            aslcontext: None,
            acq: Vec::new(),
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
            assert_eq!(
                route(Some("scout"), false, false, None, &named(), o),
                expected
            );
        }
        let dropped = Options {
            localizers: Localizers::Drop,
            ..Options::default()
        };
        assert!(!route(Some("scout"), false, false, None, &named(), dropped).is_written());
    }

    #[test]
    fn a_working_scan_is_the_source_and_stays_dicom() {
        let r = route(
            Some("working_scan"),
            false,
            false,
            None,
            &named(),
            Options::default(),
        );
        assert_eq!(r, Route::SourceData);
        assert!(r.is_source());
    }

    #[test]
    fn the_line_between_derivatives_and_nowhere_is_the_disposition() {
        // A reformat BIDS cannot name is a derivative. An acquisition BIDS
        // cannot name is a hole in the standard, and a release has to admit to
        // it rather than file it under `derivatives`.
        assert_eq!(
            route(
                Some("reformat"),
                false,
                false,
                None,
                &unnamed(),
                Options::default()
            ),
            Route::Derivatives
        );
        assert_eq!(
            route(
                Some("scanner_derived"),
                false,
                false,
                None,
                &unnamed(),
                Options::default()
            ),
            Route::Derivatives
        );
        assert_eq!(
            route(
                Some("acquisition"),
                false,
                false,
                None,
                &unnamed(),
                Options::default()
            ),
            Route::Nowhere(Why::NoSuffix)
        );
    }

    #[test]
    fn a_scanner_derivative_the_standard_does_name_stays_in_the_raw_tree() {
        // An ADC map is `dwi/ADC` in raw BIDS, so being derived is not on its
        // own a reason to leave.
        assert_eq!(
            route(
                Some("scanner_derived"),
                false,
                false,
                None,
                &named(),
                Options::default()
            ),
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
            route(Some("scanner_derived"), true, false, None, &named(), purist),
            Route::Derivatives
        );
        // The default since record 55 C4 (2026-10-09): a synthetic contrast
        // goes with its acquisition's folder, and one no folder claims stays
        // in raw `anat/`.
        assert_eq!(
            route(
                Some("scanner_derived"),
                true,
                false,
                None,
                &named(),
                Options::default()
            ),
            Route::Raw
        );
        let anat = Options {
            synthetic: Synthetic::Anat,
            ..Options::default()
        };
        assert_eq!(
            route(Some("scanner_derived"), true, false, None, &named(), anat),
            Route::Raw
        );
    }

    #[test]
    fn a_reformat_or_a_listed_construct_is_a_derivative_even_with_a_name() {
        // Record 55 C4, after the naming research.
        assert_eq!(
            route(
                Some("reformat"),
                false,
                false,
                None,
                &named(),
                Options::default()
            ),
            Route::Derivatives
        );
        assert_eq!(
            route(
                Some("scanner_derived"),
                false,
                true,
                None,
                &named(),
                Options::default()
            ),
            Route::Derivatives
        );
    }

    #[test]
    fn nowhere_carries_the_reason() {
        let r = route(
            Some("acquisition"),
            false,
            false,
            None,
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
        assert_eq!(Synthetic::default().name(), "folder");
        assert_eq!(Dicom::default().name(), "all");
        assert_eq!(Dicom::parse("folders"), Some(Dicom::Folders));
        assert_eq!(Dicom::parse("some"), None);
    }

    #[test]
    fn symri_goes_to_its_own_folder_named_or_not() {
        // Record 55 C4 (2026-10-09): SyMRI is anatomical, in a folder of
        // its own, its synthetic contrasts with it.
        for named in [named(), unnamed()] {
            assert_eq!(
                route(
                    Some("acquisition"),
                    false,
                    false,
                    Some("SyMRI"),
                    &named,
                    Options::default()
                ),
                Route::Folder("SyMRI".into())
            );
        }
        assert_eq!(
            route(
                Some("scanner_derived"),
                true,
                false,
                Some("SyMRI"),
                &named(),
                Options::default()
            ),
            Route::Folder("SyMRI".into())
        );
        // A projection of one is still a derivative, and a scout a scout.
        assert_eq!(
            route(
                Some("scanner_derived"),
                false,
                true,
                Some("SyMRI"),
                &named(),
                Options::default()
            ),
            Route::Derivatives
        );
        assert_eq!(
            route(
                Some("scout"),
                false,
                false,
                Some("SyMRI"),
                &named(),
                Options::default()
            ),
            Route::SourceData
        );
        // The earlier choices keep their meaning and use no folder.
        let anat = Options {
            synthetic: Synthetic::Anat,
            ..Options::default()
        };
        assert_eq!(
            route(
                Some("scanner_derived"),
                true,
                false,
                Some("SyMRI"),
                &named(),
                anat
            ),
            Route::Raw
        );
        let purist = Options {
            synthetic: Synthetic::Derivatives,
            ..Options::default()
        };
        assert_eq!(
            route(
                Some("scanner_derived"),
                true,
                false,
                Some("SyMRI"),
                &named(),
                purist
            ),
            Route::Derivatives
        );
    }

    #[test]
    fn the_dicom_export_mirrors_the_nifti_path_with_the_file_name_as_folder() {
        assert_eq!(
            dicom_dir("sub-a/ses-1/anat/SyMRI", "sub-a_ses-1_acq-Ax+2D_T1w"),
            "sourcedata/dicom/sub-a/ses-1/anat/SyMRI/sub-a_ses-1_acq-Ax+2D_T1w"
        );
        assert_eq!(
            dicom_dir(
                "derivatives/nils/sub-a/ses-1/anat",
                "sub-a_ses-1_desc-MIP_angio"
            ),
            "sourcedata/dicom/sub-a/ses-1/anat/sub-a_ses-1_desc-MIP_angio"
        );
        assert!(Dicom::All.carries("raw") && Dicom::All.carries("folder"));
        assert!(!Dicom::All.carries("sourcedata") && !Dicom::All.carries("nowhere"));
        assert!(Dicom::Folders.carries("folder") && !Dicom::Folders.carries("raw"));
        assert!(!Dicom::None.carries("folder"));
    }
}
