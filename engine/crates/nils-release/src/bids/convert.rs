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
//! (the refusal is one stable line; the files are removed).
//!
//! **And the refusal says why** (Wave 7b, agenda 3.8, a partner site's beta.1
//! report: about forty stacks refused for a file named `<stem>a`). `dcm2niix`
//! appends `a` to a name it would write twice (`-w`, its default "add suffix").
//! Under `--terse` that happens on two routes: one file list that comes out as
//! two volumes (two echo numbers, even under one echo time; a magnitude and
//! its phase), where `--terse` dropped the `_e2` or `_ph` that would have
//! told them apart, so the second collides with the first; and a stem another
//! stack of the run wrote already. Measured on `v1.0.20260724`: its stdout
//! holds one `Convert N DICOM as <path> (dims)` line per volume it wrote, so
//! two such lines is the first route and one line naming `<stem>a` the second.
//! For the first route the converter runs once more into a scratch directory
//! without `--terse`, so the refusal names the postfix it wanted (`_e2`,
//! `_ph`) and the report's tally groups the cause rather than the file.
//! That second full conversion runs only on a refusal of the first route (about
//! forty of six hundred stacks at the site that reported it), never on a clean
//! stack.
//!
//! **And one split is undone.** The digest holds slices as one stack when
//! their echo times agree to two decimals; `dcm2niix` splits a series on any
//! difference, so slices at 26.731 and 26.734 ms are one stack here and two
//! volumes there. Its stdout says so on the first run, `Slices not stacked:
//! echo varies (TE 26.731, 26.734; echo 10, 10)`. When every such line names
//! the same echo number and echo times equal to two decimals, the digest's own
//! rule for one stack, the stack is converted once more with `-m y`,
//! `dcm2niix`'s merge of a series' 2D slices, which folds them into the one
//! volume they are. The merge is decided from those lines and from nothing
//! else: `-m y` would also fold two real echoes (`echo 1, 2`) or a magnitude
//! and its phase into one, so it is never a default and every other split
//! keeps the refusal. A common stack is converted once; a split is converted
//! twice (the merge); a refused merge adds the run that learns the names, and
//! the refusal's second line says why the merge was refused.

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
                 an admin installs the build this version pins from the software place, \
                 nils software install dcm2niix or the desk's Settings > Software, nils \
                 setup places it beside nils, or pass --dcm2niix with the path of a current \
                 build from rordenlab/dcm2niix, which a distribution's package lags.",
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
    let made = converted(converter, &list, work, into, stem, compress);
    std::fs::remove_file(&list).ok();
    made
}

/// One run of the converter over `list`, and what appeared.
fn converted(
    converter: &Converter,
    list: &Path,
    work: &Path,
    into: &Path,
    stem: &str,
    compress: bool,
) -> Result<Made, String> {
    let before = listing(into);
    let run = run_converter(
        converter,
        list,
        into,
        stem,
        Flags::Release {
            compress,
            merge: false,
        },
    )
    .map_err(|e| format!("dcm2niix could not be run ({e})"))?;
    if !run.status.success() {
        // What it wrote before it failed is no output either (Wave 7a §8.2:
        // a failure that converted 26 of 32 files left letter-suffixed images
        // the validator found in the tree)
        for f in listing(into).difference(&before) {
            std::fs::remove_file(into.join(f)).ok();
        }
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
        // Removed, because a name nobody chose in a BIDS tree is worse than a
        // stack that did not convert, and the refusal says what it was.
        for f in &made {
            std::fs::remove_file(into.join(f)).ok();
        }
        // Why, from what the converter said: the first line is the cause, the
        // same words for the same cause so the report's tally groups them,
        // and never a file name, in one line.
        let why = match route(&written_names(&String::from_utf8_lossy(&run.stdout)), stem) {
            Some(Route::Several(volumes)) => {
                // The volumes are one echo dcm2niix split on a difference past
                // the digest's second decimal when its own lines say so, and
                // its merge of a series' 2D slices folds them back. Decided
                // from the first run's stdout only (see the module).
                let mut tried = None;
                if one_echo_split(&String::from_utf8_lossy(&run.stdout)) {
                    match merged(converter, list, into, stem, compress, &before) {
                        Ok(made) => return Ok(made),
                        Err(Merge::CouldNotRun(e)) => {
                            return Err(format!("dcm2niix could not be run ({e})"));
                        }
                        Err(Merge::Refused(why)) => tried = Some(why),
                    }
                }
                let postfixes = postfixes_without_terse(converter, list, work, stem);
                let wanted = match postfixes.is_empty() {
                    true => String::new(),
                    false => format!(" ({})", postfixes.join(", ")),
                };
                let mut line = format!(
                    "dcm2niix made {volumes} volumes of one stack{wanted}: the digest holds \
                     them as one stack"
                );
                if let Some(why) = tried {
                    line.push_str(&format!("\na merge of the slices was tried: {why}"));
                }
                line
            }
            Some(Route::NameTaken) => "dcm2niix found the name already in the folder: another \
                 stack of this run, or a file an earlier run left"
                .to_string(),
            None => "dcm2niix wrote a file it was not asked for: the volume is probably not \
                 equally spaced and a name nobody chose is not a BIDS name"
                .to_string(),
        };
        return Err(why);
    }
    Ok(Made { files: made })
}

/// How the converter is run: for the tree, or once more to learn a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flags {
    /// The release's flags: the sidecar, its anonymiser, `--terse`; and with
    /// `merge`, `-m y`, only on the evidence `one_echo_split` asks for.
    Release { compress: bool, merge: bool },
    /// No sidecar, no compression, no `--terse`: the names it would have
    /// given, in a scratch directory nobody keeps.
    Names,
}

fn run_converter(
    converter: &Converter,
    list: &Path,
    into: &Path,
    stem: &str,
    flags: Flags,
) -> std::io::Result<std::process::Output> {
    let mut run = Command::new(&converter.path);
    run.args(["-s", "y"]);
    match flags {
        Flags::Release { compress, merge } => {
            run.args(["-z", if compress { "y" } else { "n" }])
                // The sidecar, which is half of what makes the tree a BIDS dataset.
                .args(["-b", "y"])
                // And its anonymiser on top of ours. It is not a de-identification and
                // the files are already scrubbed; it costs nothing and removes what it
                // does remove.
                .args(["-ba", "y"])
                // Omit `_e2`, `_ph` and the rest: the name is already decided.
                .arg("--terse");
            if merge {
                // Merge the series' 2D slices into one volume. Never a default:
                // it folds real echoes, and a magnitude and its phase, too.
                run.args(["-m", "y"]);
            }
        }
        Flags::Names => {
            run.args(["-z", "n"]).args(["-b", "n"]);
        }
    }
    run.args(["-f", stem])
        .arg("-o")
        .arg(into)
        .arg(list)
        .output()
}

/// The two ways a conversion comes out as a name nobody chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// One file list came out as this many volumes, which `--terse` named alike.
    Several(usize),
    /// The stem was in the folder already: another stack of the run wrote it,
    /// or an earlier run left it.
    NameTaken,
}

/// The names the converter said it wrote, one per `Convert N DICOM as <path>
/// (dims)` line of its stdout, without their directories.
fn written_names(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("Convert "))
        .filter_map(|l| l.split_once(" DICOM as ").map(|(_, path)| path))
        .map(|path| path.rsplit_once(" (").map_or(path, |(p, _)| p).trim())
        .filter_map(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect()
}

/// Which route, from the names the converter wrote; none where it said nothing
/// or wrote the one name it was asked for (an `_Eq_1` beside it, say).
fn route(names: &[String], stem: &str) -> Option<Route> {
    match names {
        [] => None,
        [one] if *one == format!("{stem}a") => Some(Route::NameTaken),
        [_] => None,
        // Two or more names: volumes only where the bare stem was written;
        // without it the stem was taken and the count is not the point.
        several if several.iter().any(|n| n == stem) => Some(Route::Several(several.len())),
        _ => Some(Route::NameTaken),
    }
}

/// What the converter would have called the volumes without `--terse`: the
/// same list, converted once more into a scratch directory under `work`, the
/// postfixes beyond the stem read (`_e1`, `_e2`, `_ph`, `_real`...), and the
/// scratch removed. Empty where that run says nothing usable.
fn postfixes_without_terse(
    converter: &Converter,
    list: &Path,
    work: &Path,
    stem: &str,
) -> Vec<String> {
    let scratch = work.join("names");
    if std::fs::create_dir_all(&scratch).is_err() {
        return Vec::new();
    }
    let mut found: Vec<String> = match run_converter(converter, list, &scratch, stem, Flags::Names)
    {
        Ok(run) => written_names(&String::from_utf8_lossy(&run.stdout))
            .iter()
            .filter_map(|n| n.strip_prefix(stem))
            .filter(|rest| !rest.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    };
    std::fs::remove_dir_all(&scratch).ok();
    found.sort();
    found.dedup();
    found
}

/// Whether the first run's stdout says the volumes are one echo split on a
/// difference past the second decimal: at least one `Slices not stacked: echo
/// varies (TE a, b; echo n, m)` line, and every such line has n equal to m and
/// a and b equal once rounded to two decimals. A line that cannot be read
/// counts against.
fn one_echo_split(stdout: &str) -> bool {
    let mut seen = false;
    for line in stdout.lines() {
        let Some((_, rest)) = line.split_once("Slices not stacked: echo varies (") else {
            continue;
        };
        seen = true;
        let Some((inside, _)) = rest.split_once(')') else {
            return false;
        };
        let Some((times, echoes)) = inside.split_once(';') else {
            return false;
        };
        let pair = |text: &str, prefix: &str| -> Option<(String, String)> {
            let (a, b) = text.trim().strip_prefix(prefix)?.split_once(',')?;
            Some((a.trim().to_string(), b.trim().to_string()))
        };
        let (Some((a, b)), Some((n, m))) = (pair(times, "TE"), pair(echoes, "echo")) else {
            return false;
        };
        let (Ok(a), Ok(b)) = (a.parse::<f64>(), b.parse::<f64>()) else {
            return false;
        };
        if n != m || (a * 100.0).round() != (b * 100.0).round() {
            return false;
        }
    }
    seen
}

/// Why a merge did not give the stack.
#[derive(Debug)]
enum Merge {
    /// The converter would not start: the same error as any other run.
    CouldNotRun(String),
    /// It ran and did not give exactly the files asked for.
    Refused(String),
}

/// The stack converted once more into the tree with the release's flags and
/// `-m y`, after the first run's files are gone. What it made where that is
/// exactly the files the name asked for; otherwise why not, and then nothing
/// of it is left in the tree.
fn merged(
    converter: &Converter,
    list: &Path,
    into: &Path,
    stem: &str,
    compress: bool,
    before: &BTreeSet<String>,
) -> Result<Made, Merge> {
    let run = run_converter(
        converter,
        list,
        into,
        stem,
        Flags::Release {
            compress,
            merge: true,
        },
    )
    .map_err(|e| Merge::CouldNotRun(e.to_string()))?;
    let mut made: Vec<String> = listing(into).difference(before).cloned().collect();
    made.sort();
    let unexpected: Vec<&str> = made
        .iter()
        .filter(|f| !is_expected(f, stem, compress))
        .map(String::as_str)
        .collect();
    let why = if !run.status.success() {
        let text = String::from_utf8_lossy(&run.stderr);
        let text = match text.trim().is_empty() {
            true => String::from_utf8_lossy(&run.stdout).trim().to_string(),
            false => text.trim().to_string(),
        };
        Some(format!(
            "dcm2niix failed ({})",
            reason(text.lines().last().unwrap_or("no output"))
        ))
    } else if made.is_empty() {
        Some("dcm2niix wrote nothing".to_string())
    } else if !unexpected.is_empty() {
        Some(format!(
            "dcm2niix wrote files it was not asked for ({})",
            unexpected.join(", ")
        ))
    } else {
        None
    };
    match why {
        None => Ok(Made { files: made }),
        Some(why) => {
            for f in &made {
                std::fs::remove_file(into.join(f)).ok();
            }
            Err(Merge::Refused(why))
        }
    }
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

    /// Wave 7a §8.2: a converter that writes part of a stack and then fails
    /// leaves nothing of it in the tree, beside what was there before.
    #[cfg(unix)]
    #[test]
    fn a_failed_conversion_leaves_nothing_it_wrote() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = nils_dicom::synth::TempDir::new("convert-failed");
        let fake = dir.path().join("dcm2niix");
        // writes two letter-suffixed images where it was told to, then fails
        std::fs::write(
            &fake,
            "#!/bin/sh\nwhile [ \"$1\" != -o ]; do shift; done\ntouch \"$2/s_angioa.nii.gz\" \"$2/s_angiob.nii.gz\"\necho 'Error: Converted 26 of 32 files' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let c = Converter {
            path: fake,
            version: "stand-in".into(),
        };
        let into = dir.path().join("out");
        std::fs::create_dir_all(&into).unwrap();
        std::fs::write(into.join("kept_T1w.nii.gz"), b"0").unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let e = convert(
            &c,
            &[dir.path().join("1.dcm")],
            &work,
            &into,
            "s_angio",
            true,
        )
        .unwrap_err();
        assert!(e.contains("Converted 26 of 32"), "{e}");
        let left: Vec<String> = std::fs::read_dir(&into)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["kept_T1w.nii.gz"]);
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

    /// The lines dcm2niix v1.0.20260724 prints, measured on five synthetic
    /// series (Wave 7b, agenda 3.8 round 2): one `Convert` line per volume.
    #[test]
    fn the_names_the_converter_said_it_wrote_are_read_from_its_lines() {
        let two = "Chris Rorden's dcm2niiX version v1.0.20260724  (JP2:OpenJPEG) (JP-LS:CharLS) GCC11.4.0 x86-64 (64-bit Linux)\n\
            Found 6 files in '/data/case//files.txt'\n\
            Found 6 DICOM file(s)\n\
            Slices not stacked: echo varies (TE 5, 5; echo 1, 2). Use 'merge 2D slices' option to force stacking\n\
            ::autoBids:Siemens CSAseqFname:'' pulseSeq:'' seqName:''\n\
            Warning: Siemens MoCo? Bogus slice timing (range -1..-1, TR=500 seconds)\n\
            Convert 3 DICOM as /data/case//out/sub-01_ses-01_T2starw (16x16x3x1)\n\
            ::autoBids:Siemens CSAseqFname:'' pulseSeq:'' seqName:''\n\
            Convert 3 DICOM as /data/case//out/sub-01_ses-01_T2starwa (16x16x3x1)\n\
            Conversion required 0.006898 seconds (0.006876 for core code).\n";
        let names = written_names(two);
        assert_eq!(names, ["sub-01_ses-01_T2starw", "sub-01_ses-01_T2starwa"]);
        assert_eq!(
            route(&names, "sub-01_ses-01_T2starw"),
            Some(Route::Several(2))
        );
        // the stem was in the folder already: one line, the name with an `a`
        let taken =
            written_names("Convert 3 DICOM as /data/out/sub-01_ses-01_T2starwa (16x16x3x1)\n");
        assert_eq!(
            route(&taken, "sub-01_ses-01_T2starw"),
            Some(Route::NameTaken)
        );
        // the clean case and the resampled one: the name asked for, no route
        let clean =
            written_names("Convert 3 DICOM as /data/out/sub-01_ses-01_T2starw (16x16x3x1)\n");
        assert_eq!(route(&clean, "sub-01_ses-01_T2starw"), None);
        assert_eq!(route(&[], "sub-01_ses-01_T2starw"), None);
        // the stem taken and two volumes: `<stem>a`, `<stem>aa`, no bare stem
        let both = written_names(
            "Convert 3 DICOM as /data/out/sub-01_ses-01_T2starwa (16x16x3x1)\n\
             Convert 3 DICOM as /data/out/sub-01_ses-01_T2starwaa (16x16x3x1)\n",
        );
        assert_eq!(both, ["sub-01_ses-01_T2starwa", "sub-01_ses-01_T2starwaa"]);
        assert_eq!(
            route(&both, "sub-01_ses-01_T2starw"),
            Some(Route::NameTaken)
        );
    }

    /// A stand-in for dcm2niix v1.0.20260724 on PATH: reads the flags the
    /// release passes, logs each call, prints the lines the real one prints
    /// and runs `body` with `$terse`, `$sidecar`, `$merge`, `$stem` and `$out`
    /// set.
    #[cfg(unix)]
    fn stand_in(dir: &Path, body: &str) -> Converter {
        use std::os::unix::fs::PermissionsExt as _;
        let fake = dir.join("dcm2niix");
        let log = dir.join("calls.log");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\n\
                 terse=n; sidecar=y; merge=n; stem=; out=\n\
                 while [ $# -gt 1 ]; do\n\
                 case \"$1\" in\n\
                 --terse) terse=y;;\n\
                 -f) stem=$2; shift;;\n\
                 -o) out=$2; shift;;\n\
                 -b) sidecar=$2; shift;;\n\
                 -m) merge=$2; shift;;\n\
                 esac\n\
                 shift\n\
                 done\n\
                 echo \"terse=$terse sidecar=$sidecar merge=$merge\" >> '{}'\n\
                 echo \"Chris Rorden's dcm2niiX version v1.0.20260724  (JP2:OpenJPEG) (JP-LS:CharLS) GCC11.4.0 x86-64 (64-bit Linux)\"\n\
                 echo \"Found 3 DICOM file(s)\"\n\
                 {body}\n\
                 echo \"Conversion required 0.006898 seconds (0.006876 for core code).\"\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        Converter {
            path: fake,
            version: "stand-in".into(),
        }
    }

    #[cfg(unix)]
    fn calls(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[cfg(unix)]
    fn names_in(dir: &Path) -> Vec<String> {
        listing(dir).into_iter().collect()
    }

    /// Wave 7b, agenda 3.8 round 2: two echo numbers under one echo time are
    /// one stack to the digest and two volumes to dcm2niix, which `--terse`
    /// names alike, so the second is `<stem>a`. The refusal says so and names
    /// the postfixes a second run without `--terse` gives them.
    #[cfg(unix)]
    #[test]
    fn two_volumes_of_one_stack_are_refused_with_the_names_the_converter_wanted() {
        let dir = nils_dicom::synth::TempDir::new("convert-two-volumes");
        let c = stand_in(
            dir.path(),
            "if [ \"$terse\" = y ]; then\n\
             : > \"$out/$stem.nii\"; : > \"$out/$stem.json\"; : > \"$out/${stem}a.nii\"; : > \"$out/${stem}a.json\"\n\
             echo \"Slices not stacked: echo varies (TE 5, 10; echo 1, 2). Use 'merge 2D slices' option to force stacking\"\n\
             echo \"Convert 3 DICOM as $out/$stem (16x16x3x1)\"\n\
             echo \"Convert 3 DICOM as $out/${stem}a (16x16x3x1)\"\n\
             else\n\
             : > \"$out/${stem}_e1.nii\"; : > \"$out/${stem}_e2.nii\"\n\
             [ \"$sidecar\" = y ] && : > \"$out/${stem}_e1.json\"\n\
             echo \"Convert 3 DICOM as $out/${stem}_e1 (16x16x3x1)\"\n\
             echo \"Convert 3 DICOM as $out/${stem}_e2 (16x16x3x1)\"\n\
             fi",
        );
        let into = dir.path().join("out");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let e = convert(
            &c,
            &[dir.path().join("1.dcm")],
            &work,
            &into,
            "s_T2starw",
            false,
        )
        .unwrap_err();
        assert_eq!(
            e.lines().next().unwrap(),
            "dcm2niix made 2 volumes of one stack (_e1, _e2): the digest holds them as one stack"
        );
        assert_eq!(e.lines().count(), 1, "{e}");
        assert!(!e.contains('/'), "no path in a refusal: {e}");
        // the second run learnt the names: no --terse, no sidecar; and two
        // real echoes are never merged into one volume
        assert_eq!(
            calls(dir.path()),
            ["terse=y sidecar=y merge=n", "terse=n sidecar=n merge=n"]
        );
        // and left nothing: not in the tree, not under work
        assert!(names_in(&into).is_empty(), "{:?}", names_in(&into));
        assert!(!work.join("names").exists());
        assert!(!work.join("files.txt").exists());
    }

    /// The other route: the stem was in the folder already, written by another
    /// stack of this run, so dcm2niix wrote `<stem>a` and said so in one line.
    #[cfg(unix)]
    #[test]
    fn a_name_another_stack_wrote_is_refused_as_that_and_the_first_is_kept() {
        let dir = nils_dicom::synth::TempDir::new("convert-name-taken");
        let c = stand_in(
            dir.path(),
            ": > \"$out/${stem}a.nii\"; : > \"$out/${stem}a.json\"\n\
             echo \"Convert 3 DICOM as $out/${stem}a (16x16x3x1)\"",
        );
        let into = dir.path().join("out");
        std::fs::create_dir_all(&into).unwrap();
        std::fs::write(into.join("s_T2starw.nii"), b"0").unwrap();
        std::fs::write(into.join("s_T2starw.json"), b"{}").unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let e = convert(
            &c,
            &[dir.path().join("1.dcm")],
            &work,
            &into,
            "s_T2starw",
            false,
        )
        .unwrap_err();
        assert_eq!(
            e.lines().next().unwrap(),
            "dcm2niix found the name already in the folder: another stack of this run, or a file an earlier run left"
        );
        assert_eq!(e.lines().count(), 1, "{e}");
        // one run was enough to tell; the other stack's files stay
        assert_eq!(calls(dir.path()), ["terse=y sidecar=y merge=n"]);
        assert_eq!(names_in(&into), ["s_T2starw.json", "s_T2starw.nii"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_clean_conversion_makes_the_image_and_its_sidecar() {
        let dir = nils_dicom::synth::TempDir::new("convert-clean");
        let c = stand_in(
            dir.path(),
            ": > \"$out/$stem.nii\"; : > \"$out/$stem.json\"\n\
             echo \"Convert 3 DICOM as $out/$stem (16x16x3x1)\"",
        );
        let into = dir.path().join("out");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let made = convert(
            &c,
            &[dir.path().join("1.dcm")],
            &work,
            &into,
            "s_T2starw",
            false,
        )
        .unwrap();
        assert_eq!(made.files, ["s_T2starw.json", "s_T2starw.nii"]);
        assert_eq!(calls(dir.path()), ["terse=y sidecar=y merge=n"]);
        assert!(!work.join("files.txt").exists());
    }

    /// The resampled volume (`_Eq_1`) is neither route: one `Convert` line,
    /// the name asked for, and the refusal is the one it always was.
    #[cfg(unix)]
    #[test]
    fn a_file_beside_the_name_with_no_second_volume_keeps_the_old_refusal() {
        let dir = nils_dicom::synth::TempDir::new("convert-eq");
        let c = stand_in(
            dir.path(),
            ": > \"$out/$stem.nii\"; : > \"$out/$stem.json\"; : > \"$out/${stem}_Eq_1.nii\"\n\
             echo \"Convert 3 DICOM as $out/$stem (16x16x3x1)\"",
        );
        let into = dir.path().join("out");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let e = convert(
            &c,
            &[dir.path().join("1.dcm")],
            &work,
            &into,
            "s_T1w",
            false,
        )
        .unwrap_err();
        assert_eq!(
            e,
            "dcm2niix wrote a file it was not asked for: the volume is probably not equally spaced and a name nobody chose is not a BIDS name"
        );
        assert_eq!(calls(dir.path()), ["terse=y sidecar=y merge=n"]);
        assert!(names_in(&into).is_empty());
    }

    #[test]
    fn the_converters_echo_varies_lines_decide_the_merge() {
        // real lines, v1.0.20260724 and v1.0.20220720 alike
        let one = "Slices not stacked: echo varies (TE 26.734, 26.731; echo 10, 10). Use 'merge 2D slices' option to force stacking\n";
        let two = "Slices not stacked: echo varies (TE 5, 10; echo 1, 2). Use 'merge 2D slices' option to force stacking\n";
        assert!(one_echo_split(one));
        assert!(!one_echo_split(two));
        // differing at the second decimal: the digest would have split them
        assert!(!one_echo_split(
            "Slices not stacked: echo varies (TE 26.73, 26.74; echo 10, 10). Use 'merge'\n"
        ));
        // every such line must agree; a geometry split has none
        assert!(!one_echo_split(&format!("{one}{two}")));
        assert!(one_echo_split(&format!("{one}{one}")));
        assert!(!one_echo_split("Convert 3 DICOM as /x/y (16x16x3x1)\n"));
        assert!(!one_echo_split(""));
        assert!(!one_echo_split(
            "Slices not stacked: echo varies (TE x, y; echo 1, 1)\n"
        ));
        assert!(!one_echo_split(
            "Slices not stacked: echo varies (garbled\n"
        ));
    }

    /// Slices whose echo times differ past the digest's second decimal are one
    /// stack here and two volumes to dcm2niix, both with the same echo postfix
    /// (`_e10`, `_e10a`). On that evidence the stack is converted once more
    /// with `-m y`, and the result is the normal file set.
    #[cfg(unix)]
    #[test]
    fn one_echo_split_past_the_second_decimal_is_merged_into_the_one_volume() {
        let dir = nils_dicom::synth::TempDir::new("convert-one-echo-split");
        let c = stand_in(
            dir.path(),
            "if [ \"$merge\" = y ]; then\n\
             : > \"$out/$stem.nii.gz\"; : > \"$out/$stem.json\"\n\
             echo \"Convert 6 DICOM as $out/$stem (16x16x6x1)\"\n\
             elif [ \"$terse\" = y ]; then\n\
             : > \"$out/$stem.nii.gz\"; : > \"$out/$stem.json\"; : > \"$out/${stem}a.nii.gz\"; : > \"$out/${stem}a.json\"\n\
             echo \"Slices not stacked: echo varies (TE 26.731, 26.734; echo 10, 10). Use 'merge 2D slices' option to force stacking\"\n\
             echo \"Convert 3 DICOM as $out/$stem (16x16x3x1)\"\n\
             echo \"Convert 3 DICOM as $out/${stem}a (16x16x3x1)\"\n\
             else\n\
             : > \"$out/${stem}_e10.nii\"; : > \"$out/${stem}_e10a.nii\"\n\
             echo \"Convert 3 DICOM as $out/${stem}_e10 (16x16x3x1)\"\n\
             echo \"Convert 3 DICOM as $out/${stem}_e10a (16x16x3x1)\"\n\
             fi",
        );
        let into = dir.path().join("out");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let made = convert(
            &c,
            &[dir.path().join("1.dcm")],
            &work,
            &into,
            "s_T2starw",
            true,
        )
        .unwrap();
        assert_eq!(made.files, ["s_T2starw.json", "s_T2starw.nii.gz"]);
        assert_eq!(names_in(&into), ["s_T2starw.json", "s_T2starw.nii.gz"]);
        // the release's run and the merge, which is the only call `-m y` is
        // passed to; the names are not needed
        assert_eq!(
            calls(dir.path()),
            ["terse=y sidecar=y merge=n", "terse=y sidecar=y merge=y"]
        );
        assert!(!work.join("names").exists());
        assert!(!work.join("files.txt").exists());
    }

    /// The same evidence, but the merge still writes a file nobody asked for:
    /// the refusal is the Several line with the merge's reason, and nothing of it is left in the tree.
    #[cfg(unix)]
    #[test]
    fn a_merge_that_still_writes_extra_files_is_refused_as_several_volumes() {
        let dir = nils_dicom::synth::TempDir::new("convert-merge-failed");
        let c = stand_in(
            dir.path(),
            "if [ \"$terse\" = y ]; then\n\
             : > \"$out/$stem.nii\"; : > \"$out/$stem.json\"; : > \"$out/${stem}a.nii\"; : > \"$out/${stem}a.json\"\n\
             echo \"Slices not stacked: echo varies (TE 26.731, 26.734; echo 10, 10). Use 'merge 2D slices' option to force stacking\"\n\
             echo \"Convert 3 DICOM as $out/$stem (16x16x3x1)\"\n\
             echo \"Convert 3 DICOM as $out/${stem}a (16x16x3x1)\"\n\
             else\n\
             : > \"$out/${stem}_e10.nii\"; : > \"$out/${stem}_e10a.nii\"\n\
             echo \"Convert 3 DICOM as $out/${stem}_e10 (16x16x3x1)\"\n\
             echo \"Convert 3 DICOM as $out/${stem}_e10a (16x16x3x1)\"\n\
             fi",
        );
        let into = dir.path().join("out");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let e = convert(
            &c,
            &[dir.path().join("1.dcm")],
            &work,
            &into,
            "s_T2starw",
            false,
        )
        .unwrap_err();
        assert_eq!(
            e.lines().next().unwrap(),
            "dcm2niix made 2 volumes of one stack (_e10, _e10a): the digest holds them as one stack"
        );
        assert_eq!(
            e.lines().nth(1).unwrap(),
            "a merge of the slices was tried: dcm2niix wrote files it was not asked for (s_T2starwa.json, s_T2starwa.nii)"
        );
        assert_eq!(e.lines().count(), 2, "{e}");
        assert_eq!(
            calls(dir.path()),
            [
                "terse=y sidecar=y merge=n",
                "terse=y sidecar=y merge=y",
                "terse=n sidecar=n merge=n"
            ]
        );
        assert!(names_in(&into).is_empty(), "{:?}", names_in(&into));
        assert!(!work.join("names").exists());
    }

    /// A merge that exits non-zero is refused with the converter's last line,
    /// and the failed merge leaves nothing in the tree.
    #[cfg(unix)]
    #[test]
    fn a_merge_that_fails_says_so_in_the_second_line() {
        let dir = nils_dicom::synth::TempDir::new("convert-merge-exit");
        let c = stand_in(
            dir.path(),
            "if [ \"$merge\" = y ]; then\n\
             : > \"$out/$stem.nii\"\n\
             echo 'Error: merge failed' >&2; exit 2\n\
             elif [ \"$terse\" = y ]; then\n\
             : > \"$out/$stem.nii\"; : > \"$out/${stem}a.nii\"\n\
             echo \"Slices not stacked: echo varies (TE 26.731, 26.734; echo 10, 10). Use 'merge 2D slices' option to force stacking\"\n\
             echo \"Convert 3 DICOM as $out/$stem (16x16x3x1)\"\n\
             echo \"Convert 3 DICOM as $out/${stem}a (16x16x3x1)\"\n\
             else\n\
             echo \"Convert 3 DICOM as $out/${stem}_e10 (16x16x3x1)\"\n\
             echo \"Convert 3 DICOM as $out/${stem}_e10a (16x16x3x1)\"\n\
             fi",
        );
        let into = dir.path().join("out");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let e = convert(
            &c,
            &[dir.path().join("1.dcm")],
            &work,
            &into,
            "s_T2starw",
            false,
        )
        .unwrap_err();
        assert_eq!(
            e.lines().nth(1).unwrap(),
            "a merge of the slices was tried: dcm2niix failed (Error: merge failed)"
        );
        assert!(names_in(&into).is_empty(), "{:?}", names_in(&into));
    }
}
