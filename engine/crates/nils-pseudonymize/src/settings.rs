// SPDX-License-Identifier: AGPL-3.0-only

//! What one run is asked to do: the dataset, its trees, its identity rule,
//! what an unmapped identifier does, its tag lists and the pack's private
//! allowlist, read from the dataset a source place declares (record 26 §1)
//! and the flags of the verb.

use std::path::{Path, PathBuf};

use dicom_core::Tag;
use dicom_dictionary_std::tags;
use nils_digest::Rule;
use nils_digest::knobs::{DEFAULT_BATCH_ROWS, DEFAULT_WALK_THREADS, default_workers};
use nils_pack::private::Allowed;
use nils_registry::place::Place;
use nils_registry::time::today;
use serde_json::{Value, json};

/// What a file whose identifier the linkage store does not know does
/// (record 26 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmapped {
    /// It is not written; its row is held with the shape and a keyed lookup.
    Hold,
    /// It is coded from the identifier under the key, and its subject is
    /// marked provisional.
    Code,
}

impl Unmapped {
    pub fn name(self) -> &'static str {
        match self {
            Unmapped::Hold => "hold",
            Unmapped::Code => "code",
        }
    }
}

/// The three demographics a dataset keeps unless it opts out: sex, weight
/// and size, all in v0's patient group.
pub const DEMOGRAPHICS: [Tag; 3] = [tags::PATIENT_SEX, tags::PATIENT_WEIGHT, tags::PATIENT_SIZE];

/// The dataset's tag lists, resolved to tags: what is kept whatever the
/// categories say, and what is removed beside them. Keep wins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagLists {
    pub keep: Vec<Tag>,
    pub remove: Vec<Tag>,
}

impl TagLists {
    /// From the dataset's `tags` block: `keep_demographics`, `remove` and
    /// `keep`, each tag written `gggg,eeee`.
    pub fn of(tags: &Value) -> Result<TagLists, String> {
        let list = |key: &str| -> Result<Vec<Tag>, String> {
            tags[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(parse_tag)
                .collect()
        };
        let mut keep = list("keep")?;
        if tags["keep_demographics"].as_bool().unwrap_or(true) {
            for t in DEMOGRAPHICS {
                if !keep.contains(&t) {
                    keep.push(t);
                }
            }
        }
        let remove = list("remove")?;
        Ok(TagLists { keep, remove })
    }
}

/// A tag written `gggg,eeee`.
pub fn parse_tag(text: &str) -> Result<Tag, String> {
    let (g, e) = text
        .trim()
        .split_once(',')
        .ok_or_else(|| format!("{text} is not a tag as gggg,eeee"))?;
    let group = u16::from_str_radix(g.trim(), 16).map_err(|_| format!("{text}: {g} is not hex"))?;
    let element =
        u16::from_str_radix(e.trim(), 16).map_err(|_| format!("{text}: {e} is not hex"))?;
    Ok(Tag(group, element))
}

/// The settings of one run.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The dataset: the source place's name and id.
    pub dataset: String,
    pub place_id: i64,
    /// The originals, read; the pseudonymised tree, written.
    pub originals: PathBuf,
    pub anon: PathBuf,
    /// The batch's label; the dataset's name and today's date by default.
    pub name: String,
    /// The identity rule the originals are read under.
    pub identity: Rule,
    pub unmapped: Unmapped,
    pub tags: TagLists,
    /// The private elements the pack keeps, by creator and offset, and
    /// which pack said so.
    pub private: Vec<Allowed>,
    pub pack: Option<String>,
    pub workers: usize,
    pub walk_threads: usize,
    /// Rows per write to the registry.
    pub batch_rows: usize,
    /// Read only the held rows a map released or a person coded anyway.
    pub held: bool,
    /// Walk, read and resolve, write nothing, report.
    pub dry_run: bool,
    pub json: bool,
}

impl Settings {
    /// The settings a dataset declares: refused unless the data arrives
    /// identified and the dataset has its originals, since any other
    /// dataset's tree is read in place and never rewritten (record 26 §2).
    pub fn for_dataset(place: &Place) -> Result<Settings, String> {
        let dataset = &place.dataset;
        let arrives = dataset["arrives"].as_str().unwrap_or("deidentified");
        if arrives != "identified" {
            return Err(format!(
                "the dataset {} arrives {arrives}: its tree is read in place and there is nothing to pseudonymise; only an identified dataset has originals to rewrite",
                place.name
            ));
        }
        let originals = place.tree_path("originals").ok_or_else(|| {
            format!(
                "the dataset {} has no originals tree to read; declare it identified on its folder",
                place.name
            )
        })?;
        let anon = place
            .tree_path("anon")
            .filter(|a| a != Path::new(&place.path))
            .ok_or_else(|| {
                format!(
                    "the dataset {} has no pseudonymised tree to write into",
                    place.name
                )
            })?;
        let identity = match &dataset["identity"] {
            Value::Null => Rule::default(),
            rule => {
                let mut rule = Rule::parse(&json!({"identity": rule}).to_string())
                    .map_err(|e| format!("the identity rule of the dataset {}: {e}", place.name))?;
                rule.source = Some(format!("dataset {}", place.name));
                rule
            }
        };
        let unmapped = match dataset["unmapped"].as_str().unwrap_or("hold") {
            "code" => Unmapped::Code,
            _ => Unmapped::Hold,
        };
        let tags = TagLists::of(&dataset["tags"])
            .map_err(|e| format!("the tags of the dataset {}: {e}", place.name))?;
        Ok(Settings {
            name: format!("{}-{}", place.name, today()),
            dataset: place.name.clone(),
            place_id: place.id,
            originals,
            anon,
            identity,
            unmapped,
            tags,
            private: Vec::new(),
            pack: None,
            workers: default_workers(),
            walk_threads: DEFAULT_WALK_THREADS,
            batch_rows: DEFAULT_BATCH_ROWS,
            held: false,
            dry_run: false,
            json: false,
        })
    }

    /// `ingest_batch.config` and the job's args: what the run was asked to
    /// do, the rule and the lists included, the binary's version.
    pub fn config(&self) -> Value {
        let tag = |t: &Tag| format!("{:04X},{:04X}", t.group(), t.element());
        json!({
            "dataset": self.dataset,
            "place_id": self.place_id,
            "originals": self.originals.display().to_string(),
            "anon": self.anon.display().to_string(),
            "name": self.name,
            "identity": self.identity.to_json(),
            "unmapped": self.unmapped.name(),
            "tags": {
                "keep": self.tags.keep.iter().map(tag).collect::<Vec<_>>(),
                "remove": self.tags.remove.iter().map(tag).collect::<Vec<_>>(),
            },
            "private": {
                "pack": self.pack,
                "elements": self.private.iter().map(Allowed::text).collect::<Vec<_>>(),
            },
            "workers": self.workers,
            "walk_threads": self.walk_threads,
            "batch_rows": self.batch_rows,
            "held": self.held,
            "dry_run": self.dry_run,
            "version": env!("CARGO_PKG_VERSION"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_registry::place::Role;

    fn place(dataset: Value) -> Place {
        Place {
            id: 3,
            name: "scans".into(),
            role: Role::Source,
            path: "/data/scans".into(),
            guarantees: json!({}),
            probed: Value::Null,
            probed_at: None,
            created_at: "t".into(),
            updated_at: None,
            retired_at: None,
            handling: Value::Null,
            dataset,
        }
    }

    #[test]
    fn the_tag_lists_keep_the_demographics_unless_the_dataset_opts_out() {
        let lists = TagLists::of(&json!({"remove": ["0010,1010"], "keep": ["0008,1010"]})).unwrap();
        assert_eq!(lists.remove, [Tag(0x0010, 0x1010)]);
        assert_eq!(
            lists.keep,
            [
                Tag(0x0008, 0x1010),
                tags::PATIENT_SEX,
                tags::PATIENT_WEIGHT,
                tags::PATIENT_SIZE
            ]
        );
        let none = TagLists::of(&json!({"keep_demographics": false})).unwrap();
        assert!(none.keep.is_empty() && none.remove.is_empty());
        assert_eq!(TagLists::of(&Value::Null).unwrap().keep.len(), 3);
        assert!(TagLists::of(&json!({"remove": ["nonsense"]})).is_err());
        assert_eq!(parse_tag("0010,0010").unwrap(), tags::PATIENT_NAME);
    }

    #[test]
    fn the_settings_come_from_an_identified_dataset_and_no_other() {
        let identified = place(json!({
            "arrives": "identified",
            "trees": {"originals": "derivatives/dcm-original", "anon": "derivatives/dcm-anon"},
            "identity": {"id_type": "study-id", "from": [{"field": "PatientID"}]},
            "unmapped": "code",
            "tags": {"keep_demographics": false, "remove": [], "keep": []},
        }));
        let s = Settings::for_dataset(&identified).unwrap();
        assert_eq!(
            s.originals,
            Path::new("/data/scans/derivatives/dcm-original")
        );
        assert_eq!(s.anon, Path::new("/data/scans/derivatives/dcm-anon"));
        assert_eq!(s.identity.id_type, "study-id");
        assert_eq!(s.unmapped, Unmapped::Code);
        assert!(s.name.starts_with("scans-20"));
        let config = s.config();
        assert_eq!(config["unmapped"], "code");
        assert_eq!(config["identity"]["id_type"], "study-id");
        assert_eq!(config["tags"]["keep"], json!([]));

        let deidentified =
            place(json!({"arrives": "deidentified", "trees": {"originals": null, "anon": "."}}));
        let why = Settings::for_dataset(&deidentified).unwrap_err();
        assert!(why.contains("read in place"), "{why}");
        let coded = place(
            json!({"arrives": "coded", "trees": {"originals": null, "anon": "derivatives/dcm-anon"}}),
        );
        assert!(
            Settings::for_dataset(&coded)
                .unwrap_err()
                .contains("read in place")
        );
        // the default rule where the dataset stores none
        let plain = place(json!({
            "arrives": "identified",
            "trees": {"originals": "derivatives/dcm-original", "anon": "derivatives/dcm-anon"},
        }));
        let s = Settings::for_dataset(&plain).unwrap();
        assert_eq!(s.identity.id_type, "patient-id");
        assert_eq!(s.unmapped, Unmapped::Hold);
        assert_eq!(s.tags.keep.len(), 3);
    }
}
