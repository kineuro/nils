// SPDX-License-Identifier: AGPL-3.0-only

//! The engines a pack works with (pack contract 10): a range of engine
//! versions in the manifest's `engine` key, beside its contract.
//!
//! The contract is the key every engine checks: an engine that implements a
//! lower one refuses the pack, and engines from long before this file do so.
//! A range says what a contract cannot: that a rules release relies on an
//! engine fix that changed no contract (`>=1.0.0-alpha.80`), or that it stops
//! before a later engine (`<2.0.0`). An engine reads the key from contract 10
//! on, and the loader refuses it in a pack that declares less, so an engine
//! before 10, which would load the pack without looking at the range, refuses
//! it by its contract instead.
//!
//! Versions are ordered the way the update path orders releases: the numbers
//! left to right, then a release after the pre-releases that led to it, and a
//! development build (`1.0.0-alpha.80.dev.2`) after the release it was built
//! from and before the next one. So `<2.0.0` admits `2.0.0-alpha.1`; a range
//! that must not writes `<2.0.0-alpha`.

use std::cmp::Ordering;
use std::fmt;

/// This engine's version. The pack crate is versioned with the engine, as
/// every crate of the workspace is, so its own version is the engine's.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A version as numbers and, when it is a pre-release, what follows the
/// hyphen. A leading `v` is not part of it.
fn parts(v: &str) -> (Vec<u64>, Option<String>) {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre.to_string())),
        None => (v, None),
    };
    (
        core.split('.').map(|p| p.parse().unwrap_or(0)).collect(),
        pre,
    )
}

/// One pre-release identifier against another: numbers as numbers, the rest
/// as text, and the longer list wins when it agrees so far (`alpha.2` over
/// `alpha`).
fn pre_order(a: &str, b: &str) -> Ordering {
    let (mut left, mut right) = (a.split('.'), b.split('.'));
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    _ => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// One version against another: the numbers left to right, and then a
/// release ahead of the pre-releases that led to it.
pub fn order(a: &str, b: &str) -> Ordering {
    let (x, x_pre) = parts(a);
    let (y, y_pre) = parts(b);
    for i in 0..x.len().max(y.len()) {
        let (p, q) = (
            x.get(i).copied().unwrap_or(0),
            y.get(i).copied().unwrap_or(0),
        );
        if p != q {
            return p.cmp(&q);
        }
    }
    match (x_pre, y_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(p), Some(q)) => pre_order(&p, &q),
    }
}

/// Whether `candidate` is a later version than `than`.
pub fn newer(candidate: &str, than: &str) -> bool {
    order(candidate, than) == Ordering::Greater
}

/// How one bound compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Ge,
    Gt,
    Le,
    Lt,
    Eq,
}

impl Op {
    fn holds(self, order: Ordering) -> bool {
        match self {
            Op::Ge => order != Ordering::Less,
            Op::Gt => order == Ordering::Greater,
            Op::Le => order != Ordering::Greater,
            Op::Lt => order == Ordering::Less,
            Op::Eq => order == Ordering::Equal,
        }
    }
}

/// The engines a pack works with: every bound must hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    text: String,
    bounds: Vec<(Op, String)>,
}

/// Whether a version is spelled as an engine's is: three numbers, and after a
/// hyphen dot-separated words of letters, digits and hyphens.
fn spelled(v: &str) -> bool {
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (v, None),
    };
    let numbers: Vec<&str> = core.split('.').collect();
    numbers.len() == 3
        && numbers
            .iter()
            .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        && pre.is_none_or(|p| {
            p.split('.')
                .all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        })
}

impl Range {
    /// A range as a manifest writes it: bounds separated by commas, each an
    /// operator (`>=`, `>`, `<=`, `<`, `=`) and a version, or a version alone
    /// for that version only. `">=1.0.0-alpha.80, <2.0.0"`.
    pub fn parse(text: &str) -> Result<Range, String> {
        let mut bounds = Vec::new();
        for bound in text.split(',') {
            let bound = bound.trim();
            if bound.is_empty() {
                return Err(format!(
                    "{text:?} is not a range of engine versions: a bound is empty"
                ));
            }
            let (op, rest) = [
                (">=", Op::Ge),
                ("<=", Op::Le),
                (">", Op::Gt),
                ("<", Op::Lt),
                ("=", Op::Eq),
            ]
            .iter()
            .find_map(|(sign, op)| bound.strip_prefix(sign).map(|rest| (*op, rest)))
            .unwrap_or((Op::Eq, bound));
            let version = rest.trim().trim_start_matches('v');
            if !spelled(version) {
                return Err(format!(
                    "{text:?} is not a range of engine versions: {bound:?} names no version \
                     (three numbers, as in >=1.0.0-alpha.80)"
                ));
            }
            bounds.push((op, version.to_string()));
        }
        Ok(Range {
            text: text.trim().to_string(),
            bounds,
        })
    }

    /// Whether an engine of this version is in the range.
    pub fn admits(&self, version: &str) -> bool {
        self.bounds
            .iter()
            .all(|(op, bound)| op.holds(order(version, bound)))
    }

    /// The range as the manifest wrote it.
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_is_later_by_its_numbers_then_by_its_pre_release() {
        assert!(newer("1.0.1", "1.0.0"));
        assert!(newer("1.10.0", "1.2.0"));
        assert!(newer("1.0.0", "1.0.0-alpha.1"));
        assert!(newer("1.0.0-alpha.10", "1.0.0-alpha.2"));
        assert!(newer("1.0.0-beta.1", "1.0.0-alpha.9"));
        // a development build sits after its release and before the next
        assert!(newer("1.0.0-alpha.80.dev.1", "1.0.0-alpha.80"));
        assert!(newer("1.0.0-alpha.81", "1.0.0-alpha.80.dev.9"));
        assert_eq!(order("v1.0.0", "1.0.0"), Ordering::Equal);
        assert!(!newer("1.0.0", "1.0.0"));
        assert!(!newer("1.2.0", "1.10.0"), "numbers, not text");
    }

    #[test]
    fn a_range_admits_the_engines_between_its_bounds() {
        let r = Range::parse(">=1.0.0-alpha.80, <2.0.0").unwrap();
        assert_eq!(r.text(), ">=1.0.0-alpha.80, <2.0.0");
        for inside in [
            "1.0.0-alpha.80",
            "1.0.0-alpha.80.dev.3",
            "1.0.0-alpha.120",
            "1.0.0",
            "1.4.2",
            "2.0.0-alpha.1",
        ] {
            assert!(r.admits(inside), "{inside}");
        }
        for outside in ["1.0.0-alpha.79", "1.0.0-alpha.79.dev.4", "2.0.0", "3.1.0"] {
            assert!(!r.admits(outside), "{outside}");
        }
        assert!(
            !Range::parse("<2.0.0-alpha")
                .unwrap()
                .admits("2.0.0-alpha.1")
        );
        let one = Range::parse("1.0.0-alpha.80").unwrap();
        assert!(one.admits("1.0.0-alpha.80") && !one.admits("1.0.0-alpha.81"));
        assert!(Range::parse("> 1.0.0, <= v1.2.0").unwrap().admits("1.2.0"));
    }

    #[test]
    fn a_range_that_names_no_version_is_refused_with_its_reason() {
        for bad in [
            "",
            ">=",
            ">=1.0",
            "1.0.0,",
            ">=one.0.0",
            "~1.0.0",
            ">=1.0.0-",
        ] {
            let e = Range::parse(bad).unwrap_err();
            assert!(
                e.contains("is not a range of engine versions"),
                "{bad}: {e}"
            );
        }
    }

    #[test]
    fn this_engine_has_a_version_a_range_can_name() {
        assert!(spelled(ENGINE_VERSION), "{ENGINE_VERSION}");
        assert!(
            Range::parse(&format!(">={ENGINE_VERSION}"))
                .unwrap()
                .admits(ENGINE_VERSION)
        );
    }
}
