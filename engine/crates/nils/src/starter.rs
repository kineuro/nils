// SPDX-License-Identifier: AGPL-3.0-only

//! The starter catalog (record 49 A4): the descriptors of R1's first
//! analyses, built into the engine and seeded into its catalog when it
//! starts, so the Pipelines page is never empty. Each image is pinned by its
//! registry manifest digest, as every descriptor is.
//!
//! A starter is added when its name is not in the catalog, and a newer
//! starter (the engine's newer pins) is added as the name's next version
//! when the name's newest version is the engine's own starter. A version a
//! person added, or a starter a person retired, is never gone over. Each
//! seeded version says `origin: starter`. The setting `pipeline_starter`
//! (`nils pipeline starter --off`) turns the seeding off.
//!
//! segcsvd, R1's fourth analysis, is not here: it ships only as an image
//! archive on Hugging Face, with no public registry image to pin by digest.

use nils_pipeline::descriptor;
use nils_registry::Registry;
use nils_registry::pipeline::{self as rows, STARTER};
use serde_json::{Value, json};

/// The starter descriptors, in R1's order.
pub(crate) const CATALOG: [(&str, &str); 6] = [
    (
        "n4-bias-correction",
        include_str!("../../../../pipelines/n4-bias-correction/nils.job.yml"),
    ),
    (
        "synthstrip",
        include_str!("../../../../pipelines/synthstrip/nils.job.yml"),
    ),
    (
        "synthseg",
        include_str!("../../../../pipelines/synthseg/nils.job.yml"),
    ),
    (
        "samseg-lesions",
        include_str!("../../../../pipelines/samseg-lesions/nils.job.yml"),
    ),
    (
        "mriqc",
        include_str!("../../../../pipelines/mriqc/nils.job.yml"),
    ),
    (
        "freesurfer-recon-all",
        include_str!("../../../../pipelines/freesurfer-recon-all/nils.job.yml"),
    ),
];

/// Where the registry keeps whether the engine seeds its starters: `off`
/// turns it off; anything else, or nothing, is on.
pub(crate) const SETTING: &str = "pipeline_starter";

/// Who a seeded version was added by.
pub(crate) const BY: &str = "nils (starter catalog)";

/// Whether this registry has the engine seed its starters.
pub(crate) fn enabled(registry: &mut Registry) -> bool {
    registry.meta_value(SETTING).ok().flatten().as_deref() != Some("off")
}

/// What became of one starter.
fn state_of(registry: &mut Registry, name: &str, text: &str) -> Result<(Value, bool), String> {
    let d = descriptor::parse(text).map_err(|e| format!("the starter {name}: {e}"))?;
    let digest = d.digest();
    let versions = rows::versions(registry.store(), &d.name).map_err(|e| e.to_string())?;
    let same = versions.iter().find(|p| p.descriptor_digest == digest);
    let newest = versions.last();
    let (state, seed) = match (same, newest) {
        (Some(p), _) => (format!("in the catalog as {}", p.label()), false),
        (None, None) => ("absent".to_string(), true),
        (None, Some(n)) if n.origin.as_deref() == Some(STARTER) && n.state == "active" => {
            (format!("{} is an older starter", n.label()), true)
        }
        (None, Some(n)) if n.state != "active" => {
            (format!("{} was retired, and is left so", n.label()), false)
        }
        (None, Some(n)) => (
            format!("{} is a person's version, and is left so", n.label()),
            false,
        ),
    };
    Ok((
        json!({"name": d.name, "digest": digest, "image": d.image.reference, "state": state}),
        seed,
    ))
}

/// The starters and what the catalog holds of each.
pub(crate) fn list(registry: &mut Registry) -> Result<Vec<Value>, String> {
    CATALOG
        .iter()
        .map(|(name, text)| state_of(registry, name, text).map(|(v, _)| v))
        .collect()
}

/// Seed the starters the catalog lacks. Answers those added.
pub(crate) fn seed(registry: &mut Registry) -> Result<Vec<Value>, String> {
    let mut added = Vec::new();
    for (name, text) in CATALOG {
        let (_, wanted) = state_of(registry, name, text)?;
        if !wanted {
            continue;
        }
        let d = descriptor::parse(text).map_err(|e| format!("the starter {name}: {e}"))?;
        let digest = d.digest();
        let now = nils_registry::time::now_iso();
        let (p, fresh) = rows::add(
            registry.store(),
            &rows::New {
                name: &d.name,
                tool_version: &d.tool_version,
                descriptor: &d.document,
                descriptor_digest: &digest,
                image: &d.image.reference,
                image_digest: &d.image.digest,
                layout: d.layout.name(),
                level: d.level.name(),
                added_by: BY,
                added_at: &now,
            },
        )
        .map_err(|e| e.to_string())?;
        if !fresh {
            continue;
        }
        rows::set_origin(registry.store(), p.id, STARTER).map_err(|e| e.to_string())?;
        nils_registry::audit::record(
            registry,
            &nils_registry::audit::Entry {
                principal: BY,
                action: nils_registry::audit::Action::PipelineAdd,
                scope: json!({"pipeline": p.id, "name": p.name, "version": p.version}),
                policy: None,
                job_id: None,
                details: Some(json!({
                    "descriptor": p.descriptor_digest, "image": p.image_digest, "origin": STARTER,
                })),
            },
        )
        .map_err(|e| e.to_string())?;
        added.push(json!({"id": p.id, "label": p.label(), "image": p.image}));
    }
    Ok(added)
}

/// At the engine's start: seed where the setting allows, and say so in a
/// line. A failure is said and never stops the engine.
pub(crate) fn at_start(registry: &mut Registry) -> String {
    if !enabled(registry) {
        return "starter catalog: off (nils pipeline starter --on seeds it)".into();
    }
    match seed(registry) {
        Ok(added) if added.is_empty() => "starter catalog: in place".into(),
        Ok(added) => format!(
            "starter catalog: seeded {}",
            added
                .iter()
                .filter_map(|a| a["label"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Err(e) => format!("starter catalog: not seeded: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every starter checks against the job contract, is named as the
    /// catalog lists it, is pinned by a manifest digest, and declares what
    /// the pre-flight and the ask read: its roles, its tables and checks,
    /// and a unit's typical minutes.
    #[test]
    fn every_starter_is_a_valid_pinned_descriptor() {
        for (name, text) in CATALOG {
            let d = descriptor::parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(d.name, name);
            assert!(d.image.digest.starts_with("sha256:"), "{name}");
            assert!(
                d.unit_minutes.is_some(),
                "{name} says how long a unit takes"
            );
            assert!(d.roles.contains(&"t1w".to_string()), "{name} needs a T1w");
            if name != "n4-bias-correction" {
                assert!(
                    d.outputs.iter().any(|o| o.table.is_some()),
                    "{name} writes a table the ask reads"
                );
                assert!(!d.checks.is_empty(), "{name} declares its checks");
            }
        }
        let recon = descriptor::parse(CATALOG[5].1).unwrap();
        assert_eq!(
            recon.document["x-nils"]["secrets"][0]["env"], "FS_LICENSE",
            "recon-all reads the lab's licence as a secret input (R3)"
        );
    }
}
