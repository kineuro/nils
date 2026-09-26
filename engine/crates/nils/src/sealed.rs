// SPDX-License-Identifier: AGPL-3.0-only

//! Record 48, D1 of the move to the group's install: sealed means sealed on
//! every door. While a stack is of a sample sealed now (a `sealed_stack` row
//! no certificate has unsealed), what any system said of it (the rules'
//! values and evidence, System 1's questions and scores, a decision, a
//! review item, a suggestion, a release's route) is withheld from every
//! caller but one holding [`crate::grants::UNSEALED`], which no ladder set
//! holds, so no person holds it while they read. The file itself (its
//! header text, its physics and its pixels) is never withheld: blind hides
//! the systems' answers, never the file.
//!
//! At the keyboard the operator holds the registry itself, so the command
//! line withholds the same, unless `--unsealed-access REASON` is given; each
//! command that then reads a sealed stack writes one `sealed.read` audit row
//! with the reason. A verb a door queued runs under the grants of the caller
//! that queued it, never the worker's.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use nils_registry::store::Error as StoreError;
use nils_registry::{Registry, Store};

use crate::grants::{Access, UNSEALED};

/// Whether a caller reads what a system said of a stack sealed now.
pub(crate) fn reads(access: &Access) -> bool {
    access.holds(UNSEALED)
}

/// The stacks of these whose systems' answers a caller holding `access` is
/// not shown: every one of a sample sealed now, unless it holds the grant.
pub(crate) fn withheld(
    store: &mut Store,
    access: &Access,
    stacks: &[i64],
) -> Result<BTreeSet<i64>, StoreError> {
    if reads(access) || stacks.is_empty() {
        return Ok(BTreeSet::new());
    }
    now(store, stacks)
}

/// The stacks of these of a sample sealed now, for anyone.
pub(crate) fn now(store: &mut Store, stacks: &[i64]) -> Result<BTreeSet<i64>, StoreError> {
    nils_registry::labels::sealed_now(store, stacks, &[])
        .map(|(s, _)| s)
        .map_err(|e| StoreError::Message(e.to_string()))
}

/// The reason given with `--unsealed-access` at the keyboard, once a
/// process.
static KEYBOARD: OnceLock<Option<String>> = OnceLock::new();

/// Whether this process wrote its `sealed.read` row yet.
static AUDITED: OnceLock<()> = OnceLock::new();

/// The environment a worker sets when the caller that queued the job held
/// the grant.
pub(crate) const JOB_VAR: &str = "NILS_JOB_UNSEALED";

/// Record the reason `--unsealed-access` gave, once, before any command.
pub(crate) fn set_keyboard(reason: Option<String>) {
    let _ = KEYBOARD.set(
        reason
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty()),
    );
}

/// Whether the command line reads the systems' answers of sealed stacks:
/// under a queued job, as the caller that queued it did; at the keyboard,
/// only with `--unsealed-access REASON`.
pub(crate) fn keyboard_reads() -> bool {
    if std::env::var("NILS_JOB_DETAIL").is_ok() {
        return std::env::var(JOB_VAR).ok().as_deref() == Some("1");
    }
    KEYBOARD.get().is_some_and(Option::is_some)
}

/// The reason given at the keyboard, if any.
fn reason() -> Option<&'static str> {
    KEYBOARD.get().and_then(|r| r.as_deref())
}

/// The stacks of these the command line does not show, as [`withheld`]
/// does for a door.
pub(crate) fn keyboard_withheld(
    store: &mut Store,
    stacks: &[i64],
) -> Result<BTreeSet<i64>, StoreError> {
    if stacks.is_empty() || keyboard_reads() {
        return Ok(BTreeSet::new());
    }
    now(store, stacks)
}

/// What a command at the keyboard says when it will not show a sealed
/// stack.
pub(crate) fn refusal(what: &str) -> String {
    format!(
        "{what} is of a sample sealed for certification, and what a system said of it is withheld until a certificate unseals it (record 48); pass --unsealed-access REASON to read it, which is audited"
    )
}

/// Write the `sealed.read` audit row of this process when the keyboard
/// gave `--unsealed-access`: who, the command and why. A queued job's read
/// is its caller's, and is not the keyboard's.
pub(crate) fn audit_keyboard(registry: &mut Registry) {
    let Some(why) = reason() else {
        return;
    };
    if std::env::var("NILS_JOB_DETAIL").is_ok() || AUDITED.set(()).is_err() {
        return;
    }
    let principal = crate::actor();
    let command: Vec<String> = std::env::args().skip(1).collect();
    let _ = nils_registry::audit::record(
        registry,
        &nils_registry::audit::Entry {
            principal: &principal,
            action: nils_registry::audit::Action::SealedRead,
            scope: serde_json::json!({"command": command}),
            policy: None,
            job_id: None,
            details: Some(serde_json::json!({"why": why})),
        },
    );
}

/// The stacks one review row is about: its own stack, and a group's
/// members.
fn review_about(store: &mut Store, row: &serde_json::Value) -> Result<Vec<i64>, StoreError> {
    let mut stacks: Vec<i64> = row["ref"]["stack_id"].as_i64().into_iter().collect();
    if row["scope"] == "group"
        && let Some(id) = row["id"].as_i64()
    {
        let sql = format!(
            "SELECT stack_id FROM {} WHERE item_id = {}",
            store.qualified("review_member"),
            store.dialect().param(1, nils_registry::schema::Type::Int)
        );
        for m in store.query(&sql, &[nils_registry::Param::Int(id)])? {
            stacks.push(m.int(0)?);
        }
    }
    Ok(stacks)
}

/// Review rows as a caller who does not read sealed stacks reads them: an
/// item about a stack sealed now, or a group whose members are all sealed
/// now, is left out, as if it were not there; a group that holds some keeps
/// the others, its member count and member list without the sealed ones.
/// Answers how many items were left out. `reads` is the caller's grant.
pub(crate) fn withhold_review(
    store: &mut Store,
    rows: &mut Vec<serde_json::Value>,
    reads: bool,
) -> Result<usize, StoreError> {
    if reads || rows.is_empty() {
        return Ok(0);
    }
    let mut about: Vec<Vec<i64>> = Vec::with_capacity(rows.len());
    for r in rows.iter() {
        about.push(review_about(store, r)?);
    }
    let all: Vec<i64> = about.iter().flatten().copied().collect();
    let sealed = now(store, &all)?;
    if sealed.is_empty() {
        return Ok(0);
    }
    let before = rows.len();
    let mut kept = Vec::with_capacity(rows.len());
    for (mut r, stacks) in rows.drain(..).zip(about) {
        let hidden = stacks.iter().filter(|s| sealed.contains(s)).count();
        if hidden == 0 {
            kept.push(r);
            continue;
        }
        if r["scope"] != "group" || hidden == stacks.len() {
            continue;
        }
        if let Some(list) = r["member_stacks"].as_array_mut() {
            list.retain(|m| !m["stack_id"].as_i64().is_some_and(|s| sealed.contains(&s)));
        }
        let left = stacks.len() - hidden;
        r["members"] = serde_json::json!(left);
        r["withheld_members"] = serde_json::json!(hidden);
        kept.push(r);
    }
    *rows = kept;
    Ok(before - rows.len())
}
