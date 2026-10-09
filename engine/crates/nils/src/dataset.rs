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
pub(crate) const FIELDS: [&str; 12] = [
    "arrives",
    "move_into",
    "identity",
    "unmapped",
    "patient_id",
    "subjects",
    "copy_folder",
    "cohort",
    "tags",
    "confirm_move",
    "move_into_anon",
    // record 55 H2 (round 4): picking main scans after a sort, or not
    "picks",
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

/// What a dataset's folder holds: which trees are there, whether it is a v0
/// cohort folder, and the entries beside `derivatives/`, with those that
/// hold DICOM. Read from the folder's own listing, and of a loose entry no
/// more than the first bytes of its files, up to a bound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) derivatives: bool,
    pub(crate) originals: bool,
    pub(crate) anon: bool,
    pub(crate) raw: bool,
    /// The pseudonymised tree (or `dcm-raw`) holds something: beside the
    /// originals, the dataset is identified with its anonymised copy; an
    /// empty one, made for the pseudonymiser, is no copy yet.
    pub(crate) anon_filled: bool,
    /// The names of the top-level entries other than `derivatives` and the
    /// hidden ones, sorted.
    pub(crate) loose: Vec<String>,
    /// The loose entries that hold DICOM, or may: an entry whose look ran
    /// out of its bound before it was through counts.
    pub(crate) loose_dicom: Vec<String>,
    /// Whether this look renamed `dcm-raw` to `dcm-anon`.
    pub(crate) renamed: bool,
    /// The loose entries a confirmed move put into a tree just now.
    pub(crate) moved: Option<(&'static str, usize)>,
}

/// What a dataset's structure says (Wave 7a, Nima 2026-10-08: "NILS should
/// always get the declaration from structure").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    /// Only `derivatives/dcm-original`: identified data, which the
    /// pseudonymiser reads and writes into `dcm-anon`.
    Identified,
    /// Only `derivatives/dcm-anon` (or `dcm-raw`, renamed): already
    /// anonymised, read by the registry.
    Anonymised,
    /// Both: identified, with its anonymised copy.
    Both,
    /// Anything else: entries holding DICOM beside `derivatives/`, or no
    /// tree. Nothing is read until a person says which tree they go into.
    Unknown,
}

impl State {
    pub(crate) fn name(self) -> &'static str {
        match self {
            State::Identified => "identified",
            State::Anonymised => "anonymised",
            State::Both => "both",
            State::Unknown => "unknown",
        }
    }

    /// The arrival the structure means, as the registry keeps it.
    pub(crate) fn arrives(self) -> &'static str {
        match self {
            State::Identified | State::Both => "identified",
            State::Anonymised => "deidentified",
            State::Unknown => place::UNDECLARED,
        }
    }
}

impl Layout {
    /// A v0 cohort folder: it holds `derivatives/dcm-raw`, or held it until
    /// this look renamed it.
    fn v0(&self) -> bool {
        self.raw || self.renamed
    }

    /// What the structure says.
    pub(crate) fn state(&self) -> State {
        if !self.loose_dicom.is_empty() {
            return State::Unknown;
        }
        match (self.originals, self.anon || self.raw, self.anon_filled) {
            (true, _, true) => State::Both,
            (true, _, false) => State::Identified,
            (false, true, _) => State::Anonymised,
            (false, false, _) => State::Unknown,
        }
    }
}

/// How far the look into a loose entry goes before it says the entry may
/// hold DICOM: a page's answer, never a walk of an archive.
const LOOK_ENTRIES: usize = 2_000;
const LOOK_FOR: Duration = Duration::from_millis(500);

/// What a bounded look found of DICOM in an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dicom {
    Yes,
    No,
    /// The look ran out of its bound before it found any or was through.
    Unknown,
}

impl Dicom {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Dicom::Yes => "yes",
            Dicom::No => "no",
            Dicom::Unknown => "unknown",
        }
    }
}

/// Whether an entry holds DICOM: a file named `.dcm`, or one with the
/// `DICM` mark at byte 128; a folder holding one. A look that runs out of
/// its bound says it does not know.
pub(crate) fn look_for_dicom(path: &Path) -> Dicom {
    use std::io::Read as _;
    let until = Instant::now() + LOOK_FOR;
    let mut seen = 0usize;
    let mut queue = vec![path.to_path_buf()];
    while let Some(at) = queue.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&at) else {
            continue;
        };
        seen += 1;
        if seen > LOOK_ENTRIES || Instant::now() >= until {
            return Dicom::Unknown;
        }
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&at) {
                queue.extend(entries.flatten().map(|e| e.path()));
            }
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        if at
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dcm"))
        {
            return Dicom::Yes;
        }
        let mut head = [0u8; 132];
        if let Ok(mut f) = std::fs::File::open(&at)
            && f.read_exact(&mut head).is_ok()
            && &head[128..132] == b"DICM"
        {
            return Dicom::Yes;
        }
    }
    Dicom::No
}

/// Whether a loose entry may hold DICOM: what the look did not see it
/// cannot vouch for, so an unfinished look counts.
fn holds_dicom(path: &Path) -> bool {
    look_for_dicom(path) != Dicom::No
}

pub(crate) fn detect(path: &Path) -> Layout {
    let mut layout = Layout {
        derivatives: path.join("derivatives").is_dir(),
        originals: path.join(ORIGINALS_TREE).is_dir(),
        anon: path.join(ANON_TREE).is_dir(),
        raw: path.join(RAW_TREE).is_dir(),
        anon_filled: [ANON_TREE, RAW_TREE]
            .iter()
            .any(|t| std::fs::read_dir(path.join(t)).is_ok_and(|mut d| d.next().is_some())),
        ..Layout::default()
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
    layout.loose_dicom = layout
        .loose
        .iter()
        .filter(|n| holds_dicom(&path.join(n)))
        .cloned()
        .collect();
    layout
}

/// What a folder a source place names is (Wave 7a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    /// It holds `derivatives/`: one dataset.
    Dataset,
    /// It is a dataset's pseudonymised tree itself, `…/derivatives/dcm-raw`
    /// or `…/derivatives/dcm-anon`: read as that tree.
    Legacy,
    /// Anything else: a root, each folder under it a dataset.
    Root,
}

pub(crate) fn shape_of(path: &Path) -> Shape {
    let in_derivatives = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|p| p == "derivatives");
    let tree = path
        .file_name()
        .is_some_and(|n| n == "dcm-raw" || n == "dcm-anon");
    if in_derivatives && tree {
        Shape::Legacy
    } else if path.join("derivatives").is_dir() {
        Shape::Dataset
    } else {
        Shape::Root
    }
}

/// The most loose entries an answer names; the count is always whole.
const NAMED_ENTRIES: usize = 100;

/// The trees a move may put an unknown dataset's entries into, by the word
/// a person gives.
pub(crate) const MOVE_INTO: [(&str, &str); 2] =
    [("originals", ORIGINALS_TREE), ("anon", ANON_TREE)];

/// The layout as a door answers it (Wave 7a): which trees are there, the v0
/// folder's counts when it is one, the `state` the structure says and the
/// tree the registry `reads`; the loose entries (the first hundred named)
/// and those holding DICOM; `question`, true when the state is unknown and
/// a person must say which tree the entries go into, with `move_into` the
/// choices; what the dataset's settings ask (`settings`); and what a
/// confirmed move or the rename did just now (`moved`, `renamed`).
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
    let state = layout.state();
    let anonymised = state == State::Anonymised;
    json!({
        "v0": v0,
        "derivatives": layout.derivatives,
        "originals": layout.originals,
        "anon": layout.anon,
        "raw": layout.raw,
        "state": state.name(),
        "reads": (state != State::Unknown).then_some(ANON_TREE),
        "pseudonymises": matches!(state, State::Identified | State::Both),
        "loose": layout.loose.len(),
        "loose_entries": layout.loose.iter().take(NAMED_ENTRIES).collect::<Vec<_>>(),
        "loose_dicom": layout.loose_dicom.iter().take(NAMED_ENTRIES).collect::<Vec<_>>(),
        "question": state == State::Unknown,
        "move_into": (state == State::Unknown).then(|| json!({
            "choices": MOVE_INTO.iter().map(|(w, _)| *w).collect::<Vec<_>>(),
            "trees": {"originals": ORIGINALS_TREE, "anon": ANON_TREE},
            "entries": layout.loose_dicom.len(),
            "originals": "identified data: the pseudonymiser reads it and writes the anonymised copy",
            "anon": "already anonymised: the registry reads it",
        })),
        "settings": {
            "patient_id": {
                "choices": [place::PATIENT_ID_CODE, format!("{}<name>", place::PATIENT_ID_TYPE)],
                "required": anonymised,
                "default": matches!(state, State::Identified | State::Both).then_some(place::PATIENT_ID_CODE),
            },
            "subjects": anonymised.then(|| json!({
                "choices": place::SUBJECTS,
                "required": true,
                "map": "a map of subject codes to the dataset's ids, given or already in the registry; a file whose id no map names is held",
                "generated": "the subject code generator makes each code from the id, as from a personnummer; every subject is a subject, never provisional",
            })),
            "copy_folder": {"choices": place::FOLDERS, "default": place::FOLDERS[0]},
        },
        "moved": layout.moved.map(|(into, n)| json!({"into": into, "entries": n})),
        "renamed": layout.renamed,
    })
}

/// The one write the engine makes under a source place outside the
/// pseudonymiser: settling a dataset's folder (Wave 7a). An unknown
/// dataset's entries holding DICOM go into the tree a person names, and
/// only on the person's word: `move_into` without `confirm` is refused with
/// the question and nothing is moved. Then a v0 folder's `dcm-raw` is
/// renamed `dcm-anon` and shown, and an identified dataset gets an empty
/// `dcm-anon` for the pseudonymiser to write into. Every move is a rename
/// inside the folder, top-level entries only. The trees the dataset reads
/// from now on come back with what the structure says and the layout.
pub(crate) fn settle(
    path: &Path,
    move_into: Option<&str>,
    confirm: bool,
) -> Result<(State, Value, Layout), Refused> {
    if !path.is_dir() {
        return Err(conflict(format!(
            "{} is not a directory; a dataset is one folder",
            path.display()
        )));
    }
    let mut layout = detect(path);
    let io = |what: String, e: std::io::Error| conflict(format!("{what}: {e}"));
    if let Some(word) = move_into {
        let Some((_, tree)) = MOVE_INTO.iter().find(|(w, _)| *w == word) else {
            return Err(bad(format!(
                "move_into is originals (identified data) or anon (already anonymised), not {word}"
            )));
        };
        if layout.loose_dicom.is_empty() {
            return Err(conflict(format!(
                "{} holds no entry with DICOM beside derivatives/; nothing to move",
                path.display()
            )));
        }
        if !confirm {
            let n = layout.loose_dicom.len();
            let mut refused = conflict(format!(
                "{} holds {n} entr{} with DICOM beside derivatives/: they would be moved into {tree}, and nothing is moved without a confirmation. Nothing was written. Confirm the move (--confirm-move, or confirm_move: true)",
                path.display(),
                if n == 1 { "y" } else { "ies" },
            ));
            refused.layout = Some(layout_doc(path, &layout));
            return Err(refused);
        }
        let dir = path.join(tree);
        std::fs::create_dir_all(&dir).map_err(|e| io(format!("making {tree}"), e))?;
        // every target checked before the first rename, so a clash moves
        // nothing
        if let Some(name) = layout.loose_dicom.iter().find(|n| dir.join(n).exists()) {
            return Err(conflict(format!(
                "{tree} already holds {name}; the entries were not moved"
            )));
        }
        let moving = std::mem::take(&mut layout.loose_dicom);
        let n = moving.len();
        for name in &moving {
            std::fs::rename(path.join(name), dir.join(name))
                .map_err(|e| io(format!("moving {name} into {tree}"), e))?;
        }
        layout.loose.retain(|l| !moving.contains(l));
        layout.derivatives = true;
        if *tree == ORIGINALS_TREE {
            layout.originals = true;
        } else {
            layout.anon = true;
        }
        layout.moved = Some((tree, n));
    }
    let state = layout.state();
    if state == State::Unknown {
        return Ok((state, json!({"originals": null, "anon": null}), layout));
    }
    if layout.raw {
        if layout.anon {
            return Err(conflict(format!(
                "{} holds both {RAW_TREE} and {ANON_TREE}; keep one",
                path.display()
            )));
        }
        std::fs::rename(path.join(RAW_TREE), path.join(ANON_TREE))
            .map_err(|e| io(format!("renaming {RAW_TREE} to {ANON_TREE}"), e))?;
        layout.raw = false;
        layout.anon = true;
        layout.renamed = true;
    }
    if !layout.anon {
        std::fs::create_dir_all(path.join(ANON_TREE))
            .map_err(|e| io(format!("making {ANON_TREE}"), e))?;
        layout.anon = true;
    }
    let trees = json!({
        "originals": layout.originals.then_some(ORIGINALS_TREE),
        "anon": ANON_TREE,
    });
    Ok((state, trees, layout))
}

/// What a declaration worked out: the dataset to store, the layout found,
/// and the trees' counts for the probe.
pub(crate) struct Declared {
    pub(crate) dataset: Value,
    pub(crate) layout: Value,
    pub(crate) probed: Value,
}

/// Why `arrives` is refused: the structure says it.
const ARRIVES_IS_READ: &str = "how a dataset's files arrive is read from its folder, never declared: derivatives/dcm-original is identified data, derivatives/dcm-anon (or dcm-raw) is anonymised, both is identified with its anonymised copy; an unknown dataset's entries go into one of them with move_into (originals or anon) and confirm_move";

/// A dataset's settings and its folder settled (Wave 7a): the settings
/// asked for (what PatientID holds, how subjects are found, the folder of
/// each copy, the identity rule, the cohort, the tag lists) checked and
/// merged over the place in force; the folder refused when it is a tree of
/// another dataset; the folder settled, a confirmed move included; and
/// what the structure says stored as the dataset's arrival, state and
/// trees. `arrives` is refused: the structure says it. A place that names
/// a pseudonymised tree itself is settled as legacy, nothing written.
pub(crate) fn declare(
    store: &mut Store,
    path: &Path,
    asked: &Value,
    current: Option<&Place>,
) -> Result<Declared, Refused> {
    if asked.get("arrives").is_some_and(|a| !a.is_null()) {
        return Err(bad(ARRIVES_IS_READ));
    }
    let mut settings = asked.clone();
    if let Some(o) = settings.as_object_mut() {
        for k in [
            "move_into",
            "confirm_move",
            "move_into_anon",
            "kind",
            "state",
            "trees",
            "root",
        ] {
            o.remove(k);
        }
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
    let confirm = asked["confirm_move"].as_bool() == Some(true)
        || asked["move_into_anon"].as_bool() == Some(true);
    let move_into = match &asked["move_into"] {
        Value::String(s) => Some(s.as_str()),
        // the name a de-identified folder's move had before
        _ if asked["move_into_anon"].as_bool() == Some(true) => Some("anon"),
        _ => None,
    };
    let (structural, layout) = if shape_of(path) == Shape::Legacy {
        if move_into.is_some() {
            return Err(bad(
                "this place names a pseudonymised tree itself; there is nothing beside it to move",
            ));
        }
        (
            json!({
                "kind": "legacy",
                "arrives": "deidentified",
                "state": "anonymised",
                "trees": {"originals": null, "anon": "."},
            }),
            json!({"legacy": true, "state": "anonymised", "reads": ".", "question": false}),
        )
    } else {
        let (state, trees, found) = settle(path, move_into, confirm)?;
        (
            json!({
                "kind": "dataset",
                "arrives": state.arrives(),
                "state": state.name(),
                "trees": trees,
            }),
            layout_doc(path, &found),
        )
    };
    for (k, v) in structural.as_object().into_iter().flatten() {
        settings[k] = v.clone();
    }
    let dataset = place::dataset_of(&settings, current.map(|p| &p.dataset)).map_err(bad)?;
    if !dataset["identity"].is_null() {
        let rule = rule_of(&dataset["identity"]).map_err(|e| bad(format!("identity: {e}")))?;
        if dataset["arrives"] == "deidentified" {
            let agrees = match place::PatientId::of(&dataset).map_err(bad)? {
                Some(place::PatientId::SubjectCode) => rule.verbatim,
                Some(place::PatientId::IdType(name)) => rule.id_type == name && !rule.verbatim,
                None => true,
            };
            if !agrees {
                return Err(bad(format!(
                    "identity: the rule files {}{} and PatientID is declared to hold {}; they say the same or the rule is left out",
                    rule.id_type,
                    if rule.verbatim { " as codes" } else { "" },
                    dataset["patient_id"].as_str().unwrap_or("")
                )));
            }
        }
    }
    // Wave 7a §5.4: what PatientID holds and what names each copy's folder
    // change only on a dataset nothing was pseudonymised into yet
    let in_force = current.and_then(|c| place::dataset_of(&c.dataset, None).ok());
    if let (Some(c), Some(was)) = (current, in_force.as_ref()) {
        let changed = |k: &str| dataset[k] != was[k] && !was[k].is_null();
        if changed("patient_id") || changed("copy_folder") {
            let written = pseudonymised_files(store, c.id).map_err(|e| Refused {
                status: 500,
                message: e.to_string(),
                layout: None,
            })?;
            if written > 0 {
                return Err(conflict(format!(
                    "the dataset {} has {written} pseudonymised file(s) whose PatientID holds {}; what PatientID holds and what names each copy's folder are changed only before anything is pseudonymised",
                    c.name,
                    was["patient_id"].as_str().unwrap_or("")
                )));
            }
        }
    }
    let mut probed = crate::places::probe(path);
    probed["trees"] = count_trees(path, &dataset);
    Ok(Declared {
        dataset,
        layout,
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

/// One dataset a refresh settled, or one a person added, as an answer
/// names it.
pub(crate) struct Found {
    pub(crate) place: Place,
    pub(crate) layout: Value,
    /// Made by this act, not met again.
    pub(crate) new: bool,
}

/// A source place's folder made what it is (Wave 7a): a root is only the
/// root (Nima, 2026-10-08: "on starting page we just need to add a root";
/// a folder under it becomes a dataset only when a person adds it); a
/// dataset or a legacy tree is settled. The settings asked go to a
/// dataset; a root takes none. Returns the dataset to store on the place
/// and its layout; the place itself is the caller's to write.
pub(crate) fn shape_place(
    store: &mut Store,
    _name: &str,
    path: &Path,
    asked: &Value,
    current: Option<&Place>,
    _guarantees: &Value,
) -> Result<(Declared, Vec<Found>), Refused> {
    if asked.get("arrives").is_some_and(|a| !a.is_null()) {
        return Err(bad(ARRIVES_IS_READ));
    }
    // a folder whose entries a person puts into a tree is one dataset,
    // whatever it held before; so is a dataset added under a root, and one
    // whose structure said what it is. A place from before that said
    // nothing is looked at again, and may be a root
    let one = asked["move_into"].is_string()
        || asked["move_into_anon"].as_bool() == Some(true)
        || current.is_some_and(|c| {
            c.dataset["kind"] == "dataset"
                && (c.dataset["state"] != "unknown" || !c.dataset["root"].is_null())
        });
    if one || shape_of(path) != Shape::Root {
        return Ok((declare(store, path, asked, current)?, Vec::new()));
    }
    // a root is never a dataset's tree, nor inside one
    if let Some((other, tree)) =
        tree_of_another(store, path, current.map(|p| p.id)).map_err(|e| Refused {
            status: 500,
            message: e.to_string(),
            layout: None,
        })?
    {
        return Err(conflict(format!(
            "{} is the {tree} of the dataset {}; a source is a folder of its own",
            path.display(),
            other.name
        )));
    }
    if in_originals(path) {
        return Err(conflict(format!(
            "{} is in a dataset's originals, which the pseudonymiser alone reads",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(conflict(format!(
            "{} is not a directory; a source is a folder",
            path.display()
        )));
    }
    if fields_given(asked) {
        return Err(bad(format!(
            "{} is a root: a folder under it becomes a dataset when it is added (nils place add-dataset ROOT FOLDER, or POST /api/places with root and folder), and a dataset's settings are given on the dataset",
            path.display()
        )));
    }
    let dataset = place::dataset_of(
        &json!({"kind": "root", "arrives": place::UNDECLARED, "state": "unknown", "trees": null, "root": null}),
        current.map(|p| &p.dataset),
    )
    .map_err(bad)?;
    let folders = sub_folders(path).len();
    let layout = json!({
        "root": true,
        "folders": folders,
        "loose": root_loose(path),
    });
    let mut probed = crate::places::probe(path);
    probed["folders"] = json!(folders);
    Ok((
        Declared {
            dataset,
            layout,
            probed,
        },
        Vec::new(),
    ))
}

/// Whether a path is in a dataset's originals, whoever's.
fn in_originals(path: &Path) -> bool {
    let parts: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();
    parts
        .windows(2)
        .any(|w| w[0] == "derivatives" && w[1] == "dcm-original")
}

/// The files at a root's top, which belong to no dataset and are not read.
fn root_loose(path: &Path) -> usize {
    std::fs::read_dir(path)
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    !e.file_name().to_string_lossy().starts_with('.')
                        && e.file_type().is_ok_and(|t| !t.is_dir())
                })
                .count()
        })
        .unwrap_or(0)
}

/// The folders right under a root, hidden ones aside, sorted.
fn sub_folders(path: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(path)
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    !e.file_name().to_string_lossy().starts_with('.')
                        && e.file_type().is_ok_and(|t| t.is_dir())
                })
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// The active source place a root is, by its id or its name.
pub(crate) fn root_named(store: &mut Store, given: &str) -> Result<Place, Refused> {
    let found = match given.parse::<i64>() {
        Ok(id) => place::show(store, id),
        Err(_) => place::by_name(store, given),
    }
    .map_err(|e| Refused {
        status: 500,
        message: e.to_string(),
        layout: None,
    })?;
    match found {
        Some(p) if p.retired_at.is_none() && p.dataset["kind"] == "root" => Ok(p),
        Some(p) => Err(bad(format!("{} is no source root", p.name))),
        None => Err(Refused {
            status: 404,
            message: format!("no place {given}"),
            layout: None,
        }),
    }
}

/// The most folders one page of a root's listing names, and its default.
pub(crate) const FOLDERS_PAGE: usize = 50;
pub(crate) const FOLDERS_MOST: usize = 200;

/// A page of a root's folders (Wave 7a, Nima 2026-10-08: "if user want
/// will add one by looking up. what if the root folder has 1000 folders"):
/// one read of the root's own listing, never a look inside a folder. The
/// folders whose name holds `q` (any case), by name, after the name
/// `after`, at most `limit`; each with its path and whether, and as which
/// dataset, it was added. `next` is the name to page on from, or none at
/// the end. What a folder holds is the single look's, [`folder_look`].
pub(crate) fn folders(
    store: &mut Store,
    root: &Place,
    q: Option<&str>,
    limit: usize,
    after: Option<&str>,
) -> Result<(Vec<Value>, usize, Option<String>), Refused> {
    let path = PathBuf::from(&root.path);
    let entries =
        std::fs::read_dir(&path).map_err(|e| conflict(format!("{}: {e}", path.display())))?;
    let q = q.map(str::to_lowercase).filter(|q| !q.is_empty());
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .filter(|n| q.as_deref().is_none_or(|q| n.to_lowercase().contains(q)))
        .collect();
    names.sort();
    let matching = names.len();
    let limit = limit.clamp(1, FOLDERS_MOST);
    let page: Vec<String> = names
        .into_iter()
        .filter(|n| after.is_none_or(|a| n.as_str() > a))
        .take(limit + 1)
        .collect();
    let next = (page.len() > limit).then(|| page[limit - 1].clone());
    let added = added_places(store, &path)?;
    let root_real = std::fs::canonicalize(&path).unwrap_or(path);
    let rows = page
        .into_iter()
        .take(limit)
        .map(|name| {
            let real = root_real.join(&name);
            let p = added.iter().find(|(at, _)| *at == real).map(|(_, p)| p);
            json!({
                "name": name,
                "path": real.display().to_string(),
                "added": p.is_some(),
                "dataset_id": p.map(|p| p.id),
                "dataset": p.map(|p| p.name.clone()),
            })
        })
        .collect();
    Ok((rows, matching, next))
}

/// The active source places directly under a folder, by canonical path.
fn added_places(store: &mut Store, under: &Path) -> Result<Vec<(PathBuf, Place)>, Refused> {
    let under = std::fs::canonicalize(under).unwrap_or_else(|_| under.to_path_buf());
    Ok(place::active(store)
        .map_err(|e| Refused {
            status: 500,
            message: e.to_string(),
            layout: None,
        })?
        .into_iter()
        .filter(|p| p.role == Role::Source)
        .filter_map(|p| {
            let real = std::fs::canonicalize(&p.path).unwrap_or_else(|_| PathBuf::from(&p.path));
            (real.parent() == Some(under.as_path())).then_some((real, p))
        })
        .collect())
}

/// One folder of a root looked at before it is added (Wave 7a), nothing
/// changed: whether it was added and as which dataset, whether a bounded
/// look finds DICOM in it (yes, no or unknown where the look ran out),
/// whether it holds `derivatives/`, and the layout its structure would give
/// it as a dataset: its state, what would be read, and the question an
/// unknown one would ask. The folder is named as it is under the root.
pub(crate) fn folder_look(store: &mut Store, root: &Place, name: &str) -> Result<Value, Refused> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(bad(format!("{name}: a folder's name under the root")));
    }
    let root_path = PathBuf::from(&root.path);
    let path = root_path.join(name);
    if !path.is_dir() {
        return Err(Refused {
            status: 404,
            message: format!("no folder {name} under the root {}", root.name),
            layout: None,
        });
    }
    let added = added_places(store, &root_path)?;
    let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let p = added.iter().find(|(at, _)| *at == real).map(|(_, p)| p);
    let layout = match shape_of(&path) {
        Shape::Legacy => {
            json!({"legacy": true, "state": "anonymised", "reads": ".", "question": false})
        }
        _ => layout_doc(&path, &detect(&path)),
    };
    Ok(json!({
        "name": name,
        "path": real.display().to_string(),
        "added": p.is_some(),
        "dataset_id": p.map(|p| p.id),
        "dataset": p.map(|p| p.name.clone()),
        "holds_dicom": look_for_dicom(&path).name(),
        "has_derivatives": path.join("derivatives").is_dir(),
        "layout": layout,
    }))
}

/// A folder under a root added as a dataset, by a person's act (Wave 7a):
/// only then is its structure read and its state derived, with the move
/// question and the settings as any declaration has them. The folder is
/// named by its name under the root or by a path under it; the dataset
/// takes the name given, else the folder's, the root's name before it
/// where that is taken. A folder already a place is refused.
pub(crate) fn add_dataset(
    store: &mut Store,
    root: &Place,
    folder: &str,
    name: Option<&str>,
    asked: &Value,
) -> Result<Found, Refused> {
    let failed = |e: nils_registry::store::Error| Refused {
        status: 500,
        message: e.to_string(),
        layout: None,
    };
    let root_path = std::fs::canonicalize(&root.path).unwrap_or_else(|_| PathBuf::from(&root.path));
    let given = Path::new(folder);
    let path = if given.is_absolute() {
        given.to_path_buf()
    } else {
        if folder.split('/').any(|s| s == ".." || s.is_empty()) {
            return Err(bad(format!(
                "{folder}: a folder under the root, by its name"
            )));
        }
        root_path.join(given)
    };
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    if path == root_path || !path.starts_with(&root_path) {
        return Err(bad(format!(
            "{} is not a folder under the root {}",
            path.display(),
            root.name
        )));
    }
    if !path.is_dir() {
        return Err(conflict(format!("{} is not a directory", path.display())));
    }
    if in_originals(&path) {
        return Err(conflict(format!(
            "{} is in a dataset's originals, which the pseudonymiser alone reads",
            path.display()
        )));
    }
    if let Some(p) = place::active(store)
        .map_err(failed)?
        .into_iter()
        .find(|p| std::fs::canonicalize(&p.path).unwrap_or_else(|_| PathBuf::from(&p.path)) == path)
    {
        return Err(conflict(format!(
            "{} is already the place {}",
            path.display(),
            p.name
        )));
    }
    let name = match name {
        Some(n) => {
            if place::by_name(store, n).map_err(failed)?.is_some() {
                return Err(conflict(format!("a place is already named {n}")));
            }
            n.to_string()
        }
        None => {
            let base = dataset_name(&path);
            if base != root.name && place::by_name(store, &base).map_err(failed)?.is_none() {
                base
            } else {
                format!("{}-{base}", root.name)
            }
        }
    };
    let d = declare(store, &path, asked, None)?;
    let mut dataset = d.dataset;
    dataset["root"] = json!(root.name);
    let id = place::add(
        store,
        &place::New {
            name: &name,
            role: Role::Source,
            path: &path.display().to_string(),
            guarantees: root.guarantees.clone(),
            probed: d.probed,
            handling: Value::Null,
            dataset,
        },
    )
    .map_err(|e| conflict(e.to_string()))?;
    let p = place::show(store, id)
        .map_err(failed)?
        .ok_or_else(|| conflict(format!("place {id} was not written")))?;
    Ok(Found {
        place: p,
        layout: d.layout,
        new: true,
    })
}

/// The datasets' folders looked at again (Wave 7a): each dataset and
/// legacy place, the one named or all, settled with its own settings, its
/// state read again from its structure. Nothing is added and nothing
/// moved. A folder that cannot be settled is said, not a reason to stop.
pub(crate) fn refresh(store: &mut Store, only: Option<&str>) -> Result<Vec<Found>, Refused> {
    let failed = |e: nils_registry::store::Error| Refused {
        status: 500,
        message: e.to_string(),
        layout: None,
    };
    let places: Vec<Place> = place::active(store)
        .map_err(failed)?
        .into_iter()
        .filter(|p| p.role == Role::Source && p.dataset["kind"] != "root")
        .filter(|p| match only {
            Some(o) => {
                o == p.name
                    || o.parse::<i64>().ok() == Some(p.id)
                    || p.dataset["root"].as_str() == Some(o)
            }
            None => true,
        })
        .collect();
    let mut out = Vec::new();
    for p in places {
        let path = PathBuf::from(&p.path);
        match declare(store, &path, &json!({}), Some(&p)) {
            Ok(d) => {
                place::set(store, p.id, None, None, Some(&d.probed)).map_err(failed)?;
                let mut dataset = d.dataset;
                dataset["root"] = p.dataset["root"].clone();
                let p = place::set_dataset(store, p.id, &dataset).map_err(failed)?;
                out.push(Found {
                    place: p,
                    layout: d.layout,
                    new: false,
                });
            }
            Err(r) => out.push(Found {
                layout: json!({"error": r.message}),
                place: p,
                new: false,
            }),
        }
    }
    Ok(out)
}

/// A place's name made of a folder's: letters, digits, `-` and `_` as they
/// are, anything else `-`.
fn dataset_name(folder: &Path) -> String {
    let raw = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches('-').to_string();
    if name.is_empty() {
        "dataset".to_string()
    } else {
        name
    }
}

/// A source place's dataset, and those under it when it is a root, as the
/// answer of a door or a command reads them.
pub(crate) fn found_doc(found: &[Found]) -> Value {
    Value::from(
        found
            .iter()
            .map(|f| {
                let mut doc = if f.place.id == 0 {
                    json!({"name": f.place.name, "path": f.place.path})
                } else {
                    f.place.as_json()
                };
                doc["layout"] = f.layout.clone();
                doc["new"] = json!(f.new);
                doc
            })
            .collect::<Vec<_>>(),
    )
}

/// What a person reads about a dataset's folder: what its structure says,
/// what is read and what is not, in words (Wave 7a). The place's id is the
/// one `nils place set` names.
pub(crate) fn layout_lines(id: i64, dataset: &Value, layout: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if dataset["kind"] == "root" {
        out.push(format!(
            "a root: {} folder(s) under it, none read until it is added as a dataset (nils place folders {id} lists them, nils place add-dataset {id} FOLDER adds one); {} file(s) at its top are not read",
            layout["folders"], layout["loose"]
        ));
        return out;
    }
    if dataset["kind"] == "legacy" {
        out.push(
            "legacy: the place names a pseudonymised tree itself, read as an anonymised dataset"
                .into(),
        );
        return out;
    }
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
            "{} entries moved into {}",
            m["entries"],
            m["into"].as_str().unwrap_or("")
        ));
    }
    match layout["state"].as_str().unwrap_or("unknown") {
        "unknown" => {
            let n = layout["loose_dicom"].as_array().map_or(0, Vec::len);
            out.push(if n > 0 {
                format!(
                    "unknown: {n} entr{} with DICOM beside derivatives/; nothing in it is read",
                    if n == 1 { "y" } else { "ies" }
                )
            } else {
                "unknown: no tree under derivatives/; nothing in it is read".to_string()
            });
            if n > 0 {
                out.push(format!(
                    "say which tree they go into: nils place set {id} --move-into originals --confirm-move (identified data) or --move-into anon --confirm-move (already anonymised)"
                ));
            }
        }
        state => {
            out.push(format!(
                "{state}: reads {ANON_TREE} only{}",
                if matches!(state, "identified" | "both") {
                    ", which the pseudonymiser writes from the originals"
                } else {
                    ""
                }
            ));
            let loose = layout["loose"].as_u64().unwrap_or(0);
            if loose > 0 {
                out.push(format!(
                    "{loose} other entr{} beside derivatives/, not read",
                    if loose == 1 { "y" } else { "ies" }
                ));
            }
            if let Some(why) = place::incomplete(dataset) {
                out.push(format!("not read yet: {why}"));
            }
        }
    }
    out
}

/// Why a digest of a path is refused (Wave 7a §5.3), beside the originals'
/// own refusal: the path lies in a source place whose dataset is
/// undeclared, or holds one, or lies in a declared dataset's folder outside
/// the tree its digest reads. None where the path is no dataset's, or is
/// inside the tree a dataset's digest reads.
pub(crate) fn not_read(store: &mut Store, path: &Path) -> Option<String> {
    let theirs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // a dataset's originals, whoever's, are the pseudonymiser's alone
    let parts: Vec<_> = theirs
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();
    if parts
        .windows(2)
        .any(|w| w[0] == "derivatives" && w[1] == "dcm-original")
    {
        return Some(format!(
            "{} is in a dataset's originals (derivatives/dcm-original), which the pseudonymiser alone reads",
            path.display()
        ));
    }
    let places = place::active(store).ok()?;
    let sources: Vec<(PathBuf, &Place)> = places
        .iter()
        .filter(|p| p.role == Role::Source)
        .map(|p| {
            (
                std::fs::canonicalize(&p.path).unwrap_or_else(|_| PathBuf::from(&p.path)),
                p,
            )
        })
        .collect();
    // a path that holds a place's folder reads it whole: refused, whatever
    // the place is; each dataset is read by its own name
    if let Some((_, p)) = sources
        .iter()
        .find(|(mine, _)| mine != &theirs && mine.starts_with(&theirs))
    {
        return Some(format!(
            "{} holds the {} {}: each dataset is read by its own name, as @name",
            path.display(),
            if p.dataset["kind"] == "root" {
                "root"
            } else {
                "dataset"
            },
            p.name
        ));
    }
    // the place the path is in: the deepest, so a dataset under a root is
    // the dataset, not the root
    let (mine, p) = sources
        .iter()
        .filter(|(mine, _)| theirs.starts_with(mine))
        .max_by_key(|(mine, _)| mine.components().count())?;
    if place::is_undeclared(&p.dataset) && p.dataset["kind"] != "root" {
        return Some(format!(
            "{} is in the dataset {}, which is undeclared: its structure is unknown, and nothing in it is read until its entries are moved into derivatives/dcm-original or derivatives/dcm-anon, with nils place set {} --move-into originals|anon --confirm-move or the desk's Add a dataset",
            path.display(),
            p.name,
            p.id
        ));
    }
    // Wave 7a (Nima, 2026-10-08): never read without resolved ids, so never
    // on a declaration that does not say how they resolve
    if let Some(why) = place::incomplete(&p.dataset) {
        return Some(format!(
            "{} is in the {} {}, which is not read: {why}",
            path.display(),
            if p.dataset["kind"] == "root" {
                "root"
            } else {
                "dataset"
            },
            p.name
        ));
    }
    let anon = p.tree_path("anon")?;
    let anon = std::fs::canonicalize(&anon).unwrap_or(anon);
    if !theirs.starts_with(&anon) {
        let _ = mine;
        return Some(format!(
            "{} is in the folder of the dataset {}, whose digest reads its pseudonymised tree alone: digest @{}",
            path.display(),
            p.name,
            p.name
        ));
    }
    None
}

/// Why a dataset may not be brought in, digested or pseudonymised: it is
/// undeclared (Wave 7a §5.3), or its declaration is not whole (Wave 7a,
/// Nima 2026-10-08: a de-identified or coded dataset says what PatientID
/// holds and how its subjects are found).
pub(crate) fn undeclared_refusal(p: &Place) -> Option<String> {
    if place::is_undeclared(&p.dataset) {
        return Some(format!(
            "the dataset {} is undeclared: nothing in it is read until how its files arrive is declared, with nils place set {} --arrives identified|deidentified|coded or the desk's Add a dataset",
            p.name, p.id
        ));
    }
    place::incomplete(&p.dataset).map(|why| {
        format!(
            "the dataset {} is not read yet: {why}. Declare it whole with nils place set {} or the desk's Add a dataset",
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
///
/// Wave 7a (Nima, 2026-10-08: "we always have to have resolved IDs"): an
/// identified dataset's tree holds only what the pseudonymiser resolved, so
/// a value no subject holds there is held, never made a subject. A dataset
/// read in place resolves its ids as it declares: through a map (`map`),
/// holding what no map names, or by the subject code generator from each
/// id (`generated`), a subject and never a provisional one.
pub(crate) fn unmapped_of(store: &mut Store, path: &Path) -> nils_digest::Unmapped {
    let Ok(Some(p)) = place::tree_holding(store, "anon", path) else {
        return nils_digest::Unmapped::Subject;
    };
    if p.dataset["arrives"].as_str() == Some("identified") {
        return nils_digest::Unmapped::Hold;
    }
    match p.dataset["subjects"].as_str() {
        Some("generated") => nils_digest::Unmapped::Subject,
        _ => nils_digest::Unmapped::Hold,
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

    /// A file DICOM software reads as one: the preamble, then `DICM`.
    fn dicom_bytes() -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b.extend_from_slice(b"DICM");
        b.extend_from_slice(&[0u8; 16]);
        b
    }

    /// Wave 7a, the structure says: originals alone are identified, a
    /// pseudonymised tree alone (or dcm-raw) anonymised, both both, and
    /// anything else unknown: entries holding DICOM beside derivatives/, or
    /// no tree. Entries holding no DICOM beside the trees change nothing.
    #[test]
    fn the_structure_says_what_a_dataset_is() {
        let cases: [(&str, &[&str], State); 7] = [
            (
                "originals",
                &["derivatives/dcm-original/p1/IM_1"],
                State::Identified,
            ),
            ("anon", &["derivatives/dcm-anon/s1/IM_1"], State::Anonymised),
            ("raw", &["derivatives/dcm-raw/s1/IM_1"], State::Anonymised),
            (
                "both",
                &[
                    "derivatives/dcm-original/p1/IM_1",
                    "derivatives/dcm-raw/s1/IM_1",
                ],
                State::Both,
            ),
            (
                "beside",
                &["derivatives/dcm-anon/s1/IM_1", "p2/IM_2"],
                State::Unknown,
            ),
            ("none", &["p1/IM_1", "p1/IM_2"], State::Unknown),
            ("empty", &[], State::Unknown),
        ];
        for (name, files, state) in cases {
            let dir = TempDir::new(&format!("dataset-state-{name}"));
            for f in files {
                dir.file(f, &dicom_bytes());
            }
            assert_eq!(detect(dir.path()).state(), state, "{name}");
        }
        // notes beside the trees hold no DICOM: the structure stands
        let dir = TempDir::new("dataset-state-notes");
        dir.file("derivatives/dcm-original/p1/IM_1", &dicom_bytes());
        dir.file("notes.txt", b"n");
        dir.file("docs/readme.md", b"r");
        let found = detect(dir.path());
        assert_eq!(found.loose, ["docs", "notes.txt"]);
        assert!(found.loose_dicom.is_empty());
        assert_eq!(found.state(), State::Identified);
        // a .dcm name counts without its mark
        let dir = TempDir::new("dataset-state-named");
        dir.file("derivatives/dcm-anon/s1/IM_1", &dicom_bytes());
        dir.file("x/a.DCM", b"no mark");
        assert_eq!(detect(dir.path()).state(), State::Unknown);
    }

    /// Wave 7a: a folder holding derivatives/ is a dataset, one naming a
    /// dataset's pseudonymised tree is legacy, anything else is a root.
    #[test]
    fn a_folder_is_a_root_a_dataset_or_a_legacy_tree() {
        let dir = TempDir::new("dataset-shape");
        dir.file("study/derivatives/dcm-raw/s1/IM_1", &dicom_bytes());
        dir.file("loose/IM_1", &dicom_bytes());
        assert_eq!(shape_of(dir.path()), Shape::Root);
        assert_eq!(shape_of(&dir.path().join("study")), Shape::Dataset);
        assert_eq!(shape_of(&dir.path().join("loose")), Shape::Root);
        assert_eq!(
            shape_of(&dir.path().join("study/derivatives/dcm-raw")),
            Shape::Legacy
        );
    }

    #[test]
    fn an_unknown_dataset_moves_its_entries_only_into_the_tree_named_and_only_when_confirmed() {
        let dir = TempDir::new("dataset-identified");
        dir.file("sub-1/ses-1/a.dcm", &dicom_bytes());
        dir.file("sub-2/b", &dicom_bytes());
        dir.file("notes.txt", b"n");
        dir.file(".hidden", b"h");
        let found = detect(dir.path());
        assert_eq!(found.loose, ["notes.txt", "sub-1", "sub-2"]);
        assert_eq!(found.loose_dicom, ["sub-1", "sub-2"]);
        assert!(!found.v0());
        // left alone: unknown, no tree, nothing written
        let (state, trees, layout) = settle(dir.path(), None, false).unwrap();
        assert_eq!(state, State::Unknown);
        assert_eq!(trees, json!({"originals": null, "anon": null}));
        let doc = layout_doc(dir.path(), &layout);
        assert_eq!(doc["question"], true);
        assert_eq!(doc["loose_dicom"], json!(["sub-1", "sub-2"]));
        assert_eq!(doc["move_into"]["choices"], json!(["originals", "anon"]));
        assert_eq!(doc["move_into"]["entries"], 2);
        assert_eq!(
            names(dir.path()),
            [".hidden", "notes.txt", "sub-1", "sub-2"]
        );
        // named but not confirmed: the question, naming the entries and the
        // tree, and nothing moved or made
        let asked = settle(dir.path(), Some("originals"), false).unwrap_err();
        assert_eq!(asked.status, 409);
        assert!(asked.message.contains("2 entries"), "{}", asked.message);
        assert!(asked.message.contains(ORIGINALS_TREE), "{}", asked.message);
        assert_eq!(
            asked.layout.unwrap()["loose_dicom"],
            json!(["sub-1", "sub-2"])
        );
        assert_eq!(
            names(dir.path()),
            [".hidden", "notes.txt", "sub-1", "sub-2"]
        );
        let bad = settle(dir.path(), Some("elsewhere"), true).unwrap_err();
        assert_eq!(bad.status, 400);
        // confirmed: moved, and the structure says identified
        let (state, trees, layout) = settle(dir.path(), Some("originals"), true).unwrap();
        assert_eq!(state, State::Identified);
        assert_eq!(
            trees,
            json!({"originals": ORIGINALS_TREE, "anon": ANON_TREE})
        );
        assert_eq!(layout.moved, Some((ORIGINALS_TREE, 2)));
        assert_eq!(names(dir.path()), [".hidden", "derivatives", "notes.txt"]);
        assert!(
            dir.path()
                .join(ORIGINALS_TREE)
                .join("sub-1/ses-1/a.dcm")
                .is_file()
        );
        assert!(dir.path().join(ANON_TREE).is_dir());
        // settled again: nothing to move, the trees stand
        let (again, trees_again, _) = settle(dir.path(), None, false).unwrap();
        assert_eq!(again, State::Identified);
        assert_eq!(trees_again, trees);
        // into the pseudonymised tree, the structure says anonymised
        let other = TempDir::new("dataset-into-anon");
        other.file("s1/IM_1", &dicom_bytes());
        let (state, trees, _) = settle(other.path(), Some("anon"), true).unwrap();
        assert_eq!(state, State::Anonymised);
        assert_eq!(trees, json!({"originals": null, "anon": ANON_TREE}));
        assert!(other.path().join(ANON_TREE).join("s1/IM_1").is_file());
        // an entry the tree already holds is not moved over it, nor any other
        let clash = TempDir::new("dataset-clash");
        clash.file("s1/IM_1", &dicom_bytes());
        clash.file("s2/IM_1", &dicom_bytes());
        clash.file("derivatives/dcm-anon/s2/IM_1", &dicom_bytes());
        let why = settle(clash.path(), Some("anon"), true).unwrap_err();
        assert_eq!(why.status, 409);
        assert!(why.message.contains("already holds s2"), "{}", why.message);
        assert!(clash.path().join("s1/IM_1").is_file());
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
        assert_eq!(found.state(), State::Both);
        let (state, trees, layout) = settle(dir.path(), None, false).unwrap();
        assert_eq!(state, State::Both);
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
        assert_eq!(doc["renamed"], true);
        // both trees there is a folder to sort out by hand, not by a rename
        std::fs::create_dir_all(dir.path().join(RAW_TREE)).unwrap();
        let why = settle(dir.path(), None, false).unwrap_err();
        assert_eq!(why.status, 409);
        assert!(why.message.contains("keep one"), "{}", why.message);
    }

    /// Wave 7a (Nima, 2026-10-08: "the only assumption is when we add data
    /// and expect the structure"): adding a root adds the root alone; its
    /// folders are listed as they are, with no DICOM where there is none;
    /// a folder becomes a dataset only when added, and only then is its
    /// structure read, every kind as it is; a refresh reads each again and
    /// adds nothing.
    #[test]
    fn a_root_s_folders_become_datasets_only_when_added() {
        let dir = TempDir::new("dataset-root");
        let mut store = Store::sqlite_in_memory().unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        for (folder, files) in [
            ("ida", vec!["derivatives/dcm-original/p1/IM_1"]),
            ("anona", vec!["derivatives/dcm-anon/s1/IM_1"]),
            ("rawb", vec!["derivatives/dcm-raw/s1/IM_1"]),
            (
                "both",
                vec![
                    "derivatives/dcm-original/p1/IM_1",
                    "derivatives/dcm-raw/s1/IM_1",
                ],
            ),
            ("loose", vec!["p1/IM_1"]),
            ("mixed", vec!["derivatives/dcm-anon/s1/IM_1", "p9/IM_9"]),
            ("src", vec!["derivatives/dcm-anon/s1/IM_1"]),
        ] {
            for f in files {
                dir.file(&format!("{folder}/{f}"), &dicom_bytes());
            }
        }
        dir.file("papers/notes.txt", b"no dicom here");
        dir.file("readme.txt", b"r");
        // the root alone
        let (d, found) =
            shape_place(&mut store, "src", dir.path(), &json!({}), None, &json!({})).unwrap();
        assert!(found.is_empty());
        assert_eq!(d.dataset["kind"], "root");
        assert_eq!(d.layout["folders"], 8);
        let id = place::add(
            &mut store,
            &place::New {
                name: "src",
                role: Role::Source,
                path: &dir.path().display().to_string(),
                guarantees: json!({}),
                probed: d.probed,
                handling: Value::Null,
                dataset: d.dataset,
            },
        )
        .unwrap();
        assert_eq!(place::list(&mut store).unwrap().len(), 1);
        let root = place::show(&mut store, id).unwrap().unwrap();
        // the folders as they are, each looked at alone; nothing written,
        // nothing assumed
        let (listed, matching, next) = folders(&mut store, &root, None, 50, None).unwrap();
        assert_eq!((matching, next), (8, None));
        let shown: Vec<(String, String, bool, bool)> = listed
            .iter()
            .map(|f| {
                let name = f["name"].as_str().unwrap();
                let look = folder_look(&mut store, &root, name).unwrap();
                assert_eq!(f.get("holds_dicom"), None, "the listing never looks inside");
                (
                    name.to_string(),
                    look["holds_dicom"].as_str().unwrap().to_string(),
                    look["has_derivatives"].as_bool().unwrap(),
                    f["added"].as_bool().unwrap(),
                )
            })
            .collect();
        let want = [
            ("anona", "yes", true),
            ("both", "yes", true),
            ("ida", "yes", true),
            ("loose", "yes", false),
            ("mixed", "yes", true),
            ("papers", "no", false),
            ("rawb", "yes", true),
            ("src", "yes", true),
        ];
        assert_eq!(
            shown,
            want.iter()
                .map(|(n, d, h)| (n.to_string(), d.to_string(), *h, false))
                .collect::<Vec<_>>()
        );
        assert!(
            dir.path().join("rawb/derivatives/dcm-raw").is_dir(),
            "nothing renamed"
        );
        assert_eq!(place::list(&mut store).unwrap().len(), 1);
        // each added: its structure read, every kind as it is
        let mut states = Vec::new();
        for folder in ["anona", "both", "ida", "loose", "mixed", "rawb", "src"] {
            let f = add_dataset(&mut store, &root, folder, None, &json!({})).unwrap();
            assert_eq!(f.place.dataset["root"], "src");
            states.push((
                f.place.name.clone(),
                f.place.dataset["state"].as_str().unwrap().to_string(),
            ));
        }
        let want = [
            ("anona", "anonymised"),
            ("both", "both"),
            ("ida", "identified"),
            ("loose", "unknown"),
            ("mixed", "unknown"),
            ("rawb", "anonymised"),
            ("src-src", "anonymised"),
        ];
        assert_eq!(
            states,
            want.iter()
                .map(|(n, s)| (n.to_string(), s.to_string()))
                .collect::<Vec<_>>()
        );
        // nothing of an unknown dataset moved; dcm-raw renamed once added
        assert!(dir.path().join("loose/p1/IM_1").is_file());
        assert!(
            dir.path()
                .join("rawb/derivatives/dcm-anon/s1/IM_1")
                .is_file()
        );
        // added twice, by path, outside the root, or the root: refused
        let twice = add_dataset(&mut store, &root, "ida", None, &json!({}));
        assert!(matches!(twice, Err(Refused { status: 409, .. })));
        let by_path = dir.path().join("papers").display().to_string();
        let f = add_dataset(&mut store, &root, &by_path, Some("notes"), &json!({})).unwrap();
        assert_eq!(f.place.name, "notes");
        assert_eq!(f.place.dataset["state"], "unknown");
        assert!(add_dataset(&mut store, &root, "../elsewhere", None, &json!({})).is_err());
        assert!(add_dataset(&mut store, &root, "/tmp", None, &json!({})).is_err());
        // the folders say which are datasets now
        let (listed, _, _) = folders(&mut store, &root, None, 50, None).unwrap();
        assert!(listed.iter().all(|f| f["added"] == true), "{listed:?}");
        // the look at an added folder names its dataset and its structure
        let look = folder_look(&mut store, &root, "rawb").unwrap();
        assert_eq!(look["dataset"], "rawb");
        assert_eq!(look["layout"]["state"], "anonymised");
        assert_eq!(
            folder_look(&mut store, &root, "nowhere")
                .unwrap_err()
                .status,
            404
        );
        assert_eq!(
            folder_look(&mut store, &root, "../x").unwrap_err().status,
            400
        );
        // a refresh reads each again and adds nothing
        let before = place::list(&mut store).unwrap().len();
        let again = refresh(&mut store, None).unwrap();
        assert_eq!(again.len(), 8);
        assert!(again.iter().all(|f| !f.new));
        assert_eq!(place::list(&mut store).unwrap().len(), before);
    }

    /// Wave 7a (Nima, 2026-10-08: "what if the root folder has 1000
    /// folders"): a root of a thousand folders, each holding many files,
    /// lists a page in one read of the root's listing, never looking into a
    /// folder; it is searched by name in any case and paged by name.
    #[test]
    fn a_root_of_a_thousand_folders_lists_a_page_found_by_name() {
        let dir = TempDir::new("dataset-thousand");
        let mut store = Store::sqlite_in_memory().unwrap();
        nils_registry::migrate::migrate(&mut store, nils_registry::migrate::Kind::Registry)
            .unwrap();
        for i in 0..1000 {
            std::fs::create_dir_all(dir.path().join(format!("Study-{i:04}"))).unwrap();
        }
        // what the listing must never read: a folder heavy with files
        for i in 0..3000 {
            dir.file(&format!("Study-0500/IM_{i:05}"), &dicom_bytes());
        }
        dir.file("readme.txt", b"r");
        let id = place::add(
            &mut store,
            &place::New {
                name: "src",
                role: Role::Source,
                path: &dir.path().display().to_string(),
                guarantees: json!({}),
                probed: Value::Null,
                handling: Value::Null,
                dataset: json!({"kind": "root"}),
            },
        )
        .unwrap();
        let root = place::show(&mut store, id).unwrap().unwrap();
        let started = Instant::now();
        let (page, matching, next) = folders(&mut store, &root, None, 50, None).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(matching, 1000);
        assert_eq!(page.len(), 50);
        assert_eq!(page[0]["name"], "Study-0000");
        assert_eq!(next.as_deref(), Some("Study-0049"));
        // the next page, from the name the first ended on
        let (page, _, next) = folders(&mut store, &root, None, 50, next.as_deref()).unwrap();
        assert_eq!(page[0]["name"], "Study-0050");
        assert_eq!(next.as_deref(), Some("Study-0099"));
        // found by a part of the name, in any case; the last page says so
        let (page, matching, next) =
            folders(&mut store, &root, Some("study-05"), 200, None).unwrap();
        assert_eq!(matching, 100);
        assert_eq!(page.len(), 100);
        assert_eq!(next, None);
        let (page, matching, _) = folders(&mut store, &root, Some("0999"), 50, None).unwrap();
        assert_eq!((page.len(), matching), (1, 1));
        let (page, _, next) = folders(&mut store, &root, Some("nothing"), 50, None).unwrap();
        assert!(page.is_empty() && next.is_none());
        // a limit past the most is the most
        let (page, _, _) = folders(&mut store, &root, None, 5000, None).unwrap();
        assert_eq!(page.len(), FOLDERS_MOST);
        // the one heavy folder, looked at alone
        let look = folder_look(&mut store, &root, "Study-0500").unwrap();
        assert_eq!(look["holds_dicom"], "yes");
        assert_eq!(look["layout"]["state"], "unknown");
        assert_eq!(look["added"], false);
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
