// SPDX-License-Identifier: AGPL-3.0-only

//! `describe` (§10): one deterministic sentence per set in deal breaker
//! order (membership, then counts, then sameness, which is the order of
//! rule 5: source, near, attach, has, bind, where, pick), the conventions
//! block, the denominators by name, the mechanism that chose each attached
//! row, and the disclosure level. Pure over the document: no query.

use std::collections::BTreeSet;

use nils_registry::session::Scheme;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ast::{
    AlgOp, Arg, Ask, Clause, Dir, IntSpec, Policy, SchemeRef, Set, Src, Tie, WindowSpec,
};
use crate::validate::{Names, Scope};

/// What describe says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Description {
    /// One sentence per set, in topological order when known, else by name.
    pub sets: Vec<(String, String)>,
    pub conventions: Vec<String>,
    /// A named count and the set it counts.
    pub denominators: Vec<(String, String)>,
    /// A set, a partner name, and how its row was chosen.
    pub mechanisms: Vec<(String, String, String)>,
    pub disclosure: String,
    /// The answer, in words.
    pub answer: String,
}

fn literal(a: &Arg) -> String {
    match a {
        Arg::Clause(c) => clause_text(c),
        Arg::Text(t) => t.clone(),
        Arg::Int(i) => i.to_string(),
        Arg::Number(n) => n.to_string(),
        Arg::Bool(b) => b.to_string(),
        Arg::Null => "null".into(),
        Arg::List(items) => format!(
            "[{}]",
            items.iter().map(literal).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn opt_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => match crate::ast::clause_of(other) {
            Ok(c) => clause_text(&c),
            Err(_) => other.to_string(),
        },
    }
}

/// A clause in words, deterministic.
pub fn clause_text(c: &Clause) -> String {
    let op = c.op.as_str();
    let arg = |i: usize| c.args.get(i).map(literal).unwrap_or_default();
    let opt = |k: &str| c.opts.get(k).map(opt_text);
    let strict = c
        .opts
        .get("strict")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut text = match op {
        "field" | "axis" => c.ref_name().unwrap_or("").to_string(),
        "param" => format!("{{{}}}", c.ref_name().unwrap_or("")),
        "derived" => {
            let name = c.ref_name().unwrap_or("");
            let mut opts: Vec<String> = c
                .opts
                .iter()
                .map(|(k, v)| format!("{k} {}", opt_text(v)))
                .collect();
            opts.sort();
            if opts.is_empty() {
                name.to_string()
            } else {
                format!("{name} at {}", opts.join(", "))
            }
        }
        "=" | "<>" | ">" | ">=" | "<" | "<=" => format!("{} {op} {}", arg(0), arg(1)),
        "~=" => format!(
            "{} within {} of {}",
            arg(0),
            opt("tol").unwrap_or_default(),
            arg(1)
        ),
        "in" => format!("{} among {}", arg(0), arg(1)),
        "not_in" => format!("{} not among {}", arg(0), arg(1)),
        "has" => format!("{} has {}", arg(0), arg(1)),
        "picked" => format!("picked as {}", opt("role").unwrap_or_default()),
        "not_null" => format!("{} is known", arg(0)),
        "is_null" => format!("{} is unknown", arg(0)),
        "contains" => format!("{} contains {}", arg(0), arg(1)),
        "starts_with" => format!("{} starts with {}", arg(0), arg(1)),
        "and" | "or" => c
            .args
            .iter()
            .map(literal)
            .collect::<Vec<_>>()
            .join(&format!(" {op} ")),
        "not" => format!("not {}", arg(0)),
        "+" | "-" | "*" | "/" => format!("{} {op} {}", arg(0), arg(1)),
        "abs" => format!("|{}|", arg(0)),
        "round" => format!("{} rounded to {} places", arg(0), arg(1)),
        "coalesce" => format!(
            "the first known of {}",
            c.args.iter().map(literal).collect::<Vec<_>>().join(", ")
        ),
        "case" => format!("{} if {} else {}", arg(1), arg(0), arg(2)),
        "concat" => c.args.iter().map(literal).collect::<Vec<_>>().join(" + "),
        "days_between" => format!("days from {} to {}", arg(1), arg(0)),
        "shift" => format!(
            "{} shifted {} {}s",
            arg(0),
            arg(1),
            opt("unit").unwrap_or_else(|| "day".into())
        ),
        "age_at" => format!("age at {} from {}", arg(1), arg(0)),
        "bucket" => format!("{} by {}", arg(0), opt("unit").unwrap_or_default()),
        "part" => format!("the {} of {}", opt("unit").unwrap_or_default(), arg(0)),
        "least" | "greatest" => format!("the {op} of {} and {}", arg(0), arg(1)),
        "json" => format!("{} in {}", arg(1), arg(0)),
        "ordinal" => "the ordinal within the subject".into(),
        "prev" => format!("the previous {}", arg(0)),
        "next" => format!("the next {}", arg(0)),
        "share" => format!(
            "{} as a share of {}",
            opt("of").unwrap_or_default(),
            opt("over").unwrap_or_default()
        ),
        "change" => {
            let adjacent = c
                .opts
                .get("adjacent")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            format!(
                "the {} changes from {} to {}{}",
                opt("of").unwrap_or_else(|| "course".into()),
                opt("from").unwrap_or_default(),
                opt("to").unwrap_or_default(),
                if adjacent {
                    " (adjacent)"
                } else {
                    " (any earlier)"
                }
            )
        }
        "count" | "distinct" | "min" | "max" | "sum" | "avg" | "list" => {
            let set = opt("set").unwrap_or_default();
            match c.args.first() {
                Some(a) => format!("the {op} of {} over {set}", literal(a)),
                None => format!("the {op} of {set}"),
            }
        }
        other => format!(
            "{other}({})",
            c.args.iter().map(literal).collect::<Vec<_>>().join(", ")
        ),
    };
    if strict {
        text.push_str(" (strict)");
    }
    text
}

fn dir_text(d: Dir) -> &'static str {
    match d {
        Dir::Asc => "asc",
        Dir::Desc => "desc",
    }
}

fn window_text(w: &WindowSpec) -> String {
    match w {
        WindowSpec::Literal(win) => {
            let (lo, hi) = win.days();
            match (lo, hi) {
                (Some(a), Some(b)) => format!("{a} to {b} days"),
                (Some(a), None) => format!("from {a} days on"),
                (None, Some(b)) => format!("up to {b} days"),
                (None, None) => "any distance".into(),
            }
        }
        WindowSpec::Param(c) => clause_text(c),
    }
}

fn int_text(i: &IntSpec) -> String {
    match i {
        IntSpec::Literal(n) => n.to_string(),
        IntSpec::Param(c) => clause_text(c),
    }
}

fn src_text(s: &Src) -> String {
    match s {
        Src::Set(x) => format!(" from {x}"),
        Src::Role(r) => format!(" in the role {r}"),
        Src::Handle { id, pin } => {
            format!(" from handle {id}{}", if *pin { " (pinned)" } else { "" })
        }
        Src::Selection {
            name,
            version: Some(v),
        } => format!(" from the selection {name} as of version {v}"),
        Src::Selection {
            name,
            version: None,
        } => format!(" from the selection {name}"),
        Src::Values(v) => format!(" listed in the upload {v}"),
    }
}

/// One set's sentence, in the order of rule 5.
pub fn set_sentence(name: &str, set: &Set) -> String {
    let mut s = format!("{name}: {}s", set.grain.name());
    if let Some(a) = &set.algebra {
        let op = match a.op {
            AlgOp::Union => "the union of",
            AlgOp::Intersect => "the intersection of",
            AlgOp::Except => "the difference of",
        };
        s.push_str(&format!(" {op} {}", a.sets.join(", ")));
        if let Some(t) = &a.tag {
            s.push_str(&format!(" tagged as {t}"));
        }
    }
    if let Some(g) = &set.group {
        s = format!(
            "{name}: groups of {} by {}",
            g.of,
            g.by.iter().map(clause_text).collect::<Vec<_>>().join(", ")
        );
    }
    if let Some(of) = &set.of {
        s.push_str(&format!(" of {of}"));
    }
    if let Some(src) = &set.from {
        s.push_str(&src_text(src));
    }
    for n in &set.near {
        let policy = match n.policy {
            Policy::Nearest => format!(
                "the nearest {} (tie {})",
                n.set,
                match n.tie {
                    Some(Tie::Later) => "later",
                    _ => "earlier",
                }
            ),
            Policy::First => format!("the first {}", n.set),
            Policy::Last => format!("the last {}", n.set),
            Policy::Best => format!(
                "the best {} by {}",
                n.set,
                n.order
                    .iter()
                    .map(|o| format!("{} {}", clause_text(&o.0), dir_text(o.1)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Policy::Any => format!("any {}", n.set),
        };
        s.push_str(&format!(
            "; {} as {} within {}{}{}",
            policy,
            n.as_,
            window_text(&n.window),
            n.on.as_ref()
                .map(|o| format!(" of {o}"))
                .unwrap_or_default(),
            match (n.strict, n.optional) {
                (true, true) => " (every day inside; when present)",
                (true, false) => " (every day inside)",
                (false, true) => " (when present)",
                (false, false) => "",
            }
        ));
    }
    for a in &set.attach {
        s.push_str(&format!(
            "; with {} from {}{}",
            a.as_,
            a.set,
            if a.optional { " when present" } else { "" }
        ));
    }
    for h in &set.has {
        let bound = match (&h.min, &h.max) {
            (Some(a), Some(b)) => format!("between {} and {}", int_text(a), int_text(b)),
            (Some(a), None) => format!("at least {}", int_text(a)),
            (None, Some(IntSpec::Literal(0))) => "no".to_string(),
            (None, Some(b)) => format!("at most {}", int_text(b)),
            (None, None) => "counted".to_string(),
        };
        s.push_str(&format!(
            "; {bound} {}{}{}",
            h.set,
            h.window
                .as_ref()
                .map(|w| format!(" within {}", window_text(w)))
                .unwrap_or_default(),
            h.as_
                .as_ref()
                .map(|a| format!(" as {a}"))
                .unwrap_or_default()
        ));
    }
    for (b, c) in &set.bind.0 {
        s.push_str(&format!("; {b} = {}", clause_text(c)));
    }
    if !set.where_.is_empty() {
        s.push_str(&format!(
            "; where {}",
            set.where_
                .iter()
                .map(clause_text)
                .collect::<Vec<_>>()
                .join(" and ")
        ));
    }
    if let Some(p) = &set.pick {
        s.push_str(&format!(
            "; {} per {} by {}",
            p.n.as_ref()
                .map(|n| format!("the first {}", int_text(n)))
                .unwrap_or_else(|| "one".into()),
            p.per.name(),
            p.by.iter()
                .map(|o| format!("{} {}", clause_text(&o.0), dir_text(o.1)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    s.push('.');
    s
}

/// Whether any clause of the document carries `strict: true`.
fn strict_anywhere(ask: &Ask) -> bool {
    fn in_clause(c: &Clause) -> bool {
        let mut all = Vec::new();
        c.walk(&mut all);
        all.iter().any(|x| {
            x.opts
                .get("strict")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
    }
    ask.sets.values().any(|s| {
        s.near.iter().any(|n| n.strict)
            || s.bind.0.iter().any(|(_, c)| in_clause(c))
            || s.where_.iter().any(in_clause)
    })
}

/// Describe a document, pure.
pub fn describe(
    ask: &Ask,
    order: &[String],
    names: &dyn Names,
    scope: &Scope,
    scheme: &Scheme,
) -> Description {
    let mut sets = Vec::new();
    let mut seen = BTreeSet::new();
    for name in order.iter().chain(ask.sets.keys()) {
        if name.contains("__") || !seen.insert(name.clone()) {
            continue;
        }
        if let Some(s) = ask.sets.get(name) {
            sets.push((name.clone(), set_sentence(name, s)));
        }
    }
    let mut conventions = vec![
        "windows are days with both ends inclusive; a month is 31 days and a year 366".to_string(),
        format!(
            "a date carries its precision; comparisons and windows read a coarse date as its interval, forgiving unless the clause says strict{}",
            if strict_anywhere(ask) { " (this document asks for strict on some clauses)" } else { " (no clause here does)" }
        ),
        "cohort membership is the open interval; there is no as-of date".to_string(),
        "stack sets exclude the excluded disposition; superseded rows and withdrawn picks are never read".to_string(),
        format!(
            "the session scheme is {} with a window of {} days",
            scheme.digest(),
            scheme.window_days
        ),
    ];
    if scheme.window_days > 0 && ask.sets.values().any(|s| !s.near.is_empty()) {
        conventions.push(format!(
            "a session's own window of {} days sits beside every near window on the same row",
            scheme.window_days
        ));
    }
    let mut levels: BTreeSet<String> = BTreeSet::new();
    for s in ask.sets.values() {
        for (_, c) in &s.bind.0 {
            let mut all = Vec::new();
            c.walk(&mut all);
            for x in all {
                if x.op == "derived"
                    && x.ref_name() == Some("signature")
                    && let Some(l) = x.opts.get("level").and_then(Value::as_str)
                {
                    levels.insert(l.to_string());
                }
            }
        }
    }
    for l in levels {
        if let Some(spec) = names.level_spec(&l) {
            conventions.push(format!(
                "the level {l} compares {} exactly{}",
                spec.exact.join(", "),
                if spec.rounded.is_empty() {
                    String::new()
                } else {
                    format!(
                        " and rounds {}",
                        spec.rounded
                            .iter()
                            .map(|(k, v)| format!("{k} to {v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            ));
        }
    }
    let mut denominators = Vec::new();
    let mut mechanisms = Vec::new();
    for (name, s) in &ask.sets {
        for h in &s.has {
            if let Some(a) = &h.as_ {
                denominators.push((a.clone(), h.set.clone()));
            }
        }
        for n in &s.near {
            mechanisms.push((
                name.clone(),
                n.as_.clone(),
                format!("{:?} within {}", n.policy, window_text(&n.window)).to_lowercase(),
            ));
        }
        for a in &s.attach {
            let how = ask
                .sets
                .get(&a.set)
                .and_then(|p| p.pick.as_ref())
                .map(|p| {
                    format!(
                        "one per {} by {}",
                        p.per.name(),
                        p.by.iter()
                            .map(|o| format!("{} {}", clause_text(&o.0), dir_text(o.1)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .unwrap_or_else(|| "the set's one row per key".into());
            mechanisms.push((name.clone(), a.as_.clone(), how));
        }
    }
    for m in &ask.out.measures {
        for (kind, spec) in &m.0 {
            if kind == "share"
                && let (Some(of), Some(over)) = (
                    spec.get("of").and_then(Value::as_str),
                    spec.get("over").and_then(Value::as_str),
                )
            {
                denominators.push((format!("share.{of}"), over.to_string()));
            }
        }
    }
    let mut classes: Vec<String> = scope
        .classes
        .iter()
        .map(|c| format!("{c:?}").to_lowercase())
        .collect();
    classes.sort();
    let disclosure = format!(
        "{}{}",
        if scope.federated {
            "federated"
        } else {
            "local"
        },
        if classes.is_empty() {
            String::new()
        } else {
            format!(", projecting {}", classes.join(" and "))
        }
    );
    let answer = format!(
        "the answer is {} at the {:?} level{}",
        ask.out.set,
        ask.out.level,
        if ask.out.columns.is_empty() {
            String::new()
        } else {
            format!(
                " with {}",
                ask.out
                    .columns
                    .iter()
                    .map(clause_text)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    )
    .to_lowercase();
    Description {
        sets,
        conventions,
        denominators,
        mechanisms,
        disclosure,
        answer,
    }
}

/// Wave 4c §6.4: the six silent decisions of an answer, and two more, in
/// the same object as the number. The corpus corrected the assistant
/// twenty-two times for a decision the question never stated; every one
/// of them is a field here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Declaration {
    pub grain: String,
    pub session_scheme: SchemeNamed,
    pub membership: String,
    pub key_namespace: String,
    pub pick_rule: String,
    pub denominator: String,
    pub disclosure: String,
    pub truncated: bool,
    /// The timezone the engine read the dates under, and the day a week
    /// starts on (Wave 5 section 12.6): the registry's, never the browser's.
    pub timezone: String,
    pub week_start: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchemeNamed {
    pub name: String,
    pub digest: String,
}

/// The declaration of a document's answer, pure over the desugared
/// document and its description.
pub fn declaration(
    ask: &Ask,
    description: &Description,
    scheme_digest: &str,
    truncated: bool,
    locale: &crate::hash::Locale,
) -> Declaration {
    let out_set = ask.sets.get(&ask.out.set);
    let grain = out_set
        .map(|s| s.grain.name().to_string())
        .unwrap_or_default();
    let name = match &ask.scheme {
        None => "default".to_string(),
        Some(SchemeRef::Name(n)) => n.clone(),
        Some(SchemeRef::Inline(_)) => "inline".to_string(),
    };
    let membership = match out_set {
        Some(set) if !set.has.is_empty() => set
            .has
            .iter()
            .map(|h| {
                let min = h.min.as_ref().map(int_text).unwrap_or_else(|| "1".into());
                match &h.max {
                    Some(max) => format!(
                        "in {} at least {min} and at most {} times",
                        h.set,
                        int_text(max)
                    ),
                    None => format!("in {} at least {min} times", h.set),
                }
            })
            .collect::<Vec<_>>()
            .join("; and "),
        Some(set) if set.algebra.is_some() => {
            let a = set.algebra.as_ref().expect("checked");
            format!(
                "{} of {}",
                format!("{:?}", a.op).to_lowercase(),
                a.sets.join(", ")
            )
        }
        Some(set) => match (&set.of, &set.from) {
            (Some(of), _) => format!("every member reached through {of}"),
            (None, Some(_)) => "every member of the source it starts from".to_string(),
            (None, None) => "the whole grain".to_string(),
        },
        None => String::new(),
    };
    let key_namespace = if ask.out.identifiers.is_empty() {
        "the registry's pseudonymous key".to_string()
    } else {
        format!(
            "{}, projected raw and audited",
            ask.out.identifiers.join(", ")
        )
    };
    let mut picks: Vec<String> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut stack: Vec<&str> = vec![ask.out.set.as_str()];
    while let Some(name) = stack.pop() {
        if !seen.insert(name.to_string()) {
            continue;
        }
        if let Some(set) = ask.sets.get(name) {
            if let Some(p) = &set.pick {
                let by =
                    p.by.iter()
                        .map(|o| format!("{} {}", clause_text(&o.0), dir_name(o.1)))
                        .collect::<Vec<_>>()
                        .join(", then ");
                picks.push(format!(
                    "{name}: one per {} by {by}{}",
                    p.per.name(),
                    p.ties
                        .as_ref()
                        .map(|t| format!(", ties {t:?}").to_lowercase())
                        .unwrap_or_default()
                ));
            }
            stack.extend(set.reads());
        }
    }
    let pick_rule = if picks.is_empty() {
        "none: every row of the set".to_string()
    } else {
        picks.join("; ")
    };
    let denominator = if description.denominators.is_empty() {
        "none: a count over the set".to_string()
    } else {
        description
            .denominators
            .iter()
            .map(|(n, s)| format!("{n} over {s}"))
            .collect::<Vec<_>>()
            .join("; ")
    };
    Declaration {
        grain,
        session_scheme: SchemeNamed {
            name,
            digest: scheme_digest.to_string(),
        },
        membership,
        key_namespace,
        pick_rule,
        denominator,
        disclosure: description.disclosure.clone(),
        truncated,
        timezone: locale.timezone.clone(),
        week_start: locale.week_start.clone(),
    }
}

/// Wave 4c §6.4: one node of a document, described so that a chip label,
/// a model's tool output and an audit line are the same string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub display_name: String,
    pub long_display_name: String,
    pub flags: serde_json::Value,
}

/// A node is a set, or one entry of a set's `where`, `has`, `near`,
/// `attach`, its `pick`, or one of the answer's `columns`.
pub fn node(ask: &Ask, set: &str, part: &str, index: usize) -> Option<Node> {
    let s = ask.sets.get(set)?;
    let flags = |op: Option<&str>| serde_json::json!({"set": set, "part": part, "index": index, "op": op, "grain": s.grain.name()});
    let sentence = set_sentence(set, s);
    Some(match part {
        "set" => Node {
            display_name: set.to_string(),
            long_display_name: sentence,
            flags: flags(None),
        },
        "where" => {
            let c = s.where_.get(index)?;
            let text = clause_text(c);
            Node {
                display_name: text.clone(),
                long_display_name: format!("{set}: where {text}"),
                flags: flags(Some(c.op.as_str())),
            }
        }
        "has" => {
            let h = s.has.get(index)?;
            let text = format!(
                "has {} at least {}",
                h.set,
                h.min.as_ref().map(int_text).unwrap_or_else(|| "1".into())
            );
            Node {
                display_name: text.clone(),
                long_display_name: format!("{set}: {text}"),
                flags: flags(Some("has")),
            }
        }
        "near" => {
            let n = s.near.get(index)?;
            let text = format!("near {} as {}", n.set, n.as_);
            Node {
                display_name: text.clone(),
                long_display_name: format!("{set}: {text}"),
                flags: flags(Some("near")),
            }
        }
        "attach" => {
            let a = s.attach.get(index)?;
            let text = format!("attach {} as {}", a.set, a.as_);
            Node {
                display_name: text.clone(),
                long_display_name: format!("{set}: {text}"),
                flags: flags(Some("attach")),
            }
        }
        "pick" => {
            let p = s.pick.as_ref()?;
            let by =
                p.by.iter()
                    .map(|o| format!("{} {}", clause_text(&o.0), dir_name(o.1)))
                    .collect::<Vec<_>>()
                    .join(", then ");
            let text = format!("pick one per {} by {by}", p.per.name());
            Node {
                display_name: text.clone(),
                long_display_name: format!("{set}: {text}"),
                flags: flags(Some("pick")),
            }
        }
        "columns" => {
            if ask.out.set != set {
                return None;
            }
            let c = ask.out.columns.get(index)?;
            let text = clause_text(c);
            Node {
                display_name: text.clone(),
                long_display_name: format!("{set}: column {text}"),
                flags: flags(Some(c.op.as_str())),
            }
        }
        _ => return None,
    })
}

fn dir_name(d: Dir) -> &'static str {
    match d {
        Dir::Asc => "asc",
        Dir::Desc => "desc",
    }
}
