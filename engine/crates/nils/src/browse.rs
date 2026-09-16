// SPDX-License-Identifier: AGPL-3.0-only

//! The folders of the ingest roots, for the desk's picker: the folders inside
//! one of them, a page at a time (`POST /api/ingest/folders`), and what a few
//! of those folders hold (`POST /api/ingest/look`). A folder is named the way
//! a queued digest names it, `@root/relative`, and nothing outside the ingest
//! roots is listed: a relative part with a parent step or a leading slash is
//! refused, and so is one whose real path, links followed, leaves its root.
//!
//! A listing takes each entry's type from the directory itself, never a stat
//! per entry, and leaves links out as the digest does. The sorted names of a
//! directory are kept for a minute while the directory does not change, so
//! paging a folder that holds a hundred thousand folders reads it once, and a
//! call that asks while another reads it waits for that read rather than
//! starting its own. A look reads a few directories of each folder breadth
//! first and sniffs a few files, within a budget shared fairly between the
//! folders. Both read the disk in a thread of their own, since a network
//! mount that does not answer would otherwise hold the handler behind it;
//! past the wait the answer says so, with what was known by then.
//!
//! A root is read as `@name` resolves it (record 26): the pseudonymised
//! tree of the dataset declared on it, with `@name/originals` its
//! originals; a root no dataset is declared on is the folder it was given.

use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use nils_registry::place::{self, Place, Role};
use nils_registry::store::Store;
use serde_json::{Map, Value, json};

use crate::dataset::{self, Root};
use crate::folders::within;
use crate::places::may_enter;
use crate::serve::Reply;
use crate::supervise::Seen;

/// How long a listing reads before the call is answered with what is known.
const WAIT: Duration = Duration::from_secs(5);
/// The files a listing counts before it says there are more.
const COUNTED: usize = 10_000;
/// The entries a listing reads before it says it is partial.
const READ: usize = 500_000;
/// The names a read hands over at a time, for an answer past the wait.
const BATCH: usize = 4_096;
/// The folders of a page, when the call names no limit, and at most.
const PAGE: usize = 200;
const PAGE_MAX: usize = 1_000;
/// How long a read is kept, how many are kept, and the names they may hold
/// together before the oldest go.
const KEPT_FOR: Duration = Duration::from_secs(60);
const KEPT: usize = 8;
const KEPT_NAMES: usize = 1_000_000;
/// A look: the folders it takes when it is named none, the files it sniffs
/// of a folder, the files it takes from one directory, the entries it reads
/// of one directory and of one folder in all.
const LOOK_FOLDERS: usize = 64;
const SAMPLE: usize = 16;
const PER_DIRECTORY: usize = 4;
const DIRECTORY_ENTRIES: usize = 2_000;
const FOLDER_ENTRIES: usize = 20_000;
/// A look's budget in milliseconds, when the call names none, and at most.
const BUDGET: u64 = 8_000;
const BUDGET_MAX: u64 = 20_000;
/// The least time a folder of a look is given.
const SLICE_MIN: Duration = Duration::from_millis(250);
/// What the handler waits past a look's budget before it answers without it.
const MARGIN: Duration = Duration::from_secs(2);

const NAMED: &str = "at: a folder of an ingest location, as @root/relative";
/// What a look takes, which is either of those (record 26): the desk looks
/// at a folder before a dataset is declared on it, and such a folder is
/// under no location yet.
const GIVEN: &str = "at: a folder of an ingest location, as @root/relative, or path: the absolute path of a folder on this host";
const OUTSIDE: &str = "at: the path steps outside its location";

/// A folder of an ingest root, named as `@root/relative`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct At {
    pub(crate) root: String,
    /// The relative part made plain: no empty step and no `.`, and empty for
    /// the root itself.
    pub(crate) rel: String,
}

impl At {
    pub(crate) fn parse(text: &str) -> Result<At, &'static str> {
        let rest = text.trim().strip_prefix('@').ok_or(NAMED)?;
        let (root, rel) = rest.split_once('/').unwrap_or((rest, ""));
        if root.is_empty() {
            return Err(NAMED);
        }
        if rel.starts_with('/') || rel.split('/').any(|s| s == "..") {
            return Err(OUTSIDE);
        }
        let rel = rel
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .collect::<Vec<_>>()
            .join("/");
        Ok(At {
            root: root.to_string(),
            rel,
        })
    }

    pub(crate) fn text(&self) -> String {
        if self.rel.is_empty() {
            format!("@{}", self.root)
        } else {
            format!("@{}/{}", self.root, self.rel)
        }
    }

    fn parent(&self) -> Option<At> {
        if self.rel.is_empty() {
            return None;
        }
        let rel = self.rel.rsplit_once('/').map_or("", |(up, _)| up);
        Some(At {
            root: self.root.clone(),
            rel: rel.to_string(),
        })
    }

    /// The folder under its root, as the root resolves it.
    fn under(&self, root: &Root) -> PathBuf {
        root.resolve(&self.rel)
    }
}

/// A folder's own name: one step, never the folder above or the folder itself.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
}

fn root_of(roots: &BTreeMap<String, Root>, at: &At) -> Result<Root, Reply> {
    roots.get(&at.root).cloned().ok_or_else(|| {
        Reply::error(
            400,
            format!(
                "at: @{} is not a registered ingest location; this deployment names {}",
                at.root,
                if roots.is_empty() {
                    "none".to_string()
                } else {
                    roots.keys().cloned().collect::<Vec<_>>().join(", ")
                }
            ),
        )
    })
}

/// Where a folder is on disk.
enum Spot {
    /// Nothing is there.
    Missing,
    /// A folder on the way refused this account.
    Refused,
    /// Something that is not a directory.
    File,
    /// A directory, by its real path.
    Folder(PathBuf),
}

/// The folder an `@root/relative` names: its path under the root's tree as
/// the root resolves it, and where it is with links followed, which must be
/// inside that tree's real path.
fn resolve(root: &Root, at: &At) -> Result<(PathBuf, Spot), Reply> {
    let path = at.under(root);
    let (base, _) = root.base(&at.rel);
    let spot = match (std::fs::canonicalize(base), std::fs::canonicalize(&path)) {
        (Ok(base), Ok(real)) => {
            if !real.starts_with(&base) {
                return Err(Reply::error(400, OUTSIDE));
            }
            if real.is_dir() {
                Spot::Folder(real)
            } else {
                Spot::File
            }
        }
        (_, Err(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => Spot::Refused,
        _ => Spot::Missing,
    };
    Ok((path, spot))
}

/// A place in force with its real path, measured once a call.
struct Held {
    name: String,
    role: Role,
    real: PathBuf,
}

fn held(places: &[Place]) -> Vec<Held> {
    places
        .iter()
        .map(|p| Held {
            name: p.name.clone(),
            role: p.role,
            real: std::fs::canonicalize(&p.path).unwrap_or_else(|_| PathBuf::from(&p.path)),
        })
        .collect()
}

/// The place that holds a real path, as `{name, role}`: a source place first,
/// since a digest reads only under one, then the nearest.
fn holder(held: &[Held], real: &Path) -> Value {
    held.iter()
        .filter(|h| real.starts_with(&h.real))
        .max_by_key(|h| (h.role == Role::Source, h.real.components().count()))
        .map_or(
            Value::Null,
            |h| json!({"name": h.name, "role": h.role.name()}),
        )
}

/// The ingest roots, each as `@name` resolves it and with the place that
/// holds it, sources first.
fn roots_doc(roots: &BTreeMap<String, Root>, places: Vec<Place>) -> Value {
    let given: Vec<Root> = roots.values().cloned().collect();
    let row = |root: &Root, place: Value| {
        let mut r = root.as_json();
        r["place"] = place;
        r
    };
    // what is known without the disk, for an answer past the wait
    let known = json!({
        "at": null,
        "roots": given.iter().map(|r| row(r, Value::Null)).collect::<Vec<_>>(),
        "timed_out": true,
    });
    within(WAIT, move || {
        let held = held(&places);
        let mut rows: Vec<(bool, Value)> = given
            .iter()
            .map(|root| {
                let real = std::fs::canonicalize(&root.anon).unwrap_or_else(|_| root.anon.clone());
                let place = holder(&held, &real);
                (place["role"] == "source", row(root, place))
            })
            .collect();
        // the map gave them by name, and the sort keeps that order within each
        rows.sort_by_key(|(source, _)| !*source);
        json!({
            "at": null,
            "roots": rows.into_iter().map(|(_, r)| r).collect::<Vec<_>>(),
            "timed_out": false,
        })
    })
    .unwrap_or(known)
}

/// One read of a directory's folders, shared by the calls that ask while it
/// reads, and kept a minute after it ends.
#[derive(Default)]
struct Read {
    found: Mutex<Found>,
    ended: Condvar,
}

/// What a read found: complete, with its names sorted, once it has ended.
#[derive(Default)]
struct Found {
    names: Vec<String>,
    files: usize,
    /// There were more files than were counted.
    more: bool,
    /// The read stopped before the end of the directory.
    partial: bool,
    /// The directory could not be read.
    refused: bool,
    /// The directory's modification time as the read began.
    mtime: Option<SystemTime>,
    ended: Option<Instant>,
}

type Kept = Mutex<VecDeque<(PathBuf, Arc<Read>)>>;
static KEPT_READS: OnceLock<Kept> = OnceLock::new();

/// The read to answer from: one still going, or one that ended within the
/// minute while the directory kept its modification time; else a new read,
/// which the caller makes, and the second value says so.
fn read_of(real: &Path, mtime: Option<SystemTime>) -> (Arc<Read>, bool) {
    let fresh = Arc::new(Read::default());
    let Ok(mut kept) = KEPT_READS.get_or_init(Kept::default).lock() else {
        return (fresh, true);
    };
    if let Some(i) = kept.iter().position(|(p, _)| p == real)
        && let Some((path, read)) = kept.remove(i)
    {
        let usable = read.found.lock().is_ok_and(|f| match f.ended {
            None => true,
            Some(at) => {
                at.elapsed() < KEPT_FOR && !f.refused && mtime.is_some() && f.mtime == mtime
            }
        });
        if usable {
            kept.push_back((path, Arc::clone(&read)));
            return (read, false);
        }
    }
    kept.push_back((real.to_path_buf(), Arc::clone(&fresh)));
    // a read another call holds is counted as empty rather than waited for
    let names = |r: &Arc<Read>| r.found.try_lock().map_or(0, |f| f.names.len());
    let mut holding: usize = kept.iter().map(|(_, r)| names(r)).sum();
    while kept.len() > KEPT || (holding > KEPT_NAMES && kept.len() > 1) {
        match kept.pop_front() {
            Some((_, gone)) => holding = holding.saturating_sub(names(&gone)),
            None => break,
        }
    }
    (fresh, true)
}

/// Read a directory's folders: each entry's type from the directory itself,
/// links left out as the digest leaves them out, files counted to a limit,
/// and the names sorted once the read has ended.
fn fill(dir: &Path, read: &Read, mtime: Option<SystemTime>) {
    let mut batch: Vec<String> = Vec::new();
    let (mut files, mut more, mut partial, mut refused) = (0usize, false, false, false);
    match std::fs::read_dir(dir) {
        Err(_) => refused = true,
        Ok(entries) => {
            for (n, entry) in entries.enumerate() {
                if n >= READ {
                    partial = true;
                    break;
                }
                let Ok(entry) = entry else { continue };
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    // a name that is not UTF-8 cannot be named as @root/relative
                    if let Ok(name) = entry.file_name().into_string() {
                        batch.push(name);
                    }
                } else if kind.is_file() {
                    if files < COUNTED {
                        files += 1;
                    } else {
                        more = true;
                    }
                }
                if batch.len() >= BATCH
                    && let Ok(mut found) = read.found.lock()
                {
                    found.names.append(&mut batch);
                    found.files = files;
                }
            }
        }
    }
    // sorted outside the lock, so a call answering past the wait is not held
    let mut names = read
        .found
        .lock()
        .map(|mut f| std::mem::take(&mut f.names))
        .unwrap_or_default();
    names.append(&mut batch);
    names.sort_unstable_by(|a, b| order(a, b));
    if let Ok(mut found) = read.found.lock() {
        *found = Found {
            names,
            files,
            more,
            partial,
            refused,
            mtime,
            ended: Some(Instant::now()),
        };
    }
    read.ended.notify_all();
}

/// Wait for a read another call makes, at most `wait`; true once it ended.
fn wait_for(read: &Read, wait: Duration) -> bool {
    let Ok(found) = read.found.lock() else {
        return false;
    };
    read.ended
        .wait_timeout_while(found, wait, |f| f.ended.is_none())
        .is_ok_and(|(f, _)| f.ended.is_some())
}

/// The order folders are listed in: by name without regard to case, then by
/// the bytes, so no two names are equal.
fn order(a: &str, b: &str) -> Ordering {
    let fold = |s: &str| s.chars().flat_map(char::to_lowercase).collect::<Vec<_>>();
    if a.is_ascii() && b.is_ascii() {
        let (x, y) = (a.as_bytes(), b.as_bytes());
        return x
            .iter()
            .map(u8::to_ascii_lowercase)
            .cmp(y.iter().map(u8::to_ascii_lowercase))
            .then_with(|| a.cmp(b));
    }
    fold(a).cmp(&fold(b)).then_with(|| a.cmp(b))
}

/// Whether a name holds the filter, without regard to case.
fn holds(name: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    if name.is_ascii() && filter.is_ascii() {
        return name
            .as_bytes()
            .windows(filter.len())
            .any(|w| w.eq_ignore_ascii_case(filter.as_bytes()));
    }
    name.to_lowercase().contains(&filter.to_lowercase())
}

/// A page of sorted names: those that hold the filter and come after the
/// name `after`, at most `limit` of them, and the name to ask after next
/// when more remain.
fn page<'a>(
    names: &'a [String],
    filter: &str,
    after: Option<&str>,
    limit: usize,
) -> (Vec<&'a str>, Option<&'a str>) {
    let start = after.map_or(0, |a| {
        names.partition_point(|n| order(n, a) != Ordering::Greater)
    });
    let mut rest = names[start..]
        .iter()
        .map(String::as_str)
        .filter(|n| holds(n, filter));
    let shown: Vec<&str> = rest.by_ref().take(limit.max(1)).collect();
    let next = if rest.next().is_some() {
        shown.last().copied()
    } else {
        None
    };
    (shown, next)
}

/// What a listing is asked.
struct Asked {
    at: At,
    filter: String,
    after: Option<String>,
    limit: usize,
}

/// What a listing knows as the disk answers, for an answer past the wait.
#[derive(Default)]
struct Known {
    doc: Option<Value>,
    held: Arc<Vec<Held>>,
    real: Option<PathBuf>,
    read: Option<Arc<Read>>,
}

/// The answer about a folder before its folders are read.
fn head(at: &At, path: &Path) -> Value {
    json!({
        "at": at.text(),
        "root": at.root,
        "rel": at.rel,
        "path": path.display().to_string(),
        "parent": at.parent().map(|p| p.text()),
        "exists": true,
        "directory": true,
        "readable": true,
        "place": null,
        "folders": [],
        "next": null,
        "total": null,
        "files": {"count": 0, "more": false},
        "partial": false,
        "timed_out": false,
    })
}

/// The answer about a folder that is not there to read, or none when it is.
fn unread(doc: &mut Value, spot: Spot) -> Option<PathBuf> {
    let (exists, directory) = match spot {
        Spot::Folder(real) => return Some(real),
        Spot::Missing => (false, false),
        Spot::File => (true, false),
        Spot::Refused => (true, true),
    };
    doc["exists"] = json!(exists);
    doc["directory"] = json!(directory);
    doc["readable"] = json!(false);
    None
}

/// A page of a read, taken while the read is held and answered once it is
/// let go, so a slow disk asked for access holds up no other call.
struct Taken {
    shown: Vec<String>,
    next: Option<String>,
    total: Option<usize>,
    files: usize,
    more: bool,
    partial: bool,
    refused: bool,
}

/// The page asked for, from a read's sorted names. Before the read has ended
/// nothing is counted in full.
fn taken(found: &Found, names: &[String], asked: &Asked, ended: bool) -> Taken {
    let filter = asked.filter.trim();
    let (shown, next) = page(names, filter, asked.after.as_deref(), asked.limit);
    Taken {
        shown: shown.into_iter().map(str::to_string).collect(),
        next: next.map(str::to_string),
        total: (ended && !found.partial).then(|| {
            if filter.is_empty() {
                names.len()
            } else {
                names.iter().filter(|n| holds(n, filter)).count()
            }
        }),
        files: found.files,
        more: found.more,
        partial: found.partial || !ended,
        refused: found.refused,
    }
}

/// A listing's answer from a page taken: each folder with whether this
/// account may open it and the place that holds it. Before the read has
/// ended the access is not asked, since the disk has not answered.
fn paged(mut doc: Value, page: Taken, held: &[Held], real: &Path, ended: bool) -> Value {
    let rows: Vec<Value> = page
        .shown
        .iter()
        .map(|name| {
            // a folder listed is not a link, so its real path is its name under ours
            let inside = real.join(name);
            json!({
                "name": name,
                "readable": if ended { json!(may_enter(&inside)) } else { Value::Null },
                "place": holder(held, &inside),
            })
        })
        .collect();
    doc["readable"] = json!(!page.refused);
    doc["folders"] = json!(rows);
    doc["next"] = json!(page.next);
    doc["total"] = json!(page.total);
    doc["files"] = json!({"count": page.files, "more": page.more});
    doc["partial"] = json!(page.partial);
    doc
}

/// The listing itself, which reads the disk.
fn listing(
    root: &Root,
    places: &[Place],
    asked: &Asked,
    known: &Mutex<Known>,
) -> Result<Value, Reply> {
    let (path, spot) = resolve(root, &asked.at)?;
    let mut doc = head(&asked.at, &path);
    let Some(real) = unread(&mut doc, spot) else {
        return Ok(doc);
    };
    let held = Arc::new(held(places));
    doc["place"] = holder(&held, &real);
    let mtime = std::fs::metadata(&real).and_then(|m| m.modified()).ok();
    let (read, mine) = read_of(&real, mtime);
    if let Ok(mut k) = known.lock() {
        *k = Known {
            doc: Some(doc.clone()),
            held: Arc::clone(&held),
            real: Some(real.clone()),
            read: Some(Arc::clone(&read)),
        };
    }
    if mine {
        fill(&real, &read, mtime);
    } else if !wait_for(&read, WAIT) {
        // the call has been answered past the wait by now
        return Ok(doc);
    }
    let page = {
        let found = read
            .found
            .lock()
            .map_err(|_| Reply::error(500, "a listing's read was left unfinished"))?;
        taken(&found, &found.names, asked, true)
    };
    Ok(paged(doc, page, &held, &real, true))
}

/// A listing's answer past the wait: what the call knew by then, and the
/// folders read so far.
fn late(known: &Mutex<Known>, asked: &Asked, root: &Root) -> Value {
    let doc = known.try_lock().ok().and_then(|k| {
        let mut doc = k.doc.clone()?;
        if let (Some(read), Some(real)) = (&k.read, &k.real)
            && let Ok(found) = read.found.try_lock()
        {
            let mut names = found.names.clone();
            names.sort_unstable_by(|a, b| order(a, b));
            let page = taken(&found, &names, asked, false);
            doc = paged(doc, page, &k.held, real, false);
        }
        Some(doc)
    });
    let mut doc = doc.unwrap_or_else(|| {
        let mut doc = head(&asked.at, &asked.at.under(root));
        for key in ["exists", "directory", "readable"] {
            doc[key] = Value::Null;
        }
        doc
    });
    doc["timed_out"] = json!(true);
    doc
}

/// `POST /api/ingest/folders`: the ingest roots, or a page of the folders
/// inside a folder of one of them.
pub(crate) fn folders_door(
    roots: &BTreeMap<String, PathBuf>,
    store: &mut Store,
    doc: &Value,
) -> Result<Reply, Reply> {
    let places = place::active(store)?;
    let roots = dataset::roots(store, roots);
    let at = match &doc["at"] {
        Value::Null => return Ok(Reply::ok(roots_doc(&roots, places))),
        Value::String(text) => At::parse(text).map_err(|m| Reply::error(400, m))?,
        _ => return Err(Reply::error(400, NAMED)),
    };
    let root = root_of(&roots, &at)?;
    let filter = match &doc["filter"] {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => return Err(Reply::error(400, "filter: words a folder's name holds")),
    };
    let after = match &doc["after"] {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        _ => {
            return Err(Reply::error(
                400,
                "after: the name a page ended at, as next gave it",
            ));
        }
    };
    let limit = match &doc["limit"] {
        Value::Null => PAGE,
        v => v
            .as_u64()
            .filter(|n| *n >= 1)
            .map(|n| usize::try_from(n).unwrap_or(PAGE_MAX).min(PAGE_MAX))
            .ok_or_else(|| Reply::error(400, "limit: the folders of a page, 1 to 1,000"))?,
    };
    let asked = Arc::new(Asked {
        at,
        filter,
        after,
        limit,
    });
    let known = Arc::new(Mutex::new(Known::default()));
    let (a, k, r) = (Arc::clone(&asked), Arc::clone(&known), root.clone());
    match within(WAIT, move || listing(&r, &places, &a, &k)) {
        Some(answer) => answer.map(Reply::ok),
        None => Ok(Reply::ok(late(&known, &asked, &root))),
    }
}

/// How a look reads a tree.
#[derive(Debug, Clone, Copy)]
struct Reach {
    /// The entries read of one directory.
    entries: usize,
    /// The files taken from one directory.
    taken: usize,
    /// Whether the directories inside are read too.
    deeper: bool,
}

/// A folder of a look: a few files from each directory, breadth first.
const FOLDER_REACH: Reach = Reach {
    entries: DIRECTORY_ENTRIES,
    taken: PER_DIRECTORY,
    deeper: true,
};
/// The folder looked into itself: its own files only.
const HERE_REACH: Reach = Reach {
    entries: FOLDER_ENTRIES,
    taken: SAMPLE,
    deeper: false,
};

/// The files a look found under a folder, and the few it takes to sniff.
#[derive(Debug, Default)]
struct Sampled {
    files: usize,
    /// Some of the folder was left unread, so there may be more files.
    more: bool,
    picked: Vec<PathBuf>,
}

/// Read a tree breadth first, until `until`: at most `reach.entries` of each
/// directory and FOLDER_ENTRIES in all, taking a few files from each
/// directory, spread across it, until SAMPLE are taken. Links are left out,
/// as the digest leaves them out.
fn sample(top: &Path, reach: Reach, until: Instant) -> Sampled {
    let mut queue = VecDeque::from([top.to_path_buf()]);
    let (mut read, mut files, mut more) = (0usize, 0usize, false);
    let mut picked = Vec::new();
    while let Some(dir) = queue.pop_front() {
        if picked.len() >= SAMPLE || read >= FOLDER_ENTRIES || Instant::now() >= until {
            more = true;
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let cap = reach.entries.min(FOLDER_ENTRIES - read);
        let (mut here, mut inside) = (Vec::new(), Vec::new());
        for (n, entry) in entries.enumerate() {
            if n >= cap {
                more = true;
                break;
            }
            read += 1;
            let Ok(entry) = entry else { continue };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_file() {
                here.push(entry.file_name());
            } else if kind.is_dir() && reach.deeper {
                inside.push(entry.file_name());
            }
        }
        files += here.len();
        here.sort();
        inside.sort();
        let want = reach.taken.min(SAMPLE - picked.len());
        let step = (here.len() / want.max(1)).max(1);
        picked.extend(
            here.iter()
                .step_by(step)
                .take(want)
                .map(|name| dir.join(name)),
        );
        queue.extend(inside.into_iter().map(|name| dir.join(name)));
    }
    Sampled {
        files: files.min(COUNTED),
        more: more || files > COUNTED,
        picked,
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// What a look found under a folder: its files, and those it took sniffed for
/// DICOM until `until`, one at least, so a folder that took its whole slice
/// to read still says what it holds.
fn what(sampled: &Sampled, until: Instant) -> Map<String, Value> {
    let mut seen = Seen::default();
    let mut sniffed = 0usize;
    for file in &sampled.picked {
        if sniffed > 0 && Instant::now() >= until {
            break;
        }
        seen.add(file);
        sniffed += 1;
    }
    object(json!({
        "sampled": sniffed,
        "dicom": seen.dicom,
        "modalities": seen.modalities,
        "scanners": seen.scanners.len(),
        "files": {"count": sampled.files, "more": sampled.more},
    }))
}

/// What a look is asked.
struct LookAsked {
    at: At,
    /// Record 26: the absolute path the call named, for a folder under no
    /// ingest location. The look is the same one, bounded the same way, on
    /// a root made of that folder; the answer names no location.
    given: Option<PathBuf>,
    names: Option<Vec<String>>,
    budget: Duration,
}

impl LookAsked {
    /// How the answer names the folder: the location and the part under it,
    /// or nothing where the call gave a path of its own.
    fn named(&self) -> (Value, Value, Value) {
        match self.given {
            Some(_) => (Value::Null, Value::Null, Value::Null),
            None => (
                json!(self.at.text()),
                json!(self.at.root),
                json!(self.at.rel),
            ),
        }
    }
}

/// What a look knows as the disk answers, for an answer past the wait.
#[derive(Default)]
struct LookKnown {
    doc: Option<Value>,
    names: Vec<String>,
    here: Option<Value>,
    folders: Vec<Value>,
}

/// The look itself, which reads the disk.
fn looking(root: &Root, asked: &LookAsked, known: &Mutex<LookKnown>) -> Result<Value, Reply> {
    let begun = Instant::now();
    let (path, spot) = resolve(root, &asked.at)?;
    let (at, in_root, rel) = asked.named();
    let mut doc = json!({
        "at": at,
        "root": in_root,
        "rel": rel,
        "path": path.display().to_string(),
        "exists": true,
        "directory": true,
        "readable": true,
        "layout": null,
        "here": null,
        "folders": [],
        "timed_out": false,
    });
    let Some(real) = unread(&mut doc, spot) else {
        return Ok(doc);
    };
    // record 26: what the folder holds before it is declared a dataset
    doc["layout"] = dataset::layout_doc(&real, &dataset::detect(&real));
    let names = match &asked.names {
        Some(names) => names.clone(),
        None => {
            // the first folders in the order a listing gives them
            let mtime = std::fs::metadata(&real).and_then(|m| m.modified()).ok();
            let (read, mine) = read_of(&real, mtime);
            if mine {
                fill(&real, &read, mtime);
            } else if !wait_for(&read, asked.budget) {
                return Ok(doc);
            }
            let found = read
                .found
                .lock()
                .map_err(|_| Reply::error(500, "a look's read was left unfinished"))?;
            doc["readable"] = json!(!found.refused);
            found.names.iter().take(LOOK_FOLDERS).cloned().collect()
        }
    };
    if let Ok(mut k) = known.lock() {
        k.doc = Some(doc.clone());
        k.names = names.clone();
    }
    // the budget left, shared fairly between the folders left, each given a
    // quarter second at least; none once the budget is spent
    let slice = |left: usize| {
        let spent = begun.elapsed();
        (spent < asked.budget).then(|| {
            let fair = (asked.budget - spent) / u32::try_from(left.max(1)).unwrap_or(u32::MAX);
            Instant::now() + fair.max(SLICE_MIN)
        })
    };
    if let Some(until) = slice(names.len() + 1) {
        let here = Value::Object(what(&sample(&real, HERE_REACH, until), until));
        if let Ok(mut k) = known.lock() {
            k.here = Some(here.clone());
        }
        doc["here"] = here;
    }
    let mut folders = Vec::with_capacity(names.len());
    for (i, name) in names.iter().enumerate() {
        let entry = match slice(names.len() - i) {
            None => json!({"name": name, "looked": false}),
            Some(until) => {
                let mut entry = object(json!({"name": name, "looked": true}));
                let inside = real.join(name);
                // a link is not followed, as the digest does not follow it
                match std::fs::symlink_metadata(&inside) {
                    Ok(m) if m.is_dir() => {
                        entry.extend(what(&sample(&inside, FOLDER_REACH, until), until));
                    }
                    _ => {
                        entry.insert("directory".to_string(), json!(false));
                        entry.extend(what(&Sampled::default(), until));
                    }
                }
                Value::Object(entry)
            }
        };
        if let Ok(mut k) = known.lock() {
            k.folders.push(entry.clone());
        }
        folders.push(entry);
    }
    doc["folders"] = json!(folders);
    Ok(doc)
}

/// A look's answer past its budget and the margin: what the look knew by
/// then, and every folder it did not reach as not looked.
fn look_late(known: &Mutex<LookKnown>, asked: &LookAsked, root: &Root) -> Value {
    let not_looked = |name: &String| json!({"name": name, "looked": false});
    let doc = known.try_lock().ok().and_then(|k| {
        let mut doc = k.doc.clone()?;
        let mut folders = k.folders.clone();
        folders.extend(k.names.iter().skip(k.folders.len()).map(not_looked));
        doc["here"] = k.here.clone().unwrap_or(Value::Null);
        doc["folders"] = json!(folders);
        Some(doc)
    });
    let (at, in_root, rel) = asked.named();
    let mut doc = doc.unwrap_or_else(|| {
        json!({
            "at": at,
            "root": in_root,
            "rel": rel,
            "path": asked.at.under(root).display().to_string(),
            "exists": null,
            "directory": null,
            "readable": null,
            "layout": null,
            "here": null,
            "folders": asked.names.iter().flatten().map(not_looked).collect::<Vec<_>>(),
        })
    });
    doc["timed_out"] = json!(true);
    doc
}

/// `POST /api/ingest/look`: what the files directly inside a folder of an
/// ingest root are, what a sample of each folder inside it holds, and the
/// layout a dataset declared on it would find.
pub(crate) fn look_door(
    roots: &BTreeMap<String, PathBuf>,
    store: &mut Store,
    doc: &Value,
) -> Result<Reply, Reply> {
    // A folder of a location, or, for a folder no dataset is declared on
    // yet, the path itself: the desk asks what a folder holds before it
    // declares a dataset on it, and such a folder is under no location
    // (record 26). The path is read under `data:work`, as the door is, and
    // the look is bounded exactly as a location's is.
    let (at, root, given) = match (doc["at"].as_str(), doc["path"].as_str()) {
        (Some(text), _) => {
            let at = At::parse(text).map_err(|m| Reply::error(400, m))?;
            let root = root_of(&dataset::roots(store, roots), &at)?;
            (at, root, None)
        }
        (None, Some(text)) => {
            let path = PathBuf::from(text.trim());
            if !path.is_absolute() || path.components().any(|c| c.as_os_str() == "..") {
                return Err(Reply::error(400, GIVEN));
            }
            let at = At {
                root: String::new(),
                rel: String::new(),
            };
            let root = Root::plain("", &path);
            (at, root, Some(path))
        }
        (None, None) => return Err(Reply::error(400, GIVEN)),
    };
    let names = match &doc["names"] {
        Value::Null => None,
        Value::Array(list) => {
            let mut names: Vec<String> = Vec::new();
            for v in list {
                let name = v.as_str().filter(|n| plain_name(n)).ok_or_else(|| {
                    Reply::error(400, "names: the names of folders directly inside at")
                })?;
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
            }
            if names.len() > LOOK_FOLDERS {
                return Err(Reply::error(400, "names: at most 64 folders a look"));
            }
            Some(names)
        }
        _ => {
            return Err(Reply::error(
                400,
                "names: the names of folders directly inside at",
            ));
        }
    };
    let budget = match &doc["budget_ms"] {
        Value::Null => BUDGET,
        v => v
            .as_u64()
            .filter(|n| *n >= 1)
            .ok_or_else(|| Reply::error(400, "budget_ms: milliseconds, 1 to 20,000"))?
            .min(BUDGET_MAX),
    };
    let asked = Arc::new(LookAsked {
        at,
        given,
        names,
        budget: Duration::from_millis(budget),
    });
    let known = Arc::new(Mutex::new(LookKnown::default()));
    let (a, k, r) = (Arc::clone(&asked), Arc::clone(&known), root.clone());
    match within(asked.budget + MARGIN, move || looking(&r, &a, &k)) {
        Some(answer) => answer.map(Reply::ok),
        None => Ok(Reply::ok(look_late(&known, &asked, &root))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_dicom::synth::TempDir;
    use std::collections::BTreeSet;

    #[test]
    fn a_folder_is_named_under_its_root_and_never_outside_it() {
        let at = At::parse(" @scans/sub-001//ses-1/./ ").unwrap();
        assert_eq!(
            at,
            At {
                root: "scans".into(),
                rel: "sub-001/ses-1".into()
            }
        );
        assert_eq!(at.text(), "@scans/sub-001/ses-1");
        assert_eq!(at.parent().unwrap().text(), "@scans/sub-001");
        assert_eq!(
            At::parse("@scans/a").unwrap().parent().unwrap().text(),
            "@scans"
        );
        assert_eq!(At::parse("@scans").unwrap().parent(), None);
        assert_eq!(
            At::parse("@scans")
                .unwrap()
                .under(&Root::plain("scans", Path::new("/srv/scans"))),
            Path::new("/srv/scans")
        );
        for bad in ["/etc", "scans/a", "@", "@/a", ""] {
            assert_eq!(At::parse(bad), Err(NAMED), "{bad}");
        }
        for bad in [
            "@scans/../x",
            "@scans//etc",
            "@scans/a/../../x",
            "@scans/..",
        ] {
            assert_eq!(At::parse(bad), Err(OUTSIDE), "{bad}");
        }
        assert!(plain_name("sub-001") && plain_name(".hidden"));
        assert!(!plain_name("..") && !plain_name(".") && !plain_name("a/b") && !plain_name(""));
    }

    #[test]
    fn names_are_ordered_without_case_and_paged_after_a_name() {
        let mut names: Vec<String> = ["beta", "Alpha", "alpha", "gamma", "Beta2", "Émile"]
            .map(String::from)
            .to_vec();
        names.sort_unstable_by(|a, b| order(a, b));
        assert_eq!(names, ["Alpha", "alpha", "beta", "Beta2", "gamma", "Émile"]);
        assert_eq!(
            page(&names, "", None, 2),
            (vec!["Alpha", "alpha"], Some("alpha"))
        );
        assert_eq!(
            page(&names, "", Some("alpha"), 2),
            (vec!["beta", "Beta2"], Some("Beta2"))
        );
        assert_eq!(page(&names, "", Some("gamma"), 2), (vec!["Émile"], None));
        assert_eq!(page(&names, "BET", None, 1), (vec!["beta"], Some("beta")));
        assert_eq!(page(&names, "BET", Some("beta"), 1), (vec!["Beta2"], None));
        assert_eq!(page(&names, "émi", None, 5), (vec!["Émile"], None));
        // a name that is no longer there still pages from where it would stand
        assert_eq!(page(&names, "", Some("b"), 1), (vec!["beta"], Some("beta")));
        assert!(holds("Sub-001", "sub") && !holds("alpha", "z") && !holds("a", "abc"));
    }

    #[cfg(unix)]
    #[test]
    fn a_read_is_kept_while_its_directory_keeps_its_time() {
        let dir = TempDir::new("browse-kept");
        for name in ["b", "A", "c", ".hidden"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        dir.file("one.dcm", b"x");
        std::os::unix::fs::symlink(dir.path().join("b"), dir.path().join("link")).unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        let mtime = std::fs::metadata(&real).and_then(|m| m.modified()).ok();
        let (read, mine) = read_of(&real, mtime);
        assert!(mine);
        fill(&real, &read, mtime);
        {
            let found = read.found.lock().unwrap();
            assert_eq!(found.names, [".hidden", "A", "b", "c"], "the link left out");
            assert_eq!((found.files, found.more, found.partial), (1, false, false));
        }
        let (again, mine) = read_of(&real, mtime);
        assert!(!mine && Arc::ptr_eq(&read, &again), "kept");
        let (other, mine) = read_of(&real, Some(SystemTime::UNIX_EPOCH));
        assert!(
            mine && !Arc::ptr_eq(&read, &other),
            "a changed directory is read again"
        );
        fill(&real, &other, mtime);
        assert!(wait_for(&other, Duration::from_millis(10)));
    }

    #[test]
    fn a_look_takes_a_few_files_from_each_directory_breadth_first() {
        let dir = TempDir::new("browse-sample");
        for series in ["s1", "s2", "s3", "s4", "s5"] {
            for i in 0..10 {
                dir.file(&format!("subject/{series}/IM_{i}"), b"x");
            }
        }
        dir.file("subject/notes.txt", b"x");
        let far = Instant::now() + Duration::from_secs(60);
        let s = sample(&dir.path().join("subject"), FOLDER_REACH, far);
        assert_eq!(s.picked.len(), SAMPLE, "{s:?}");
        let from: BTreeSet<&Path> = s.picked.iter().filter_map(|p| p.parent()).collect();
        assert_eq!(from.len(), 5, "spread across the directories: {s:?}");
        assert_eq!(s.files, 41, "{s:?}");
        assert!(s.more, "a series was left unread: {s:?}");
        let here = sample(&dir.path().join("subject/s1"), HERE_REACH, far);
        assert_eq!((here.files, here.more, here.picked.len()), (10, false, 10));
        let whole = sample(&dir.path().join("subject/s2"), FOLDER_REACH, far);
        assert_eq!(
            (whole.files, whole.more, whole.picked.len()),
            (10, false, 4)
        );
        let spent = sample(&dir.path().join("subject"), FOLDER_REACH, Instant::now());
        assert!(spent.picked.is_empty() && spent.more, "{spent:?}");
    }
}
