// SPDX-License-Identifier: AGPL-3.0-only

//! The dataset on a source place, on disk (record 26 §1, §2 and §14). The
//! registry keeps what a dataset is (`nils_registry::place`); this module
//! looks at the folder when one is declared, makes the one kind of write the
//! engine makes under a source place, counts the trees for a page, and says
//! where `@name` points once a dataset has a pseudonymised tree.
//!
//! A dataset's two trees are `derivatives/dcm-original`, the originals the
//! pseudonymiser reads and nothing else does, and `derivatives/dcm-anon`,
//! the pseudonymised tree the registry reads. A folder with neither reads
//! itself as the pseudonymised tree, which is how every place declared before
//! record 26 keeps working. A v0 cohort folder holds `derivatives/dcm-raw`,
//! which is renamed `dcm-anon` on declaration and never rewritten. Loose
//! entries beside `derivatives/` are moved into the originals when the data
//! arrives identified, or into the pseudonymised tree when asked, by a rename
//! on the same filesystem; nothing is copied and nothing is read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use nils_registry::place::{self, ANON_TREE, ORIGINALS_TREE, Place, Role};
use nils_registry::store::Store;
use serde_json::{Value, json};

/// v0's pseudonymised tree, renamed on declaration.
const RAW_TREE: &str = "derivatives/dcm-raw";

/// The entries a count reads before it says it is partial, and how long it
/// reads: a page's number, never a walk of the whole archive.
const COUNT_ENTRIES: usize = 200_000;
const COUNT_FOR: Duration = Duration::from_secs(2);

/// The keys a declaration may set on a dataset, beside the trees the engine
/// sets itself.
pub(crate) const FIELDS: [&str; 7] = [
    "arrives",
    "identity",
    "unmapped",
    "cohort",
    "tags",
    "originals_kept",
    "move_into_anon",
];

/// Why a declaration is refused: the status a door answers with and the
/// sentence naming what was wrong.
#[derive(Debug, Clone)]
pub(crate) struct Refused {
    pub(crate) status: u16,
    pub(crate) message: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

fn bad(message: impl Into<String>) -> Refused {
    Refused {
        status: 400,
        message: message.into(),
    }
}

fn conflict(message: impl Into<String>) -> Refused {
    Refused {
        status: 409,
        message: message.into(),
    }
}

/// Whether a body names any dataset field, a null (no cohort, no rule) as
/// much as a value.
pub(crate) fn fields_given(doc: &Value) -> bool {
    doc.as_object()
        .is_some_and(|o| FIELDS.iter().any(|k| o.contains_key(*k)))
}

/// What a folder holds: which trees are there, whether it is a v0 cohort
/// folder, and the loose entries beside `derivatives/`, which a declaration
/// may move. Read from the folder's own listing and nothing deeper.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) originals: bool,
    pub(crate) anon: bool,
    pub(crate) raw: bool,
    /// The names of the top-level entries other than `derivatives` and the
    /// hidden ones, sorted.
    pub(crate) loose: Vec<String>,
    /// Whether a declaration renamed `dcm-raw` to `dcm-anon` just now.
    pub(crate) renamed: bool,
}

impl Layout {
    /// A v0 cohort folder: it holds `derivatives/dcm-raw`, or held it until
    /// this declaration renamed it.
    fn v0(&self) -> bool {
        self.raw || self.renamed
    }
}

pub(crate) fn detect(path: &Path) -> Layout {
    let mut layout = Layout {
        originals: path.join(ORIGINALS_TREE).is_dir(),
        anon: path.join(ANON_TREE).is_dir(),
        raw: path.join(RAW_TREE).is_dir(),
        loose: Vec::new(),
        renamed: false,
    };
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "derivatives" || name.starts_with('.') {
                continue;
            }
            layout.loose.push(name);
        }
    }
    layout.loose.sort_unstable();
    layout
}

/// The layout as a door answers it: the v0 folder's counts when it is one,
/// which trees are there, and how many loose entries wait beside them.
pub(crate) fn layout_doc(path: &Path, layout: &Layout) -> Value {
    let v0 = layout.v0().then(|| {
        let raw = if layout.raw {
            path.join(RAW_TREE)
        } else {
            path.join(ANON_TREE)
        };
        let originals = count(&path.join(ORIGINALS_TREE));
        let raw = count(&raw);
        json!({
            "original_files": originals["files"],
            "raw_files": raw["files"],
            "partial": originals["partial"].as_bool() == Some(true) || raw["partial"].as_bool() == Some(true),
            "renamed": layout.renamed,
        })
    });
    json!({
        "v0": v0,
        "originals": layout.originals,
        "anon": layout.anon,
        "loose": layout.loose.len(),
    })
}

/// The one write the engine makes under a source place outside the
/// pseudonymiser: the look at the folder when a dataset is declared. A v0
/// folder's `dcm-raw` becomes `dcm-anon`; when the data arrives identified
/// the loose entries are moved into the originals and an empty
/// pseudonymised tree is made beside them; when it arrives de-identified or
/// coded and the caller asked, the loose entries are moved into the
/// pseudonymised tree. Every move is a rename inside the folder, top-level
/// entries only. The trees the dataset reads from now on come back with the
/// layout found.
pub(crate) fn look(
    path: &Path,
    arrives: &str,
    move_into_anon: bool,
) -> Result<(Value, Layout), Refused> {
    if !path.is_dir() {
        return Err(conflict(format!(
            "{} is not a directory; a dataset is one folder",
            path.display()
        )));
    }
    let mut layout = detect(path);
    let io = |what: String, e: std::io::Error| conflict(format!("{what}: {e}"));
    if layout.raw {
        if layout.anon {
            return Err(conflict(format!(
                "{} holds both {RAW_TREE} and {ANON_TREE}; keep one before declaring it",
                path.display()
            )));
        }
        std::fs::rename(path.join(RAW_TREE), path.join(ANON_TREE))
            .map_err(|e| io(format!("renaming {RAW_TREE} to {ANON_TREE}"), e))?;
        layout.raw = false;
        layout.anon = true;
        layout.renamed = true;
    }
    let into = |tree: &str, layout: &mut Layout| -> Result<(), Refused> {
        let dir = path.join(tree);
        std::fs::create_dir_all(&dir).map_err(|e| io(format!("making {tree}"), e))?;
        for name in std::mem::take(&mut layout.loose) {
            let target = dir.join(&name);
            if target.exists() {
                return Err(conflict(format!(
                    "{tree} already holds {name}; the loose {name} was not moved"
                )));
            }
            std::fs::rename(path.join(&name), &target)
                .map_err(|e| io(format!("moving {name} into {tree}"), e))?;
        }
        Ok(())
    };
    match arrives {
        "identified" => {
            into(ORIGINALS_TREE, &mut layout)?;
            std::fs::create_dir_all(path.join(ANON_TREE))
                .map_err(|e| io(format!("making {ANON_TREE}"), e))?;
            layout.originals = true;
            layout.anon = true;
        }
        _ if move_into_anon && !layout.loose.is_empty() => {
            into(ANON_TREE, &mut layout)?;
            layout.anon = true;
        }
        _ => {}
    }
    let trees = json!({
        "originals": layout.originals.then_some(ORIGINALS_TREE),
        "anon": if layout.anon { ANON_TREE } else { "." },
    });
    Ok((trees, layout))
}

/// What a declaration worked out: the dataset to store, the layout found,
/// and the trees' counts for the probe.
pub(crate) struct Declared {
    pub(crate) dataset: Value,
    pub(crate) layout: Value,
    pub(crate) probed: Value,
}

/// A dataset declared on a folder: the fields asked for, checked and merged
/// over the place in force; the identity rule parsed as the digest parses
/// it; the folder refused when it is a tree of another dataset; then the
/// look at the folder, whose trees the dataset reads from now on; and the
/// probe with the trees counted.
pub(crate) fn declare(
    store: &mut Store,
    path: &Path,
    asked: &Value,
    current: Option<&Place>,
) -> Result<Declared, Refused> {
    let mut dataset = place::dataset_of(asked, current.map(|p| &p.dataset)).map_err(bad)?;
    if !dataset["identity"].is_null() {
        rule_of(&dataset["identity"]).map_err(|e| bad(format!("identity: {e}")))?;
    }
    if let Some((other, tree)) =
        tree_of_another(store, path, current.map(|p| p.id)).map_err(|e| Refused {
            status: 500,
            message: e.to_string(),
        })?
    {
        return Err(conflict(format!(
            "{} is the {tree} of the dataset {}; a dataset is declared on its own folder",
            path.display(),
            other.name
        )));
    }
    let move_into_anon = asked["move_into_anon"].as_bool() == Some(true);
    let arrives = dataset["arrives"]
        .as_str()
        .unwrap_or("deidentified")
        .to_string();
    let (trees, layout) = look(path, &arrives, move_into_anon)?;
    dataset["trees"] = trees;
    let mut probed = crate::places::probe(path);
    probed["trees"] = count_trees(path, &dataset);
    Ok(Declared {
        dataset,
        layout: layout_doc(path, &layout),
        probed,
    })
}

/// The source place whose originals or pseudonymised tree holds a path, other
/// than the place given: a dataset is declared on its own folder, never
/// inside another's trees. A dataset reading its folder itself has no tree
/// another folder could be.
fn tree_of_another(
    store: &mut Store,
    path: &Path,
    except: Option<i64>,
) -> Result<Option<(Place, &'static str)>, nils_registry::store::Error> {
    for tree in ["originals", "anon"] {
        if let Some(p) = place::tree_holding(store, tree, path)?
            && Some(p.id) != except
            && p.dataset["trees"][tree].as_str().is_some_and(|t| t != ".")
        {
            return Ok(Some((p, tree)));
        }
    }
    Ok(None)
}

/// A bounded count of a tree: its files and bytes, when it was last written,
/// and whether the count stopped short. Links are left out, as the digest
/// leaves them out. A tree that is not there counts nothing.
pub(crate) fn count(path: &Path) -> Value {
    let until = Instant::now() + COUNT_FOR;
    let (mut files, mut bytes, mut last, mut read, mut partial) = (0u64, 0u64, 0u64, 0usize, false);
    let mut queue = vec![path.to_path_buf()];
    'walk: while let Some(dir) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if read >= COUNT_ENTRIES || Instant::now() >= until {
                partial = true;
                break 'walk;
            }
            read += 1;
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                queue.push(entry.path());
            } else if kind.is_file()
                && let Ok(meta) = entry.metadata()
            {
                files += 1;
                bytes += meta.len();
                if let Ok(secs) = meta
                    .modified()
                    .unwrap_or(UNIX_EPOCH)
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                {
                    last = last.max(secs);
                }
            }
        }
    }
    json!({
        "files": files,
        "bytes": bytes,
        "last_written": (last > 0).then(|| nils_registry::time::iso_of(last)),
        "partial": partial,
    })
}

/// The counts of a dataset's trees, for `probed.trees`.
pub(crate) fn count_trees(path: &Path, dataset: &Value) -> Value {
    let of = |rel: Option<&str>| {
        rel.map(|rel| {
            if rel == "." {
                count(path)
            } else {
                count(&path.join(rel))
            }
        })
    };
    json!({
        "originals": of(dataset["trees"]["originals"].as_str()),
        "anon": of(dataset["trees"]["anon"].as_str()),
    })
}

/// A place measured again: what the filesystem says about its path and,
/// for a dataset, its trees counted.
pub(crate) fn probe_place(p: &Place) -> Value {
    let path = Path::new(&p.path);
    let mut probed = crate::places::probe(path);
    if p.role == Role::Source {
        let dataset =
            place::dataset_of(&p.dataset, None).unwrap_or_else(|_| place::default_dataset(None));
        probed["trees"] = count_trees(path, &dataset);
    }
    probed
}

/// An ingest root as `@name` resolves it: the pseudonymised tree of the
/// dataset declared on it, or the folder itself, and the originals, which
/// `@name/originals` names for the pseudonymiser and never for a digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Root {
    pub(crate) name: String,
    /// The path the deployment gave.
    pub(crate) given: PathBuf,
    /// Where `@name` points.
    pub(crate) anon: PathBuf,
    /// Where `@name/originals` points, when the dataset has originals.
    pub(crate) originals: Option<PathBuf>,
    /// The source place declared on the root, when one is.
    pub(crate) place: Option<String>,
}

impl Root {
    /// A root no dataset is declared on: the folder it was given.
    pub(crate) fn plain(name: &str, path: &Path) -> Root {
        Root {
            name: name.to_string(),
            given: path.to_path_buf(),
            anon: path.to_path_buf(),
            originals: None,
            place: None,
        }
    }

    /// The tree a relative part is under and the part under it: the
    /// originals for `originals` and what is beneath it, the pseudonymised
    /// tree for anything else.
    pub(crate) fn base<'a>(&self, rel: &'a str) -> (&Path, &'a str) {
        if let Some(originals) = &self.originals {
            if rel == "originals" {
                return (originals, "");
            }
            if let Some(rest) = rel.strip_prefix("originals/") {
                return (originals, rest);
            }
        }
        (&self.anon, rel)
    }

    /// The path an `@name/rel` names.
    pub(crate) fn resolve(&self, rel: &str) -> PathBuf {
        let (base, rest) = self.base(rel);
        if rest.is_empty() {
            base.to_path_buf()
        } else {
            base.join(rest)
        }
    }

    pub(crate) fn as_json(&self) -> Value {
        json!({
            "name": self.name,
            "path": self.anon.display().to_string(),
            "given": self.given.display().to_string(),
            "originals": self.originals.as_ref().map(|p| p.display().to_string()),
            "place": self.place,
        })
    }
}

/// The ingest roots as `@name` resolves them: each root the deployment gave,
/// pointed at the pseudonymised tree of the source place declared on it. A
/// root no dataset is declared on, and every root when the registry does
/// not answer, points where it was given.
pub(crate) fn roots(
    store: &mut Store,
    given: &BTreeMap<String, PathBuf>,
) -> BTreeMap<String, Root> {
    let places = place::active(store).unwrap_or_default();
    let sources: Vec<(PathBuf, &Place)> = places
        .iter()
        .filter(|p| p.role == Role::Source)
        .map(|p| {
            let path = Path::new(&p.path);
            (
                std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
                p,
            )
        })
        .collect();
    given
        .iter()
        .map(|(name, path)| {
            let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let root = match sources.iter().find(|(p, _)| *p == real) {
                Some((_, p)) => Root {
                    name: name.clone(),
                    given: path.clone(),
                    anon: p.tree_path("anon").unwrap_or_else(|| path.clone()),
                    originals: p.tree_path("originals"),
                    place: Some(p.name.clone()),
                },
                None => Root::plain(name, path),
            };
            (name.clone(), root)
        })
        .collect()
}

/// The roots of the registry's own source places, by name, for a command
/// line with no `--ingest-root`: `nils digest @name` reads the dataset's
/// pseudonymised tree.
pub(crate) fn place_roots(store: &mut Store) -> BTreeMap<String, PathBuf> {
    place::active(store)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.role == Role::Source)
        .map(|p| (p.name.clone(), PathBuf::from(&p.path)))
        .collect()
}

/// The source place whose originals hold a path: a digest of it is refused,
/// since the registry never points at an identified file.
pub(crate) fn originals_holding(store: &mut Store, path: &Path) -> Option<Place> {
    place::tree_holding(store, "originals", path).ok().flatten()
}

/// The identity rule a dataset stores, parsed as `nils digest
/// --identity-rule` parses a file: the stored value is the file's
/// `identity` block.
pub(crate) fn rule_of(identity: &Value) -> Result<nils_digest::Rule, String> {
    nils_digest::Rule::parse(&json!({"identity": identity}).to_string()).map_err(|e| e.to_string())
}

/// The `identity` block of a rule file, as the dataset stores it, once the
/// file has parsed as a rule.
pub(crate) fn identity_from_file(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    nils_digest::Rule::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: Value =
        serde_saphyr::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    doc.get("identity")
        .cloned()
        .filter(Value::is_object)
        .ok_or_else(|| format!("{}: no identity block", path.display()))
}

/// The rule stored on the dataset whose pseudonymised tree holds a path,
/// for a digest that names none of its own; none where no dataset holds the
/// path or the dataset stores no rule.
pub(crate) fn stored_rule(
    store: &mut Store,
    path: &Path,
) -> Result<Option<nils_digest::Rule>, String> {
    let Some(p) = place::tree_holding(store, "anon", path).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let identity = &p.dataset["identity"];
    if !identity.is_object() {
        return Ok(None);
    }
    let mut rule = rule_of(identity)
        .map_err(|e| format!("the identity rule of the dataset {}: {e}", p.name))?;
    rule.source = Some(format!("dataset {}", p.name));
    Ok(Some(rule))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_dicom::synth::TempDir;

    fn names(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        out.sort_unstable();
        out
    }

    #[test]
    fn an_identified_dataset_moves_its_loose_entries_into_the_originals_and_makes_the_anon_tree() {
        let dir = TempDir::new("dataset-identified");
        dir.file("sub-1/ses-1/a.dcm", b"x");
        dir.file("sub-2/b.dcm", b"y");
        dir.file("notes.txt", b"n");
        dir.file(".hidden", b"h");
        let found = detect(dir.path());
        assert_eq!(found.loose, ["notes.txt", "sub-1", "sub-2"]);
        assert!(!found.v0());
        let (trees, layout) = look(dir.path(), "identified", false).unwrap();
        assert_eq!(
            trees,
            json!({"originals": ORIGINALS_TREE, "anon": ANON_TREE})
        );
        assert!(layout.loose.is_empty() && !layout.renamed);
        assert_eq!(names(dir.path()), [".hidden", "derivatives"]);
        assert_eq!(
            names(&dir.path().join(ORIGINALS_TREE)),
            ["notes.txt", "sub-1", "sub-2"]
        );
        assert!(
            dir.path()
                .join(ORIGINALS_TREE)
                .join("sub-1/ses-1/a.dcm")
                .is_file()
        );
        assert!(dir.path().join(ANON_TREE).is_dir());
        assert!(names(&dir.path().join(ANON_TREE)).is_empty());
        // declared again, nothing is left to move and the trees stand
        let (again, layout) = look(dir.path(), "identified", false).unwrap();
        assert_eq!(again, trees);
        assert!(layout.loose.is_empty());
        let doc = layout_doc(dir.path(), &layout);
        assert_eq!(doc["v0"], Value::Null);
        assert_eq!(doc["loose"], 0);
        assert_eq!(doc["originals"], true);
    }

    #[test]
    fn a_v0_cohort_folder_has_its_raw_tree_renamed_and_nothing_else_touched() {
        let dir = TempDir::new("dataset-v0");
        dir.file("derivatives/dcm-original/sub-1/a.dcm", b"xx");
        dir.file("derivatives/dcm-raw/sub-1/a.dcm", b"yyy");
        dir.file("derivatives/dcm-raw/sub-1/b.dcm", b"zzz");
        dir.file("derivatives/other/keep.txt", b"k");
        let found = detect(dir.path());
        assert!(found.raw && found.originals && !found.anon && found.v0());
        assert!(found.loose.is_empty());
        let (trees, layout) = look(dir.path(), "deidentified", false).unwrap();
        assert_eq!(
            trees,
            json!({"originals": ORIGINALS_TREE, "anon": ANON_TREE})
        );
        assert!(layout.renamed && layout.anon && !layout.raw);
        assert!(!dir.path().join(RAW_TREE).exists());
        assert!(dir.path().join(ANON_TREE).join("sub-1/b.dcm").is_file());
        assert!(dir.path().join("derivatives/other/keep.txt").is_file());
        let doc = layout_doc(dir.path(), &layout);
        assert_eq!(doc["v0"]["original_files"], 1);
        assert_eq!(doc["v0"]["raw_files"], 2);
        assert_eq!(doc["v0"]["renamed"], true);
        // both trees there is a folder to sort out by hand, not by a rename
        std::fs::create_dir_all(dir.path().join(RAW_TREE)).unwrap();
        let why = look(dir.path(), "deidentified", false).unwrap_err();
        assert_eq!(why.status, 409);
        assert!(why.message.contains("keep one"), "{}", why.message);
    }

    #[test]
    fn a_deidentified_dataset_moves_into_the_anon_tree_only_when_asked() {
        let dir = TempDir::new("dataset-deidentified");
        dir.file("sub-1/a.dcm", b"x");
        // not asked: the folder itself is the pseudonymised tree
        let (trees, layout) = look(dir.path(), "deidentified", false).unwrap();
        assert_eq!(trees, json!({"originals": null, "anon": "."}));
        assert_eq!(layout.loose, ["sub-1"]);
        assert_eq!(names(dir.path()), ["sub-1"]);
        // asked: moved, and the tree is dcm-anon
        let (trees, layout) = look(dir.path(), "coded", true).unwrap();
        assert_eq!(trees, json!({"originals": null, "anon": ANON_TREE}));
        assert!(layout.loose.is_empty());
        assert!(dir.path().join(ANON_TREE).join("sub-1/a.dcm").is_file());
        assert_eq!(names(dir.path()), ["derivatives"]);
        // an empty folder declared with nothing to move reads itself
        let empty = TempDir::new("dataset-empty");
        let (trees, _) = look(empty.path(), "deidentified", true).unwrap();
        assert_eq!(trees, json!({"originals": null, "anon": "."}));
        // a loose entry the tree already holds is not moved over it
        let clash = TempDir::new("dataset-clash");
        clash.file("sub-1/a.dcm", b"x");
        clash.file("derivatives/dcm-anon/sub-1/a.dcm", b"y");
        let why = look(clash.path(), "deidentified", true).unwrap_err();
        assert_eq!(why.status, 409);
        assert!(
            why.message.contains("already holds sub-1"),
            "{}",
            why.message
        );
    }

    #[test]
    fn a_count_is_bounded_and_leaves_links_out() {
        let dir = TempDir::new("dataset-count");
        dir.file("a/1", b"12345");
        dir.file("a/b/2", b"12");
        dir.file("3", b"1");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("3"), dir.path().join("link")).unwrap();
        let counted = count(dir.path());
        assert_eq!(counted["files"], 3, "{counted}");
        assert_eq!(counted["bytes"], 8, "{counted}");
        assert_eq!(counted["partial"], false);
        assert!(
            counted["last_written"]
                .as_str()
                .is_some_and(|s| s.ends_with('Z'))
        );
        let none = count(&dir.path().join("nowhere"));
        assert_eq!(none["files"], 0);
        assert_eq!(none["last_written"], Value::Null);
        let trees = count_trees(
            dir.path(),
            &json!({"trees": {"originals": null, "anon": "."}}),
        );
        assert_eq!(trees["originals"], Value::Null);
        assert_eq!(trees["anon"]["files"], 3);
    }

    #[test]
    fn a_root_points_at_the_anon_tree_and_names_the_originals_beside_it() {
        let root = Root {
            name: "scans".into(),
            given: PathBuf::from("/data/scans"),
            anon: PathBuf::from("/data/scans/derivatives/dcm-anon"),
            originals: Some(PathBuf::from("/data/scans/derivatives/dcm-original")),
            place: Some("scans".into()),
        };
        assert_eq!(
            root.resolve(""),
            Path::new("/data/scans/derivatives/dcm-anon")
        );
        assert_eq!(
            root.resolve("sub-1"),
            Path::new("/data/scans/derivatives/dcm-anon/sub-1")
        );
        assert_eq!(
            root.resolve("originals"),
            Path::new("/data/scans/derivatives/dcm-original")
        );
        assert_eq!(
            root.resolve("originals/sub-1"),
            Path::new("/data/scans/derivatives/dcm-original/sub-1")
        );
        assert_eq!(
            root.resolve("originals-2"),
            Path::new("/data/scans/derivatives/dcm-anon/originals-2")
        );
        // a root with no originals has a folder named originals like any other
        let plain = Root::plain("scans", Path::new("/data/scans"));
        assert_eq!(
            plain.resolve("originals"),
            Path::new("/data/scans/originals")
        );
        assert_eq!(plain.as_json()["originals"], Value::Null);
    }

    #[test]
    fn a_stored_identity_is_the_rule_file_s_identity_block() {
        let dir = TempDir::new("dataset-rule");
        let file = dir.file(
            "rule.yml",
            b"identity:\n  id_type: study-id\n  from:\n    - field: PatientID\n      pattern: '^(?P<id>[0-9]+)$'\n",
        );
        let identity = identity_from_file(&file).unwrap();
        assert_eq!(identity["id_type"], "study-id");
        assert_eq!(identity["from"][0]["field"], "PatientID");
        let rule = rule_of(&identity).unwrap();
        assert_eq!(rule.id_type, "study-id");
        assert!(rule_of(&json!({"id_type": "study-id"})).is_err());
        let bad = dir.file("bad.yml", b"identity:\n  id_type: Study ID\n  from: []\n");
        assert!(identity_from_file(&bad).is_err());
    }
}
