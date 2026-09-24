// SPDX-License-Identifier: AGPL-3.0-only

//! The pack's own shape (record 41, S3): which values of its vocabulary no
//! rule can reach, and which clauses only restate another axis.
//!
//! Both are facts about the pack and not about any stack, so they are worked
//! out from the loaded rules alone, once, without a registry.
//!
//! **A value is reached** when some rule of some rule set can write it: the
//! rule names it (or names a value its set derives, and a case of the derive
//! gives it), and the rule can fire. Whether a rule can fire is decided by an
//! abstract evaluation of its conditions over the values reached so far, in
//! the order the rule sets run: an atom on the fingerprint, a flag or a word
//! may be either true or false, and an axis atom `{axis: a, is: v}` can hold
//! only if `a = v` was reached by a rule that runs before it. The walk follows
//! the evaluator's own order (rule sets in the pack's order, rules in their
//! set's order; the disposition phase sees every class axis, its default
//! included, because it is seeded from what was stored), so a rule that reads
//! an axis decided only after it is correctly found unable to see that value.
//!
//! What it proves and what it does not. A value it reports unreachable is
//! unreachable by the rules on every stack: the evaluation over-approximates
//! what can hold, so nothing it drops could have been written. The converse
//! is not claimed. A value it calls reached may still be unreachable in fact,
//! because it treats every flag and every word as free, ignores that two
//! atoms of one condition may be correlated (`a` and `not a`), and does not
//! follow an axis a single-valued rule set closed before a later one could
//! write it. A person's decision can write any value; this is about the rules.
//!
//! **An exclusion group** removes a value in one case the walk can prove:
//! every rule that writes it writes, in the same breath and unconditionally,
//! a member of its group that wins over it.
//!
//! **A pass** fills an axis from a reference of stacks a rule or a person
//! decided, so it writes values the rules already reach and never a new one.
//! The report names the axes each pass writes and counts nothing for them.
//!
//! **A schema implication** is a clause whose condition, together with its
//! rule's `requires`, reads other axes' values and nothing else: no field, no
//! flag, no word. `technique = MPRAGE` implying `base = T1w` is one. It is
//! true of the schema, not evidence read from the stack, and a model that
//! counts it as a witness counts the other axis twice (record 39).

use serde::Serialize;

use crate::expr::Expr;
use crate::pack::Pack;
use crate::rules::{AxisPhase, Clause, Rule, RuleSet, Which};

/// What a pack's shape is.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Shape {
    pub pack: String,
    pub axes: Vec<AxisShape>,
    /// Every clause that only restates another axis, in rule order.
    pub implications: Vec<Implication>,
}

/// One axis: how many of its values the rules reach, and which they do not.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AxisShape {
    pub axis: String,
    pub phase: String,
    pub values: usize,
    pub reached: usize,
    /// The literal the axis takes when nothing matched, which no rule writes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// The passes that may fill this axis, from values the rules reach.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub filled_by_passes: Vec<String>,
    pub unreachable: Vec<Unreached>,
}

/// A value no rule can write, and why.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Unreached {
    /// The value's identity.
    pub value: String,
    /// `no_rule`: no rule names it. `no_valid_combination`: rules name it,
    /// and none of them can fire given what the other axes can be; `rules`
    /// says which. `exclusion_group`: every rule that writes it also writes
    /// `by`, which wins in `group`.
    pub why: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

/// A clause that restates other axes rather than reading the stack.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Implication {
    pub rule_set: String,
    pub rule: String,
    /// The clause's position in its rule, from zero.
    pub clause: usize,
    /// The axes the condition reads.
    pub reads: Vec<String>,
    /// What the rule writes, as `axis=value`, or `axis=` for nothing and
    /// `axis=<derive>` for a value its set works out.
    pub writes: Vec<String>,
}

impl Shape {
    /// How many values of every axis no rule reaches.
    pub fn unreachable(&self) -> usize {
        self.axes.iter().map(|a| a.unreachable.len()).sum()
    }

    /// How many values in all, and how many reached.
    pub fn counts(&self) -> (usize, usize) {
        self.axes
            .iter()
            .fold((0, 0), |(v, r), a| (v + a.values, r + a.reached))
    }
}

impl Expr {
    /// Whether this expression reads other axes' values and nothing else: at
    /// least one axis atom, and no field, flag, word, predicate or candidate.
    /// A literal is neither evidence nor an axis, and does not count.
    pub fn reads_only_axes(&self) -> bool {
        fn walk(e: &Expr, axes: &mut bool) -> bool {
            match e {
                Expr::Axis { .. } | Expr::AxisMissingOr { .. } => {
                    *axes = true;
                    true
                }
                Expr::Lit(_) => true,
                Expr::Any(es) | Expr::All(es) => es.iter().all(|e| walk(e, axes)),
                Expr::Not(e) => walk(e, axes),
                _ => false,
            }
        }
        let mut axes = false;
        walk(self, &mut axes) && axes
    }
}

impl Rule {
    /// Whether clause `i` only restates other axes: its condition, and the
    /// rule's `requires` when it has one, read axis values and nothing else.
    /// Such a clause is a schema implication, not evidence (record 39).
    pub fn restates(&self, i: usize) -> bool {
        let Some(Clause::When { expr, .. }) = self.clauses.get(i) else {
            return false;
        };
        expr.reads_only_axes()
            && self
                .requires
                .as_ref()
                .is_none_or(|r| r.reads_only_axes() || matches!(r, Expr::Lit(true)))
    }
}

/// Whether an expression can be true, and whether it can be false, given the
/// values each axis can hold at this point. Anything that is not an axis
/// atom can be either.
fn can(e: &Expr, reached: &[Vec<String>]) -> (bool, bool) {
    match e {
        Expr::Lit(b) => (*b, !*b),
        Expr::Axis { axis, value } => (reached[*axis].iter().any(|v| v == value), true),
        // An axis may always still be empty: nothing has to fire.
        Expr::AxisMissingOr { .. } => (true, true),
        Expr::Not(x) => {
            let (t, f) = can(x, reached);
            (f, t)
        }
        Expr::All(xs) => xs.iter().fold((true, false), |(t, f), x| {
            let (xt, xf) = can(x, reached);
            (t && xt, f || xf)
        }),
        Expr::Any(xs) => xs.iter().fold((false, true), |(t, f), x| {
            let (xt, xf) = can(x, reached);
            (t || xt, f && xf)
        }),
        _ => (true, true),
    }
}

fn may_hold(e: &Expr, reached: &[Vec<String>]) -> bool {
    can(e, reached).0
}

/// Whether a clause can hold on some stack.
fn clause_may_hold(c: &Clause, reached: &[Vec<String>]) -> bool {
    match c {
        Clause::Flag { .. } | Clause::AnyFlag { .. } => true,
        Clause::Keywords { list, .. } => !list.is_empty(),
        Clause::Combination { flags, .. } => !flags.is_empty(),
        Clause::When { expr, .. } => may_hold(expr, reached),
    }
}

/// Every (axis, value index) a rule's `sets` may name at all, whatever holds.
fn named(set: &RuleSet, rule: &Rule, into: &mut Vec<(usize, usize)>) {
    for s in &rule.sets {
        for v in &s.values {
            match v.value {
                Which::Fixed(i) => into.push((s.axis, i)),
                Which::Derived(d) => {
                    for c in &set.derives[d].cases {
                        into.push((s.axis, c.value));
                    }
                }
                Which::Nothing => {}
            }
        }
    }
}

impl Pack {
    /// The pack's shape: its unreachable values and its implications.
    pub fn shape(&self) -> Shape {
        shape(self)
    }
}

/// Work out the pack's shape.
pub fn shape(pack: &Pack) -> Shape {
    let n = pack.axes.len();
    // What each axis can hold so far, as stored; what reached it.
    let mut reached: Vec<Vec<String>> = vec![Vec::new(); n];
    let mut hit: Vec<Vec<bool>> = pack
        .axes
        .iter()
        .map(|a| vec![false; a.values.len()])
        .collect();
    // Per value: the rules that name it, and for a grouped value whether a
    // write of it that is not beaten by its group was seen.
    let mut naming: Vec<Vec<Vec<String>>> = pack
        .axes
        .iter()
        .map(|a| vec![Vec::new(); a.values.len()])
        .collect();
    let mut unbeaten: Vec<Vec<bool>> = pack
        .axes
        .iter()
        .map(|a| vec![false; a.values.len()])
        .collect();
    let mut beaten_by: Vec<Vec<Option<usize>>> = pack
        .axes
        .iter()
        .map(|a| vec![None; a.values.len()])
        .collect();

    for phase in [AxisPhase::Class, AxisPhase::Disposition] {
        if phase == AxisPhase::Disposition {
            // The disposition phase is seeded from what was stored, and a
            // class axis that nothing decided stored its default.
            for (ai, a) in pack.axes.iter().enumerate() {
                if a.phase == AxisPhase::Class
                    && let Some(d) = &a.default
                    && !reached[ai].contains(d)
                {
                    reached[ai].push(d.clone());
                }
            }
        }
        for set in pack.rule_sets.iter().filter(|s| s.phase == phase) {
            for rule in &set.rules {
                let mut own = Vec::new();
                named(set, rule, &mut own);
                for (a, i) in own {
                    let id = format!("{}/{}", set.name, rule.id);
                    if !naming[a][i].contains(&id) {
                        naming[a][i].push(id);
                    }
                }
            }
            if set
                .enter_when
                .as_ref()
                .is_some_and(|e| !may_hold(e, &reached))
            {
                continue;
            }
            for rule in &set.rules {
                // The derives are worked out before the rules run, over what
                // was decided when the set was entered; reading them per rule
                // over what has been reached since is a wider net, never a
                // narrower one.
                let derived: Vec<Vec<usize>> = set
                    .derives
                    .iter()
                    .map(|d| {
                        d.cases
                            .iter()
                            .filter(|c| c.when.as_ref().is_none_or(|w| may_hold(w, &reached)))
                            .map(|c| c.value)
                            .collect()
                    })
                    .collect();
                if rule
                    .requires
                    .as_ref()
                    .is_some_and(|g| !may_hold(g, &reached))
                {
                    continue;
                }
                if !rule.clauses.iter().any(|c| clause_may_hold(c, &reached)) {
                    continue;
                }
                for s in &rule.sets {
                    let axis = &pack.axes[s.axis];
                    // The values this entry writes whatever else holds: what
                    // an exclusion group is judged against.
                    let sure: Vec<usize> = s
                        .values
                        .iter()
                        .filter(|v| v.when.is_none())
                        .filter_map(|v| match v.value {
                            Which::Fixed(i) => Some(i),
                            _ => None,
                        })
                        .collect();
                    let mut wrote: Vec<usize> = Vec::new();
                    for v in &s.values {
                        if v.when.as_ref().is_some_and(|w| !may_hold(w, &reached)) {
                            continue;
                        }
                        match v.value {
                            Which::Fixed(i) => wrote.push(i),
                            Which::Derived(d) => wrote.extend(derived[d].iter().copied()),
                            Which::Nothing => {}
                        }
                    }
                    for i in wrote {
                        hit[s.axis][i] = true;
                        let stored = axis.stored(i).to_string();
                        if !reached[s.axis].contains(&stored) {
                            reached[s.axis].push(stored);
                        }
                        let beaten = axis.values[i].group.as_deref().and_then(|g| {
                            let p = axis.values[i].priority.unwrap_or(i64::MAX);
                            sure.iter().copied().find(|&w| {
                                let wv = &axis.values[w];
                                w != i
                                    && wv.group.as_deref() == Some(g)
                                    && (wv.priority.unwrap_or(i64::MAX) < p
                                        || (wv.priority.unwrap_or(i64::MAX) == p && w < i))
                            })
                        });
                        match beaten {
                            None => unbeaten[s.axis][i] = true,
                            Some(w) => beaten_by[s.axis][i] = Some(w),
                        }
                    }
                }
            }
        }
    }

    let axes = pack
        .axes
        .iter()
        .enumerate()
        .map(|(ai, a)| {
            let mut unreachable = Vec::new();
            let mut count = 0;
            for (i, v) in a.values.iter().enumerate() {
                let by_default = a.default.as_deref() == Some(a.stored(i));
                if by_default || (hit[ai][i] && unbeaten[ai][i]) {
                    count += 1;
                    continue;
                }
                unreachable.push(if hit[ai][i] {
                    let w =
                        beaten_by[ai][i].expect("a value written and never unbeaten was beaten");
                    Unreached {
                        value: v.id.clone(),
                        why: "exclusion_group".into(),
                        rules: naming[ai][i].clone(),
                        group: v.group.clone(),
                        by: Some(a.values[w].id.clone()),
                    }
                } else if naming[ai][i].is_empty() {
                    Unreached {
                        value: v.id.clone(),
                        why: "no_rule".into(),
                        rules: Vec::new(),
                        group: None,
                        by: None,
                    }
                } else {
                    Unreached {
                        value: v.id.clone(),
                        why: "no_valid_combination".into(),
                        rules: naming[ai][i].clone(),
                        group: None,
                        by: None,
                    }
                });
            }
            AxisShape {
                axis: a.name.clone(),
                phase: a.phase.name().to_string(),
                values: a.values.len(),
                reached: count,
                default: a.default.clone(),
                filled_by_passes: pack
                    .passes
                    .iter()
                    .filter(|p| p.vote().is_some_and(|v| v.writes.contains(&ai)))
                    .map(|p| p.name.clone())
                    .collect(),
                unreachable,
            }
        })
        .collect();

    let mut implications = Vec::new();
    for set in &pack.rule_sets {
        for rule in &set.rules {
            for (ci, c) in rule.clauses.iter().enumerate() {
                if !rule.restates(ci) {
                    continue;
                }
                let mut read = Vec::new();
                if let Clause::When { expr, .. } = c {
                    expr.axes_read(&mut read);
                }
                if let Some(r) = &rule.requires {
                    r.axes_read(&mut read);
                }
                read.sort_unstable();
                read.dedup();
                let writes = rule
                    .sets
                    .iter()
                    .flat_map(|s| {
                        let axis = &pack.axes[s.axis];
                        s.values.iter().map(move |v| match v.value {
                            Which::Fixed(i) => format!("{}={}", axis.name, axis.values[i].id),
                            Which::Nothing => format!("{}=", axis.name),
                            Which::Derived(d) => {
                                format!("{}=<{}>", axis.name, set.derives[d].name)
                            }
                        })
                    })
                    .collect();
                implications.push(Implication {
                    rule_set: set.name.clone(),
                    rule: rule.id.clone(),
                    clause: ci,
                    reads: read.iter().map(|a| pack.axes[*a].name.clone()).collect(),
                    writes,
                });
            }
        }
    }

    Shape {
        pack: pack.id(),
        axes,
        implications,
    }
}

impl std::fmt::Display for Shape {
    /// The short form: the counts, every unreachable value, and the
    /// implications counted per rule set. `--json` has them one by one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (values, reached) = self.counts();
        writeln!(
            f,
            "{}: {reached} of {values} values reached by a rule, {} unreachable, {} clauses restate another axis",
            self.pack,
            self.unreachable(),
            self.implications.len()
        )?;
        for a in &self.axes {
            for u in &a.unreachable {
                let why = match u.why.as_str() {
                    "no_rule" => "no rule writes it".to_string(),
                    "no_valid_combination" => {
                        format!("{} can never fire", u.rules.join(", "))
                    }
                    _ => format!(
                        "{} beats it in group {}",
                        u.by.as_deref().unwrap_or(""),
                        u.group.as_deref().unwrap_or("")
                    ),
                };
                writeln!(f, "  unreachable  {}.{}  {why}", a.axis, u.value)?;
            }
        }
        let mut per: Vec<(&str, usize)> = Vec::new();
        for i in &self.implications {
            match per.iter_mut().find(|(s, _)| *s == i.rule_set) {
                Some((_, n)) => *n += 1,
                None => per.push((&i.rule_set, 1)),
            }
        }
        for (set, n) in per {
            writeln!(f, "  implications {set}: {n}")?;
        }
        Ok(())
    }
}
