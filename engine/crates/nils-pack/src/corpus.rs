// SPDX-License-Identifier: AGPL-3.0-only

//! The pack's own corpus, and the check at load
//! (`docs/specs/wave2-fingerprint-and-classify.md`, §5.2 and §11.3).
//!
//! A case is a stack in and the facts a pack author asserts out. The engine
//! refuses to load a pack whose corpus fails, at load and not at test time,
//! which is what makes a pack written elsewhere safe to install: a pack that
//! does not do what its own author says it does never judges anything.

use std::path::Path;

use crate::error::{Error, R};
use crate::eval::Evaluated;
use crate::pack::Pack;
use crate::stack::{Stack, Value};
use crate::yaml::{self, File};

/// One expectation.
pub struct Case {
    pub name: String,
    pub stack: Stack,
    /// Flags asserted by name, each with the value the author expects.
    pub flags: Vec<(String, bool)>,
    /// Axes asserted by name, each with what the row should store.
    pub axes: Vec<(String, String)>,
}

/// Read every case file the pack's `corpus/` directory holds, in name order,
/// and run them. Returns how many passed, which is all of them or none.
pub fn check(pack: &Pack, dir: &Path) -> R<usize> {
    let cases = read(dir)?;
    if cases.is_empty() {
        return Err(Error::at(
            "corpus",
            "a pack ships the cases that show it does what it says; this one has none",
        ));
    }
    run(pack, &cases, "corpus")?;
    Ok(cases.len())
}

/// Run cases against a pack. The failure names every case that does not hold,
/// not the first, because a pack author fixing one at a time is a pack author
/// we have wasted an afternoon of.
pub fn run(pack: &Pack, cases: &[(std::path::PathBuf, Case)], what: &str) -> R<()> {
    let mut failures: Vec<String> = Vec::new();
    let mut asserted = 0;
    for (file, case) in cases {
        let e = Evaluated::new(pack, &case.stack);
        for (flag, want) in &case.flags {
            asserted += 1;
            let Some(got) = e.flag(flag) else {
                return Err(Error::at(
                    format!("cases.{}.flags.{flag}", case.name),
                    format!("the pack has no flag named {flag}"),
                )
                .in_file(file, None));
            };
            if got != *want {
                failures.push(format!(
                    "  {}: {flag} is {got}, the case says {want}",
                    case.name
                ));
            }
        }
        if case.axes.is_empty() {
            continue;
        }
        let verdict = e.classify();
        for (axis, want) in &case.axes {
            asserted += 1;
            if pack.axis_index(axis).is_none() {
                return Err(Error::at(
                    format!("cases.{}.axes.{axis}", case.name),
                    format!("the pack has no axis named {axis}"),
                )
                .in_file(file, None));
            }
            let got = verdict.stored(axis);
            if got != *want {
                failures.push(format!(
                    "  {}: {axis} is {got:?}, the case says {want:?}",
                    case.name
                ));
            }
        }
    }
    if !failures.is_empty() {
        return Err(Error::at(
            what,
            format!(
                "{} of {asserted} case assertions do not hold, so it does not load:\n{}",
                failures.len(),
                failures.join("\n")
            ),
        ));
    }
    Ok(())
}

/// Every case in the pack's corpus directory, in file-name order so that a
/// run is the same twice.
pub fn read(dir: &Path) -> R<Vec<(std::path::PathBuf, Case)>> {
    let corpus = dir.join("corpus");
    if !corpus.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&corpus)
        .map_err(|e| Error {
            file: Some(corpus.clone()),
            line: None,
            path: String::new(),
            message: format!("cannot be read: {e}"),
        })?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "yml" || x == "yaml"))
        .collect();
    files.sort();

    let mut out = Vec::new();
    for path in files {
        let f = File::read(&path)?;
        let root = f.blame(yaml::obj(&f.value, "cases"))?;
        out.extend(cases_of(&f, yaml::get(root, "cases", "cases")?)?);
    }
    Ok(out)
}

/// The cases a value holds, from a pack's corpus file or from an overlay.
pub fn cases_of(f: &File, v: &serde_json::Value) -> R<Vec<(std::path::PathBuf, Case)>> {
    let list = f.blame(yaml::arr(v, "cases"))?;
    let mut out = Vec::new();
    for (i, c) in list.iter().enumerate() {
        let at = format!("cases[{i}]");
        let m = f.blame(yaml::obj(c, &at))?;
        let name = f.blame(yaml::text(yaml::get(m, "name", &at)?, &at))?;
        let mut stack = Stack::new();
        for (k, v) in f.blame(yaml::obj(yaml::get(m, "stack", &at)?, &at))? {
            let where_ = format!("{at}.stack.{k}");
            let value = match v {
                serde_json::Value::Number(_) => {
                    Value::Num(Some(f.blame(yaml::number(v, &where_))?))
                }
                _ => Value::Text(None),
            };
            let owned;
            let value = match value {
                Value::Text(_) => {
                    owned = f.blame(yaml::text(v, &where_))?;
                    Value::Text(Some(owned.as_str()))
                }
                other => other,
            };
            stack
                .set(k, value)
                .map_err(|e| Error::at(&where_, e).in_file(&f.path, Some(&f.source)))?;
        }
        let mut axes = Vec::new();
        if let Some(a) = m.get("axes") {
            for (k, v) in f.blame(yaml::obj(a, &format!("{at}.axes")))? {
                axes.push((
                    k.clone(),
                    f.blame(yaml::text(v, &format!("{at}.axes.{k}")))?,
                ));
            }
        }
        let mut flags = Vec::new();
        for (k, v) in f.blame(yaml::obj(
            m.get("flags")
                .unwrap_or(&serde_json::Value::Object(Default::default())),
            &at,
        ))? {
            let want = v.as_bool().ok_or_else(|| {
                Error::at(format!("{at}.flags.{k}"), "expected true or false")
                    .in_file(&f.path, Some(&f.source))
            })?;
            flags.push((k.clone(), want));
        }
        if flags.is_empty() && axes.is_empty() {
            return Err(Error::at(&at, "asserts nothing about flags or axes")
                .in_file(&f.path, Some(&f.source)));
        }
        out.push((
            f.path.clone(),
            Case {
                name,
                stack,
                flags,
                axes,
            },
        ));
    }
    Ok(out)
}
