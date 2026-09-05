// SPDX-License-Identifier: AGPL-3.0-only

//! `dcm2niix` (`docs/specs/wave3-anonymize-and-bids.md`, §9.6).
//!
//! **A converter is not a thing to discover halfway through an archive**, so a
//! release preflights it: it refuses to start when the converter is absent, and
//! it records the version it found, on the run and in `GeneratedBy`, so a tree
//! says which converter made it.
//!
//! Two things this does that v0 does not, and one it does the same way.
//!
//! The same way: **one conversion per stack, from an explicit file list**, so
//! that a multi-echo series that shares a directory produces one file per stack
//! rather than one per directory. v0 found that and it is right.
//!
//! Different, first: **it converts the released files and never the source.**
//! `dcm2niix` writes a sidecar from the DICOM headers, and its own anonymiser
//! does not de-identify: measured on `v1.0.20260724`, `-ba y`, the default and
//! the strongest of the three, leaves `InstitutionName` and `AcquisitionTime`
//! in the JSON and drops seven keys of fifty-two. So a NIfTI tree converted
//! from the archive carries the institution in every sidecar. Converting the
//! scrubbed file instead means the sidecar inherits the release's policy for
//! free, including its dates.
//!
//! And one thing neither of us knew: **the file list is recognised by its
//! extension**. `dcm2niix -s y <list>` reads the file as a list of DICOM paths
//! only when it is named `.txt`; under any other name it is opened as a DICOM,
//! fails to parse, prints `Not a DICOM image`, **exits 0 and writes nothing**.
//! Neither `-h` nor `-s`'s own help says so. v0 happens to be right by using
//! `NamedTemporaryFile(suffix=".txt")`.
//!
//! Different, second: **it checks what appeared, not the exit code.**
//! `dcm2niix` returns 0 and writes a second file when it decides a volume needs
//! resampling to equal slice spacing: `..._Eq_1.nii` beside the name it was
//! given. A tree that took the exit code at its word holds files whose names
//! nobody chose, and `sub-x_ses-1_T1w_Eq_1.nii.gz` is not a BIDS name. So the
//! output directory is read afterwards and anything unexpected is a refusal
//! with the extra files named.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a release found when it looked for the converter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converter {
    pub path: PathBuf,
    /// The whole first line it prints, which carries the version and the build.
    pub version: String,
}

impl Converter {
    /// Find it and ask its version, or say what is missing.
    ///
    /// Run once, before anything is written. v0 discovers a missing converter
    /// per stack, in a worker process, as N identical failures.
    pub fn find(path: &Path) -> Result<Converter, String> {
        let out = Command::new(path).arg("--version").output().map_err(|e| {
            format!(
                "{} is not runnable ({e}). dcm2niix is a prerequisite of a deployment: \
                 install a current build from rordenlab/dcm2niix, which a distribution's \
                 package lags, and pass --dcm2niix if it is not on the path.",
                path.display()
            )
        })?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text
            .lines()
            .find(|l| l.to_lowercase().contains("version"))
            .or_else(|| text.lines().next())
            .unwrap_or("")
            .trim();
        if line.is_empty() {
            return Err(format!(
                "{} ran and said nothing about its version",
                path.display()
            ));
        }
        Ok(Converter {
            path: path.to_path_buf(),
            version: line.to_string(),
        })
    }

    /// How the run and `GeneratedBy` name it.
    pub fn describe(&self) -> String {
        self.version.clone()
    }
}

/// What one conversion produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Made {
    /// The files it wrote, relative to the output directory, in a fixed order.
    pub files: Vec<String>,
}

/// Convert one stack's files into one named output.
///
/// `stem` is the whole BIDS name without an extension: the name is decided
/// before the converter runs, so nothing downstream renames anything.
pub fn convert(
    converter: &Converter,
    files: &[PathBuf],
    work: &Path,
    into: &Path,
    stem: &str,
    compress: bool,
) -> Result<Made, String> {
    if files.is_empty() {
        return Err("no files to convert".to_string());
    }
    std::fs::create_dir_all(into).map_err(|e| format!("no directory to write into ({e})"))?;
    let before = listing(into);

    // The list goes in a file because a stack is not a directory: a multi-echo
    // series shares one, and converting the directory would make one output for
    // all of them. This is v0's discovery and it is right.
    //
    // **Named `.txt`**, which is what makes it a list rather than a DICOM that
    // will not parse, and outside the tree, so that an interrupted run leaves
    // nothing in the dataset that is not part of it.
    let list = work.join("files.txt");
    let mut text = String::new();
    for f in files {
        text.push_str(&f.display().to_string());
        text.push('\n');
    }
    std::fs::write(&list, text).map_err(|e| format!("could not write the file list ({e})"))?;

    let run = Command::new(&converter.path)
        .args(["-s", "y"])
        .args(["-z", if compress { "y" } else { "n" }])
        // The sidecar, which is half of what makes the tree a BIDS dataset.
        .args(["-b", "y"])
        // And its anonymiser on top of ours. It is not a de-identification and
        // the files are already scrubbed; it costs nothing and removes what it
        // does remove.
        .args(["-ba", "y"])
        // Omit `_e2`, `_ph` and the rest: the name is already decided.
        .arg("--terse")
        .args(["-f", stem])
        .arg("-o")
        .arg(into)
        .arg(&list)
        .output();
    std::fs::remove_file(&list).ok();
    let run = run.map_err(|e| format!("dcm2niix could not be run ({e})"))?;
    if !run.status.success() {
        let text = String::from_utf8_lossy(&run.stderr);
        let text = match text.trim().is_empty() {
            true => String::from_utf8_lossy(&run.stdout).trim().to_string(),
            false => text.trim().to_string(),
        };
        return Err(format!(
            "dcm2niix failed ({})",
            reason(text.lines().last().unwrap_or("no output"))
        ));
    }

    // What actually appeared, which is not what the exit code claims.
    let mut made: Vec<String> = listing(into).difference(&before).cloned().collect();
    made.sort();
    if made.is_empty() {
        return Err("dcm2niix wrote nothing".to_string());
    }
    let unexpected: Vec<&String> = made
        .iter()
        .filter(|f| !is_expected(f, stem, compress))
        .collect();
    if !unexpected.is_empty() {
        let named: Vec<&str> = unexpected.iter().map(|f| f.as_str()).collect();
        // Removed, because a name nobody chose in a BIDS tree is worse than a
        // stack that did not convert, and the refusal says what it was.
        for f in &named {
            std::fs::remove_file(into.join(f)).ok();
        }
        for f in &made {
            std::fs::remove_file(into.join(f)).ok();
        }
        return Err(format!(
            "dcm2niix wrote {} it was not asked for ({}); the volume is probably not \
             equally spaced, and a name nobody chose is not a BIDS name",
            match named.len() {
                1 => "a file".to_string(),
                n => format!("{n} files"),
            },
            named.join(", ")
        ));
    }
    Ok(Made { files: made })
}

/// What the converter said, with the paths taken out.
///
/// A converter names the file it choked on, and a report that repeats it names
/// the tree. Two reasons: the same rule as every other refusal in the release,
/// and a tally keyed on a message with a path in it is a tally of one each.
fn reason(line: &str) -> String {
    line.split_whitespace()
        .filter(|word| !word.contains('/') && !word.contains('\\'))
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches([':', ',', '.', ' '])
        .trim()
        .to_string()
}

/// The names one conversion of `stem` may produce.
fn is_expected(file: &str, stem: &str, compress: bool) -> bool {
    let image = match compress {
        true => format!("{stem}.nii.gz"),
        false => format!("{stem}.nii"),
    };
    // The sidecar, the image, and the two diffusion files, which dcm2niix
    // writes only for a diffusion series and which BIDS wants beside it.
    file == image
        || file == format!("{stem}.json")
        || file == format!("{stem}.bval")
        || file == format!("{stem}.bvec")
}

/// Everything in a directory, by name.
fn listing(dir: &Path) -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_converter_is_one_refusal_and_not_n_identical_failures() {
        // §9.6: a converter is not a thing to discover halfway through an
        // archive. v0 discovers it per stack, in a worker process.
        let e = Converter::find(Path::new("/nonexistent/dcm2niix")).unwrap_err();
        assert!(e.contains("rordenlab/dcm2niix"), "{e}");
        assert!(e.contains("--dcm2niix"), "{e}");
    }

    #[test]
    fn only_the_files_the_name_asked_for_are_expected() {
        assert!(is_expected("sub-x_T1w.nii.gz", "sub-x_T1w", true));
        assert!(is_expected("sub-x_T1w.json", "sub-x_T1w", true));
        assert!(is_expected("sub-x_dwi.bval", "sub-x_dwi", true));
        assert!(is_expected("sub-x_dwi.bvec", "sub-x_dwi", true));
        // The one that matters: dcm2niix returns 0 and writes this beside the
        // name it was given when it resamples a volume to equal spacing.
        assert!(!is_expected("sub-x_T1w_Eq_1.nii", "sub-x_T1w", true));
        assert!(!is_expected("sub-x_T1w_e2.nii.gz", "sub-x_T1w", true));
        // And compression is not a guess.
        assert!(!is_expected("sub-x_T1w.nii", "sub-x_T1w", true));
        assert!(is_expected("sub-x_T1w.nii", "sub-x_T1w", false));
    }

    #[test]
    fn a_conversion_with_nothing_to_convert_says_so() {
        let c = Converter {
            path: PathBuf::from("/nonexistent/dcm2niix"),
            version: "test".into(),
        };
        assert_eq!(
            convert(
                &c,
                &[],
                Path::new("/tmp"),
                Path::new("/tmp"),
                "sub-x_T1w",
                true
            ),
            Err("no files to convert".to_string())
        );
    }

    #[test]
    fn a_refusal_names_the_reason_and_never_the_file() {
        // The same rule as every other refusal in the release. And a tally
        // keyed on a message with a path in it is a tally of one each, which
        // is what the first run of this on a corpus produced.
        assert_eq!(
            reason("Warning: File not large enough to store image data: /srv/x/00002516.dcm"),
            "Warning: File not large enough to store image data"
        );
        assert_eq!(
            reason("Error: Not a DICOM image : /tmp/a"),
            "Error: Not a DICOM image"
        );
        assert_eq!(reason("no output"), "no output");
    }

    #[test]
    fn the_list_is_named_txt_because_that_is_what_makes_it_a_list() {
        // Measured, not read: `dcm2niix -s y <list>` reads a file as a list of
        // DICOM paths only when it is named `.txt`. Under any other name it is
        // opened as a DICOM, fails to parse, **exits 0 and writes nothing**.
        // The name is built here, so this is where the fact belongs.
        let work = std::path::Path::new("/tmp/nils-convert-probe");
        assert_eq!(work.join("files.txt").extension().unwrap(), "txt");
    }
}
