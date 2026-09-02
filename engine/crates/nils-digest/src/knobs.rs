// SPDX-License-Identifier: AGPL-3.0-only

//! The knobs of a batch (`docs/specs/wave1-parse-and-digest.md`, §11) and the
//! settings of one run. `--describe` prints the table with the effective
//! values.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use nils_dicom::extract::{MODALITIES, SOP_CLASSES};
use nils_registry::time::today;
use serde_json::json;

use crate::rule::Rule;
use crate::walk::Filter;

/// The default of `batch_rows` (§9.1).
pub const DEFAULT_BATCH_ROWS: usize = 2_000;

/// The default of `walk_threads` (§5.1).
pub const DEFAULT_WALK_THREADS: usize = 8;

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
pub const SLICE: u8 = 4;

/// The settings of one run.
#[derive(Debug, Clone)]
pub struct Settings {
    pub root: PathBuf,
    pub name: String,
    pub filter: Filter,
    /// The identity rule (§7.3): the default, or the file of `--identity-rule`.
    pub identity: Rule,
    pub workers: usize,
    pub walk_threads: usize,
    /// Instances per write (§9.1).
    pub batch_rows: usize,
    /// Read again what an earlier run quarantined (§5.2).
    pub retry_quarantine: bool,
    /// Ignore `source_file` for this run and parse everything (§5.2).
    pub restart: bool,
    pub dry_run: bool,
    pub json: bool,
}

impl Settings {
    /// The defaults for a root: the name from the basename and today's date,
    /// every file a candidate, one worker per core, eight walker threads,
    /// 2,000 instances per batch.
    pub fn new(root: impl Into<PathBuf>) -> Settings {
        let root = root.into();
        Settings {
            name: default_name(&root),
            root,
            filter: Filter::All,
            identity: Rule::default(),
            workers: default_workers(),
            walk_threads: DEFAULT_WALK_THREADS,
            batch_rows: DEFAULT_BATCH_ROWS,
            retry_quarantine: false,
            restart: false,
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
            "identity" => self.identity.describe(),
            "workers" => self.workers.to_string(),
            "walk_threads" => self.walk_threads.to_string(),
            "batch_rows" => self.batch_rows.to_string(),
            "charset_fallback" => "iso-8859-1".into(),
            "retry_quarantine" => self.retry_quarantine.to_string(),
            "name" => self.name.clone(),
            _ => String::new(),
        }
    }

    /// `ingest_batch.config` (§4.2): every knob as resolved, the identity
    /// rule, the filter, the workers, the binary's version, and what the run
    /// was asked to do.
    pub fn config(&self) -> serde_json::Value {
        json!({
            "files": self.filter.to_string(),
            "sop_classes": SOP_CLASSES,
            "modalities": MODALITIES,
            "identity": self.identity.to_json(),
            "workers": self.workers,
            "walk_threads": self.walk_threads,
            "batch_rows": self.batch_rows,
            "charset_fallback": "iso-8859-1",
            "retry_quarantine": self.retry_quarantine,
            "name": self.name,
            "restart": self.restart,
            "version": env!("CARGO_PKG_VERSION"),
        })
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(page.contains("PatientID, then StudyInstanceUID"));
        assert!(!page.contains("(settable from slice"));
        assert_eq!(s.value_of("workers"), "3");
        assert_eq!(s.value_of("batch_rows"), "2000");
        let config = s.config();
        assert_eq!(config["workers"], 3);
        assert_eq!(config["files"], "all");
        assert_eq!(config["restart"], false);
        assert_eq!(config["identity"]["id_type"], "patient-id");
        assert_eq!(config["identity"]["from"][0]["field"], "PatientID");
        assert_eq!(config["identity"]["fallback"], "StudyInstanceUID");
        assert_eq!(config["sop_classes"].as_array().unwrap().len(), 9);
        assert_eq!(s.value_of("sop_classes"), "9 (v0's nine)");
        assert_eq!(s.value_of("modalities"), "MR, CT, PT");
    }
}
