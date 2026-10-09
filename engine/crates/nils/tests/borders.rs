// SPDX-License-Identifier: AGPL-3.0-only
//! Record 51 slice D: v0's nine reasons for a look at a pick, each raised by
//! `nils pick run` on a session planted for it, and the new role `t2w`
//! picked on its own tables. A synthetic registry, its stacks rewritten to
//! invented values: no stack here was ever a person's. On SQLite always, and
//! on Postgres where a test DSN is set.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::process::{Command, Stdio};

use nils_dicom::synth::TempDir;
use nils_registry::Store;

fn nils() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nils"))
}

fn packs() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packs")
        .to_str()
        .unwrap()
        .to_string()
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

    fn json(&self, args: &[&str]) -> serde_json::Value {
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

/// A synthetic registry with its sessions built and no stack a candidate
/// for any role yet.
fn registry(pg: Option<(String, String)>) -> Home {
    let home = Home {
        dir: TempDir::new("borders-home"),
        pg,
    };
    let (good, _, err) = home.run(&["key", "add", "k"], Some("a borders test key\n"));
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
    home.ok(&["synth", "--seed", "11", "--subjects", "12"]);
    home.ok(&["session", "rebuild"]);
    let mut store = home.store();
    let axis = store.qualified("classification_axis");
    store
        .execute(&format!("DELETE FROM {axis} WHERE axis = 'role'"), &[])
        .unwrap();
    home
}

/// One invented stack: the fingerprint fields a pick reads and the axes.
#[derive(Clone)]
struct Stack {
    fields: BTreeMap<&'static str, Option<String>>,
    axes: BTreeMap<&'static str, Option<String>>,
}

impl Stack {
    /// v0's `make_row`: an MPRAGE, 3D, 176 slices, a 240 mm field of view,
    /// sagittal, RawRecon, no contrast, a T1w candidate.
    fn mprage() -> Stack {
        let s = |v: &str| Some(v.to_string());
        Stack {
            fields: [
                ("n_instances", s("176")),
                ("mr_acquisition_type", s("3D")),
                ("fov_x", s("240")),
                ("image_orientation_patient", s("0\\1\\0\\0\\0\\-1")),
                ("orientation", s("Sagittal")),
                ("echo_time", s("2.98")),
                ("repetition_time", s("2300")),
                ("inversion_time", s("900")),
                ("flip_angle", s("8")),
            ]
            .into_iter()
            .collect(),
            axes: [
                ("base", s("T1w")),
                ("technique", s("MPRAGE")),
                ("modifier", None),
                ("construct", None),
                ("provenance", s("RawRecon")),
                ("post_contrast", s("0")),
                ("directory_type", s("anat")),
                ("body_part", s("Brain")),
                ("disposition", s("acquisition")),
                ("role", s("t1w")),
            ]
            .into_iter()
            .collect(),
        }
    }

    fn field(mut self, name: &'static str, v: Option<&str>) -> Stack {
        self.fields.insert(name, v.map(str::to_string));
        self
    }

    fn axis(mut self, name: &'static str, v: Option<&str>) -> Stack {
        self.axes.insert(name, v.map(str::to_string));
        self
    }
}

/// The subjects, each with the stacks of its first study, which is one
/// occasion under the default scheme.
fn first_studies(store: &mut Store) -> Vec<(i64, Vec<i64>)> {
    let rows = store
        .query(
            &format!(
                "SELECT subject_id, study_id, stack_id FROM {} ORDER BY subject_id, study_id, stack_id",
                store.qualified("stack_fingerprint")
            ),
            &[],
        )
        .unwrap();
    let mut out: Vec<(i64, i64, Vec<i64>)> = Vec::new();
    for r in rows {
        let (subject, study, stack) = (r.int(0).unwrap(), r.int(1).unwrap(), r.int(2).unwrap());
        match out.last_mut() {
            Some((s, st, stacks)) if *s == subject => {
                if *st == study {
                    stacks.push(stack);
                }
            }
            _ => out.push((subject, study, vec![stack])),
        }
    }
    // Studies of five stacks, so every scenario has room.
    out.into_iter()
        .filter(|(_, _, stacks)| stacks.len() >= 4)
        .map(|(s, _, stacks)| (s, stacks))
        .collect()
}

fn literal(v: &Option<String>, number: bool) -> String {
    match v {
        None => "NULL".to_string(),
        Some(x) if number => x.clone(),
        Some(x) => format!("'{}'", x.replace('\'', "''")),
    }
}

/// Write these stacks over the given stack ids.
fn plant(store: &mut Store, ids: &[i64], stacks: &[Stack]) {
    assert!(stacks.len() <= ids.len());
    let fp = store.qualified("stack_fingerprint");
    let axis = store.qualified("classification_axis");
    for (id, s) in ids.iter().zip(stacks) {
        let sets: Vec<String> = s
            .fields
            .iter()
            .map(|(k, v)| {
                let number = !matches!(
                    *k,
                    "mr_acquisition_type"
                        | "image_orientation_patient"
                        | "orientation"
                        | "sop_class_uid"
                        | "image_type"
                        | "earliest_acquisition_date"
                        | "earliest_acquisition_time"
                );
                format!("{k} = {}", literal(v, number))
            })
            .collect();
        store
            .execute(
                &format!("UPDATE {fp} SET {} WHERE stack_id = {id}", sets.join(", ")),
                &[],
            )
            .unwrap();
        store
            .execute(&format!("DELETE FROM {axis} WHERE stack_id = {id}"), &[])
            .unwrap();
        for (k, v) in &s.axes {
            let Some(v) = v else { continue };
            // A multi-valued axis is one row per value.
            for one in v.split(',') {
                store
                    .execute(
                        &format!(
                            "INSERT INTO {axis} (stack_id, axis, value, confidence, tier) \
                             VALUES ({id}, '{k}', '{one}', 1.0, 'rule')"
                        ),
                        &[],
                    )
                    .unwrap();
            }
        }
    }
}

/// Each reason, the stacks of the session planted for it, and every border
/// its pick is expected to raise.
fn scenarios() -> Vec<(&'static str, Vec<Stack>, Vec<&'static str>)> {
    let m = Stack::mprage;
    let vibe = || m().axis("technique", Some("VIBE"));
    vec![
        // v0's close_runner_up: two acquisitions alike in all but the plane.
        (
            "too_close",
            vec![m(), m().field("orientation", Some("Axial"))],
            vec!["too_close"],
        ),
        // v0's rare_technique: the only T1w a CISS.
        (
            "rare",
            vec![m().axis("technique", Some("CISS"))],
            vec!["rare"],
        ),
        // v0's no_canonical_construct: a Dixon of a fat and an out-of-phase
        // image only.
        (
            "nothing_eligible",
            vec![
                vibe()
                    .axis("modifier", Some("Dixon"))
                    .axis("construct", Some("Fat"))
                    .field("echo_time", Some("2.4")),
                vibe()
                    .axis("modifier", Some("Dixon"))
                    .axis("construct", Some("OutPhase"))
                    .field("echo_time", Some("2.5")),
            ],
            vec!["nothing_eligible"],
        ),
        // v0's retake: the same acquisition twice.
        ("retake", vec![m(), m()], vec!["retake"]),
        (
            "unknown_dim",
            vec![m().field("mr_acquisition_type", None)],
            vec!["unknown_dim"],
        ),
        (
            "slice_count_outlier",
            vec![m().field("n_instances", Some("40"))],
            vec!["slice_count_outlier"],
        ),
        // v0's pre_post_twin: before and after contrast, the after one an
        // MP2RAGE, which only its share in the population puts behind: at
        // 86 percent of the MPRAGE, so not too close and still a twin.
        (
            "pre_post_twin",
            vec![
                m(),
                m().axis("technique", Some("MP2RAGE"))
                    .axis("post_contrast", Some("1")),
            ],
            vec!["pre_post_twin"],
        ),
        (
            "epimix_fallback",
            vec![m().axis("provenance", Some("EPIMix"))],
            vec!["epimix_fallback"],
        ),
        // v0's dixon_vs_plain: a Dixon's in-phase and water images and a
        // plain VIBE of the same session. The Dixon's images state no
        // orientation, which is what brings the plain one within a tenth and
        // keeps it more than a twentieth behind.
        (
            "dixon_vs_plain",
            vec![
                vibe()
                    .axis("modifier", Some("Dixon"))
                    .axis("construct", Some("InPhase"))
                    .field("echo_time", Some("2.4"))
                    .field("image_orientation_patient", None),
                vibe()
                    .axis("modifier", Some("Dixon"))
                    .axis("construct", Some("Water"))
                    .field("echo_time", Some("2.5"))
                    .field("image_orientation_patient", None),
                vibe(),
            ],
            vec!["dixon_vs_plain"],
        ),
    ]
}

fn nine(home: &Home) {
    let p = packs();
    let mut store = home.store();
    let subjects = first_studies(&mut store);
    let planted = scenarios();
    assert!(
        subjects.len() >= planted.len() + 12,
        "room for every scenario and a population beside them: {}",
        subjects.len()
    );
    let mut by_subject: BTreeMap<i64, (&str, Vec<&str>)> = BTreeMap::new();
    for ((subject, ids), (reason, stacks, want)) in subjects.iter().zip(&planted) {
        plant(&mut store, ids, stacks);
        by_subject.insert(*subject, (reason, want.clone()));
    }
    // The population beside them: one ordinary MPRAGE a session, the slice
    // counts spread as a cohort's are.
    let spread = ["160", "168", "176", "176", "176", "184", "192"];
    for (i, (_, ids)) in subjects[planted.len()..].iter().enumerate() {
        plant(
            &mut store,
            &ids[..1],
            &[Stack::mprage().field("n_instances", Some(spread[i % spread.len()]))],
        );
    }

    let report = home.json(&["pick", "run", "--pack-dir", &p, "--json"]);
    let items = home.json(&["review", "list", "--kind", "pick.border", "--json"]);
    let open: Vec<&serde_json::Value> = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["status"] == "open")
        .collect();
    let mut seen: BTreeMap<&str, i64> = BTreeMap::new();
    for item in &open {
        let subject = item["ref"]["subject_id"].as_i64().unwrap();
        let got: Vec<&str> = item["evidence"]["borders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_str().unwrap())
            .collect();
        let Some((reason, want)) = by_subject.get(&subject) else {
            panic!("a session of the population raised {got:?}: {item}");
        };
        assert_eq!(&got, want, "the session planted for {reason}: {item}");
        *seen.entry(reason).or_insert(0) += 1;
        match *reason {
            "retake" => assert!(
                item["evidence"]["notes"]["retake"]
                    .as_str()
                    .unwrap()
                    .starts_with("plain: 2"),
                "{item}"
            ),
            "pre_post_twin" | "dixon_vs_plain" => assert!(
                item["evidence"]["notes"][*reason].is_string(),
                "the other candidate is named: {item}"
            ),
            "slice_count_outlier" => assert!(
                item["evidence"]["notes"]["slice_count_outlier"]
                    .as_str()
                    .unwrap()
                    .starts_with("40 outside"),
                "{item}"
            ),
            _ => {}
        }
    }
    // Each of the nine, raised once, on its own session, and counted.
    for (reason, _, _) in &planted {
        if !seen.contains_key(reason) {
            let subject = by_subject.iter().find(|(_, (r, _))| r == reason).unwrap().0;
            let rows = store
                .query(
                    &format!(
                        "SELECT id FROM {} WHERE subject_id = {subject} AND author_kind = 'agent'",
                        store.qualified("pick")
                    ),
                    &[],
                )
                .unwrap();
            for r in rows {
                eprintln!(
                    "{}",
                    home.ok(&["pick", "explain", &r.int(0).unwrap().to_string()])
                );
            }
        }
        assert_eq!(seen.get(reason), Some(&1), "{reason}: {seen:?}");
        assert_eq!(report["borders"][reason], 1, "{reason}: {report}");
    }
    assert_eq!(report["raised"], planted.len(), "{report}");
    assert_eq!(report["standing"], 0, "{report}");

    // The pick row carries what the borders found, beside the scores.
    let row = store
        .query(
            &format!(
                "SELECT parts FROM {} WHERE borders = 'retake' AND author_kind = 'agent'",
                store.qualified("pick")
            ),
            &[],
        )
        .unwrap();
    let parts: serde_json::Value = serde_json::from_str(row[0].text(0).unwrap()).unwrap();
    assert!(
        parts["notes"]["retake"]
            .as_str()
            .unwrap()
            .starts_with("plain"),
        "{parts}"
    );

    // And a second run of the same registry raises the same, and no more.
    let again = home.json(&["pick", "run", "--pack-dir", &p, "--json"]);
    assert_eq!(again["borders"], report["borders"], "{again}");
    let items = home.json(&["review", "list", "--kind", "pick.border", "--json"]);
    let still = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["status"] == "open")
        .count();
    assert_eq!(still, open.len());
}

#[test]
fn each_of_v0_s_nine_reasons_raises_its_border() {
    nine(&registry(None));
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

#[test]
fn each_of_v0_s_nine_reasons_raises_its_border_on_postgres_too() {
    postgres("nils_borders_nine", nine);
}

/// Record 51: v0's MP2RAGE preference. A session of an MP2RAGE's four
/// images is one candidate, its denoised uniform image, and no retake; a
/// session that ran it three times is.
fn mp2rage(home: &Home) {
    let p = packs();
    let mut store = home.store();
    let subjects = first_studies(&mut store);
    let of = |construct: &str, n: &str| {
        Stack::mprage()
            .axis("technique", Some("MP2RAGE"))
            .axis("construct", Some(construct))
            .field("n_instances", Some(n))
    };
    let (one, ids) = &subjects[0];
    plant(
        &mut store,
        ids,
        &[
            of("INV1", "176"),
            of("INV2", "176"),
            of("Uniform", "176"),
            of("UniformDenoised", "176"),
        ],
    );
    let denoised = ids[3];
    let (three, ids3) = &subjects[1];
    plant(
        &mut store,
        ids3,
        &[
            of("UniformDenoised", "176"),
            of("UniformDenoised", "176"),
            of("UniformDenoised", "176"),
            of("INV2", "176"),
        ],
    );
    // A population of MP2RAGE sessions beside them.
    for (_, ids) in &subjects[2..14] {
        plant(&mut store, &ids[..1], &[of("UniformDenoised", "176")]);
    }
    home.json(&["pick", "run", "--pack-dir", &p, "--json"]);
    // The run's pick of a subject's occasion: its stacks and its borders.
    let mut of_subject = |s: i64| -> (Vec<i64>, String) {
        let rows = store
            .query(
                &format!(
                    "SELECT p.borders, ps.stack_id FROM {} p JOIN {} ps ON ps.pick_id = p.id \
                     WHERE p.subject_id = {s} AND p.role = 't1w' AND p.author_kind = 'agent' \
                     ORDER BY ps.stack_id",
                    store.qualified("pick"),
                    store.qualified("pick_stack")
                ),
                &[],
            )
            .unwrap();
        assert!(!rows.is_empty(), "a pick of subject {s}");
        (
            rows.iter().map(|r| r.int(1).unwrap()).collect(),
            rows[0].opt_text(0).unwrap().unwrap_or_default().to_string(),
        )
    };
    let (stacks, borders) = of_subject(*one);
    assert_eq!(stacks, [denoised], "the denoised uniform image alone");
    assert!(!borders.contains("retake"), "{borders}");
    let (stacks, borders) = of_subject(*three);
    assert_eq!(stacks.len(), 3, "{stacks:?}");
    assert!(borders.contains("retake"), "{borders}");
    // And `nils pick explain` says which retake it is.
    let id = store
        .query(
            &format!(
                "SELECT id FROM {} WHERE subject_id = {three} AND role = 't1w' AND author_kind = 'agent'",
                store.qualified("pick")
            ),
            &[],
        )
        .unwrap()[0]
        .int(0)
        .unwrap();
    let text = home.ok(&["pick", "explain", &id.to_string()]);
    assert!(text.contains("retake: mp2rage: 3 stacks"), "{text}");
}

#[test]
fn an_mp2rage_keeps_its_denoised_uniform_image_and_a_true_retake_borders() {
    mp2rage(&registry(None));
}

#[test]
fn an_mp2rage_keeps_its_denoised_uniform_image_on_postgres_too() {
    postgres("nils_borders_mp2rage", mp2rage);
}

// ------------------------------------------------- the 2026-10-10 borders
//
// The study of the 59 pick borders of a real corpus: two engine changes
// that follow standing rulings. The stacks are synthetic and their values
// invented, as above.

/// The SOP class of an enhanced MR image: one file that holds every frame
/// of a volume.
const ENHANCED_MR: &str = "1.2.840.10008.5.1.4.1.1.4.1";

/// The run's T1w pick of a subject's occasion: its stacks, its borders and
/// what its slice count part saw.
fn t1w_pick(store: &mut Store, subject: i64) -> (Vec<i64>, String, String) {
    let rows = store
        .query(
            &format!(
                "SELECT p.borders, p.parts, ps.stack_id FROM {} p JOIN {} ps ON ps.pick_id = p.id \
                 WHERE p.subject_id = {subject} AND p.role = 't1w' AND p.author_kind = 'agent' \
                 ORDER BY ps.stack_id",
                store.qualified("pick"),
                store.qualified("pick_stack")
            ),
            &[],
        )
        .unwrap();
    assert!(!rows.is_empty(), "a pick of subject {subject}");
    let parts: serde_json::Value = serde_json::from_str(rows[0].text(1).unwrap()).unwrap();
    let slices = parts["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "slices")
        .map(|p| p["saw"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    (
        rows.iter().map(|r| r.int(2).unwrap()).collect(),
        rows[0].opt_text(0).unwrap().unwrap_or_default().to_string(),
        slices,
    )
}

/// One file of `frames` frames for the stack, as the digest files an
/// enhanced multi-frame image: one instance, naming the stack.
fn one_file_of_frames(store: &mut Store, stack: i64, frames: i64) {
    let rows = store
        .query(
            &format!(
                "SELECT series_id, first_batch_id FROM {} WHERE id = {stack}",
                store.qualified("stack")
            ),
            &[],
        )
        .unwrap();
    let (series, batch) = (rows[0].int(0).unwrap(), rows[0].int(1).unwrap());
    store
        .execute(
            &format!(
                "INSERT INTO {} (sop_instance_uid, series_id, stack_id, number_of_frames, first_batch_id) \
                 VALUES ('2.25.{stack}', {series}, {stack}, {frames}, {batch})",
                store.qualified("instance")
            ),
            &[],
        )
        .unwrap();
}

/// One file of `frames` frames split over `stacks` in equal parts (record
/// 37 S8): its instance names the first stack, and `instance_frame` gives
/// each stack its frames.
fn one_file_split(store: &mut Store, stacks: &[i64], frames: i64) {
    one_file_of_frames(store, stacks[0], frames);
    let instance = store
        .query(
            &format!(
                "SELECT id, first_batch_id FROM {} WHERE sop_instance_uid = '2.25.{}'",
                store.qualified("instance"),
                stacks[0]
            ),
            &[],
        )
        .unwrap();
    let (id, batch) = (instance[0].int(0).unwrap(), instance[0].int(1).unwrap());
    let each = frames / stacks.len() as i64;
    for (i, stack) in stacks.iter().enumerate() {
        let first = 1 + i as i64 * each;
        store
            .execute(
                &format!(
                    "INSERT INTO {} (instance_id, stack_id, n_frames, first_frame, frames, first_batch_id) \
                     VALUES ({id}, {stack}, {each}, {first}, '{first}-{}', {batch})",
                    store.qualified("instance_frame"),
                    first + each - 1
                ),
                &[],
            )
            .unwrap();
    }
}

/// A population beside the planted sessions: one ordinary MPRAGE a
/// session, the slice counts spread as a cohort's are.
fn population(store: &mut Store, subjects: &[(i64, Vec<i64>)]) {
    let spread = ["160", "168", "176", "176", "176", "184", "192"];
    for (i, (_, ids)) in subjects.iter().enumerate() {
        plant(
            store,
            &ids[..1],
            &[Stack::mprage().field("n_instances", Some(spread[i % spread.len()]))],
        );
    }
}

/// R5: a multi-frame file counts its frames as slices. An MPRAGE stored as
/// one enhanced file of 176 frames is one instance, and the pick read it as
/// a volume of one slice: below every percentile of the population, so a
/// slice-count outlier and a poor score. It is 176 slices.
fn multi_frame(home: &Home) {
    let p = packs();
    let mut store = home.store();
    let subjects = first_studies(&mut store);
    let (one, ids) = &subjects[0];
    plant(
        &mut store,
        &ids[..1],
        &[Stack::mprage()
            .field("n_instances", Some("1"))
            .field("sop_class_uid", Some(ENHANCED_MR))],
    );
    one_file_of_frames(&mut store, ids[0], 176);
    // And one file of 352 frames whose frames were split over two stacks,
    // the MPRAGE and another image: the MPRAGE is its own 176.
    let (two, ids2) = &subjects[1];
    plant(
        &mut store,
        &ids2[..2],
        &[
            Stack::mprage()
                .field("n_instances", Some("1"))
                .field("sop_class_uid", Some(ENHANCED_MR)),
            Stack::mprage()
                .field("n_instances", Some("1"))
                .field("sop_class_uid", Some(ENHANCED_MR))
                .axis("role", None),
        ],
    );
    one_file_split(&mut store, &ids2[..2], 352);
    population(&mut store, &subjects[2..14]);
    home.json(&["pick", "run", "--pack-dir", &p, "--json"]);
    let (stacks, borders, slices) = t1w_pick(&mut store, *one);
    assert_eq!(stacks, [ids[0]]);
    assert_eq!(borders, "", "an ordinary MPRAGE of 176 frames: {slices}");
    assert!(slices.starts_with("176 in slices:3D"), "{slices}");
    let (stacks, borders, slices) = t1w_pick(&mut store, *two);
    assert_eq!(stacks, [ids2[0]]);
    assert_eq!(borders, "", "the frames of its own part: {slices}");
    assert!(slices.starts_with("176 in slices:3D"), "{slices}");
}

#[test]
fn a_multi_frame_file_counts_its_frames_as_slices() {
    multi_frame(&registry(None));
}

#[test]
fn a_multi_frame_file_counts_its_frames_as_slices_on_postgres_too() {
    postgres("nils_borders_frames", multi_frame);
}
