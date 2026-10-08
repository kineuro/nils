// SPDX-License-Identifier: AGPL-3.0-only

//! The dataset on a source place, on disk (record 26 §1, §2 and §14). The
//! registry keeps what a dataset is (`nils_registry::place`); this module
//! looks at the folder when one is declared, makes the one kind of write the
//! engine makes under a source place, counts the trees for a page, and says
//! where `@name` points once a dataset has a pseudonymised tree.
//!
//! A dataset's two trees are `derivatives/dcm-original`, the originals the
//! pseudonymiser reads and nothing else does, and `derivatives/dcm-anon`,
//! the pseudonymised tree the registry reads. A v0 cohort folder holds
//! `derivatives/dcm-raw`, which is renamed `dcm-anon` on declaration, shown,
//! and never rewritten.
//!
//! Wave 7a §5.3: a folder is never read without the layout. A source place
//! nobody declared is `undeclared`, has no tree, and nothing in it is read.
//! Declaring it says how its files arrive, and the tree that arrival reads
//! must be there: where it is not and loose entries wait beside
//! `derivatives/`, the declaration names them and the tree they would go
//! into and is refused until the move is confirmed (`--confirm-move`,
//! `confirm_move: true`). Identified data goes into the originals, de-identified
//! or coded data into the pseudonymised tree, by a rename on the same
//! filesystem; nothing is copied and nothing is read. A dataset declared
//! before this, reading its folder itself, keeps that declaration.

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
pub(crate) const FIELDS: [&str; 8] = [
    "arrives",
    "identity",
    "unmapped",
    "patient_id",
    "cohort",
    "tags",
    "confirm_move",
    "move_into_anon",
];

/// The dataset's own fields the engine writes and a declaration may not
/// (lab 26c, finding 4): what became of the originals is what an act did
/// to them. Declaring `purged` on a dataset whose identified files sit on
/// disk would make every page say they are gone, and would take the acts
/// themselves off the page that says so.
pub(crate) const ENGINE_WRITTEN: [&str; 2] = ["originals_kept", "originals_vault"];

/// Why a declaration is refused: the status a door answers with and the
/// sentence naming what was wrong.
#[derive(Debug, Clone)]
pub(crate) struct Refused {
    pub(crate) status: u16,
    pub(crate) message: String,
    /// What the folder holds, when the refusal is the question of §5.3: the
    /// loose entries and the tree they would be moved into.
    pub(crate) layout: Option<Value>,
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
        layout: None,
    }
}

fn conflict(message: impl Into<String>) -> Refused {
    Refused {
        status: 409,
        message: message.into(),
        layout: None,
    }
}

/// Whether a body declares a field only an act writes, and the words it is
/// refused with: the act that sets it, by name, so a person who meant to
/// vault or purge the originals is told how (lab 26c, finding 4).
pub(crate) fn engine_written_refused(doc: &Value) -> Option<Refused> {
    let object = doc.as_object()?;
    let named = ENGINE_WRITTEN.iter().find(|k| object.contains_key(**k))?;
    Some(bad(format!(
        "{named} says what became of a dataset's originals and is written by the act that did it, never declared: vault or purge them with nils place originals <dataset> --vault --into PLACE --why TEXT or --purge --why TEXT, or POST /api/places/{{id}}/originals, and the dataset records it when the job succeeds"
    )))
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
    /// The loose entries a confirmed declaration moved just now, and the
    /// tree they went into.
    pub(crate) moved: Option<(&'static str, usize)>,
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
        moved: None,
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

/// The most loose entries an answer names; the count is always whole.
const NAMED_ENTRIES: usize = 100;

/// The tree a declared arrival reads, which its loose entries are moved
/// into: the originals for identified data, the pseudonymised tree
/// otherwise.
fn tree_for(arrives: &str) -> &'static str {
    if arrives == "identified" {
        ORIGINALS_TREE
    } else {
        ANON_TREE
    }
}

/// Whether a folder already holds the tree a declared arrival reads, a v0
/// folder's `dcm-raw` counting as the pseudonymised tree it becomes.
fn has_tree_for(layout: &Layout, arrives: &str) -> bool {
    if arrives == "identified" {
        layout.originals
    } else {
        layout.anon || layout.raw
    }
}

/// The layout as a door answers it (Wave 7a §5.3): the v0 folder's counts
/// when it is one, which trees are there, the loose entries beside them by
/// name (the first hundred) and in number, and for each declaration the
/// tree it reads and what it would move there: `needed` when that tree is
/// missing, so the declaration is refused until the move is confirmed.
/// `question` is true when neither tree is there: then nothing is read
/// until a person says how the files arrive. `moved` is what a confirmed
/// declaration moved just now.
pub(crate) fn layout_doc(path: &Path, layout: &Layout) -> Value {
    layout_doc_with(path, layout, true)
}

/// The layout from the folder's own listing alone, for a door that answers
/// every place at once: a v0 folder's files are not counted.
pub(crate) fn layout_doc_listed(path: &Path, layout: &Layout) -> Value {
    layout_doc_with(path, layout, false)
}

fn layout_doc_with(path: &Path, layout: &Layout, counted: bool) -> Value {
    let v0 = layout.v0().then(|| {
        if !counted {
            return json!({
                "original_files": null,
                "raw_files": null,
                "partial": true,
                "renamed": layout.renamed,
            });
        }
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
    let declarations: serde_json::Map<String, Value> = place::ARRIVALS
        .iter()
        .filter(|a| **a != place::UNDECLARED)
        .map(|a| {
            let tree = tree_for(a);
            let there = has_tree_for(layout, a);
            (
                a.to_string(),
                json!({
                    "reads": tree,
                    "tree_there": there,
                    "moves": layout.loose.len(),
                    "into": tree,
                    "needed": !there && !layout.loose.is_empty(),
                }),
            )
        })
        .collect();
    json!({
        "v0": v0,
        "originals": layout.originals,
        "anon": layout.anon,
        "raw": layout.raw,
        "loose": layout.loose.len(),
        "loose_entries": layout.loose.iter().take(NAMED_ENTRIES).collect::<Vec<_>>(),
        "question": !layout.originals && !layout.anon && !layout.raw,
        "declarations": declarations,
        "moved": layout.moved.map(|(into, n)| json!({"into": into, "entries": n})),
        "renamed": layout.renamed,
    })
}

/// The one write the engine makes under a source place outside the
/// pseudonymiser: the look at the folder when a dataset is declared
/// (Wave 7a §5.3). An undeclared dataset is only looked at: nothing is
/// written and it has no tree. A declared one has a v0 folder's `dcm-raw`
/// renamed `dcm-anon`. Its loose entries are moved into the tree its
/// arrival reads (the originals for identified data, the pseudonymised tree
/// otherwise) only when the move is confirmed; where that tree is missing
/// and loose entries wait, the declaration is refused with the question,
/// so nothing beside the layout is ever read. An empty tree is made where
/// one is missing and nothing would go into it. Every move is a rename
/// inside the folder, top-level entries only. A dataset declared before
/// Wave 7a that reads its folder itself (`keep_folder`) keeps doing so
/// unless a move is confirmed. The trees the dataset reads from now on come
/// back with the layout found.
pub(crate) fn look(
    path: &Path,
    arrives: &str,
    confirm_move: bool,
    keep_folder: bool,
) -> Result<(Value, Layout), Refused> {
    if !path.is_dir() {
        return Err(conflict(format!(
            "{} is not a directory; a dataset is one folder",
            path.display()
        )));
    }
    let mut layout = detect(path);
    if arrives == place::UNDECLARED {
        return Ok((json!({"originals": null, "anon": null}), layout));
    }
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
    let tree = tree_for(arrives);
    let there = has_tree_for(&layout, arrives);
    if keep_folder && arrives != "identified" && !layout.anon && !confirm_move {
        // declared before Wave 7a to read its folder itself: kept
        let trees = json!({
            "originals": layout.originals.then_some(ORIGINALS_TREE),
            "anon": ".",
        });
        return Ok((trees, layout));
    }
    if !layout.loose.is_empty() && !confirm_move && !there {
        let n = layout.loose.len();
        let mut refused = conflict(format!(
            "{} holds {n} loose entr{} and no {tree}: declared {arrives}, they would be moved into {tree}, and nothing is moved without a confirmation. Nothing was written. Confirm the move (--confirm-move, or confirm_move: true), or leave the folder undeclared",
            path.display(),
            if n == 1 { "y" } else { "ies" },
        ));
        refused.layout = Some(layout_doc(path, &layout));
        return Err(refused);
    }
    if confirm_move && !layout.loose.is_empty() {
        let dir = path.join(tree);
        std::fs::create_dir_all(&dir).map_err(|e| io(format!("making {tree}"), e))?;
        // every target checked before the first rename, so a clash moves
        // nothing
        if let Some(name) = layout.loose.iter().find(|n| dir.join(n).exists()) {
            return Err(conflict(format!(
                "{tree} already holds {name}; the loose entries were not moved"
            )));
        }
        let n = layout.loose.len();
        for name in std::mem::take(&mut layout.loose) {
            std::fs::rename(path.join(&name), dir.join(&name))
                .map_err(|e| io(format!("moving {name} into {tree}"), e))?;
        }
        layout.moved = Some((tree, n));
    }
    if arrives == "identified" {
        std::fs::create_dir_all(path.join(ORIGINALS_TREE))
            .map_err(|e| io(format!("making {ORIGINALS_TREE}"), e))?;
        layout.originals = true;
    }
    std::fs::create_dir_all(path.join(ANON_TREE))
        .map_err(|e| io(format!("making {ANON_TREE}"), e))?;
    layout.anon = true;
    let trees = json!({
        "originals": layout.originals.then_some(ORIGINALS_TREE),
        "anon": ANON_TREE,
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
/// probe with the trees counted. `confirm_move` (or `move_into_anon`, its
/// name from before) is the person's word that the loose entries may move.
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
            layout: None,
        })?
    {
        return Err(conflict(format!(
            "{} is the {tree} of the dataset {}; a dataset is declared on its own folder",
            path.display(),
            other.name
        )));
    }
    // Wave 7a §5.4: what PatientID holds is changed only on a dataset
    // nothing was pseudonymised into yet, since its tree would hold two kinds
    let in_force = current
        .and_then(|c| place::dataset_of(&c.dataset, None).ok())
        .map(|d| d["patient_id"].clone())
        .unwrap_or(Value::Null);
    if let Some(c) = current
        && dataset["patient_id"] != in_force
        && !in_force.is_null()
    {
        let written = pseudonymised_files(store, c.id).map_err(|e| Refused {
            status: 500,
            message: e.to_string(),
            layout: None,
        })?;
        if written > 0 {
            return Err(conflict(format!(
                "the dataset {} has {written} pseudonymised file(s) whose PatientID holds {}; what PatientID holds is changed only before anything is pseudonymised",
                c.name,
                in_force.as_str().unwrap_or("")
            )));
        }
    }
    let confirm_move = asked["confirm_move"].as_bool() == Some(true)
        || asked["move_into_anon"].as_bool() == Some(true);
    let arrives = dataset["arrives"]
        .as_str()
        .unwrap_or(place::UNDECLARED)
        .to_string();
    // a dataset declared before Wave 7a to read its folder itself, and
    // declared the same way again, keeps reading it
    let keep_folder = current.is_some_and(|c| {
        c.dataset["trees"]["anon"].as_str() == Some(".")
            && c.dataset["arrives"].as_str() == Some(arrives.as_str())
            && current.map(|c| Path::new(&c.path)) == Some(path)
    });
    let (trees, layout) = look(path, &arrives, confirm_move, keep_folder)?;
    dataset["trees"] = trees;
    let mut probed = crate::places::probe(path);
    probed["trees"] = count_trees(path, &dataset);
    Ok(Declared {
        dataset,
        layout: layout_doc(path, &layout),
        probed,
    })
}

/// How many of a dataset's originals have a pseudonymised copy.
fn pseudonymised_files(
    store: &mut Store,
    place_id: i64,
) -> Result<i64, nils_registry::store::Error> {
    if !nils_registry::migrate::table_exists(store, "pseudonym_file")? {
        return Ok(0);
    }
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE place_id = {} AND out_path IS NOT NULL",
        store.qualified("pseudonym_file"),
        store.dialect().param(1, nils_registry::schema::Type::Int)
    );
    Ok(store
        .query_opt(&sql, &[nils_registry::store::Param::Int(place_id)])?
        .map(|r| r.int(0))
        .transpose()?
        .unwrap_or(0))
}

/// What a source place is now, for a path that adds one without declaring
/// it (`nils setup`'s source place): undeclared, its folder looked at and
/// nothing written, with the layout found (Wave 7a §5.3).
pub(crate) fn undeclared(path: &Path) -> (Value, Value) {
    let layout = detect(path);
    (place::default_dataset(None), layout_doc(path, &layout))
}

/// What a person reads about a dataset's folder after it was added or
/// declared: what is read and what is not, in words (Wave 7a §5.3). The
/// place's id is the one `nils place set` names.
pub(crate) fn layout_lines(id: i64, dataset: &Value, layout: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(v0) = layout["v0"].as_object() {
        out.push(format!(
            "a v0 cohort folder: {} original files, {} pseudonymised files",
            v0["original_files"], v0["raw_files"]
        ));
    }
    if layout["renamed"].as_bool() == Some(true) {
        out.push(format!("{RAW_TREE} is now {ANON_TREE}"));
    }
    if let Some(m) = layout["moved"].as_object() {
        out.push(format!(
            "{} loose entries moved into {}",
            m["entries"],
            m["into"].as_str().unwrap_or("")
        ));
    }
    let loose = layout["loose"].as_u64().unwrap_or(0);
    let arrives = dataset["arrives"].as_str().unwrap_or(place::UNDECLARED);
    if arrives == place::UNDECLARED {
        let found = match (
            layout["originals"].as_bool() == Some(true),
            layout["anon"].as_bool() == Some(true) || layout["raw"].as_bool() == Some(true),
        ) {
            (true, true) => format!("{ORIGINALS_TREE} and a pseudonymised tree"),
            (true, false) => ORIGINALS_TREE.to_string(),
            (false, true) => "a pseudonymised tree".to_string(),
            (false, false) => format!("neither {ORIGINALS_TREE} nor {ANON_TREE}"),
        };
        out.push(format!(
            "undeclared: nothing in it is read. It holds {found}, and {loose} loose entr{} beside derivatives/",
            if loose == 1 { "y" } else { "ies" }
        ));
        out.push(format!(
            "declare how its files arrive: nils place set {id} --arrives identified|deidentified|coded{}",
            if loose > 0 && layout["question"].as_bool() == Some(true) {
                " --confirm-move (identified moves the loose entries into the originals, the others into the pseudonymised tree)"
            } else {
                ""
            }
        ));
        return out;
    }
    let reads = dataset["trees"]["anon"].as_str().unwrap_or("");
    if reads == "." {
        out.push(
            "reads the folder itself, as declared before the layout; never its originals".into(),
        );
    } else {
        out.push(format!("reads {reads} only"));
    }
    if loose > 0 && reads != "." {
        out.push(format!(
            "{loose} loose entr{} beside derivatives/, not read; nils place set {id} --confirm-move moves them into {}",
            if loose == 1 { "y" } else { "ies" },
            tree_for(arrives)
        ));
    }
    out
}

/// Why a digest of a path is refused (Wave 7a §5.3), beside the originals'
/// own refusal: the path lies in a source place whose dataset is
/// undeclared, or holds one, or lies in a declared dataset's folder outside
/// the tree its digest reads. None where the path is no dataset's, or is
/// inside the tree a dataset's digest reads.
pub(crate) fn not_read(store: &mut Store, path: &Path) -> Option<String> {
    let places = place::active(store).ok()?;
    let theirs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for p in places.iter().filter(|p| p.role == Role::Source) {
        let mine = std::fs::canonicalize(&p.path).unwrap_or_else(|_| PathBuf::from(&p.path));
        let inside = theirs.starts_with(&mine);
        let holds = !inside && mine.starts_with(&theirs);
        if !inside && !holds {
            continue;
        }
        if place::is_undeclared(&p.dataset) {
            return Some(format!(
                "{} {} the dataset {}, which is undeclared: nothing in it is read until how its files arrive is declared, with nils place set {} --arrives identified|deidentified|coded or the desk's Places page",
                path.display(),
                if inside { "is in" } else { "holds" },
                p.name,
                p.id
            ));
        }
        let Some(anon) = p.tree_path("anon") else {
            continue;
        };
        let anon = std::fs::canonicalize(&anon).unwrap_or(anon);
        if holds || !theirs.starts_with(&anon) {
            return Some(format!(
                "{} {} the dataset {}, whose digest reads its pseudonymised tree alone: digest @{}",
                path.display(),
                if inside {
                    "is in the folder of"
                } else {
                    "holds"
                },
                p.name,
                p.name
            ));
        }
    }
    None
}

/// Why a dataset may not be brought in, digested or pseudonymised: it is
/// undeclared (Wave 7a §5.3).
pub(crate) fn undeclared_refusal(p: &Place) -> Option<String> {
    place::is_undeclared(&p.dataset).then(|| {
        format!(
            "the dataset {} is undeclared: nothing in it is read until how its files arrive is declared, with nils place set {} --arrives identified|deidentified|coded or the desk's Places page",
            p.name, p.id
        )
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

/// The rule a pseudonymised tree is read under (record 26 §3): the code the
/// pseudonymiser wrote into `PatientID`, taken verbatim as the subject's
/// own, filed under the `subject-code` type, and never a pseudonym of the
/// pseudonym. The pattern takes any value of the code alphabet, whatever
/// its length, because a tree may hold codes another scheme made: a v0
/// cohort folder carries sixteen hex characters where this registry derives
/// twelve, and every person keeps the code they had. What a value is the
/// resolver decides, since only the registry knows its own codes: one a
/// subject holds is that subject's, one of the shape this registry makes
/// stands as its own code, and one that is neither is no code of this
/// registry, so a code is derived from it as from any identifier. A value
/// outside the alphabet falls back to the study UID, as any identifier the
/// rule cannot read does.
pub(crate) fn anon_rule() -> String {
    "identity:\n  id_type: subject-code\n  from:\n    - field: PatientID\n      pattern: '^(?<id>[0-9a-hjkmnp-tv-z]+)$'\n  code: verbatim\n".to_string()
}

/// The rule a pseudonymised tree whose PatientID holds an id type's value
/// is read under (Wave 7a §5.4): the value, as that type, found through
/// the linkage store.
pub(crate) fn id_type_rule(name: &str) -> String {
    format!("identity:\n  id_type: {name}\n  from:\n    - field: PatientID\n")
}

/// The rule stored on the dataset whose pseudonymised tree holds a path,
/// for a digest that names none of its own; none where no dataset holds the
/// path or the dataset stores no rule. An identified dataset's own rule is
/// for its originals, which the pseudonymiser reads; its pseudonymised tree
/// is read by what the dataset declares PatientID holds (Wave 7a §5.4):
/// the subject's code under [`anon_rule`], or an id type's value through
/// the linkage store. A dataset read in place that declares what PatientID
/// holds and stores no rule of its own is read the same way.
pub(crate) fn stored_rule(
    store: &mut Store,
    path: &Path,
) -> Result<Option<nils_digest::Rule>, String> {
    let Some(p) = place::tree_holding(store, "anon", path).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let identified = p.dataset["arrives"].as_str() == Some("identified");
    let declared =
        place::PatientId::of(&p.dataset).map_err(|e| format!("the dataset {}: {e}", p.name))?;
    let by_declaration = identified || (declared.is_some() && !p.dataset["identity"].is_object());
    if by_declaration {
        let mut rule = match declared.unwrap_or(place::PatientId::SubjectCode) {
            place::PatientId::SubjectCode => {
                let mut rule = nils_digest::Rule::parse(&anon_rule()).map_err(|e| e.to_string())?;
                // an identified dataset's tree is this registry's own, so
                // the resolver reads its codes as codes of this registry and
                // nothing else as one (record 26 §3)
                rule.own_codes = identified;
                rule
            }
            place::PatientId::IdType(name) => {
                nils_digest::Rule::parse(&id_type_rule(&name)).map_err(|e| e.to_string())?
            }
        };
        rule.source = Some(format!("the pseudonymised tree of the dataset {}", p.name));
        return Ok(Some(rule));
    }
    let identity = &p.dataset["identity"];
    if !identity.is_object() {
        return Ok(None);
    }
    let mut rule = rule_of(identity)
        .map_err(|e| format!("the identity rule of the dataset {}: {e}", p.name))?;
    rule.source = Some(format!("dataset {}", p.name));
    Ok(Some(rule))
}

/// Record 26 §4: what a digest of the dataset whose pseudonymised tree
/// holds a path does with a file whose identifier the linkage store does
/// not know. A dataset that arrives identified had the question answered by
/// the pseudonymiser, which held or coded the file before it wrote the tree
/// the digest reads, so a digest of that tree makes subjects as every
/// digest has; a dataset read in place answers it here.
pub(crate) fn unmapped_of(store: &mut Store, path: &Path) -> nils_digest::Unmapped {
    let Ok(Some(p)) = place::tree_holding(store, "anon", path) else {
        return nils_digest::Unmapped::Subject;
    };
    if p.dataset["arrives"].as_str() == Some("identified") {
        return nils_digest::Unmapped::Subject;
    }
    match p.dataset["unmapped"].as_str() {
        Some("hold") => nils_digest::Unmapped::Hold,
        Some("code") => nils_digest::Unmapped::Code,
        _ => nils_digest::Unmapped::Subject,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_dicom::synth::TempDir;

    /// Lab 26c, finding 4: what became of a dataset's originals is what an
    /// act did to them. A declaration naming it is refused, and the
    /// refusal says which act writes it, since a person who types
    /// `purged` means to purge.
    #[test]
    fn what_became_of_the_originals_is_never_a_declaration() {
        let refused =
            engine_written_refused(&json!({"cohort": "a", "originals_kept": "purged"})).unwrap();
        assert_eq!(refused.status, 400);
        assert!(
            refused.message.contains("nils place originals"),
            "{}",
            refused.message
        );
        assert!(
            refused.message.contains("never declared"),
            "{}",
            refused.message
        );
        assert!(engine_written_refused(&json!({"originals_vault": "archive"})).is_some());
        assert!(engine_written_refused(&json!({"cohort": "a"})).is_none());
        // and neither is a field a declaration may name any more
        for key in ENGINE_WRITTEN {
            assert!(!FIELDS.contains(&key), "{key}");
        }
        assert!(!fields_given(&json!({"originals_kept": "purged"})));
    }

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
        // not confirmed: the question, naming the entries and the tree, and
        // nothing moved or made
        let asked = look(dir.path(), "identified", false, false).unwrap_err();
        assert_eq!(asked.status, 409);
        assert!(
            asked.message.contains("3 loose entries"),
            "{}",
            asked.message
        );
        assert!(asked.message.contains(ORIGINALS_TREE), "{}", asked.message);
        let layout = asked.layout.unwrap();
        assert_eq!(
            layout["loose_entries"],
            json!(["notes.txt", "sub-1", "sub-2"])
        );
        assert_eq!(layout["question"], true);
        assert_eq!(layout["declarations"]["identified"]["into"], ORIGINALS_TREE);
        assert_eq!(layout["declarations"]["identified"]["needed"], true);
        assert_eq!(layout["declarations"]["deidentified"]["into"], ANON_TREE);
        assert_eq!(
            names(dir.path()),
            [".hidden", "notes.txt", "sub-1", "sub-2"]
        );
        // confirmed: moved
        let (trees, layout) = look(dir.path(), "identified", true, false).unwrap();
        assert_eq!(
            trees,
            json!({"originals": ORIGINALS_TREE, "anon": ANON_TREE})
        );
        assert!(layout.loose.is_empty() && !layout.renamed);
        assert_eq!(layout.moved, Some((ORIGINALS_TREE, 3)));
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
        let (again, layout) = look(dir.path(), "identified", false, false).unwrap();
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
        // undeclared: looked at, nothing renamed
        let (trees, layout) = look(dir.path(), "undeclared", false, false).unwrap();
        assert_eq!(trees, json!({"originals": null, "anon": null}));
        assert!(layout.raw && !layout.renamed);
        assert!(dir.path().join(RAW_TREE).is_dir());
        let (trees, layout) = look(dir.path(), "deidentified", false, false).unwrap();
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
        let why = look(dir.path(), "deidentified", false, false).unwrap_err();
        assert_eq!(why.status, 409);
        assert!(why.message.contains("keep one"), "{}", why.message);
    }

    #[test]
    fn a_deidentified_dataset_moves_into_the_anon_tree_only_when_confirmed() {
        let dir = TempDir::new("dataset-deidentified");
        dir.file("sub-1/a.dcm", b"x");
        // not confirmed: refused, the folder untouched and never read as
        // the pseudonymised tree
        let why = look(dir.path(), "deidentified", false, false).unwrap_err();
        assert_eq!(why.status, 409);
        assert!(why.message.contains(ANON_TREE), "{}", why.message);
        assert_eq!(names(dir.path()), ["sub-1"]);
        // undeclared: nothing written, no tree
        let (trees, layout) = look(dir.path(), "undeclared", true, false).unwrap();
        assert_eq!(trees, json!({"originals": null, "anon": null}));
        assert_eq!(layout.loose, ["sub-1"]);
        assert_eq!(names(dir.path()), ["sub-1"]);
        // confirmed: moved, and the tree is dcm-anon
        let (trees, layout) = look(dir.path(), "coded", true, false).unwrap();
        assert_eq!(trees, json!({"originals": null, "anon": ANON_TREE}));
        assert!(layout.loose.is_empty());
        assert_eq!(layout.moved, Some((ANON_TREE, 1)));
        assert!(dir.path().join(ANON_TREE).join("sub-1/a.dcm").is_file());
        assert_eq!(names(dir.path()), ["derivatives"]);
        // an empty folder declared with nothing to move gets an empty tree
        let empty = TempDir::new("dataset-empty");
        let (trees, _) = look(empty.path(), "deidentified", false, false).unwrap();
        assert_eq!(trees, json!({"originals": null, "anon": ANON_TREE}));
        assert!(empty.path().join(ANON_TREE).is_dir());
        // a dataset declared before to read its folder keeps reading it
        let kept = TempDir::new("dataset-kept");
        kept.file("sub-1/a.dcm", b"x");
        let (trees, _) = look(kept.path(), "deidentified", false, true).unwrap();
        assert_eq!(trees, json!({"originals": null, "anon": "."}));
        assert_eq!(names(kept.path()), ["sub-1"]);
        // where the tree is there, loose entries wait unread and unmoved
        let beside = TempDir::new("dataset-beside");
        beside.file("notes/a.txt", b"x");
        beside.file("derivatives/dcm-anon/sub-1/a.dcm", b"y");
        let (trees, layout) = look(beside.path(), "deidentified", false, false).unwrap();
        assert_eq!(trees["anon"], ANON_TREE);
        assert_eq!(layout.loose, ["notes"]);
        assert_eq!(layout.moved, None);
        // a loose entry the tree already holds is not moved over it
        let clash = TempDir::new("dataset-clash");
        clash.file("sub-1/a.dcm", b"x");
        clash.file("derivatives/dcm-anon/sub-1/a.dcm", b"y");
        let why = look(clash.path(), "deidentified", true, false).unwrap_err();
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
    fn the_pseudonymised_tree_is_read_under_the_verbatim_code_rule() {
        let rule = nils_digest::Rule::parse(&anon_rule()).unwrap();
        assert!(rule.verbatim);
        assert_eq!(rule.id_type, "subject-code");
        let mut x = {
            use nils_dicom::synth::{MetaFields, TempDir, minimal_mr, part10};
            let dir = TempDir::new("anon-rule");
            let path = dir.file(
                "a.dcm",
                &part10(
                    &MetaFields::mr("1.2.3.4.5"),
                    &minimal_mr("1.2.3", "1.2.3.4", "1.2.3.4.5"),
                    true,
                ),
            );
            nils_dicom::extract(&path).unwrap()
        };
        let read = |rule: &nils_digest::Rule, x: &mut nils_dicom::Extracted, value: &str| {
            x.identity = nils_dicom::Identity {
                values: vec![Some(value.into())],
            };
            rule.apply(x, "s/1.dcm")
        };
        let ident = read(&rule, &mut x, "xg5pf9g20xwm");
        assert_eq!(ident.value, "xg5pf9g20xwm");
        assert!(!ident.fell_back);
        // outside the code alphabet: the fallback, never a pseudonym of it
        assert!(read(&rule, &mut x, "19900101-1234").fell_back);
        assert!(
            read(&rule, &mut x, "xg5pf9g20xwl").fell_back,
            "no l in the alphabet"
        );
        assert!(read(&rule, &mut x, "Xg5pf9g20xwm").fell_back, "lower case");
        // any length of the alphabet is read, and the resolver decides what
        // the value is: a v0 cohort's sixteen hex characters are read under
        // a registry whose own codes are twelve, so that a person a map
        // named keeps the code they had (record 26 §3)
        assert!(!read(&rule, &mut x, "xg5pf9g20xw").fell_back, "shorter");
        assert!(!read(&rule, &mut x, "xg5pf9g20xwmk").fell_back, "longer");
        assert!(!read(&rule, &mut x, "771c4326c89c082c").fell_back, "v0's");
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
