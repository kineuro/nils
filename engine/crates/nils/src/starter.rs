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
//! (`nils pipeline starter --off`) turns the seeding off. A starter whose
//! new version fixes the earlier ones says so (`x-nils.replaces`, Nima's
//! ruling of 2026-10-12), and a new run no longer takes those
//! ([`crate::versions`]); one whose new version changes what it makes, as
//! N4's spline distance did, leaves them selectable.

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
                pack: None,
                pack_digest: None,
                version_digest: &nils_pipeline::pack::version_digest(&digest, None),
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
            // a check reads a column its unit writes, else it holds nothing
            for c in &d.checks {
                assert!(
                    d.outputs
                        .iter()
                        .filter_map(|o| o.table.as_ref())
                        .any(|t| t.columns.iter().any(|col| col.name == c.metric)),
                    "{name}: the check {} reads no column of its tables",
                    c.text()
                );
            }
        }
        let starter = |name: &str| {
            let (_, text) = CATALOG.iter().find(|(n, _)| *n == name).unwrap();
            descriptor::parse(text).unwrap()
        };
        // the review of 2026-10-12: FreeSurfer 8's recon-all takes about
        // 24 GB (release notes) to 80 GB at a peak (install notes)
        assert!(
            starter("freesurfer-recon-all").needs.memory_gb >= 32.0,
            "recon-all declares the memory FreeSurfer 8 takes"
        );
        // MRIQC uploads its IQMs and reports usage unless told not to
        let mriqc = starter("mriqc");
        let line = mriqc.command_line.as_deref().unwrap_or_default();
        for flag in ["--no-sub", "--notrack"] {
            assert!(
                line.split_whitespace().any(|w| w == flag),
                "mriqc passes {flag}: {line}"
            );
        }
        // N4 sets its spline distance, convergence and sharpening
        // explicitly, records the distance as a parameter, and writes the
        // bias field as an output of its own
        let n4 = starter("n4-bias-correction");
        assert_eq!(
            n4.resolve(&[]).unwrap()["spline_distance"],
            serde_json::json!(180)
        );
        let line = n4.command_line.as_deref().unwrap_or_default();
        for words in [
            r#"-b "[[SPLINE],3]""#,
            // the levels knob builds the convergence, 50 iterations a level
            r#"-c "[$c,0.0]""#,
            "c=50;",
            r#"-t "[0.15,0.01,200]""#,
            r#"_desc-n4_biasfield.nii.gz]""#,
        ] {
            assert!(line.contains(words), "n4: {words}: {line}");
        }
        assert!(
            n4.outputs
                .iter()
                .any(|o| o.template.ends_with("*_desc-n4_biasfield.nii.gz")),
            "n4 declares its bias field"
        );
        // the two starters that place a FLAIR on the T1w by the headers
        // alone check the alignment, a flag at 0.85, never a failure
        for name in ["samseg-lesions", "segcsvd"] {
            let d = starter(name);
            let c = d
                .checks
                .iter()
                .find(|c| c.metric == "flair_t1_brain_dice")
                .unwrap_or_else(|| panic!("{name} checks the FLAIR's alignment"));
            assert!(c.holds(0.85) && c.holds(0.97) && !c.holds(0.84), "{name}");
        }
        // Nima's ruling of 2026-10-12: a version that fixes the earlier
        // ones replaces them, so a new run no longer takes them; N4's new
        // version changes its output (the spline distance, the bias field),
        // so its earlier version stays selectable
        for (name, _) in CATALOG {
            let r = starter(name).replaces;
            let fixes = [
                "freesurfer-recon-all",
                "mriqc",
                "samseg-lesions",
                "segcsvd",
                "n4-bias-correction",
                "synthstrip",
            ]
            .contains(&name);
            assert_eq!(r.is_some(), fixes, "{name} replaces the earlier versions");
            if let Some(r) = r {
                assert_eq!(
                    r.versions, None,
                    "{name}: every earlier version, in any catalog"
                );
                assert!(r.why.len() > 20, "{name} says why");
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
    /// without its suffix, and its own templates find them. Both check the
    /// FLAIR they took against the session's T1w (the review of
    /// 2026-10-12) and write the agreement into the unit's table, where it
    /// had a FLAIR. Their command lines run on the host, each container
    /// path written as a folder of the test's, and each tool of the images
    /// is a script that keeps its words, so no container runs.
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
        let stubs: [(&str, &str); 7] = [
            ("sbtResliceLike", r#"printf x > "$3""#),
            (
                "segment_wmh",
                r#"printf x > "$3"; printf x > "$(dirname "$3")/thr_wmh.nii.gz""#,
            ),
            (
                "segment_pvs",
                r#"printf x > "$4"; printf x > "$(dirname "$4")/thr_pvs.nii.gz""#,
            ),
            // the alignment check, masks that agree; the volumes of the
            // masks it is given in pairs, each there, and the numbers
            // beside them; otherwise an image written to its last word
            // the manifest's word on what made the label map is not
            // there, so the unit names the run by its folder alone
            (
                "python3",
                r#"case "$2" in *made_by*) exit 1;; *threshold_otsu*) echo 0.97;; *json.dumps*) shift 2; s=; while [ $# -gt 1 ]; do v=$2; case "$1" in *_volume) [ -s "$2" ] || exit 1; v=1.0;; flair_source|synthseg_pipeline|synthseg_contrast) v=\"$2\";; esac; s="$s${s:+, }\"$1\": $v"; shift 2; done; echo "{$s}";; *) for l; do :; done; printf x > "$l";; esac"#,
            ),
            // the alignment check, masks that agree; SAMSEG's folder read
            // for its measures, and the numbers after it; otherwise an
            // image written to its last word
            (
                "fspython",
                r#"case "$2" in *affine_transform*) echo 0.97;; *samseg.stats*) shift 3; s='"Intra-Cranial": 1500000.0'; while [ $# -gt 1 ]; do v=$2; [ "$1" = flair_source ] && v=\"$2\"; s="$s, \"$1\": $v"; shift 2; done; echo "{$s}";; *) for l; do :; done; printf x > "$l";; esac"#,
            ),
            (
                "run_samseg",
                r#"while [ $# -gt 0 ]; do [ "$1" = -o ] && o=$2; shift; done; mkdir -p "$o"; printf x > "$o/seg.mgz""#,
            ),
            // a brain mask of the image it is given
            (
                "mri_synthstrip",
                r#"while [ $# -gt 0 ]; do [ "$1" = -m ] && m=$2; shift; done; printf x > "$m""#,
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

            // and the alignment check, each session's T1w beside the same
            // FLAIR: the T1w and the FLAIR are the first two words after
            // its code
            let mut checked: Vec<String> = calls
                .lines()
                .map(|l| l.split('\t').collect::<Vec<_>>())
                .filter_map(|w| match w.as_slice() {
                    ["python3" | "fspython", "-c", code, t1, flair, ..]
                        if code.contains("brain masks agree") =>
                    {
                        assert!(t1.ends_with("_T1w.nii.gz"), "{name}: {t1}");
                        Some(flair.to_string())
                    }
                    _ => None,
                })
                .collect();
            checked.sort();
            assert_eq!(checked, want, "{name}: {calls}");

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
                    assert_eq!(volumes["synthseg_run"], 7, "{ses}: {volumes}");
                    assert!(volumes.get("synthseg_robust").is_none(), "{volumes}");
                }
                // the alignment, a column of the unit's table where it
                // had a FLAIR to check
                let table = anat.join(format!(
                    "{t1}_desc-{}_volumes.json",
                    name.trim_end_matches("-lesions")
                ));
                if flair.is_some() {
                    let volumes: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&table).unwrap()).unwrap();
                    assert_eq!(
                        volumes["flair_t1_brain_dice"],
                        serde_json::json!(0.97),
                        "{name}, session {ses}: {volumes}"
                    );
                } else if let Ok(bytes) = std::fs::read(&table) {
                    let volumes: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    assert!(
                        volumes.get("flair_t1_brain_dice").is_none(),
                        "{name}, session {ses}: {volumes}"
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

    /// A starter's command line run on the host as the flair test runs
    /// it: each container path written as a folder under `root` (its
    /// `input`, `inputs` and `output`), and each tool a script in
    /// `root/bin` that keeps its words in `root/tools.log`, a line a call,
    /// and then runs its body. Answers the run and the calls.
    #[cfg(unix)]
    fn stand_in_run(
        name: &str,
        given: &[(&str, &str)],
        root: &std::path::Path,
        tools: &[(&str, String)],
    ) -> (std::process::Output, Vec<Vec<String>>) {
        use std::os::unix::fs::PermissionsExt;
        let log = root.join("tools.log");
        let _ = std::fs::remove_file(&log);
        for (tool, body) in tools {
            let keep = format!(
                "l=${{0##*/}}; for a; do l=\"$l\t$a\"; done; printf '%s\\n' \"$l\" >> '{}'",
                log.display()
            );
            let f = root.join("bin").join(tool);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, format!("#!/bin/sh\n{keep}\n{body}\n")).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let _ = std::fs::remove_dir_all(root.join("output"));
        std::fs::create_dir_all(root.join("output")).unwrap();
        let (_, text) = CATALOG.iter().find(|(n, _)| *n == name).unwrap();
        let d = descriptor::parse(text).unwrap();
        let given: Vec<(String, String)> = given
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let at = [
            ("/inputs", root.join("inputs")),
            ("/input", root.join("input")),
            ("/output", root.join("output")),
        ];
        let words: Vec<String> = d
            .argv(&d.resolve(&given).unwrap(), &[])
            .unwrap()
            .iter()
            .map(|w| on_host(w, &at))
            .collect();
        let mut path = root.join("bin").into_os_string();
        if let Some(p) = std::env::var_os("PATH") {
            path.push(":");
            path.push(p);
        }
        let out = std::process::Command::new(&words[0])
            .args(&words[1..])
            .env("PATH", &path)
            .output()
            .unwrap();
        let calls = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(|l| l.split('\t').map(str::to_string).collect())
            .collect();
        (out, calls)
    }

    /// Record 57 P11 (research 02, SAMSEG's wiki): SAMSEG separates the
    /// pallidum from the white matter only with the FLAIR among its
    /// inputs, never from the T1w alone, and the unit's table says which
    /// it did, since the two give other pallidum and white-matter volumes.
    #[cfg(unix)]
    #[test]
    fn samseg_separates_the_pallidum_only_beside_a_flair_and_says_so() {
        use nils_dicom::synth::TempDir;
        if std::process::Command::new("bash")
            .args(["-c", "true"])
            .status()
            .is_err()
        {
            eprintln!(
                "bash is not installed; the starters' command lines need it, so this test is skipped"
            );
            return;
        }
        let root = TempDir::new("starter-samseg-pallidum");
        // one session with a FLAIR, one without
        for f in [
            "ses-1/anat/sub-a_ses-1_acq-3D+MPRAGE_T1w",
            "ses-1/anat/sub-a_ses-1_acq-Ax+2D+FLAIR+IRTSE_T2w",
            "ses-2/anat/sub-a_ses-2_acq-3D+MPRAGE_T1w",
        ] {
            root.file(&format!("input/sub-a/{f}.nii.gz"), b"x");
        }
        let tools: Vec<(&str, String)> = vec![
            (
                "fspython",
                r#"case "$2" in *affine_transform*) echo 0.97;; *samseg.stats*) shift 3; s='"Intra-Cranial": 1500000.0'; while [ $# -gt 1 ]; do v=$2; [ "$1" = flair_source ] && v=\"$2\"; s="$s, \"$1\": $v"; shift 2; done; echo "{$s}";; *) for l; do :; done; printf x > "$l";; esac"#.into(),
            ),
            (
                "run_samseg",
                r#"while [ $# -gt 0 ]; do [ "$1" = -o ] && o=$2; shift; done; mkdir -p "$o"; printf x > "$o/seg.mgz""#.into(),
            ),
            (
                "mri_synthstrip",
                r#"while [ $# -gt 0 ]; do [ "$1" = -m ] && m=$2; shift; done; printf x > "$m""#.into(),
            ),
        ];
        let table = |ses: &str| -> Option<serde_json::Value> {
            let p = root.path().join(format!(
                "output/sub-a/{ses}/anat/sub-a_{ses}_acq-3D+MPRAGE_desc-samseg_volumes.json"
            ));
            std::fs::read(p)
                .ok()
                .map(|b| serde_json::from_slice(&b).unwrap())
        };
        // each case: the settings, and for each session whether SAMSEG ran,
        // with the FLAIR, and separated the pallidum
        type Case<'a> = (
            &'a [(&'a str, &'a str)],
            [(&'a str, Option<(bool, bool)>); 2],
        );
        let cases: [Case<'_>; 3] = [
            // the defaults: lesions on, the setting on; the session
            // without a FLAIR is not run
            (&[], [("ses-1", Some((true, true))), ("ses-2", None)]),
            // lesions off: the T1w alone, so never separated, whatever the
            // setting says
            (
                &[("lesion", "off")],
                [
                    ("ses-1", Some((false, false))),
                    ("ses-2", Some((false, false))),
                ],
            ),
            // the setting off: with the FLAIR, not separated
            (
                &[("pallidum_separate", "false")],
                [("ses-1", Some((true, false))), ("ses-2", None)],
            ),
        ];
        for (given, sessions) in cases {
            let (out, calls) = stand_in_run("samseg-lesions", given, root.path(), &tools);
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success(),
                "{given:?}: {said}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let samseg: Vec<&Vec<String>> = calls.iter().filter(|c| c[0] == "run_samseg").collect();
            assert_eq!(
                samseg.len(),
                sessions.iter().filter(|(_, s)| s.is_some()).count(),
                "{given:?}: {calls:?}"
            );
            for (ses, ran) in sessions {
                let call = samseg
                    .iter()
                    .find(|c| c.iter().any(|w| w.contains(&format!("/{ses}/"))));
                match ran {
                    None => {
                        assert!(call.is_none(), "{given:?}, {ses}: {calls:?}");
                        assert!(table(ses).is_none(), "{given:?}, {ses}");
                    }
                    Some((flair, separate)) => {
                        let call = call.unwrap_or_else(|| panic!("{given:?}, {ses}: {calls:?}"));
                        let inputs = call.iter().filter(|w| *w == "-i").count();
                        assert_eq!(inputs, if flair { 2 } else { 1 }, "{given:?}: {call:?}");
                        assert_eq!(
                            call.iter().any(|w| w == "--pallidum-separate"),
                            separate,
                            "{given:?}, {ses}: {call:?}"
                        );
                        let t = table(ses).unwrap_or_else(|| panic!("{given:?}, {ses}"));
                        assert_eq!(
                            t["pallidum_separate"],
                            serde_json::json!(if separate { 1 } else { 0 }),
                            "{given:?}, {ses}: {t}"
                        );
                    }
                }
            }
        }
        // and the table declares the column the ask reads, a whole number
        let (_, text) = CATALOG
            .iter()
            .find(|(n, _)| *n == "samseg-lesions")
            .unwrap();
        let d = descriptor::parse(text).unwrap();
        let col = d
            .outputs
            .iter()
            .filter_map(|o| o.table.as_ref())
            .flat_map(|t| &t.columns)
            .find(|c| c.name == "pallidum_separate")
            .expect("samseg-lesions declares pallidum_separate");
        assert_eq!(col.ty, descriptor::ColumnType::Integer);
        assert!(
            d.replaces
                .as_ref()
                .is_some_and(|r| r.why.contains("pallidum")),
            "the new version says it replaces the earlier ones for the pallidum"
        );
    }

    /// Record 57 P11 (research 04): segcsvd's results name the SynthSeg
    /// they read, its run, its pipeline as name@version, whether it ran
    /// robust and the contrast it read, from what the engine's manifest
    /// says made each label map (`made_by`); without that word, the run
    /// alone, by the folder the map came in.
    #[cfg(unix)]
    #[test]
    fn segcsvd_names_the_synthseg_it_read() {
        use nils_dicom::synth::TempDir;
        let Some(python) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|d| d.join("python3"))
            .find(|p| p.is_file())
        else {
            eprintln!(
                "python3 is not installed; segcsvd's command line reads the manifest with it, so this test is skipped"
            );
            return;
        };
        let root = TempDir::new("starter-segcsvd-synthseg");
        // three sessions, each a T1w (and no FLAIR, so no WMH)
        let stem = |ses: &str| format!("sub-a_ses-{ses}_acq-3D+MPRAGE");
        for ses in ["1", "2", "3"] {
            root.file(
                &format!("input/sub-a/ses-{ses}/anat/{}_T1w.nii.gz", stem(ses)),
                b"x",
            );
        }
        // the label maps the run takes, as the runner links them: the
        // pipeline, its run, the unit; session 2 has one of two runs, and
        // the newer is read
        let map = |ses: &str, run: i64| {
            format!(
                "synthseg/{run}/sub-a_ses-{ses}/sub-a/ses-{ses}/anat/{}_desc-synthseg_dseg.nii.gz",
                stem(ses)
            )
        };
        let maps = [("1", 7), ("2", 7), ("2", 9), ("3", 7)];
        for (ses, run) in maps {
            root.file(&format!("inputs/synthseg/{}", map(ses, run)), b"x");
        }
        // the manifest: run 7 robust, read from the T1w; run 9 not
        // robust, read from a FLAIR (a T2w with the token); session 3's
        // map has no word on what made it
        let made = |run: i64, version: i64, robust: bool, grid: &str| {
            serde_json::json!({
                "run": run, "pipeline": "synthseg", "version": version,
                "params": {"robust": robust, "threads": 4},
                "grid_stack_id": 40 + run, "grid_stem": grid,
            })
        };
        let listed: Vec<serde_json::Value> = maps
            .iter()
            .map(|(ses, run)| {
                let by = match (*ses, *run) {
                    ("3", _) => serde_json::Value::Null,
                    (_, 7) => made(7, 2, true, &format!("{}_T1w", stem(ses))),
                    _ => made(9, 3, false, "sub-a_ses-2_acq-Ax+2D+FLAIR+IRTSE_T2w"),
                };
                serde_json::json!({"id": run * 10, "kind": "mask", "path": map(ses, *run), "made_by": by})
            })
            .collect();
        root.file(
            "inputs/manifest.json",
            serde_json::to_vec(&serde_json::json!({"derivatives": {"synthseg": listed}}))
                .unwrap()
                .as_slice(),
        );
        // segcsvd's tools; its python3 runs the manifest's reading as it
        // is, writes the empty prior, and writes the volumes as JSON, the
        // numbers as numbers and the rest as text
        let tools: Vec<(&str, String)> = vec![
            ("sbtResliceLike", r#"printf x > "$3""#.into()),
            (
                "segment_pvs",
                r#"printf x > "$4"; printf x > "$(dirname "$4")/thr_pvs.nii.gz""#.into(),
            ),
            (
                "python3",
                format!(
                    r#"case "$2" in *made_by*) exec '{}' "$@";; *json.dumps*) shift 2; s=; while [ $# -gt 1 ]; do v=$2; case "$1" in *_volume) v=1.0;; esac; case "$v" in *[!0-9.]*) v="\"$v\"";; esac; s="$s${{s:+, }}\"$1\": $v"; shift 2; done; echo "{{$s}}";; *) for l; do :; done; printf x > "$l";; esac"#,
                    python.display()
                ),
            ),
        ];
        let (out, _) = stand_in_run("segcsvd", &[], root.path(), &tools);
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{said}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let table = |ses: &str| -> serde_json::Value {
            serde_json::from_slice(
                &std::fs::read(root.path().join(format!(
                    "output/sub-a/ses-{ses}/anat/{}_desc-segcsvd_volumes.json",
                    stem(ses)
                )))
                .unwrap(),
            )
            .unwrap()
        };
        let read = |ses: &str| -> serde_json::Value {
            let t = table(ses);
            serde_json::Value::Object(
                t.as_object()
                    .unwrap()
                    .iter()
                    .filter(|(k, _)| k.starts_with("synthseg_"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        };
        assert_eq!(
            read("1"),
            serde_json::json!({"synthseg_run": 7, "synthseg_pipeline": "synthseg@2", "synthseg_robust": 1, "synthseg_contrast": "T1w"}),
            "{said}"
        );
        assert_eq!(
            read("2"),
            serde_json::json!({"synthseg_run": 9, "synthseg_pipeline": "synthseg@3", "synthseg_robust": 0, "synthseg_contrast": "FLAIR"}),
            "{said}"
        );
        assert_eq!(read("3"), serde_json::json!({"synthseg_run": 7}), "{said}");
        assert!(
            said.contains("the SynthSeg label map of run 9 (synthseg_run 9 synthseg_pipeline synthseg@3 synthseg_robust 0 synthseg_contrast FLAIR)"),
            "{said}"
        );

        // the table declares the four, typed, and the new version says it
        // replaces the earlier ones for them
        let (_, text) = CATALOG.iter().find(|(n, _)| *n == "segcsvd").unwrap();
        let d = descriptor::parse(text).unwrap();
        let columns: Vec<(String, descriptor::ColumnType)> = d
            .outputs
            .iter()
            .filter_map(|o| o.table.as_ref())
            .flat_map(|t| &t.columns)
            .filter(|c| c.name.starts_with("synthseg_"))
            .map(|c| (c.name.clone(), c.ty))
            .collect();
        use descriptor::ColumnType::{Integer, Text};
        assert_eq!(
            columns,
            [
                ("synthseg_run".to_string(), Integer),
                ("synthseg_pipeline".to_string(), Text),
                ("synthseg_robust".to_string(), Integer),
                ("synthseg_contrast".to_string(), Text),
            ]
        );
        assert!(
            d.replaces
                .as_ref()
                .is_some_and(|r| r.why.contains("SynthSeg")),
            "the new version says it replaces the earlier ones for the SynthSeg it read"
        );
    }
}
