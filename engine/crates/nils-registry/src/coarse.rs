// SPDX-License-Identifier: AGPL-3.0-only

//! Two committed answers that do not agree, on two axes that say one
//! thing at two grains.
//!
//! A pack may declare one axis the coarse form of another, each coarse
//! value covering some values of the finer axis. The sort holds a stack's
//! two rows to that: a rule's value never stands against a committed
//! answer on the other axis. Where both axes were decided and the coarse
//! decision does not cover the finer one, the sort leaves both as they
//! were decided and asks nothing, and this counts them, so that the
//! people who decided can look.
//!
//! The registry holds no pack, so it is handed the pack's coarse forms as
//! data ([`Form`]), as it is handed the spellings ([`crate::spelling`]).
//! A row is read in the form the pack stores; a row that holds another
//! spelling is counted by the spellings' check and not here.

use std::collections::{BTreeMap, BTreeSet};

use crate::schema::Type;
use crate::store::{Error, Param, Store};

/// The tier of a row a decision wrote on the axis it was made on.
pub const DECISION: &str = "decision";

/// The tier of a coarse value folded from a committed finer one, on its
/// row and on the evidence that says what it was folded from and who
/// decided that.
pub const FOLDED: &str = "folded";

/// The tier of the evidence row that says a value was set aside by a
/// committed answer on the other axis. No row of an axis carries it: the
/// axis is left with no row, and this evidence row is the mark that tells
/// "set aside" from "nobody said".
pub const SET_ASIDE: &str = "set_aside";

/// What the evidence of either names as its rule set: the pack's key.
pub const RULE_SET: &str = "coarse_of";

/// Whether a row of this tier holds a committed answer: a decision on its
/// own axis, or the value a decision on the finer axis folds to. Neither
/// is the rules' answer, and whatever tells "the rules said this" from
/// "someone decided this" asks here.
pub fn decided(tier: &str) -> bool {
    tier == DECISION || tier == FOLDED
}

/// [`decided`] for a statement: the tiers, as a list a `tier IN` reads.
pub const DECIDED_SQL: &str = "('decision', 'folded')";

/// The axes of each stack whose value a committed answer on the other
/// axis set aside at the stack's last sort: the axis holds no row, and it
/// is no axis the rules left open. Read from the evidence rows that mark
/// it, for some stacks.
pub fn set_aside(
    store: &mut Store,
    stacks: &[i64],
) -> Result<BTreeMap<i64, BTreeSet<String>>, Error> {
    let mut out: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for chunk in stacks.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT stack_id, axis FROM {} WHERE stack_id IN ({list}) AND tier = '{SET_ASIDE}'",
            store.qualified("classification_evidence")
        );
        for r in store.query(&sql, &[])? {
            out.entry(r.int(0)?)
                .or_default()
                .insert(r.text(1)?.to_string());
        }
    }
    Ok(out)
}

/// The condition that a stack's axis was set aside at its last sort, for
/// a statement that has the stack's id as `stack` and names the axis by
/// a literal: `EXISTS (...)` over the evidence rows that mark it.
pub fn set_aside_sql(store: &Store, stack: &str, axis: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM {} sa WHERE sa.stack_id = {stack} AND sa.axis = '{}' \
         AND sa.tier = '{SET_ASIDE}')",
        store.qualified("classification_evidence"),
        axis.replace('\'', "''"),
    )
}

/// One coarse form of a pack, as data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    /// The coarse axis and the finer one it folds.
    pub coarse: String,
    pub fine: String,
    /// By value of the coarse axis, as a row stores it: the values of the
    /// finer axis, as a row stores them, that it does not cover.
    pub apart: Vec<(String, Vec<String>)>,
}

/// The stacks whose rows hold a decision on both axes of a form, where
/// the coarse one does not cover the finer one: by coarse axis, an entry
/// for every form, 0 where none is apart. One statement a form, read
/// through the key on (axis, value).
pub fn count_apart(store: &mut Store, forms: &[Form]) -> Result<BTreeMap<String, i64>, Error> {
    let mut out = BTreeMap::new();
    for form in forms {
        let d = store.dialect();
        let mut params = vec![
            Param::from(form.coarse.as_str()),
            Param::from(form.fine.as_str()),
        ];
        let mut pairs: Vec<String> = Vec::new();
        for (coarse, fine) in &form.apart {
            if fine.is_empty() {
                continue;
            }
            params.push(Param::from(coarse.as_str()));
            let value = d.param(params.len(), Type::Text);
            let mut marks = Vec::with_capacity(fine.len());
            for f in fine {
                params.push(Param::from(f.as_str()));
                marks.push(d.param(params.len(), Type::Text));
            }
            pairs.push(format!(
                "(c.value = {value} AND f.value IN ({}))",
                marks.join(", ")
            ));
        }
        let n = if pairs.is_empty() {
            0
        } else {
            let table = store.qualified("classification_axis");
            let sql = format!(
                "SELECT COUNT(DISTINCT c.stack_id) FROM {table} c \
                 JOIN {table} f ON f.stack_id = c.stack_id \
                 WHERE c.axis = {coarse} AND c.tier IN {DECIDED_SQL} \
                 AND f.axis = {fine} AND f.tier IN {DECIDED_SQL} AND ({pairs})",
                coarse = d.param(1, Type::Text),
                fine = d.param(2, Type::Text),
                pairs = pairs.join(" OR "),
            );
            store.query(&sql, &params)?[0].int(0)?
        };
        out.insert(form.coarse.clone(), n);
    }
    Ok(out)
}
