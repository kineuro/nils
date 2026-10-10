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

use nils_pipeline::descriptor;
use nils_registry::Registry;
use nils_registry::pipeline::{self as rows, STARTER};
use serde_json::{Value, json};

/// The starter descriptors, in R1's order.
pub(crate) const CATALOG: [(&str, &str); 7] = [
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
        "segcsvd",
        include_str!("../../../../pipelines/segcsvd/nils.job.yml"),
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
        // mri_synthstrip peaked at 5.7 GB on the CPU; a unit held to 4 GB
        // was killed (record 49 slice G)
        let strip = descriptor::parse(CATALOG[1].1).unwrap();
        assert!(
            strip.document["x-nils"]["needs"]["memory-gb"].as_f64() >= Some(8.0),
            "synthstrip declares the memory it peaks at"
        );
        let (_, recon) = CATALOG
            .iter()
            .find(|(name, _)| *name == "freesurfer-recon-all")
            .unwrap();
        let recon = descriptor::parse(recon).unwrap();
        assert_eq!(
            recon.document["x-nils"]["secrets"][0]["env"], "FS_LICENSE",
            "recon-all reads the lab's licence as a secret input (R3)"
        );
    }

    /// Record 49, after review: each starter runs its sessions apart,
    /// keeps its work under its own output (never a /tmp that apptainer
    /// keeps small and podman keeps in the container's layer), and tells
    /// its tool the threads and memory the unit declares.
    #[test]
    fn every_starter_runs_apart_works_in_its_output_and_is_held_to_its_needs() {
        for (name, text) in CATALOG {
            let d = descriptor::parse(text).unwrap();
            assert_eq!(d.units, descriptor::Units::Apart, "{name}");
            let line = d.document["command-line"].as_str().unwrap_or_default();
            assert!(!line.contains("/tmp"), "{name}: {line}");
            assert!(
                !line.contains("mktemp -d)"),
                "{name} makes its work folder where it writes: {line}"
            );
            for p in &d.params {
                if ["threads", "processes"].contains(&p.id.as_str()) {
                    assert_eq!(
                        d.needs.cores_input.as_deref(),
                        Some(p.id.as_str()),
                        "{name}"
                    );
                }
                if p.id == "memory_gb" {
                    assert_eq!(d.needs.memory_input.as_deref(), Some("memory_gb"), "{name}");
                }
            }
        }
    }

    /// A word of a command line with each container path written as the
    /// host folder mounted there, as the pipelines tests' stand-in podman
    /// writes it: the longest path first, and only where a path ends.
    #[cfg(unix)]
    fn on_host(word: &str, mounts: &[(&str, std::path::PathBuf)]) -> String {
        let mut out = String::new();
        let mut rest = word;
        'next: while let Some(c) = rest.chars().next() {
            for (at, host) in mounts {
                if let Some(after) = rest.strip_prefix(at)
                    && !after
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    out.push_str(&host.to_string_lossy());
                    rest = after;
                    continue 'next;
                }
            }
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
        out
    }

    /// Record 55 C4 (Nima's ruling of 2026-10-08, "FLAIR is always a
    /// modifier"): the two starters that read a FLAIR take the one the
    /// release names a T2 FLAIR, a T2w with FLAIR among the `+` tokens of
    /// its `acq-` in either BIDS style, or the `_FLAIR` of a tree an older
    /// release wrote; never a T2w without the token, a token that only
    /// holds the word (`SynFLAIR`), the word in another entity, or another
    /// suffix (a T1-FLAIR). segcsvd names the WMH for the FLAIR's stem
    /// without its suffix, and its own templates find them. Their command
    /// lines run on the host, each container path written as a folder of
    /// the test's, and each tool of the images is a script that keeps its
    /// words, so no container runs.
    #[cfg(unix)]
    #[test]
    fn the_starters_that_read_a_flair_take_it_as_the_release_names_it() {
        use nils_dicom::synth::TempDir;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        if Command::new("bash").args(["-c", "true"]).status().is_err() {
            eprintln!(
                "bash is not installed; the starters' command lines need it, so this test is skipped"
            );
            return;
        }
        // each session of one subject: its T1w, the rest of its anat
        // folder, and the FLAIR a starter must take there
        type Session<'a> = (&'a str, &'a str, &'a [&'a str], Option<&'a str>);
        let sessions: [Session<'_>; 5] = [
            // the full style, beside a T2w that is no FLAIR and sorts first
            (
                "full",
                "acq-Sag+3D+MPRAGE_T1w",
                &["acq-Ax+2D+DIR+TSE_T2w", "acq-Ax+2D+FLAIR+IRTSE_T2w"],
                Some("acq-Ax+2D+FLAIR+IRTSE_T2w"),
            ),
            // the minimal style, with what a name conflict added
            (
                "minimal",
                "acq-3D+MPRAGE_T1w",
                &["acq-2D+FLAIR+IRTSE+3mm_T2w"],
                Some("acq-2D+FLAIR+IRTSE+3mm_T2w"),
            ),
            // entities after `acq-`
            (
                "entities",
                "acq-Sag+3D+MPRAGE_T1w",
                &["acq-Sag+3D+FLAIR+SPACE_ce-contrast_run-2_T2w"],
                Some("acq-Sag+3D+FLAIR+SPACE_ce-contrast_run-2_T2w"),
            ),
            // a tree an older release wrote
            (
                "older",
                "acq-SagMPRAGE_T1w",
                &["acq-BrainAx2DIRTSEFLAIR_FLAIR"],
                Some("acq-BrainAx2DIRTSEFLAIR_FLAIR"),
            ),
            // none: the T1w is a T1-FLAIR, and no T2w holds the token in
            // its `acq-`
            (
                "none",
                "acq-Ax+2D+FLAIR+IRTSE_T1w",
                &[
                    "acq-Ax+2D+TSE_T2w",
                    "acq-Ax+3D+SynFLAIR_T2w",
                    "rec-FLAIR_T2w",
                ],
                None,
            ),
        ];
        let stem = |ses: &str, rest: &str| format!("sub-a_ses-{ses}_{rest}");
        // a stem without its suffix, which a derivative is named for
        let base = |stem: &str| stem.rsplit_once('_').map_or(stem, |(b, _)| b).to_string();

        // the images' tools: each keeps its words, a line a call, and
        // writes what the command line reads next
        let stubs: [(&str, &str); 6] = [
            ("sbtResliceLike", r#"printf x > "$3""#),
            (
                "segment_wmh",
                r#"printf x > "$3"; printf x > "$(dirname "$3")/thr_wmh.nii.gz""#,
            ),
            (
                "segment_pvs",
                r#"printf x > "$4"; printf x > "$(dirname "$4")/thr_pvs.nii.gz""#,
            ),
            // the volumes of the masks it is given in pairs, each there;
            // otherwise an image written to its last word
            (
                "python3",
                r#"case "$2" in *json.dumps*) shift 2; s=; while [ $# -gt 1 ]; do [ -s "$2" ] || exit 1; s="$s${s:+, }\"$1\": 1.0"; shift 2; done; echo "{$s}";; *) for l; do :; done; printf x > "$l";; esac"#,
            ),
            // SAMSEG's folder read for its measures; otherwise an image
            // written to its last word
            (
                "fspython",
                r#"for l; do :; done; if [ -d "$l" ]; then echo '{"Intra-Cranial": 1500000.0}'; else printf x > "$l"; fi"#,
            ),
            (
                "run_samseg",
                r#"while [ $# -gt 0 ]; do [ "$1" = -o ] && o=$2; shift; done; mkdir -p "$o"; printf x > "$o/seg.mgz""#,
            ),
        ];

        let mut texts = Vec::new();
        for name in ["segcsvd", "samseg-lesions"] {
            let (_, text) = CATALOG.iter().find(|(n, _)| *n == name).unwrap();
            let d = descriptor::parse(text).unwrap();
            texts.push(d.command_line.clone().unwrap_or_default());
            let root = TempDir::new(&format!("starter-flair-{name}"));
            let log = root.path().join("tools.log");
            for (tool, body) in stubs {
                let keep = format!(
                    "l=${{0##*/}}; for a; do l=\"$l\t$a\"; done; printf '%s\\n' \"$l\" >> '{}'",
                    log.display()
                );
                let f = root.file(
                    &format!("bin/{tool}"),
                    format!("#!/bin/sh\n{keep}\n{body}\n").as_bytes(),
                );
                std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            for (ses, t1w, rest, _) in &sessions {
                for n in std::iter::once(t1w).chain(rest.iter()) {
                    root.file(
                        &format!("input/sub-a/ses-{ses}/anat/{}.nii.gz", stem(ses, n)),
                        b"x",
                    );
                }
                // the T1w's SynthSeg label map, where a synthseg run's
                // derivative input holds it: its pipeline, run and unit
                root.file(
                    &format!(
                        "inputs/synthseg/synthseg/7/sub-a_ses-{ses}/sub-a/ses-{ses}/anat/{}_desc-synthseg_dseg.nii.gz",
                        base(&stem(ses, t1w))
                    ),
                    b"x",
                );
            }
            std::fs::create_dir_all(root.path().join("output")).unwrap();
            let at = [
                ("/inputs", root.path().join("inputs")),
                ("/input", root.path().join("input")),
                ("/output", root.path().join("output")),
            ];
            let words: Vec<String> = d
                .argv(&d.resolve(&[]).unwrap(), &[])
                .unwrap()
                .iter()
                .map(|w| on_host(w, &at))
                .collect();
            let mut path = root.path().join("bin").into_os_string();
            if let Some(p) = std::env::var_os("PATH") {
                path.push(":");
                path.push(p);
            }
            let out = Command::new(&words[0])
                .args(&words[1..])
                .env("PATH", &path)
                .output()
                .unwrap();
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success(),
                "{name}: {said}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                said.contains("sub-a/ses-none/anat holds no FLAIR"),
                "{name}: {said}"
            );

            // the FLAIR each session's tool was handed: segment_wmh's first
            // word, and the first after the code of the resampling
            let calls = std::fs::read_to_string(&log).unwrap();
            let mut handed: Vec<String> = calls
                .lines()
                .map(|l| l.split('\t').collect::<Vec<_>>())
                .filter_map(|w| match w.as_slice() {
                    ["segment_wmh", flair, ..] => Some(flair.to_string()),
                    ["fspython", "-c", code, flair, ..] if code.contains("resample_like") => {
                        Some(flair.to_string())
                    }
                    _ => None,
                })
                .collect();
            handed.sort();
            let mut want: Vec<String> = sessions
                .iter()
                .filter_map(|(ses, _, _, flair)| {
                    flair.map(|f| format!("sub-a/ses-{ses}/anat/{}.nii.gz", stem(ses, f)))
                })
                .collect();
            want.sort();
            assert_eq!(handed, want, "{name}: {calls}");

            // what each session wrote, each a file its templates find
            let output = root.path().join("output");
            for (ses, t1w, _, flair) in &sessions {
                let t1 = base(&stem(ses, t1w));
                let mut want: Vec<String> = match (name, flair) {
                    ("segcsvd", _) => [
                        format!("{t1}_label-PVS_desc-segcsvd_mask.nii.gz"),
                        format!("{t1}_label-PVS_desc-segcsvd_probseg.nii.gz"),
                        format!("{t1}_desc-segcsvd_volumes.json"),
                    ]
                    .into_iter()
                    .chain(flair.iter().flat_map(|f| {
                        let fl = base(&stem(ses, f));
                        [
                            format!("{fl}_label-WMH_desc-segcsvd_mask.nii.gz"),
                            format!("{fl}_label-WMH_desc-segcsvd_probseg.nii.gz"),
                        ]
                    }))
                    .collect(),
                    (_, Some(_)) => vec![
                        format!("{t1}_desc-samseg_dseg.nii.gz"),
                        format!("{t1}_desc-samseg_volumes.json"),
                    ],
                    (_, None) => Vec::new(),
                };
                want.sort();
                let anat = output.join(format!("sub-a/ses-{ses}/anat"));
                let mut wrote: Vec<String> = std::fs::read_dir(&anat)
                    .unwrap()
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
                wrote.sort();
                assert_eq!(wrote, want, "{name}, session {ses}");
                let vars = [("subject", "a"), ("session", *ses)];
                let found: Vec<String> = d
                    .outputs
                    .iter()
                    .flat_map(|o| nils_pipeline::files::found(&output, &o.template, &vars))
                    .collect();
                for f in &want {
                    assert!(
                        found.contains(&format!("sub-a/ses-{ses}/anat/{f}")),
                        "{name}: {f} is found by no template: {found:?}"
                    );
                }
                if name == "segcsvd" {
                    let volumes: serde_json::Value = serde_json::from_slice(
                        &std::fs::read(anat.join(format!("{t1}_desc-segcsvd_volumes.json")))
                            .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        volumes.get("wmh_volume").is_some(),
                        flair.is_some(),
                        "{ses}: {volumes}"
                    );
                }
            }
        }
        // one way of finding a FLAIR, the same in both
        let flair_of = |line: &str| {
            let from = line.find("flair() {").expect("the starter defines flair()");
            line[from..].split_once("done; };").unwrap().0.to_string()
        };
        assert_eq!(flair_of(&texts[0]), flair_of(&texts[1]));
    }
}
