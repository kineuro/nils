// SPDX-License-Identifier: AGPL-3.0-only
//! The pick borders study of 2026-10-10, R2: scans that are never a
//! session's main scan, measured. A synthetic registry (`nils synth`) has
//! stacks of its first studies rewritten to the scans the study names (a
//! 6-slice and a 9-slice planning series, a tri-plane scout split into
//! single images, a TOF and a time-resolved contrast MRA, a 5-slice by
//! 11-phase dynamic, a 130 mm inner-ear slab and a 170 mm orbit series) and
//! to ordinary brain scans that keep their roles. The registry is sorted and
//! picked by the pack as it was before the rules (the shipped pack with
//! them taken out), `nils pack rehearse` reports what the rules' typed
//! operations move, and a re-sort and a pick run by the shipped pack must
//! move exactly that. Every stack is invented. On SQLite always, and on
//! Postgres where a test DSN is set.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use nils_dicom::synth::TempDir;
use nils_registry::Store;
use serde_json::Value;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/patches")
        .join(name)
}

struct Home {
    dir: TempDir,
    pg: Option<(String, String)>,
}

impl Home {
    fn run(&self, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
        let mut child = nils()
            .arg("--registry")
            .arg(self.dir.path())
            .args(args)
            .env("USER", "anna")
            .env("HOSTNAME", "ward-3")
            .env_remove("NILS_DSN")
            .env_remove("NILS_PACK_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(text) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
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
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}: {out}"))
    }

    fn store(&self) -> Store {
        match &self.pg {
            Some((dsn, schema)) => Store::connect_postgres(dsn, schema).unwrap(),
            None => Store::open_sqlite(&self.dir.path().join("registry.db")).unwrap(),
        }
    }
}

/// A synthetic registry with its sessions built.
fn registry(pg: Option<(String, String)>) -> Home {
    let home = Home {
        dir: TempDir::new("not-main-home"),
        pg,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a not-main-scans test key\n"));
    assert!(good, "{err}");
    match &home.pg {
        Some((dsn, schema)) => {
            home.ok(&[
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
            home.ok(&["init", "--key", "k"]);
        }
    }
    home.ok(&["synth", "--seed", "5", "--subjects", "30"]);
    home.ok(&["session", "rebuild"]);
    home
}

fn postgres(schema: &str, test: impl FnOnce(&Home)) {
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
    test(&registry(Some((dsn.clone(), schema.to_string()))));
    drop();
}

// ---------------------------------------------------------------------------
// The scans, as the fingerprint holds them (cases of the pack's
// `corpus/planning-scans.yml` and `corpus/never-a-main-scan.yml`).

type Fields = &'static [(&'static str, &'static str)];

const PLANNING_6: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND"),
    ("scanning_sequence", "SE"),
    ("sequence_variant", "SK\\SP"),
    ("text_sequence_name", "*se2d1"),
    ("text_series_description", "loc sag t1 se"),
    ("mr_acquisition_type", "2D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "12"),
    ("repetition_time", "450"),
    ("flip_angle", "90"),
    ("n_instances", "6"),
    ("n_slices", "6"),
    ("fov_x", "230"),
    ("fov_y", "230"),
    ("orientation", "Sagittal"),
    ("image_role", "original_primary"),
];

const PLANNING_9: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M_SE\\M\\SE"),
    ("scanning_sequence", "SE"),
    ("sequence_variant", "SK"),
    ("mr_acquisition_type", "2D"),
    ("manufacturer", "Philips Medical Systems"),
    ("text_series_description", "T2W_TSE survey"),
    ("text_protocol_name", "T2W_TSE survey"),
    ("echo_time", "100"),
    ("repetition_time", "3000"),
    ("flip_angle", "90"),
    ("echo_train_length", "15"),
    ("n_instances", "9"),
    ("n_slices", "9"),
    ("fov_x", "230"),
    ("fov_y", "230"),
    ("orientation", "Axial"),
    ("image_role", "original_primary"),
];

/// One plane of a tri-plane scout split into single images, with no slice
/// positions counted.
const TRI_PLANE: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND"),
    ("scanning_sequence", "GR"),
    ("sequence_variant", "SP"),
    ("text_sequence_name", "*fl2d1"),
    ("text_series_description", "t1 fl2d tripilot"),
    ("mr_acquisition_type", "2D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "5"),
    ("repetition_time", "20"),
    ("flip_angle", "40"),
    ("n_instances", "1"),
    ("stacks_in_series", "3"),
    ("split_reason", "multi_orientation"),
    ("fov_x", "250"),
    ("fov_y", "250"),
    ("image_role", "original_primary"),
];

const TOF: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND"),
    ("scanning_sequence", "GR"),
    ("sequence_variant", "SP\\OSP"),
    ("text_sequence_name", "*fl3d1r_t70"),
    ("text_series_description", "tof_3d_multi-slab"),
    ("mr_acquisition_type", "3D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "3.6"),
    ("repetition_time", "22"),
    ("flip_angle", "18"),
    ("n_instances", "160"),
    ("n_slices", "160"),
    ("fov_x", "200"),
    ("fov_y", "181"),
    ("orientation", "Axial"),
    ("image_role", "original_primary"),
];

/// A time-resolved contrast MRA of 20 phases.
const TWIST: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND"),
    ("scanning_sequence", "GR"),
    ("sequence_variant", "SP"),
    ("text_sequence_name", "*fl3d1_ns"),
    ("text_series_description", "twist mra cor +c"),
    ("mr_acquisition_type", "3D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "1.2"),
    ("repetition_time", "3"),
    ("flip_angle", "25"),
    ("n_instances", "1440"),
    ("n_slices", "72"),
    ("temporal_positions", "20"),
    ("temporal_resolution", "2500"),
    ("fov_x", "400"),
    ("fov_y", "300"),
    ("orientation", "Coronal"),
    ("image_role", "original_primary"),
];

/// A pituitary dynamic of 5 slices by 11 phases 20 s apart: 55 images.
const DYNAMIC: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND"),
    ("scanning_sequence", "SE"),
    ("sequence_variant", "SK\\SP"),
    ("text_sequence_name", "*tse2d1_3"),
    ("text_series_description", "t1 tse cor dyn hypofys"),
    ("mr_acquisition_type", "2D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "10"),
    ("repetition_time", "400"),
    ("flip_angle", "150"),
    ("n_instances", "55"),
    ("n_slices", "5"),
    ("temporal_positions", "11"),
    ("temporal_resolution", "20000"),
    ("fov_x", "180"),
    ("fov_y", "180"),
    ("orientation", "Coronal"),
    ("image_role", "original_primary"),
];

/// A 3D T2 slab of the inner ears, 130 mm across.
const INNER_EAR: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND"),
    ("scanning_sequence", "SE"),
    ("sequence_variant", "SK\\SP\\OSP"),
    ("text_sequence_name", "*spcR_282ns"),
    ("text_series_description", "t2_spc_tra inner ear"),
    ("mr_acquisition_type", "3D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "139"),
    ("repetition_time", "1200"),
    ("flip_angle", "120"),
    ("n_instances", "64"),
    ("n_slices", "64"),
    ("fov_x", "130"),
    ("fov_y", "130"),
    ("orientation", "Axial"),
    ("image_role", "original_primary"),
];

/// A T2 of the orbits, 170 mm across.
const ORBIT: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND\\NORM"),
    ("scanning_sequence", "SE"),
    ("sequence_variant", "SK\\SP\\OSP"),
    ("text_sequence_name", "*tse2d1_15"),
    ("text_series_description", "t2_tse_cor orbita"),
    ("mr_acquisition_type", "2D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "89"),
    ("repetition_time", "4000"),
    ("flip_angle", "150"),
    ("n_instances", "20"),
    ("n_slices", "20"),
    ("fov_x", "170"),
    ("fov_y", "170"),
    ("orientation", "Coronal"),
    ("image_role", "original_primary"),
];

/// The ordinary brain scans, with every field the rules read: one temporal
/// position, fields of view at and above 175 mm.
const MPRAGE: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND\\NORM"),
    ("scanning_sequence", "GR"),
    ("sequence_variant", "SK\\SP\\MP"),
    ("text_sequence_name", "*tfl3d1_16"),
    ("text_series_description", "t1_mprage_sag"),
    ("mr_acquisition_type", "3D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "2.98"),
    ("repetition_time", "2300"),
    ("inversion_time", "900"),
    ("flip_angle", "9"),
    ("n_instances", "176"),
    ("n_slices", "176"),
    ("temporal_positions", "1"),
    ("fov_x", "256"),
    ("fov_y", "256"),
    ("orientation", "Sagittal"),
    ("image_role", "original_primary"),
];

const T2_TSE: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND\\NORM"),
    ("scanning_sequence", "SE"),
    ("sequence_variant", "SK\\SP\\OSP"),
    ("text_sequence_name", "*tse2d1_17"),
    ("text_series_description", "t2_tse_tra"),
    ("mr_acquisition_type", "2D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "98"),
    ("repetition_time", "6000"),
    ("flip_angle", "150"),
    ("n_instances", "44"),
    ("n_slices", "44"),
    ("fov_x", "220"),
    ("fov_y", "175"),
    ("orientation", "Axial"),
    ("image_role", "original_primary"),
];

const FLAIR: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND\\NORM"),
    ("scanning_sequence", "SE\\IR"),
    ("sequence_variant", "SK\\SP\\MP"),
    ("text_sequence_name", "*spcir_278ns"),
    ("text_series_description", "t2_space_dark-fluid_sag"),
    ("mr_acquisition_type", "3D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "386"),
    ("repetition_time", "5000"),
    ("inversion_time", "1800"),
    ("flip_angle", "120"),
    ("n_instances", "176"),
    ("n_slices", "176"),
    ("fov_x", "250"),
    ("fov_y", "250"),
    ("orientation", "Sagittal"),
    ("image_role", "original_primary"),
];

const T1_SE: Fields = &[
    ("image_type", "ORIGINAL\\PRIMARY\\M\\ND"),
    ("scanning_sequence", "SE"),
    ("sequence_variant", "SK\\SP"),
    ("text_sequence_name", "*se2d1"),
    ("text_series_description", "ax t1 se"),
    ("mr_acquisition_type", "2D"),
    ("manufacturer", "SIEMENS"),
    ("echo_time", "12"),
    ("repetition_time", "500"),
    ("flip_angle", "90"),
    ("n_instances", "20"),
    ("n_slices", "20"),
    ("fov_x", "230"),
    ("fov_y", "230"),
    ("orientation", "Axial"),
    ("image_role", "original_primary"),
];

/// The scanner's reformat of a session's MPRAGE, typed as one, which no
/// role takes: written over the synthetic one, whose fingerprint says too
/// little for the rules to tell it from an acquisition.
const REFORMAT: Fields = &[
    ("image_type", "DERIVED\\SECONDARY\\MPR"),
    ("text_series_description", "t1_mprage_MPR_tra"),
    ("mr_acquisition_type", "3D"),
    ("manufacturer", "SIEMENS"),
    ("n_instances", "176"),
    ("orientation", "Axial"),
    ("image_role", "derived"),
];

/// The fingerprint columns the synthetic registry fills that a rule or a
/// pick reads, cleared before a scan is written over a stack, so the stack
/// is the scan and nothing of what it was. Every scan states its
/// orientation, which the table requires.
const CLEARED: &[&str] = &[
    "text_series_description",
    "text_protocol_name",
    "text_all",
    "echo_time",
    "repetition_time",
    "inversion_time",
    "flip_angle",
    "magnetic_field_strength",
    "slice_thickness",
    "spacing_between_slices",
    "mr_acquisition_type",
    "orientation_confidence",
    "rows",
    "columns",
    "pixel_spacing",
    "manufacturer_model_name",
    "station_name",
    "field_strength_normalized",
    "acquisition_type_filled",
    "acquisition_type_source",
];

/// Write a scan over a stack's fingerprint.
fn plant(store: &mut Store, stack: i64, fields: Fields, extra: &[(&str, &str)]) {
    let fp = store.qualified("stack_fingerprint");
    let mut sets: BTreeMap<&str, String> = CLEARED.iter().map(|c| (*c, "NULL".into())).collect();
    // a series of one stack, unless the scan says otherwise
    sets.insert("stacks_in_series", "1".into());
    for (k, v) in fields.iter().chain(extra) {
        sets.insert(k, format!("'{}'", v.replace('\'', "''")));
    }
    let sets: Vec<String> = sets.iter().map(|(k, v)| format!("{k} = {v}")).collect();
    store
        .execute(
            &format!(
                "UPDATE {fp} SET {} WHERE stack_id = {stack}",
                sets.join(", ")
            ),
            &[],
        )
        .unwrap();
}

/// Each subject's first study that is alone on its day, so it is one
/// session of its own: its stacks, by the synthetic protocol each was made
/// from.
fn first_studies(store: &mut Store) -> Vec<(i64, BTreeMap<&'static str, i64>)> {
    let day = store.dialect().text_of(
        nils_registry::schema::table("study")
            .column("study_date")
            .unwrap(),
    );
    let mut days: BTreeMap<(i64, String), Vec<i64>> = BTreeMap::new();
    let mut first: BTreeMap<i64, (String, i64)> = BTreeMap::new();
    for r in store
        .query(
            &format!(
                "SELECT subject_id, id, {day} FROM {} ORDER BY subject_id, {day}, id",
                store.qualified("study")
            ),
            &[],
        )
        .unwrap()
    {
        let (subject, study) = (r.int(0).unwrap(), r.int(1).unwrap());
        let d = r.opt_text(2).unwrap().unwrap_or("").to_string();
        days.entry((subject, d.clone())).or_default().push(study);
        first.entry(subject).or_insert((d, study));
    }
    let alone: BTreeSet<i64> = first
        .iter()
        .filter(|(s, (d, _))| days[&(**s, d.clone())].len() == 1)
        .map(|(_, (_, study))| *study)
        .collect();
    let rows = store
        .query(
            &format!(
                "SELECT subject_id, study_id, stack_id, text_series_description FROM {} \
                 ORDER BY subject_id, study_id, stack_id",
                store.qualified("stack_fingerprint")
            ),
            &[],
        )
        .unwrap();
    let mut out: BTreeMap<i64, BTreeMap<&'static str, i64>> = BTreeMap::new();
    for r in rows {
        let (subject, study, stack) = (r.int(0).unwrap(), r.int(1).unwrap(), r.int(2).unwrap());
        if !alone.contains(&study) {
            continue;
        }
        let name = r.opt_text(3).unwrap().unwrap_or("").to_string();
        let slot = match name.as_str() {
            "localizer" => "localizer",
            n if n.starts_with("t1_mprage_MPR") => "mpr",
            n if n.starts_with("t1_mprage") => "t1",
            n if n.starts_with("t2_space_dark-fluid") => "flair",
            n if n.starts_with("t2_tse") => "t2",
            _ => "other",
        };
        out.entry(subject).or_default().insert(slot, stack);
    }
    out.into_iter()
        .filter(|(_, slots)| {
            ["localizer", "t1", "t2", "mpr"]
                .iter()
                .all(|k| slots.contains_key(k))
        })
        .collect()
}

/// The planted sessions, each a subject's first study: what is written over
/// which of its stacks.
fn plant_all(home: &Home) {
    let mut store = home.store();
    let subjects = first_studies(&mut store);
    assert!(
        subjects.len() >= 9,
        "room for every session: {}",
        subjects.len()
    );
    let at = |i: usize, slot: &str| subjects[i].1[slot];
    // no session keeps a reformat a rule could take for its T1w
    for (i, (_, slots)) in subjects.iter().enumerate().take(9) {
        if i != 2 {
            plant(&mut store, slots["mpr"], REFORMAT, &[]);
        }
    }
    // the only T1w of its session, a 6-slice planning series
    plant(&mut store, at(0, "t1"), PLANNING_6, &[]);
    // the only T2w, a 9-slice survey
    plant(&mut store, at(1, "t2"), PLANNING_9, &[]);
    // a tri-plane scout split into three single images, the only T1w
    for (slot, plane) in [
        ("localizer", "Axial"),
        ("t1", "Coronal"),
        ("mpr", "Sagittal"),
    ] {
        plant(
            &mut store,
            at(2, slot),
            TRI_PLANE,
            &[("orientation", plane)],
        );
    }
    // angiography, the only T1w of its session
    plant(&mut store, at(3, "t1"), TOF, &[]);
    plant(&mut store, at(4, "t1"), TWIST, &[]);
    // the dynamic beside an ordinary 2D T1 spin echo
    plant(&mut store, at(5, "t1"), DYNAMIC, &[]);
    plant(&mut store, at(5, "localizer"), T1_SE, &[]);
    // small fields of view, the inner ear the only T2w of its session and
    // the orbit beside an ordinary T2
    plant(&mut store, at(6, "t2"), INNER_EAR, &[]);
    plant(&mut store, at(7, "t2"), ORBIT, &[]);
    plant(&mut store, at(7, "localizer"), T2_TSE, &[]);
    // the ordinary brain scans with every field set
    plant(&mut store, at(8, "t1"), MPRAGE, &[]);
    plant(&mut store, at(8, "t2"), T2_TSE, &[]);
    if let Some(f) = subjects[8].1.get("flair") {
        plant(&mut store, *f, FLAIR, &[]);
    }
}

// ---------------------------------------------------------------------------
// The pack before the rules, and the rules as operations.

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.path().is_dir() {
            copy(&e.path(), &to.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

fn yaml(path: &Path) -> Value {
    serde_saphyr::from_str(&std::fs::read_to_string(path).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The MRI pack's version before R2a and R2b, which the patches name; the
/// version that shipped them is the one the pack holds now.
const BEFORE: &str = "1.0.2";

/// The version the shipped MRI pack holds.
fn shipped() -> String {
    yaml(&packs().join("mri/pack.yml"))["version"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The shipped packs copied under `home` with these rules taken out of
/// these rule sets of the MRI pack, and the cases of these patches out of
/// its corpus: the pack as it was before them, at its version then.
fn packs_without(home: &Home, name: &str, rules: &[(&str, &[&str])], fixtures: &[&str]) -> PathBuf {
    let dir = home.dir.path().join(name);
    copy(&packs(), &dir);
    let manifest = dir.join("mri/pack.yml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let line = format!("version: {}", shipped());
    assert!(text.contains(&line), "{}: {line}", manifest.display());
    std::fs::write(
        &manifest,
        text.replacen(&line, &format!("version: {BEFORE}"), 1),
    )
    .unwrap();
    for f in fixtures {
        std::fs::remove_file(dir.join("mri/corpus").join(f)).unwrap();
    }
    for (file, ids) in rules {
        let path = dir.join("mri").join(file);
        let mut doc = yaml(&path);
        for id in *ids {
            assert!(
                doc["rules"].as_object_mut().unwrap().remove(*id).is_some(),
                "{file} holds the rule {id}"
            );
        }
        let order: Vec<Value> = doc["order"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| !ids.contains(&r.as_str().unwrap()))
            .cloned()
            .collect();
        doc["order"] = Value::Array(order);
        // JSON is YAML
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    }
    dir
}

/// One patch of the operations of these fixtures, written under `home`,
/// with the cases of the pack's corpus file of the same name as its own:
/// the rehearsal judges them against the patched pack.
fn patch_of(home: &Home, fixtures: &[&str]) -> PathBuf {
    let mut whole = yaml(&fixture(fixtures[0]));
    whole["cases"] = Value::Array(Vec::new());
    for (i, f) in fixtures.iter().enumerate() {
        let more = yaml(&fixture(f));
        if i > 0 {
            whole["operations"]
                .as_array_mut()
                .unwrap()
                .extend(more["operations"].as_array().unwrap().iter().cloned());
        }
        let cases = yaml(&packs().join("mri/corpus").join(f));
        whole["cases"]
            .as_array_mut()
            .unwrap()
            .extend(cases["cases"].as_array().unwrap().iter().cloned());
    }
    let path = home.dir.path().join(format!("{}.json", fixtures.join("+")));
    std::fs::write(&path, serde_json::to_string_pretty(&whole).unwrap()).unwrap();
    path
}

/// The rule sets the operations wrote, as the shipped pack holds them:
/// applying the patch to the pack before gives the pack shipped.
fn applies_to_the_shipped_pack(home: &Home, before: &Path, patch: &Path, files: &[&str]) {
    let out = home.dir.path().join(format!(
        "applied-{}",
        patch.file_stem().unwrap().to_string_lossy()
    ));
    home.ok(&[
        "pack",
        "apply",
        before.join("mri").to_str().unwrap(),
        "--ops",
        patch.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--version",
        &shipped(),
    ]);
    for f in files {
        assert_eq!(
            yaml(&out.join(f)),
            yaml(&packs().join("mri").join(f)),
            "{f}: the patch applied to the pack before is the pack shipped"
        );
    }
}

// ---------------------------------------------------------------------------
// What a sort and a pick run leave.

/// Every stored axis of every stack, values in order.
fn stored(store: &mut Store) -> BTreeMap<i64, BTreeMap<String, String>> {
    let sql = format!(
        "SELECT stack_id, axis, value FROM {} ORDER BY id",
        store.qualified("classification_axis")
    );
    let mut out: BTreeMap<i64, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for r in store.query(&sql, &[]).unwrap() {
        let Some(v) = r.opt_text(2).unwrap().filter(|v| !v.is_empty()) else {
            continue;
        };
        out.entry(r.int(0).unwrap())
            .or_default()
            .entry(r.text(1).unwrap().to_string())
            .or_default()
            .push(v.to_string());
    }
    out.into_iter()
        .map(|(s, m)| {
            let m = m
                .into_iter()
                .map(|(a, mut v)| {
                    v.sort();
                    (a, v.join(","))
                })
                .collect();
            (s, m)
        })
        .collect()
}

/// (axis, from, to) -> stacks, between two sorts.
fn diff(
    before: &BTreeMap<i64, BTreeMap<String, String>>,
    after: &BTreeMap<i64, BTreeMap<String, String>>,
) -> BTreeMap<(String, String, String), i64> {
    let mut out = BTreeMap::new();
    let none = BTreeMap::new();
    let stacks: BTreeSet<&i64> = before.keys().chain(after.keys()).collect();
    for s in stacks {
        let (b, a) = (
            before.get(s).unwrap_or(&none),
            after.get(s).unwrap_or(&none),
        );
        let axes: BTreeSet<&String> = b.keys().chain(a.keys()).collect();
        for axis in axes {
            let x = b.get(axis).cloned().unwrap_or_default();
            let y = a.get(axis).cloned().unwrap_or_default();
            if x != y {
                *out.entry((axis.clone(), x, y)).or_insert(0) += 1;
            }
        }
    }
    out
}

fn sort_csv(v: &str) -> String {
    let mut parts: Vec<&str> = v.split(',').filter(|x| !x.is_empty()).collect();
    parts.sort_unstable();
    parts.join(",")
}

/// (axis, from, to) -> stacks, from a rehearsal.
fn report_moves(doc: &Value) -> BTreeMap<(String, String, String), i64> {
    let mut out = BTreeMap::new();
    for a in doc["axes"].as_array().unwrap() {
        for t in a["transitions"].as_array().unwrap() {
            *out.entry((
                a["axis"].as_str().unwrap().to_string(),
                sort_csv(t["from"].as_str().unwrap()),
                sort_csv(t["to"].as_str().unwrap()),
            ))
            .or_insert(0) += t["stacks"].as_i64().unwrap();
        }
    }
    out
}

/// The run's picks: (role, subject, session day) -> the stacks.
fn picks(store: &mut Store) -> BTreeMap<(String, i64, String), Vec<i64>> {
    let d = store.dialect();
    let day = d.text_of(
        nils_registry::schema::table("pick")
            .column("session_day")
            .unwrap(),
    );
    let rows = store
        .query(
            &format!(
                "SELECT p.role, p.subject_id, {day}, ps.stack_id FROM {} p JOIN {} ps ON ps.pick_id = p.id \
                 WHERE p.author_kind = 'agent' AND p.withdrawn_at IS NULL ORDER BY ps.stack_id",
                store.qualified("pick"),
                store.qualified("pick_stack")
            ),
            &[],
        )
        .unwrap();
    let mut out: BTreeMap<(String, i64, String), Vec<i64>> = BTreeMap::new();
    for r in rows {
        out.entry((
            r.text(0).unwrap().to_string(),
            r.int(1).unwrap(),
            r.opt_text(2).unwrap().unwrap_or("").to_string(),
        ))
        .or_default()
        .push(r.int(3).unwrap());
    }
    out
}

/// role -> (kept, changed, removed, added), between two pick runs.
fn pick_moves(
    before: &BTreeMap<(String, i64, String), Vec<i64>>,
    after: &BTreeMap<(String, i64, String), Vec<i64>>,
) -> BTreeMap<String, [i64; 4]> {
    let mut out: BTreeMap<String, [i64; 4]> = BTreeMap::new();
    let keys: BTreeSet<&(String, i64, String)> = before.keys().chain(after.keys()).collect();
    for k in keys {
        let i = match (before.get(k), after.get(k)) {
            (Some(x), Some(y)) if x == y => 0,
            (Some(_), Some(_)) => 1,
            (Some(_), None) => 2,
            (None, Some(_)) => 3,
            (None, None) => continue,
        };
        out.entry(k.0.clone()).or_default()[i] += 1;
    }
    out
}

/// The rehearsal's pick counts in the same shape.
fn report_picks(doc: &Value) -> BTreeMap<String, [i64; 4]> {
    let mut out = BTreeMap::new();
    let model = &doc["picks"]["models"].as_array().unwrap()[0];
    for (role, c) in model["by_role"].as_object().unwrap() {
        let n = |k: &str| c[k].as_i64().unwrap();
        out.insert(
            role.clone(),
            [n("kept"), n("changed"), n("removed"), n("added")],
        );
    }
    out
}

/// The registry sorted and picked by the pack before, the rehearsal of the
/// patch, and the re-sort and pick run by the pack shipped, which must move
/// what the rehearsal said. Answers the rehearsal.
fn rehearse_and_resort(home: &Home, before: &Path, patch: &Path) -> Value {
    let before = before.to_str().unwrap();
    plant_all(home);
    home.ok(&["classify", "--pack-dir", before, "--no-previews"]);
    home.ok(&["pick", "run", "--pack-dir", before, "--json"]);
    let mut store = home.store();
    let sorted = stored(&mut store);
    let picked = picks(&mut store);

    let doc = home.json(&[
        "pack",
        "rehearse",
        "--ops",
        patch.to_str().unwrap(),
        "--pack-dir",
        before,
        "--examples",
        "20",
        "--json",
    ]);
    assert_eq!(
        doc["patch"]["cases"]["held"], true,
        "the corpus cases hold on the patched pack: {}",
        doc["patch"]["cases"]
    );
    assert_eq!(doc["ships"]["as"], "rules release", "{doc}");

    let shipped = packs();
    let shipped = shipped.to_str().unwrap();
    home.ok(&["classify", "--pack-dir", shipped, "--no-previews"]);
    // A pick run rewrites the run's pick of an occasion that has a winner
    // and leaves the one of an occasion that no longer has one, so the
    // run's picks go first: what is compared is what the pack picks.
    for t in ["pick_stack", "pick"] {
        let t = store.qualified(t);
        store.execute(&format!("DELETE FROM {t}"), &[]).unwrap();
    }
    home.ok(&["pick", "run", "--pack-dir", shipped, "--json"]);
    let resorted = stored(&mut store);
    let repicked = picks(&mut store);
    assert_eq!(
        report_moves(&doc),
        diff(&sorted, &resorted),
        "the rehearsal moves what the shipped pack's sort moves"
    );
    let moved = pick_moves(&picked, &repicked);
    let reported = report_picks(&doc);
    for (role, m) in &moved {
        assert_eq!(
            reported.get(role).map(|r| [r[1], r[2], r[3]]),
            Some([m[1], m[2], m[3]]),
            "{role}: the rehearsal's picks changed, removed and added are the pick run's: {}",
            doc["picks"]
        );
    }
    doc
}

/// (axis, from, to) -> stacks, the moves expected.
fn moves(rows: &[(&str, &str, &str, i64)]) -> BTreeMap<(String, String, String), i64> {
    rows.iter()
        .map(|(a, f, t, n)| ((a.to_string(), f.to_string(), t.to_string()), *n))
        .collect()
}

// ---------------------------------------------------------------------------
// R2a: a planning scan is no main scan.

const R2A: (&str, &[&str]) = (
    "rules/disposition.yml",
    &[
        "planning_named_loc",
        "planning_named_localizer",
        "planning_named_scout",
        "planning_named_survey",
        "planning_named_sikt",
        "planning_tri_plane",
    ],
);

fn planning_scans(home: &Home) {
    let before = packs_without(home, "packs-before", &[R2A], &["planning-scans.yml"]);
    let patch = patch_of(home, &["planning-scans.yml"]);
    applies_to_the_shipped_pack(home, &before, &patch, &["rules/disposition.yml"]);
    let doc = rehearse_and_resort(home, &before, &patch);
    // the 6-slice and the 9-slice planning series and the three planes of
    // the tri-plane scout, and nothing else
    assert_eq!(
        report_moves(&doc),
        moves(&[
            ("disposition", "acquisition", "scout", 5),
            ("role", "t1w", "", 4),
            ("role", "t2w", "", 1),
        ]),
        "{}",
        doc["axes"]
    );
    assert_eq!(doc["moved"]["stacks"], 5, "{}", doc["moved"]);
    // The two planning series were the only T1w and T2w of their sessions,
    // which lose the pick. The tri-plane scout's single images were never
    // picked (a fragment of one image is set aside), so its session had no
    // pick to lose. Nothing else moves: the dynamic holds no role already
    // (R2b), and the pick's rules take the T1 spin echo beside it anyway.
    let picks = report_picks(&doc);
    assert_eq!(picks["t1w"][1..], [0, 1, 0], "{}", doc["picks"]);
    assert_eq!(picks["t2w"][1..], [0, 1, 0], "{}", doc["picks"]);
    assert_eq!(picks["flair"][1..], [0, 0, 0], "{}", doc["picks"]);
}

#[test]
fn a_planning_scan_is_no_main_scan() {
    planning_scans(&registry(None));
}

#[test]
fn a_planning_scan_is_no_main_scan_on_postgres_too() {
    postgres("nils_not_main_planning", planning_scans);
}

// ---------------------------------------------------------------------------
// R2b: what holds no role.

const R2B: (&str, &[&str]) = (
    "rules/role.yml",
    &[
        "angiography_tof",
        "angiography_ce",
        "angiography_pc",
        "dynamic",
        "small_field",
    ],
);

fn never_main(home: &Home) {
    let before = packs_without(home, "packs-before", &[R2B], &["never-a-main-scan.yml"]);
    let patch = patch_of(home, &["never-a-main-scan.yml"]);
    applies_to_the_shipped_pack(home, &before, &patch, &["rules/role.yml"]);
    let doc = rehearse_and_resort(home, &before, &patch);
    // the TOF, the contrast MRA and the dynamic lose the T1w role, the inner
    // ear and the orbit the T2w; nothing else moves
    assert_eq!(
        report_moves(&doc),
        moves(&[("role", "t1w", "", 3), ("role", "t2w", "", 2)]),
        "{}",
        doc["axes"]
    );
    assert_eq!(doc["moved"]["stacks"], 5, "{}", doc["moved"]);
    // The TOF, the contrast MRA and the inner ear were the only candidates
    // of their sessions. The dynamic and the orbit each had an ordinary scan
    // beside them that the pick already took (the pick's rules, version 1,
    // take the 2D SE kind before the dynamic's 2D TSE), so those picks stay.
    let picks = report_picks(&doc);
    assert_eq!(picks["t1w"][1..], [0, 2, 0], "{}", doc["picks"]);
    assert_eq!(picks["t2w"][1..], [0, 1, 0], "{}", doc["picks"]);
    assert_eq!(picks["flair"][1..], [0, 0, 0], "{}", doc["picks"]);
}

#[test]
fn an_angiography_a_dynamic_and_a_small_slab_hold_no_role() {
    never_main(&registry(None));
}

#[test]
fn an_angiography_a_dynamic_and_a_small_slab_hold_no_role_on_postgres_too() {
    postgres("nils_not_main_never", never_main);
}

// ---------------------------------------------------------------------------
// R2 whole: both, on the pack before either.

fn not_main(home: &Home) {
    let before = packs_without(
        home,
        "packs-before",
        &[R2A, R2B],
        &["planning-scans.yml", "never-a-main-scan.yml"],
    );
    let patch = patch_of(home, &["planning-scans.yml", "never-a-main-scan.yml"]);
    applies_to_the_shipped_pack(
        home,
        &before,
        &patch,
        &["rules/disposition.yml", "rules/role.yml"],
    );
    let doc = rehearse_and_resort(home, &before, &patch);
    // every planted scan but the ordinary ones: ten stacks
    assert_eq!(
        report_moves(&doc),
        moves(&[
            ("disposition", "acquisition", "scout", 5),
            ("role", "t1w", "", 7),
            ("role", "t2w", "", 3),
        ]),
        "{}",
        doc["axes"]
    );
    assert_eq!(doc["moved"]["stacks"], 10, "{}", doc["moved"]);
    // Removed: the 6-slice planning series, the TOF and the contrast MRA
    // (T1w), the 9-slice survey and the inner ear (T2w). The dynamic's
    // session keeps the T1 spin echo beside it, which the pick's rules
    // (version 1) take by its kind before either rule, and the orbit's its
    // ordinary T2; every other session keeps its pick.
    let picks = report_picks(&doc);
    assert_eq!(picks["t1w"][1..], [0, 3, 0], "{}", doc["picks"]);
    assert_eq!(picks["t2w"][1..], [0, 2, 0], "{}", doc["picks"]);
    assert_eq!(picks["flair"][1..], [0, 0, 0], "{}", doc["picks"]);
    assert_eq!(doc["picks"]["moved"], 5, "{}", doc["picks"]);
}

#[test]
fn never_a_main_scan_as_a_whole() {
    not_main(&registry(None));
}

#[test]
fn never_a_main_scan_as_a_whole_on_postgres_too() {
    postgres("nils_not_main_whole", not_main);
}
