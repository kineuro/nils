// SPDX-License-Identifier: AGPL-3.0-only
//! A release keeps the main scans it read (record 55, rulings of
//! 2026-10-12, item 6, carried to releases). A scope's newer rules replace
//! the picks of the version before as their run ends, and the pack's run
//! makes its own again, so a release freezes the picks it read when it is
//! made and every reader of its picks after it reads that list.
//!
//! One registry of two datasets: `alpha`, whose own rules take the 2D SE
//! (and then the 2D TSE), and `beta` with none, which reads the pack's (the
//! 3D MPRAGE); and a cohort `trio` across both, whose own rules take the 2D
//! TSE (and then the 2D SE). A release of each, and a pipeline run over
//! each, is made under the first; then the second rules are saved and run,
//! which drops the first's picks, and a sort that moves the pack's picks
//! off the MPRAGE is followed by the pack's run. Every reader of the first
//! releases still names the first picks' stacks: the frozen list, the
//! releases door and the history, a mask's grid by its role, a check's
//! panels and a resumed run's units. A release made before releases froze
//! their picks reads the live picks while they are there, and says plainly
//! that they are gone once they are. On SQLite always, and on Postgres
//! where a test DSN is set.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write as _;
use std::process::{Command, Stdio};

use dicom_core::VR;
use dicom_dictionary_std::tags;
use nils_dicom::synth::{self, MetaFields, TempDir};
use nils_registry::Store;
use serde_json::{Value, json};

fn packs() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packs")
        .to_str()
        .unwrap()
        .to_string()
}

fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

const OPS: &str = "an-administrator-token-of-length";

/// A stand-in podman that answers what a run asks of it, and runs nothing.
const FAKE_PODMAN: &str = "#!/bin/sh\ncase \"$1\" in\n  info) case \"$*\" in *GraphRoot*) echo /fake/containers/storage ;; *) echo true ;; esac ;;\n  *) echo \"podman version 9.9.9-test\" ;;\nesac\nexit 0\n";

/// A bids pipeline whose input is each session's T1w and whose output is a
/// mask on the T1w's grid.
const NEEDS_T1W: &str = r#"name: needs-t1w
schema-version: "0.5"
tool-version: "1"
container-image:
  type: docker
  image: "example.org/needs-t1w@sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
command-line: "true [InputDataset] [OutputLocation]"
x-nils:
  analysis-level: session
  input: {layout: bids, roles: [t1w]}
  units: apart
  outputs:
    - id: lesions
      kind: mask
      path-template: "sub-{subject}/ses-{session}/anat/*_lesions.nii.gz"
      grid: t1w
      parts:
        - {key: lesions, words: Lesions, kind: components, labels: [99]}
  needs: {cores: 1, memory-gb: 1, unit-minutes: 2}
"#;

/// Each visit's three series: number, description, dimension, technique.
const SERIES: [(&str, &str, &str, &str); 3] = [
    ("1", "t1_mprage_sag", "3D", "MPRAGE"),
    ("2", "t1_se_tra", "2D", "SE"),
    ("3", "t1_tse_tra", "2D", "TSE"),
];

/// A dataset's folder under the root, already pseudonymised: each person's
/// visits, three series a visit in one frame of reference.
fn tree(dir: &TempDir, dataset: &str, people: &[(&str, &[&str], &str)]) {
    for &(patient, days, root) in people {
        for (v, day) in days.iter().enumerate() {
            let study = format!("{root}.{}", v + 1);
            for (n, description, dimension, _) in SERIES {
                let series = format!("{study}.{n}");
                for slice in 1..=12u32 {
                    let sop = format!("{series}.{slice}");
                    let mut e = synth::minimal_mr(&study, &series, &sop);
                    let pixels: Vec<u8> = (0..32u32 * 32)
                        .flat_map(|i| (200 + (i % 32) as u16 * 7 + slice as u16).to_le_bytes())
                        .collect();
                    e.extend([
                        synth::text(tags::PATIENT_ID, VR::LO, patient),
                        synth::text(tags::STUDY_DATE, VR::DA, day),
                        synth::text(tags::SERIES_TIME, VR::TM, &format!("0{n}1415")),
                        synth::text(tags::SERIES_NUMBER, VR::IS, n),
                        synth::text(tags::SERIES_DESCRIPTION, VR::LO, description),
                        synth::text(tags::PROTOCOL_NAME, VR::LO, description),
                        synth::text(tags::MR_ACQUISITION_TYPE, VR::CS, dimension),
                        synth::text(tags::IMAGE_TYPE, VR::CS, "ORIGINAL\\PRIMARY\\M\\ND"),
                        synth::text(tags::MANUFACTURER, VR::LO, "SYNTHETIC"),
                        synth::text(tags::BURNED_IN_ANNOTATION, VR::CS, "NO"),
                        synth::text(tags::FRAME_OF_REFERENCE_UID, VR::UI, &format!("{study}.9")),
                        synth::us(tags::ROWS, 32),
                        synth::us(tags::COLUMNS, 32),
                        synth::us(tags::BITS_ALLOCATED, 16),
                        synth::us(tags::BITS_STORED, 12),
                        synth::us(tags::HIGH_BIT, 11),
                        synth::us(tags::PIXEL_REPRESENTATION, 0),
                        synth::us(tags::SAMPLES_PER_PIXEL, 1),
                        synth::text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
                        synth::text(tags::PIXEL_SPACING, VR::DS, "8.0\\8.0"),
                        synth::text(tags::SLICE_THICKNESS, VR::DS, "1.0"),
                        synth::text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "0\\1\\0\\0\\0\\-1"),
                        synth::text(
                            tags::IMAGE_POSITION_PATIENT,
                            VR::DS,
                            &format!("{slice}\\0\\0"),
                        ),
                        synth::text(tags::INSTANCE_NUMBER, VR::IS, &slice.to_string()),
                        synth::bytes(tags::PIXEL_DATA, VR::OW, pixels),
                    ]);
                    dir.file(
                        &format!("{dataset}/derivatives/dcm-anon/{patient}/{day}/{n}/{slice}"),
                        &synth::part10(&MetaFields::mr(&sop), &e, true),
                    );
                }
            }
        }
    }
}

/// A NIfTI-1 mask of u8 labels, 32 by 32 by 12, with an sform, a few
/// voxels of label 99 in it.
fn mask_bytes() -> Vec<u8> {
    let dims = [32usize, 32, 12];
    let mut h = vec![0u8; 352];
    h[0..4].copy_from_slice(&348i32.to_le_bytes());
    let dim: [i16; 8] = [3, 32, 32, 12, 1, 1, 1, 1];
    for (i, d) in dim.iter().enumerate() {
        h[40 + 2 * i..42 + 2 * i].copy_from_slice(&d.to_le_bytes());
    }
    h[70..72].copy_from_slice(&2i16.to_le_bytes());
    h[72..74].copy_from_slice(&8i16.to_le_bytes());
    for (i, v) in [1.0f32, 8.0, 8.0, 1.0].iter().enumerate() {
        h[76 + 4 * i..80 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    h[108..112].copy_from_slice(&352f32.to_le_bytes());
    h[254..256].copy_from_slice(&1i16.to_le_bytes());
    let affine = [
        [-8.0f32, 0.0, 0.0, 0.0],
        [0.0, 8.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ];
    for (r, at) in [280usize, 296, 312].iter().enumerate() {
        for c in 0..4 {
            h[at + 4 * c..at + 4 * c + 4].copy_from_slice(&affine[r][c].to_le_bytes());
        }
    }
    h[344..348].copy_from_slice(b"n+1\0");
    let mut labels = vec![0u8; dims.iter().product()];
    for (i, j, k) in [(5, 5, 3), (6, 5, 3), (20, 20, 7)] {
        labels[i + dims[0] * (j + dims[1] * k)] = 99;
    }
    h.extend_from_slice(&labels);
    h
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

fn list(ids: impl IntoIterator<Item = impl std::borrow::Borrow<i64>>) -> String {
    ids.into_iter()
        .map(|i| i.borrow().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

struct Lab {
    home: TempDir,
    _bin: TempDir,
    work: TempDir,
    root: TempDir,
    out: TempDir,
    path: OsString,
    pg: Option<(String, String)>,
}

impl Lab {
    fn run(&self, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nils"))
            .arg("--registry")
            .arg(self.home.path())
            .args(args)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env("PATH", &self.path)
            .env("NILS_PACK_DIR", packs())
            .env_remove("NILS_DSN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).unwrap();
        }
        drop(input);
        let out = child.wait_with_output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (good, out, err) = self.run(args, None);
        assert!(good, "nils {args:?} failed: {err}\n{out}");
        out
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }

    fn store(&self) -> Store {
        match &self.pg {
            Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
            None => Store::open_sqlite(&self.home.path().join("registry.db")).unwrap(),
        }
    }

    /// One integer a statement answers, its tables written `{table}`.
    fn int(&self, sql: &str) -> i64 {
        let mut store = self.store();
        let sql = self.tables(&store, sql);
        store.query(&sql, &[]).unwrap()[0].int(0).unwrap()
    }

    fn ints(&self, sql: &str) -> BTreeSet<i64> {
        let mut store = self.store();
        let sql = self.tables(&store, sql);
        store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| r.int(0).unwrap())
            .collect()
    }

    fn exec(&self, sql: &str) {
        let mut store = self.store();
        let sql = self.tables(&store, sql);
        store.batch(&sql).unwrap();
    }

    /// `{table}` written in the store's schema.
    fn tables(&self, store: &Store, sql: &str) -> String {
        let mut out = sql.to_string();
        for t in [
            "release_pick",
            "release_stack",
            "release",
            "pipeline_run",
            "pipeline_unit",
            "derivative",
            "campaign_check_panel",
            "classification_axis",
            "stack_fingerprint",
            "pick_stack",
            "pick",
            "place",
            "series",
            "stack",
            "study",
            "job",
        ] {
            out = out.replace(&format!("{{{t}}}"), &store.qualified(t));
        }
        out
    }

    /// Two datasets read, fingerprinted and sorted, each visit's three
    /// series a T1w of its own technique; the pack's picks run; a working
    /// place, an export place and a bids pipeline.
    fn new(pg: Option<(String, String)>) -> Lab {
        let bin = TempDir::new("freeze-bin");
        let podman = bin.file("podman", FAKE_PODMAN.as_bytes());
        let no_gpu = bin.file("nvidia-smi", b"#!/bin/sh\nexit 1\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in [&podman, &no_gpu] {
                std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let mut path = bin.path().as_os_str().to_owned();
        if let Some(p) = std::env::var_os("PATH") {
            path.push(":");
            path.push(p);
        }
        let lab = Lab {
            home: TempDir::new("freeze-home"),
            _bin: bin,
            work: TempDir::new("freeze-work"),
            root: TempDir::new("freeze-root"),
            out: TempDir::new("freeze-out"),
            path,
            pg,
        };
        let (good, _, err) = lab.run(&["key", "add", "k"], Some("a release freeze test key\n"));
        assert!(good, "{err}");
        match &lab.pg {
            Some((dsn, schema)) => {
                lab.ok(&[
                    "init",
                    "--backend",
                    "postgres",
                    "--dsn",
                    dsn,
                    "--schema",
                    schema,
                    "--key",
                    "k",
                ]);
            }
            None => {
                lab.ok(&["init", "--key", "k"]);
            }
        }
        tree(
            &lab.root,
            "alpha",
            &[
                (
                    "A1",
                    &["20220115", "20230115"],
                    "1.2.826.0.1.3680043.8.498.91",
                ),
                ("A2", &["20220301"], "1.2.826.0.1.3680043.8.498.92"),
            ],
        );
        tree(
            &lab.root,
            "beta",
            &[
                ("B1", &["20220401"], "1.2.826.0.1.3680043.8.498.93"),
                ("B2", &["20220501"], "1.2.826.0.1.3680043.8.498.94"),
            ],
        );
        let p = packs();
        let root = lab.root.path().to_str().unwrap().to_string();
        lab.ok(&["place", "add", "data", &root, "--role", "source"]);
        for name in ["alpha", "beta"] {
            lab.ok(&[
                "place",
                "add-dataset",
                "data",
                name,
                "--patient-id",
                "id-type:patient-id",
                "--subjects",
                "generated",
            ]);
            let tree = lab.root.path().join(name).join("derivatives/dcm-anon");
            lab.ok(&[
                "digest",
                "--name",
                name,
                "--no-private",
                tree.to_str().unwrap(),
            ]);
        }
        lab.ok(&["fingerprint"]);
        lab.ok(&["classify", "--pack-dir", &p]);
        // each series a T1w of its own technique and nothing else
        lab.exec(
            "DELETE FROM {classification_axis} WHERE axis IN ('role', 'technique', 'disposition', 'post_contrast')",
        );
        for (_, description, _, technique) in SERIES {
            for (name, value) in [("role", "t1w"), ("technique", technique)] {
                lab.exec(&format!(
                    "INSERT INTO {{classification_axis}} (stack_id, axis, value, confidence, tier) \
                     SELECT stack_id, '{name}', '{value}', 1.0, 'rule' FROM {{stack_fingerprint}} \
                     WHERE text_series_description = '{description}'"
                ));
            }
        }
        lab.ok(&["pick", "run", "--pack-dir", &p]);
        let work = lab.work.path().join("place");
        std::fs::create_dir_all(&work).unwrap();
        lab.ok(&[
            "place",
            "add",
            "scratch",
            work.to_str().unwrap(),
            "--role",
            "working",
            "--fast",
        ]);
        let out = lab.out.path().to_str().unwrap().to_string();
        lab.ok(&["place", "add", "out", &out, "--role", "export"]);
        let descriptor = lab.work.path().join("needs-t1w.yml");
        std::fs::write(&descriptor, NEEDS_T1W).unwrap();
        lab.ok(&["pipeline", "add", descriptor.to_str().unwrap(), "--json"]);
        lab
    }

    /// Each subject by the patient it was read from.
    fn subjects(&self) -> BTreeMap<&'static str, i64> {
        let mut store = self.store();
        let sql = format!(
            "SELECT DISTINCT se.subject_id, so.root_canonical, \
               (SELECT COUNT(DISTINCT sy.id) FROM {} sy WHERE sy.subject_id = se.subject_id) \
             FROM {} st JOIN {} se ON se.id = st.series_id \
             JOIN {} b ON b.id = st.first_batch_id JOIN {} so ON so.id = b.source_id \
             ORDER BY se.subject_id",
            store.qualified("study"),
            store.qualified("stack"),
            store.qualified("series"),
            store.qualified("ingest_batch"),
            store.qualified("source"),
        );
        let alpha = std::fs::canonicalize(self.root.path().join("alpha")).unwrap();
        let mut a = Vec::new();
        let mut b = Vec::new();
        for r in store.query(&sql, &[]).unwrap() {
            let id = r.int(0).unwrap();
            let visits = r.int(2).unwrap();
            if std::path::Path::new(r.text(1).unwrap()).starts_with(&alpha) {
                a.push((visits, id));
            } else {
                b.push((visits, id));
            }
        }
        a.sort();
        b.sort();
        assert_eq!((a.len(), b.len()), (2, 2), "{a:?} {b:?}");
        BTreeMap::from([
            ("A1", a[1].1),
            ("A2", a[0].1),
            ("B1", b[0].1),
            ("B2", b[1].1),
        ])
    }

    /// The stacks of these subjects' series of a technique.
    fn stacks_of(&self, subjects: &[i64], technique: &str) -> BTreeSet<i64> {
        let description = SERIES.iter().find(|s| s.3 == technique).unwrap().1;
        self.ints(&format!(
            "SELECT stack_id FROM {{stack_fingerprint}} WHERE text_series_description = '{description}' \
             AND subject_id IN ({})",
            list(subjects)
        ))
    }

    /// The stacks a release froze as picked for a role.
    fn frozen(&self, release: i64, role: &str) -> BTreeSet<i64> {
        self.ints(&format!(
            "SELECT stack_id FROM {{release_pick}} WHERE release_id = {release} AND role = '{role}'"
        ))
    }

    /// What a release's row records of the picks it read.
    fn picks_of(&self, release: i64) -> Value {
        let mut store = self.store();
        let sql = format!(
            "SELECT CAST(pick_rules AS TEXT) FROM {} WHERE id = {release}",
            store.qualified("release")
        );
        let rows = store.query(&sql, &[]).unwrap();
        rows.first()
            .and_then(|r| r.opt_text(0).unwrap())
            .map_or(Value::Null, |t| serde_json::from_str(t).unwrap())
    }

    /// A release of the picked stacks of a scope, in the descriptive
    /// layout; answers its id.
    fn release(&self, name: &str, scope: &[&str]) -> i64 {
        let dir = self.out.path().join(name);
        let p = packs();
        let mut args = vec![
            "release",
            "--name",
            name,
            "--out",
            dir.to_str().unwrap(),
            "--picked",
            "--on-unknown",
            "write",
            "--pack-dir",
            &p,
        ];
        args.extend(scope);
        self.ok(&args);
        self.int(&format!(
            "SELECT MAX(id) FROM {{release}} WHERE name = '{name}'"
        ))
    }

    /// A run of needs-t1w over a selection; answers the run and its input
    /// release.
    fn pipeline_run(&self, select: &str) -> (i64, i64) {
        let before = self.int("SELECT COALESCE(MAX(id), 0) FROM {pipeline_run}");
        let (_, out, err) = self.run(&["run", "needs-t1w", "--select", select, "--json"], None);
        let run = self.int(&format!(
            "SELECT COALESCE(MAX(id), 0) FROM {{pipeline_run}} WHERE id > {before}"
        ));
        assert!(run > 0, "no run over {select}: {err}\n{out}");
        let release = self.int(&format!(
            "SELECT COALESCE(input_release_id, 0) FROM {{pipeline_run}} WHERE id = {run}"
        ));
        assert!(release > 0, "run {run} released no input: {err}\n{out}");
        (run, release)
    }

    /// Each stack a run's input release placed in a session's folder, with
    /// the folder and its subject: `(stack, dir, subject)`.
    fn placed(&self, release: i64) -> Vec<(i64, String, i64)> {
        let mut store = self.store();
        let sql = format!(
            "SELECT rs.stack_id, rs.dir, se.subject_id FROM {} r \
             JOIN {} rs ON rs.dataset_id = r.dataset_id JOIN {} st ON st.id = rs.stack_id \
             JOIN {} se ON se.id = st.series_id WHERE r.id = {release} ORDER BY rs.stack_id",
            store.qualified("release"),
            store.qualified("release_stack"),
            store.qualified("stack"),
            store.qualified("series"),
        );
        store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r.int(0).unwrap(),
                    r.text(1).unwrap().to_string(),
                    r.int(2).unwrap(),
                )
            })
            .filter(|(_, dir, _)| dir.starts_with("sub-"))
            .collect()
    }

    /// A mask a run made of the session whose folder is `dir` (a release's
    /// `sub-<s>/ses-<t>/anat`), its grid named where `grid` is given and
    /// left to be read otherwise; answers its id.
    fn mask(&self, run: i64, dir: &str, subject: i64, name: &str, grid: Option<i64>) -> i64 {
        let label = dir
            .split('/')
            .nth(1)
            .and_then(|s| s.strip_prefix("ses-"))
            .unwrap();
        let day = format!("{}-{}-{}", &label[..4], &label[4..6], &label[6..8]);
        let rel = format!("derivatives/needs-t1w/{run}/{dir}/{name}_lesions.nii.gz");
        let place = self.work.path().join("place");
        std::fs::create_dir_all(place.join(&rel).parent().unwrap()).unwrap();
        let bytes = mask_bytes();
        std::fs::write(place.join(&rel), &bytes).unwrap();
        let place_id = self.int("SELECT id FROM {place} WHERE name = 'scratch'");
        let mut store = self.store();
        let belongs =
            nils_registry::derivative::belongs(&mut store, None, None, Some(subject), Some(&day))
                .unwrap_or_else(|_| panic!("subject {subject} on {day}"));
        let id = nils_registry::derivative::insert_of_run(
            &mut store,
            &nils_registry::derivative::New {
                kind: "mask",
                belongs: &belongs,
                place_id,
                path: &rel,
                bytes: bytes.len() as i64,
                sha256: &sha256(&bytes),
                media_type: "application/x-nifti",
                registered_by: "ops@lab",
                actor: None,
                model_id: None,
                run_id: None,
                preprocess_version: None,
                supersedes_id: None,
                created_at: "2026-10-12T10:00:00Z",
            },
            run,
        )
        .unwrap();
        drop(store);
        if let Some(stack) = grid {
            self.exec(&format!(
                "UPDATE {{derivative}} SET grid_stack_id = {stack} WHERE id = {id}"
            ));
        }
        id
    }

    /// The scan a mask was read as made on, once its parts are read.
    fn grid_of(&self, mask: i64) -> Option<i64> {
        let mut store = self.store();
        let sql = format!(
            "SELECT grid_stack_id FROM {} WHERE id = {mask}",
            store.qualified("derivative")
        );
        store.query(&sql, &[]).unwrap()[0].opt_int(0).unwrap()
    }

    /// Mark a run as one whose engine went away with every unit in flight,
    /// as the lane finds it, so `nils run --resume` takes it up again.
    fn interrupt(&self, run: i64) {
        self.exec(&format!(
            "UPDATE {{pipeline_run}} SET status = 'interrupted', results_digest = NULL, \
             finished_at = NULL WHERE id = {run};\n\
             UPDATE {{pipeline_unit}} SET state = 'queued', outcome = NULL, exit_code = NULL, \
             finished_at = NULL WHERE run_id = {run}"
        ));
    }

    /// A run taken up again; answers each unit's outcome.
    fn resume(&self, run: i64) -> Vec<Value> {
        self.interrupt(run);
        let (_, out, err) = self.run(&["run", "--resume", &run.to_string(), "--json"], None);
        let mut store = self.store();
        let sql = format!(
            "SELECT CAST(outcome AS TEXT) FROM {} WHERE run_id = {run} ORDER BY id",
            store.qualified("pipeline_unit")
        );
        let outcomes: Vec<Value> = store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| {
                r.opt_text(0)
                    .unwrap()
                    .map_or(Value::Null, |t| serde_json::from_str(t).unwrap())
            })
            .collect();
        assert!(
            !outcomes.is_empty() && outcomes.iter().all(|o| o.is_object()),
            "run {run} was not taken up again: {err}\n{out}\n{outcomes:?}"
        );
        outcomes
    }

    /// Strip a release to what a release made before releases froze their
    /// picks holds: no frozen list, and no `frozen` in what it recorded.
    fn made_before(&self, release: i64) {
        let mut doc = self.picks_of(release);
        doc.as_object_mut().unwrap().remove("frozen");
        let text = doc.to_string().replace('\'', "''");
        self.exec(&format!(
            "DELETE FROM {{release_pick}} WHERE release_id = {release};\n\
             UPDATE {{release}} SET pick_rules = '{text}' WHERE id = {release}"
        ));
    }
}

/// A `nils serve` with its worker, killed when dropped.
struct Served {
    child: std::process::Child,
    port: u16,
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Served {
    fn start(lab: &Lab) -> Served {
        use std::io::BufRead as _;
        let child = Command::new(env!("CARGO_BIN_EXE_nils"))
            .arg("--registry")
            .arg(lab.home.path())
            .args([
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "2",
                "--worker",
            ])
            .args(["--auth", "token", "--pack-dir", &packs()])
            .env("NILS_TOKENS", format!("{OPS}=ops@lab:admin"))
            .env("NILS_PACK_DIR", packs())
            .env("PATH", &lab.path)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .env_remove("NILS_JOB_ID")
            .env_remove("NILS_JOB_DETAIL")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut held = Served { child, port: 0 };
        let stdout = held.child.stdout.take().unwrap();
        let mut lines = std::io::BufReader::new(stdout).lines();
        let Some(Ok(first)) = lines.next() else {
            panic!("nils serve did not listen");
        };
        let addr = first.split_whitespace().nth(2).unwrap();
        held.port = addr.rsplit(':').next().unwrap().parse().unwrap();
        std::thread::spawn(move || for _ in lines {});
        held
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        use std::io::Read as _;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\nAuthorization: Bearer {OPS}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let response = String::from_utf8_lossy(&bytes).to_string();
        let (headers, text) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status: u16 = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (
            status,
            serde_json::from_str(text).unwrap_or(Value::String(text.to_string())),
        )
    }

    fn ok(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, doc) = self.call(method, path, body);
        assert!(
            (200..300).contains(&status),
            "{method} {path}: {status} {doc}"
        );
        doc
    }

    /// A job once it is over, three minutes at most.
    fn over(&self, job: i64) -> Value {
        for _ in 0..1800 {
            let (_, j) = self.call("GET", &format!("/api/jobs/{job}"), None);
            if matches!(j["state"].as_str(), Some("done" | "failed" | "cancelled")) {
                return j;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("job {job} did not end");
    }

    /// Save a scope's rules with these kinds first as the version after
    /// `based_on`, and wait for its run.
    fn save(&self, scope: &str, kinds: &[&str], based_on: i64) {
        let doc = self.ok("GET", &format!("/api/picks/rules?{scope}"), None);
        let mut rules = doc["current"]["rules"].clone();
        rules["roles"]["t1w"]["kinds_in_order"] = json!(kinds);
        rules["roles"]["t1w"]["keep_alike"] = json!("across_the_data");
        let saved = self.ok(
            "POST",
            &format!("/api/picks/rules?{scope}"),
            Some(json!({"rules": rules, "reason": "a release freeze test", "based_on": based_on})),
        );
        assert_eq!(saved["version"], based_on + 1, "{saved}");
        let job = self.over(saved["job"].as_i64().unwrap());
        assert_eq!(job["state"], "done", "{job}");
    }

    /// A check of a run's masks, as the preview answers it.
    fn preview(&self, run: i64) -> Value {
        self.ok("POST", "/api/campaigns/preview", Some(check_body(run)))
    }
}

/// A check of a run's masks, with the made-on scan and the visit's T1w.
fn check_body(run: i64) -> Value {
    json!({
        "name": format!("check-{run}"),
        "question": {"kind": "check", "part_set": "lesions", "grain": "result",
                     "reasons": [{"id": "under", "words": "Under-segmented"}]},
        "source": {"run": run, "output": "lesions"},
        "panels": ["made_on", "t1w"],
        "raters": ["anna@desk.example"],
        "raters_per_item": 1,
        "closes_into": "check",
    })
}

/// The Postgres DSN of the tests, and a schema of this file's dropped
/// before and after `run`.
fn on_postgres(schema: &str, run: impl FnOnce(Option<(String, String)>)) {
    let Some(dsn) = std::env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    let drop = || {
        let mut store = Store::connect_postgres(&dsn, schema).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; DROP SCHEMA IF EXISTS {schema}_linkage CASCADE"
            ))
            .expect("drop");
    };
    drop();
    run(Some((dsn.clone(), schema.to_string())));
    drop();
}

#[test]
fn every_reader_of_a_release_reads_the_picks_it_froze() {
    frozen(None);
}

#[test]
fn every_reader_of_a_release_reads_the_picks_it_froze_on_postgres_too() {
    on_postgres("nils_release_freeze", frozen);
}

/// One run's readers: the mask with no grid is read onto the unit's T1w as
/// the release froze it, and the mask made on the visit's other scan shows
/// that T1w beside it.
struct Masks {
    run: i64,
    release: i64,
    /// The mask whose grid is read by its role, and the stack it should
    /// be read onto.
    by_role: (i64, i64),
    /// The mask made on another scan of the visit, and that scan.
    beside: (i64, i64),
}

/// Masks of a run's first session: one read by the output's grid role, one
/// made on the session's `other` scan.
fn masks_of(
    lab: &Lab,
    run: i64,
    release: i64,
    t1w: &BTreeSet<i64>,
    other: &BTreeSet<i64>,
) -> Masks {
    let placed = lab.placed(release);
    let (main, dir, subject) = placed
        .iter()
        .find(|(s, _, _)| t1w.contains(s))
        .cloned()
        .unwrap_or_else(|| panic!("release {release} placed no main T1w: {placed:?}"));
    let session = dir.split('/').take(2).collect::<Vec<_>>().join("/");
    let beside_scan = lab
        .ints(&format!(
            "SELECT st.id FROM {{stack}} st JOIN {{series}} se ON se.id = st.series_id \
             WHERE se.study_id = (SELECT se2.study_id FROM {{stack}} st2 \
             JOIN {{series}} se2 ON se2.id = st2.series_id WHERE st2.id = {main})"
        ))
        .into_iter()
        .find(|s| other.contains(s))
        .unwrap_or_else(|| panic!("the session of {main} holds none of {other:?}"));
    let by_role = lab.mask(run, &dir, subject, "a", None);
    let beside = lab.mask(
        run,
        &format!("{session}/anat"),
        subject,
        "b",
        Some(beside_scan),
    );
    Masks {
        run,
        release,
        by_role: (by_role, main),
        beside: (beside, beside_scan),
    }
}

fn frozen(pg: Option<(String, String)>) {
    if !have("dcm2niix") {
        eprintln!(
            "dcm2niix is not installed; a run's bids input is released with it, so this test is skipped"
        );
        return;
    }
    let lab = Lab::new(pg);
    let who = lab.subjects();
    let (a1, a2, b1, b2) = (who["A1"], who["A2"], who["B1"], who["B2"]);
    let codes: BTreeMap<i64, String> = {
        let mut store = lab.store();
        let sql = format!("SELECT id, code FROM {}", store.qualified("subject"));
        store
            .query(&sql, &[])
            .unwrap()
            .iter()
            .map(|r| (r.int(0).unwrap(), r.text(1).unwrap().to_string()))
            .collect()
    };
    lab.ok(&["clinical", "cohort", "make", "trio"]);
    lab.ok(&[
        "clinical",
        "cohort",
        "add",
        "trio",
        &codes[&a2],
        &codes[&b1],
        "--why",
        "a release freeze test",
    ]);
    let p = packs();
    for (name, subjects) in [("alpha-all", vec![a1, a2]), ("beta-all", vec![b1, b2])] {
        let ids: Vec<i64> = lab
            .ints(&format!(
                "SELECT stack_id FROM {{stack_fingerprint}} WHERE subject_id IN ({})",
                list(&subjects)
            ))
            .into_iter()
            .collect();
        let file = lab.work.path().join(format!("{name}.json"));
        std::fs::write(
            &file,
            json!({
                "ast_version": 1,
                "params": {"ids": {"type": "list", "value": ids}},
                "sets": {"s": {"grain": "stack", "where": [["in", {}, ["field", {}, "id"], ["param", {}, "ids"]]]}},
                "out": {"set": "s", "level": "record"},
            })
            .to_string(),
        )
        .unwrap();
        lab.ok(&[
            "ask",
            "selections",
            "save",
            "--name",
            name,
            "--file",
            file.to_str().unwrap(),
            "--pack-dir",
            &p,
        ]);
    }

    // 1. the first picks: alpha's own rules take the 2D SE, trio's the 2D
    // TSE; beta reads the pack's, the 3D MPRAGE
    let server = Served::start(&lab);
    server.save("dataset=alpha", &["2D SE", "2D TSE", "3D MPRAGE"], 1);
    server.save("cohort=trio", &["2D TSE", "2D SE", "3D MPRAGE"], 1);
    let alpha_se = lab.stacks_of(&[a1, a2], "SE");
    let trio_tse = lab.stacks_of(&[a2, b1], "TSE");
    let beta_mprage = lab.stacks_of(&[b1, b2], "MPRAGE");
    assert_eq!(
        (alpha_se.len(), trio_tse.len(), beta_mprage.len()),
        (3, 2, 2)
    );

    // 2. the first releases: of each scope, and a run's input over each,
    // each freezing what it read
    let rel_alpha = lab.release("rel-alpha", &["--dataset", "alpha"]);
    let rel_trio = lab.release("rel-trio", &["--cohort", "trio"]);
    let rel_beta = lab.release("rel-beta", &["--dataset", "beta"]);
    for (release, scope, stacks) in [
        (rel_alpha, Some(("dataset", "alpha")), &alpha_se),
        (rel_trio, Some(("cohort", "trio")), &trio_tse),
        (rel_beta, None, &beta_mprage),
    ] {
        assert_eq!(&lab.frozen(release, "t1w"), stacks, "release {release}");
        let read = lab.picks_of(release);
        match scope {
            Some((kind, name)) => {
                assert_eq!(read["scope"]["kind"], kind, "{read}");
                assert_eq!(read["scope"]["name"], name, "{read}");
                assert_eq!(read["version"], 2, "{read}");
            }
            None => assert_eq!(read["defaults"], true, "the pack's: {read}"),
        }
        let n = stacks.len();
        assert_eq!(
            read["frozen"],
            json!({"visits": n, "picks": n, "stacks": n}),
            "{read}"
        );
    }
    let (run_a, in_a) = lab.pipeline_run("selection:alpha-all@1");
    let (run_t, in_t) = lab.pipeline_run("main:t1w@cohort:trio");
    let (run_b, in_b) = lab.pipeline_run("selection:beta-all@1");
    // two more over alpha and beta, to be made into releases from before
    // releases froze their picks
    let (old_a, old_in_a) = lab.pipeline_run("selection:alpha-all@1");
    let (old_b, old_in_b) = lab.pipeline_run("selection:beta-all@1");
    assert_eq!(lab.frozen(in_a, "t1w"), alpha_se, "run {run_a}'s input");
    assert_eq!(lab.frozen(in_t, "t1w"), trio_tse, "run {run_t}'s input");
    assert_eq!(lab.frozen(in_b, "t1w"), beta_mprage, "run {run_b}'s input");
    // a run's list says its role, and none of it was frozen from the list
    // alone, since its version's picks were there
    let read = lab.picks_of(in_t);
    assert_eq!(read["main"], "t1w", "{read}");
    assert_eq!(read["frozen"]["listed"], 0, "{read}");

    let alpha_mprage = lab.stacks_of(&[a1, a2], "MPRAGE");
    let trio_mprage = lab.stacks_of(&[a2, b1], "MPRAGE");
    let beta_se = lab.stacks_of(&[b1, b2], "SE");
    let runs = [
        masks_of(&lab, run_a, in_a, &alpha_se, &alpha_mprage),
        masks_of(&lab, run_t, in_t, &trio_tse, &trio_mprage),
        masks_of(&lab, run_b, in_b, &beta_mprage, &beta_se),
    ];
    let old = [
        masks_of(&lab, old_a, old_in_a, &alpha_se, &alpha_mprage),
        masks_of(&lab, old_b, old_in_b, &beta_mprage, &beta_se),
    ];

    // a release from before reads the live picks of what it recorded while
    // they are there
    for m in &old {
        lab.made_before(m.release);
        assert!(lab.frozen(m.release, "t1w").is_empty());
        lab.ok(&["derivative", "parts", "--run", &m.run.to_string(), "--json"]);
        assert_eq!(
            lab.grid_of(m.by_role.0),
            Some(m.by_role.1),
            "run {}'s mask, by the live picks",
            m.run
        );
        let pre = server.preview(m.run);
        assert_eq!(pre["panels_gone"], json!([]), "{pre}");
        assert_eq!(pre["panels_missing"], json!({}), "{pre}");
        assert_eq!(pre["panels"]["most"], 2, "{pre}");
    }

    // 3. the second picks: alpha's and trio's rules change and their runs
    // drop the first version's picks; a sort takes the MPRAGE out of the
    // T1w, and the pack's run makes its picks again
    server.save("dataset=alpha", &["2D TSE", "2D SE", "3D MPRAGE"], 2);
    server.save("cohort=trio", &["2D SE", "2D TSE", "3D MPRAGE"], 2);
    let first =
        lab.int("SELECT COUNT(*) FROM {pick} WHERE rules_version = 2 AND author_kind = 'agent'");
    assert_eq!(first, 0, "the first version's picks are gone");
    lab.exec(&format!(
        "DELETE FROM {{classification_axis}} WHERE axis = 'role' AND stack_id IN ({})",
        list(lab.stacks_of(&[a1, a2, b1, b2], "MPRAGE"))
    ));
    lab.ok(&["pick", "run", "--pack-dir", &p]);
    let live = lab.ints(&format!(
        "SELECT ps.stack_id FROM {{pick_stack}} ps JOIN {{pick}} p ON p.id = ps.pick_id \
         WHERE p.rules_scope_kind IS NULL AND p.withdrawn_at IS NULL AND p.subject_id IN ({})",
        list([b1, b2])
    ));
    assert!(
        !live.is_empty() && live.is_disjoint(&beta_mprage),
        "the pack's picks moved off the MPRAGE: {live:?}"
    );

    // 4. every reader of the first releases still names the first picks
    for (release, stacks) in [
        (rel_alpha, &alpha_se),
        (rel_trio, &trio_tse),
        (rel_beta, &beta_mprage),
        (in_a, &alpha_se),
        (in_t, &trio_tse),
        (in_b, &beta_mprage),
    ] {
        assert_eq!(&lab.frozen(release, "t1w"), stacks, "release {release}");
    }
    // the releases door and the history say what each read and kept
    let doors = server.ok("GET", "/api/releases?runs=true&limit=200", None);
    let row = |id: i64| -> Value {
        doors["releases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no release {id}: {doors}"))
    };
    assert_eq!(row(rel_alpha)["picks"]["version"], 2);
    assert_eq!(row(rel_alpha)["picks"]["frozen"]["picks"], 3);
    assert_eq!(row(rel_trio)["picks"]["scope"]["name"], "trio");
    assert_eq!(row(rel_beta)["picks"]["defaults"], true);
    assert!(row(old_in_a)["picks"]["frozen"].is_null());
    let doc = lab.json(&["release", "--history", "--json", "--name", "rel-trio"]);
    assert_eq!(
        doc["releases"][0]["picks"]["frozen"],
        json!({"visits": 2, "picks": 2, "stacks": 2}),
        "{doc}"
    );
    let history = lab.ok(&["release", "--history", "--name", "rel-alpha"]);
    assert!(
        history.contains("dataset alpha's rules, version 2; kept: 3 picks of 3 visits, 3 stacks"),
        "{history}"
    );
    let history = lab.ok(&[
        "release",
        "--history",
        "--runs",
        "--name",
        &format!("pipeline-run-{old_a}"),
    ]);
    assert!(history.contains("not kept"), "{history}");

    // a mask's grid, by the output's role: the unit's first main T1w; one
    // made on a scan named keeps it
    for m in &runs {
        lab.ok(&["derivative", "parts", "--run", &m.run.to_string(), "--json"]);
        assert_eq!(
            lab.grid_of(m.by_role.0),
            Some(m.by_role.1),
            "run {}'s mask is read onto the T1w its input release froze",
            m.run
        );
        assert_eq!(lab.grid_of(m.beside.0), Some(m.beside.1), "run {}", m.run);
    }
    // a check's panels: the visit's T1w as the run's input release froze it
    for m in &runs {
        let pre = server.preview(m.run);
        assert_eq!(pre["panels_gone"], json!([]), "run {}: {pre}", m.run);
        assert_eq!(pre["panels_missing"], json!({}), "run {}: {pre}", m.run);
        assert_eq!(pre["panels"]["most"], 2, "run {}: {pre}", m.run);
        let made = server.ok("POST", "/api/campaigns", Some(check_body(m.run)));
        let campaign = made["id"].as_i64().unwrap_or_else(|| panic!("{made}"));
        let shown = lab.ints(&format!(
            "SELECT stack_id FROM {{campaign_check_panel}} WHERE campaign_id = {campaign} AND role = 't1w'"
        ));
        assert_eq!(
            shown,
            BTreeSet::from([m.by_role.1]),
            "run {}'s check shows the T1w its input release froze beside the other scan",
            m.run
        );
    }
    drop(server);
    // a run taken up again: no unit is missing its T1w
    for m in &runs {
        for o in lab.resume(m.run) {
            assert!(
                !o["error"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("missing an input"),
                "run {}: {o}",
                m.run
            );
        }
    }

    // 5. a release from before whose picks are gone says so plainly
    let server = Served::start(&lab);
    for (m, why) in [
        (
            &old[0],
            "dataset alpha's main scans under its own rules, version 2, which version 3 replaced",
        ),
        (&old[1], "which no longer name 2 of the stacks it holds"),
    ] {
        let pre = server.preview(m.run);
        let gone = &pre["panels_gone"][0];
        assert!(
            gone["why"].as_str().is_some_and(
                |w| w.contains(why) && w.contains(&format!("run {}'s input release", m.run))
            ),
            "{pre}"
        );
        assert_eq!(gone["results"], 2, "{pre}");
        assert_eq!(pre["panels"]["most"], 1, "the made-on scan alone: {pre}");
    }
    drop(server);
    for (m, why) in [
        (
            &old[0],
            "which of its stacks stands for t1w is not known: the picks its input release read are gone (dataset alpha's main scans under its own rules, version 2, which version 3 replaced)",
        ),
        (&old[1], "which no longer name 2 of the stacks it holds"),
    ] {
        // none of its units can run, so taking it up again is refused
        // before it starts, saying why of each unit (a run taken up again is
        // checked as a new run is)
        lab.interrupt(m.run);
        let pre = lab.json(&[
            "run",
            "--resume",
            &m.run.to_string(),
            "--preflight",
            "--json",
        ]);
        assert_eq!(pre["units"]["ready"], 0, "run {}: {pre}", m.run);
        let missing = pre["missing"].as_array().unwrap();
        assert!(!missing.is_empty(), "run {}: {pre}", m.run);
        for u in missing {
            assert!(
                u["why"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|w| w.as_str().is_some_and(|w| w.contains(why))),
                "run {}: {u}",
                m.run
            );
        }
        let (good, out, err) = lab.run(&["run", "--resume", &m.run.to_string(), "--json"], None);
        assert!(!good, "run {}: {out}", m.run);
        assert!(
            err.contains("refused before it started") && err.contains(why),
            "run {}: {err}",
            m.run
        );
        let ran = lab.int(&format!(
            "SELECT COUNT(*) FROM {{pipeline_unit}} WHERE run_id = {} AND outcome IS NOT NULL",
            m.run
        ));
        assert_eq!(ran, 0, "run {}: no unit was taken up", m.run);
    }
}
