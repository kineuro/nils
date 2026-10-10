// SPDX-License-Identifier: AGPL-3.0-only

//! A pack document rewritten where its value changed, and only there
//! (record 56 §5.5: a patched pack is written as a reviewable diff).
//!
//! The pack's files are block YAML with comments that carry their
//! provenance, and a document written again whole from its value would lose
//! every one of them for one word. So a changed document is rewritten key by
//! key: where a mapping's key holds a block mapping on both sides, the
//! rewrite goes inside it; where a key's value changed, the lines of that key
//! are written again, keeping the comments above it; a key added is written
//! after the last of its mapping; one removed goes with its lines. Anything
//! else (a key order that moved, a flow mapping changed inside, a document
//! that is not a mapping) is not attempted. The result is read back and must
//! equal the value wanted, or there is no rewrite and the caller writes the
//! document whole.

use serde_json::{Map, Value};

/// `source`, whose value is `old`, rewritten to hold `new`; None where the
/// rewrite cannot be done exactly.
pub fn rewrite(source: &str, old: &Value, new: &Value) -> Option<String> {
    let (Value::Object(o), Value::Object(n)) = (old, new) else {
        return None;
    };
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut edits: Vec<Edit> = Vec::new();
    let indent = lines.iter().find_map(|l| content_indent(l)).unwrap_or(0);
    diff(&lines, 0, lines.len(), indent, o, n, &mut edits)?;
    edits.sort_by_key(|e| std::cmp::Reverse((e.from, e.to)));
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    for e in edits {
        out.splice(e.from..e.to, e.with.lines().map(|l| format!("{l}\n")));
    }
    let mut text: String = out.concat();
    if !source.ends_with('\n') && text.ends_with('\n') {
        text.pop();
    }
    let back: Value = serde_saphyr::from_str(&text).ok()?;
    (back == *new).then_some(text)
}

/// Lines `from..to` of the source replaced by `with` (lines, each ending
/// with a newline; empty to delete).
struct Edit {
    from: usize,
    to: usize,
    with: String,
}

/// The indentation of a line that holds YAML, or None for a blank line or
/// a comment.
fn content_indent(line: &str) -> Option<usize> {
    let t = line.trim_start_matches(' ');
    if t.trim().is_empty() || t.starts_with('#') {
        return None;
    }
    Some(line.len() - t.len())
}

/// The key a line at this indentation declares, unquoted, and what follows
/// its colon.
fn key_of(line: &str, indent: usize) -> Option<(String, String)> {
    if content_indent(line)? != indent {
        return None;
    }
    let t = line[indent..].trim_end_matches(['\n', '\r']);
    if t.starts_with("- ") || t == "-" {
        return None;
    }
    let (key, rest) = if let Some(q) = t.strip_prefix('\'') {
        let end = q.find("':")?;
        (q[..end].replace("''", "'"), &q[end + 2..])
    } else if let Some(q) = t.strip_prefix('"') {
        let end = q.find("\":")?;
        (q[..end].to_string(), &q[end + 2..])
    } else {
        let at = t.find(':')?;
        let after = &t[at + 1..];
        if !(after.is_empty() || after.starts_with(' ')) {
            return None;
        }
        (t[..at].trim_end().to_string(), after)
    };
    Some((key, rest.trim().to_string()))
}

/// Where each key of the mapping at `indent` inside lines `from..to` sits:
/// (key, first line, the line after its last content line, what follows
/// its colon). Comments and blank lines between two keys belong to the
/// second, the one they introduce.
fn keys(
    lines: &[&str],
    from: usize,
    to: usize,
    indent: usize,
) -> Vec<(String, usize, usize, String)> {
    let mut starts: Vec<(String, usize, String)> = Vec::new();
    for (i, line) in lines.iter().enumerate().take(to).skip(from) {
        if let Some((k, rest)) = key_of(line, indent) {
            starts.push((k, i, rest));
        }
    }
    let mut out = Vec::new();
    for (n, (k, at, rest)) in starts.iter().enumerate() {
        let next = starts.get(n + 1).map(|s| s.1).unwrap_or(to);
        let mut end = next;
        while end > at + 1 && content_indent(lines[end - 1]).is_none() {
            end -= 1;
        }
        out.push((k.clone(), *at, end, rest.clone()));
    }
    out
}

fn diff(
    lines: &[&str],
    from: usize,
    to: usize,
    indent: usize,
    old: &Map<String, Value>,
    new: &Map<String, Value>,
    edits: &mut Vec<Edit>,
) -> Option<()> {
    let spans = keys(lines, from, to, indent);
    // every key of the old mapping is found where it is written, once
    if spans.len() != old.len() || !old.keys().all(|k| spans.iter().any(|s| &s.0 == k)) {
        return None;
    }
    // and the keys both hold keep their order
    let kept_old: Vec<&String> = old.keys().filter(|k| new.contains_key(*k)).collect();
    let kept_new: Vec<&String> = new.keys().filter(|k| old.contains_key(*k)).collect();
    if kept_old != kept_new {
        return None;
    }
    for (k, at, end, rest) in &spans {
        match new.get(k) {
            None => edits.push(Edit {
                from: *at,
                to: *end,
                with: String::new(),
            }),
            Some(nv) if nv != &old[k] => {
                let ov = &old[k];
                if let (Value::Object(om), Value::Object(nm)) = (ov, nv)
                    && (rest.is_empty() || rest.starts_with('#'))
                    && let Some(inner) = lines[at + 1..*end].iter().find_map(|l| content_indent(l))
                    && inner > indent
                {
                    diff(lines, at + 1, *end, inner, om, nm, edits)?;
                    continue;
                }
                let flow = rest.starts_with('[') || rest.starts_with('{');
                edits.push(Edit {
                    from: *at,
                    to: *end,
                    with: render(k, nv, indent, flow)?,
                });
            }
            Some(_) => {}
        }
    }
    // the keys added, after the mapping's last
    let added: Vec<(&String, &Value)> = new.iter().filter(|(k, _)| !old.contains_key(*k)).collect();
    if !added.is_empty() {
        let at = spans.last().map(|s| s.2).unwrap_or(to);
        let mut text = String::new();
        for (k, v) in added {
            text.push_str(&render(k, v, indent, false)?);
        }
        edits.push(Edit {
            from: at,
            to: at,
            with: text,
        });
    }
    Some(())
}

/// `key: value` at `indent`, ending with a newline: a scalar or a short
/// list of scalars on the line, anything else in flow style where it was
/// written so, else as a block below the key.
fn render(key: &str, v: &Value, indent: usize, flow: bool) -> Option<String> {
    let pad = " ".repeat(indent);
    let k = scalar_text(key);
    let simple_list =
        matches!(v, Value::Array(a) if a.iter().all(|x| !x.is_object() && !x.is_array()));
    if !v.is_object() && !v.is_array() || simple_list || flow {
        return Some(format!("{pad}{k}: {}\n", flow_text(v)));
    }
    let body = serde_saphyr::to_string(v).ok()?;
    let mut out = format!("{pad}{k}:\n");
    for line in body.lines() {
        if line.is_empty() {
            continue;
        }
        out.push_str(&format!("{pad}  {line}\n"));
    }
    Some(out)
}

/// A value in YAML's flow style.
fn flow_text(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => scalar_text(s),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(flow_text).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", scalar_text(k), flow_text(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// A string as a YAML scalar: plain where that reads back as the same
/// string, single-quoted otherwise.
fn scalar_text(s: &str) -> String {
    let reserved = [
        "true", "false", "null", "yes", "no", "on", "off", "y", "n", "~",
    ];
    // a version (two dots or more between digits) reads back as the word
    let version = s.split('.').count() > 2
        && s.split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let plain = version
        || !s.is_empty()
            && s.trim() == s
            && s.chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ' '))
            && !s.contains(" #")
            && !reserved.contains(&s.to_ascii_lowercase().as_str())
            && s.parse::<f64>().is_err();
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn value(text: &str) -> Value {
        serde_saphyr::from_str(text).unwrap()
    }

    const DOC: &str = "\
# SPDX-License-Identifier: AGPL-3.0-only
#
# a pack

pack: t
version: 1.0.0

# the order, with a comment
order: [a, b, c]

review:
  # why the threshold
  low_confidence: 0.7
  by_model: [body_part]
buckets:
  # the words
  words:
    ['x', ' -k',
     'y']
  other: [z]
";

    #[test]
    fn a_changed_list_is_written_again_and_every_comment_stays() {
        let old = value(DOC);
        let mut new = old.clone();
        new["order"] = json!(["b", "a", "c"]);
        new["review"]["by_model"] = json!(["body_part", "post_contrast"]);
        new["buckets"]["words"] = json!(["x", " -k", "y", "mdc"]);
        let text = rewrite(DOC, &old, &new).unwrap();
        assert_eq!(value(&text), new);
        for comment in [
            "# the order, with a comment",
            "# why the threshold",
            "# the words",
            "# a pack",
        ] {
            assert!(text.contains(comment), "{comment} kept:\n{text}");
        }
        assert!(text.contains("order: [b, a, c]"), "{text}");
        assert!(
            text.contains("  by_model: [body_part, post_contrast]"),
            "{text}"
        );
        assert!(text.contains("' -k'"), "{text}");
    }

    #[test]
    fn a_key_added_comes_after_its_mapping_and_one_removed_goes() {
        let old = value(DOC);
        let mut new = old.clone();
        new["review"]["silent_when"] =
            json!({"any": [{"axis": "directory_type", "is": "excluded"}]});
        new["buckets"].as_object_mut().unwrap().remove("other");
        let text = rewrite(DOC, &old, &new).unwrap();
        assert_eq!(value(&text), new);
        assert!(!text.contains("other:"), "{text}");
        assert!(text.contains("# why the threshold"));
    }

    #[test]
    fn a_reordered_mapping_is_not_attempted() {
        let old = value("a: 1\nb: 2\n");
        let new = json!({"b": 2, "a": 3});
        assert!(rewrite("a: 1\nb: 2\n", &old, &new).is_none());
    }

    #[test]
    fn a_scalar_is_plain_only_where_it_reads_back_the_same() {
        for s in [
            "T1w", "3D", "yes", "0.5", " -k", "a: b", "it's", "", "x #y", "'q'", "1.0.2", "1.0",
        ] {
            let text = format!("k: {}\n", scalar_text(s));
            let back: Value = serde_saphyr::from_str(&text).unwrap();
            assert_eq!(back["k"], json!(s), "{s:?} as {text}");
        }
    }
}
