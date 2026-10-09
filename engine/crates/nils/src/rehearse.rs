// SPDX-License-Identifier: AGPL-3.0-only

//! Record 56 §5.4 and §5.5 (2026-10-09): a rule change rehearsed over the
//! registry before anyone decides. Nima: "the change in rule should show how
//! it affect the sorting (like how many stack and to what)".
//!
//! A patch of typed operations (`nils_pack::patch`), or an overlay document
//! read as the word edits it is, is applied to a copy of the pack and checked
//! by the pack's own loader; the registry in scope is sorted both ways
//! without a row written (`nils_classify::effect`); and the answer says what
//! moves per axis and from what to what, the names and the main-scan picks
//! that change, the Review questions that appear and go, what it does to
//! the answers people settled, and what it would ship as. `nils pack
//! rehearse` prints it; `POST /api/packs/{name}/rehearse` answers it.

use std::collections::BTreeMap;
use std::path::Path;

use nils_classify::effect::{self, NameFacts, Settings};
use nils_pack::Pack;
use nils_pack::patch::Patch;
use nils_registry::Registry;
use serde_json::Value;

/// A patch from a document: a patch (`patch: 1`), an overlay document
/// (pack contract 5), or a door's body naming one of them or a list of
/// `operations` for `pack`.
pub(crate) fn patch_of(name: &str, pack: Option<&str>, doc: &Value) -> Result<Patch, String> {
    let overlay = |o: &Value| -> Result<Patch, String> {
        let o = nils_pack::Overlay::parse(name, &o.to_string()).map_err(|e| e.to_string())?;
        Ok(Patch::from_overlay(&o))
    };
    match (doc.get("patch"), doc.get("overlay"), doc.get("operations")) {
        // {patch: {...}} or {overlay: {...}}
        (Some(p), _, _) if p.is_object() => Patch::of_value(name, p).map_err(|e| e.to_string()),
        (_, Some(o), _) if o.is_object() => overlay(o),
        // the patch itself, or the overlay document itself
        (Some(_), _, _) => Patch::of_value(name, doc).map_err(|e| e.to_string()),
        (_, Some(_), _) => overlay(doc),
        // a list of operations for the pack the door names
        (None, None, Some(ops)) => {
            let pack = pack
                .map(str::to_string)
                .or_else(|| doc.get("pack").and_then(Value::as_str).map(str::to_string))
                .ok_or("operations name no pack")?;
            let mut whole = serde_json::json!({
                "patch": nils_pack::patch::FORMAT, "pack": pack, "operations": ops,
            });
            for k in ["version", "ships", "reason", "evidence", "cases"] {
                if let Some(v) = doc.get(k) {
                    whole[k] = v.clone();
                }
            }
            Patch::of_value(name, &whole).map_err(|e| e.to_string())
        }
        _ => Err("the body names operations, a patch or an overlay".into()),
    }
}

/// A patch from a file: a patch, or an overlay document.
pub(crate) fn patch_file(path: &Path) -> Result<Patch, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.display().to_string();
    let value: Value =
        serde_saphyr::from_str(&text).map_err(|e| format!("{name}: not YAML: {e}"))?;
    if value.get("overlay").is_some_and(Value::is_string) {
        let o = nils_pack::Overlay::parse(&name, &text).map_err(|e| e.to_string())?;
        return Ok(Patch::from_overlay(&o));
    }
    Patch::parse(&name, &text).map_err(|e| e.to_string())
}

/// What a stack is called, by the release's grammar.
fn names(
    pack: &Pack,
    axes: &BTreeMap<String, String>,
    facts: &NameFacts,
) -> (String, Option<String>) {
    let n = nils_release::run::name_of(Some(pack), axes, facts);
    (n.name, n.bids)
}

/// The effect report of `patch` on the pack in `dir` over the registry.
pub(crate) fn report(
    registry: &mut Registry,
    dir: &Path,
    patch: &Patch,
    settings: &Settings,
) -> Result<Value, effect::Error> {
    let base = nils_pack::load(dir, None)?;
    if base.name != patch.pack {
        return Err(effect::Error::Refused(format!(
            "the patch amends {}, and the pack here is {}",
            patch.pack, base.name
        )));
    }
    effect::run(registry.store(), dir, &base, patch, settings, &names)
}

/// Record 55 K7: each transition's scanners, by station name, answered as
/// their shapes, as the value sampler shows a quasi-identifying field.
pub(crate) fn shape_scanners(doc: &mut Value) {
    for a in doc["axes"].as_array_mut().into_iter().flatten() {
        for t in a["transitions"].as_array_mut().into_iter().flatten() {
            let Some(m) = t["scanners"].as_object() else {
                continue;
            };
            let mut shaped = serde_json::Map::new();
            for (k, v) in m {
                let key = if k == "(none)" {
                    k.clone()
                } else {
                    crate::scans::shape(k)
                };
                let n =
                    shaped.get(&key).and_then(Value::as_i64).unwrap_or(0) + v.as_i64().unwrap_or(0);
                shaped.insert(key, Value::from(n));
            }
            t["scanners"] = Value::Object(shaped);
        }
    }
}

fn count(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

fn tally(m: &Value) -> String {
    let mut rows: Vec<(String, i64)> = m
        .as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), count(v))).collect())
        .unwrap_or_default();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    rows.iter()
        .take(4)
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
        + if rows.len() > 4 { ", ..." } else { "" }
}

/// The report as text, for the command line.
pub(crate) fn text(doc: &Value) -> String {
    let mut out = String::new();
    let ops = doc["patch"]["operations"].as_array().map_or(0, Vec::len);
    out.push_str(&format!(
        "rehearsal of {ops} operation(s) on {}, over {}: {} stacks ({} sealed left out)\n",
        doc["pack"].as_str().unwrap_or(""),
        doc["scope"]["replayed"].as_str().unwrap_or(""),
        count(&doc["scope"]["stacks"]),
        count(&doc["scope"]["sealed_left_out"]),
    ));
    let ships = &doc["ships"];
    if ships["as"] == "rules release" {
        out.push_str(&format!(
            "ships as a rules release: {} {} (from {})\n",
            ships["pack"].as_str().unwrap_or(""),
            ships["version"].as_str().unwrap_or(""),
            ships["from"].as_str().unwrap_or("")
        ));
    } else {
        out.push_str(&format!(
            "ships as an overlay on {} {}, scoped to {}{}\n",
            ships["pack"].as_str().unwrap_or(""),
            ships["on"].as_str().unwrap_or(""),
            ships["scopes"]
                .as_array()
                .map(|a| a
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
            if ships["with_pack_edits"] == true {
                ", with pack edits beside it"
            } else {
                ""
            }
        ));
    }
    out.push_str("\noperations\n");
    for a in doc["patch"]["applied"].as_array().into_iter().flatten() {
        let changes: Vec<&str> = a["changes"]
            .as_array()
            .map(|c| c.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let by = doc["scope"]["by_operation"]
            .as_array()
            .and_then(|b| b.iter().find(|x| x["at"] == a["at"]))
            .map(|x| count(&x["stacks"]))
            .unwrap_or(0);
        out.push_str(&format!(
            "  {} {:<13} {} ({by} stacks): {}\n",
            count(&a["at"]),
            a["op"].as_str().unwrap_or(""),
            a["scope"].as_str().unwrap_or(""),
            changes.join("; ")
        ));
    }
    match doc["patch"]["cases"]["failures"].as_str() {
        None => out.push_str("  the pack's own cases and the patch's hold\n"),
        Some(f) => {
            out.push_str("  cases that no longer hold:\n");
            for line in f.lines().take(12) {
                out.push_str(&format!("    {}\n", line.trim()));
            }
        }
    }
    out.push_str(&format!(
        "\nmoved: {} stacks\n",
        count(&doc["moved"]["stacks"])
    ));
    for a in doc["axes"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  {}: {} moved{}\n",
            a["axis"].as_str().unwrap_or(""),
            count(&a["moved"]),
            match count(&a["held_by_decision"]) {
                0 => String::new(),
                n => format!(", {n} more held by a decision"),
            }
        ));
        for t in a["transitions"].as_array().into_iter().flatten().take(12) {
            let from = t["from"]
                .as_str()
                .filter(|v| !v.is_empty())
                .unwrap_or("(none)");
            let to = t["to"]
                .as_str()
                .filter(|v| !v.is_empty())
                .unwrap_or("(none)");
            let examples: Vec<String> = t["examples"]
                .as_array()
                .map(|e| e.iter().map(|x| x["stack"].to_string()).collect())
                .unwrap_or_default();
            out.push_str(&format!(
                "    {from} -> {to}: {}  [datasets {}; sites {}; makes {}; scanners {}; e.g. stacks {}]\n",
                count(&t["stacks"]),
                tally(&t["datasets"]),
                tally(&t["sites"]),
                tally(&t["makes"]),
                tally(&t["scanners"]),
                examples.join(", ")
            ));
        }
    }
    out.push_str(&format!(
        "\nnames: {} descriptive and {} BIDS names change\n",
        count(&doc["names"]["descriptive"]["changed"]),
        count(&doc["names"]["bids"]["changed"])
    ));
    for e in doc["names"]["examples"]
        .as_array()
        .into_iter()
        .flatten()
        .take(8)
    {
        let name = |side: &str| e[side]["name"].as_str().unwrap_or("").to_string();
        let bids = |side: &str| e[side]["bids"].as_str().unwrap_or("(none)").to_string();
        out.push_str(&format!(
            "  stack {}: {} -> {}  |  {} -> {}\n",
            e["stack"],
            name("before"),
            name("after"),
            bids("before"),
            bids("after")
        ));
    }
    let picks = &doc["picks"];
    match picks["why"].as_str() {
        Some(why) => out.push_str(&format!("\npicks: not replayed: {why}\n")),
        None => {
            out.push_str(&format!(
                "\npicks: {} of {} occasions change\n",
                count(&picks["changed"]),
                count(&picks["occasions"])
            ));
            for m in picks["models"].as_array().into_iter().flatten() {
                for (role, r) in m["by_role"].as_object().into_iter().flatten() {
                    out.push_str(&format!(
                        "  {} {role}: {} changed, {} appear, {} go, {} held by a person's pick; borders {} appear, {} go\n",
                        m["model"].as_str().unwrap_or(""),
                        count(&r["changed"]),
                        count(&r["appear"]),
                        count(&r["disappear"]),
                        count(&r["held_by_a_person"]),
                        count(&r["borders"]["appear"]),
                        count(&r["borders"]["disappear"]),
                    ));
                }
            }
        }
    }
    let review = &doc["review"];
    out.push_str(&format!(
        "\nreview: stacks asked {} -> {}\n",
        count(&review["stacks_asked"]["before"]),
        count(&review["stacks_asked"]["after"])
    ));
    for k in review["by_kind"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  {}: +{} -{}\n",
            k["kind"].as_str().unwrap_or(""),
            count(&k["appear"]),
            count(&k["disappear"])
        ));
    }
    let answers = &doc["answers"];
    for s in answers["sets"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "\nanswers: {} ({} answers, {} sealed left out), on {}\n",
            s["name"].as_str().unwrap_or(""),
            count(&s["answers"]),
            count(&s["sealed_left_out"]),
            answers["measured_on"].as_str().unwrap_or("")
        ));
    }
    out.push_str(&format!(
        "  fixes {}, breaks {}, net {}\n",
        count(&answers["fixes"]),
        count(&answers["breaks"]),
        count(&answers["net"])
    ));
    for (axis, t) in answers["by_axis"].as_object().into_iter().flatten() {
        out.push_str(&format!(
            "  {axis}: {} answers; right->wrong {}, wrong->right {}, unanswered->right {}, unanswered->wrong {}, right->unanswered {}, unchanged {}\n",
            count(&t["answers"]),
            count(&t["right_to_wrong"]),
            count(&t["wrong_to_right"]),
            count(&t["unanswered_to_right"]),
            count(&t["unanswered_to_wrong"]),
            count(&t["right_to_unanswered"]),
            count(&t["unchanged"]),
        ));
    }
    let secs = &doc["seconds"];
    out.push_str(&format!(
        "\nseconds: {}\n",
        secs.as_object()
            .map(|m| m
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect::<Vec<_>>()
                .join(", "))
            .unwrap_or_default()
    ));
    out
}
