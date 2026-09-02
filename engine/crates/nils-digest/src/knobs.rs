// SPDX-License-Identifier: AGPL-3.0-only

//! The knobs of a batch (`docs/specs/wave1-parse-and-digest.md`, §11) and the
//! settings of one run. `--describe` prints the table with the effective
//! values.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use nils_dicom::extract::{MODALITIES, SOP_CLASSES};

use crate::walk::Filter;

/// One knob as the spec lists it.
#[derive(Debug, Clone, Copy)]
pub struct Knob {
    pub name: &'static str,
    pub kind: &'static str,
    pub default: &'static str,
    pub note: &'static str,
    /// Which slice makes the knob settable; until then its default holds.
    pub settable_since: u8,
}

/// The table of §11, in its order.
pub static KNOBS: &[Knob] = &[
    Knob {
        name: "files",
        kind: "all | dcm | no-ext | <glob>",
        default: "all",
        note: "which file names are candidates",
        settable_since: 2,
    },
    Knob {
        name: "sop_classes",
        kind: "list of UIDs",
        default: "v0's nine",
        note: "accepted SOP classes; any other is unsupported_sop_class",
        settable_since: 3,
    },
    Knob {
        name: "modalities",
        kind: "list of codes",
        default: "MR, CT, PT",
        note: "accepted modalities; any other is unsupported_modality",
        settable_since: 3,
    },
    Knob {
        name: "identity",
        kind: "rule",
        default: "PatientID, then StudyInstanceUID",
        note: "how the subject key is formed",
        settable_since: 4,
    },
    Knob {
        name: "workers",
        kind: "integer",
        default: "the machine's cores",
        note: "parser threads",
        settable_since: 2,
    },
    Knob {
        name: "walk_threads",
        kind: "integer",
        default: "8",
        note: "directory listing threads",
        settable_since: 3,
    },
    Knob {
        name: "batch_rows",
        kind: "integer",
        default: "2000",
        note: "rows per write",
        settable_since: 3,
    },
    Knob {
        name: "charset_fallback",
        kind: "code",
        default: "iso-8859-1",
        note: "how text with no usable character set is read",
        settable_since: 3,
    },
    Knob {
        name: "retry_quarantine",
        kind: "bool",
        default: "false",
        note: "re-read the files an earlier run quarantined",
        settable_since: 3,
    },
    Knob {
        name: "name",
        kind: "text",
        default: "root basename and date",
        note: "the batch's label",
        settable_since: 2,
    },
];

/// The slice this build implements; knobs with a later `settable_since` hold
/// their default.
pub const SLICE: u8 = 2;

/// The settings of one run.
#[derive(Debug, Clone)]
pub struct Settings {
    pub root: PathBuf,
    pub name: String,
    pub filter: Filter,
    pub workers: usize,
    pub walk_threads: usize,
    pub dry_run: bool,
    pub json: bool,
}

impl Settings {
    /// The defaults for a root: the name from the basename and today's date,
    /// every file a candidate, one worker per core, eight walker threads.
    pub fn new(root: impl Into<PathBuf>) -> Settings {
        let root = root.into();
        Settings {
            name: default_name(&root),
            root,
            filter: Filter::All,
            workers: default_workers(),
            walk_threads: 8,
            dry_run: false,
            json: false,
        }
    }

    /// The effective value of a knob, as `--describe` prints it.
    pub fn value_of(&self, knob: &str) -> String {
        match knob {
            "files" => self.filter.to_string(),
            "sop_classes" => format!("{} (v0's nine)", SOP_CLASSES.len()),
            "modalities" => MODALITIES.join(", "),
            "identity" => "PatientID, then StudyInstanceUID".into(),
            "workers" => self.workers.to_string(),
            "walk_threads" => self.walk_threads.to_string(),
            "batch_rows" => "2000".into(),
            "charset_fallback" => "iso-8859-1".into(),
            "retry_quarantine" => "false".into(),
            "name" => self.name.clone(),
            _ => String::new(),
        }
    }

    /// The `--describe` page.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "nils digest{}",
            if self.dry_run { " (dry run)" } else { "" }
        );
        let _ = writeln!(out, "  root   {}", self.root.display());
        let _ = writeln!(out, "  name   {}", self.name);
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  {:<18} {:<36} {:<34} note",
            "knob", "value", "default"
        );
        for k in KNOBS {
            let value = self.value_of(k.name);
            let note = if k.settable_since > SLICE {
                format!("{} (settable from slice {})", k.note, k.settable_since)
            } else {
                k.note.to_string()
            };
            let _ = writeln!(
                out,
                "  {:<18} {:<36} {:<34} {}",
                k.name, value, k.default, note
            );
        }
        out
    }
}

/// One parser thread per core, or four when the count is unknown.
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// The default batch name: the root's basename and today's date.
pub fn default_name(root: &Path) -> String {
    let base = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "root".into());
    format!("{base}-{}", today())
}

/// Today's date in UTC as `YYYY-MM-DD`.
pub fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_come_out_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(20_698), (2026, 9, 2));
        assert_eq!(today().len(), 10);
    }

    #[test]
    fn the_default_name_is_basename_and_date() {
        let name = default_name(Path::new("/scratch/nils/source/nmosd"));
        assert!(name.starts_with("nmosd-20"));
        assert_eq!(name.len(), "nmosd-".len() + 10);
        assert!(default_name(Path::new("/")).starts_with("root-"));
    }

    #[test]
    fn describe_lists_every_knob_with_its_value() {
        let mut s = Settings::new("/data/x");
        s.dry_run = true;
        s.workers = 3;
        let page = s.describe();
        for k in KNOBS {
            assert!(page.contains(k.name), "{} missing", k.name);
        }
        assert!(page.contains("nils digest (dry run)"));
        assert!(page.contains("(settable from slice 3)"));
        assert_eq!(s.value_of("workers"), "3");
        assert_eq!(s.value_of("sop_classes"), "9 (v0's nine)");
        assert_eq!(s.value_of("modalities"), "MR, CT, PT");
    }
}
