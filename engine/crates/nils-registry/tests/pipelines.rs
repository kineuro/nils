// SPDX-License-Identifier: AGPL-3.0-only

//! Record 43 S4 and S6 on both backends: the embedding cache, keyed by
//! stack, encoder and preprocessing version, and a pipeline's proposals,
//! which become grouped `<axis>:model` review items and staged decisions
//! that nothing puts in force but a person.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::embedding::{self, Embedding, Encoder, Header, Registered};
use nils_registry::home::{Home, InitOptions};
use nils_registry::labels::{self, DecisionQuery};
use nils_registry::proposals::{self, Run};
use nils_registry::review::{self, Apply, Author, CommitFilter};
use nils_registry::schema::{Type, table};
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store};
use serde_json::json;

static POSTGRES: Mutex<()> = Mutex::new(());

const SCHEMA: &str = "nils_pipelines_test";

fn postgres_dsn() -> Option<String> {
    match env::var("NILS_TEST_POSTGRES_DSN") {
        Ok(dsn) if !dsn.is_empty() => Some(dsn),
        _ => {
            eprintln!("NILS_TEST_POSTGRES_DSN is not set; the Postgres half is skipped");
            None
        }
    }
}

struct Lab {
    name: &'static str,
    registry: Registry,
    dir: TempDir,
    _guard: Option<MutexGuard<'static, ()>>,
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(dsn) = postgres_dsn().filter(|_| self._guard.is_some()) {
            let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
            store
                .batch(&format!(
                    "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
                ))
                .expect("drop");
        }
    }
}

fn lab(name: &'static str, backend: Backend, dsn: Option<String>) -> Lab {
    let dir = TempDir::new("pipelines-home");
    let home = Home::new(dir.path());
    home.keys(None)
        .add("k", b"nils-pipelines-test-key")
        .unwrap();
    let registry = home
        .init(&InitOptions {
            backend,
            dsn,
            schema: (backend == Backend::Postgres).then(|| SCHEMA.to_string()),
            scheme: Scheme::DEFAULT,
            key: "k".to_string(),
            display_length: 12,
            session_scheme: None,
        })
        .unwrap();
    Lab {
        name,
        registry,
        dir,
        _guard: None,
    }
}

fn labs() -> Vec<Lab> {
    let mut out = vec![lab("sqlite", Backend::Sqlite, None)];
    if let Some(dsn) = postgres_dsn() {
        let guard = POSTGRES.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = Store::connect_postgres(&dsn, SCHEMA).expect("connect");
        store
            .batch(&format!(
                "DROP SCHEMA IF EXISTS {SCHEMA} CASCADE; DROP SCHEMA IF EXISTS {SCHEMA}_linkage CASCADE"
            ))
            .expect("drop");
        let mut l = lab("postgres", Backend::Postgres, Some(dsn));
        l._guard = Some(guard);
        out.push(l);
    }
    out
}

fn filler(ty: Type) -> Param {
    match ty {
        Type::Int | Type::Id => Param::Int(1),
        Type::Double => Param::Double(1.0),
        Type::Bool => Param::Bool(false),
        Type::Date => Param::from("2026-01-01"),
        Type::Time => Param::from("00:00:00"),
        Type::Timestamp => Param::from("2026-01-01T00:00:00Z"),
        Type::Json => Param::from("{}"),
        Type::Text => Param::from("x"),
        Type::Bytes => Param::Bytes(vec![0]),
    }
}

fn row(store: &mut Store, name: &str, given: &[(&str, Param)]) -> i64 {
    let t = table(name);
    let mut cols: Vec<&str> = Vec::new();
    let mut vals: Vec<Param> = Vec::new();
    for c in t.columns.iter().filter(|c| c.ty != Type::Id) {
        if let Some((_, v)) = given.iter().find(|(n, _)| *n == c.name) {
            cols.push(c.name);
            vals.push(v.clone());
        } else if c.not_null {
            cols.push(c.name);
            vals.push(filler(c.ty));
        }
    }
    store
        .insert(&Insert::new(t, &cols).returning(&["id"]), &[vals])
        .unwrap()[0]
        .int(0)
        .unwrap()
}

/// Two subjects, a series each, and `per` stacks in each series.
fn stacks(reg: &mut Registry, per: usize) -> Vec<i64> {
    let store = reg.store();
    let mut out = Vec::new();
    for (n, uid) in ["1.2.840.9.1", "1.2.840.9.2"].iter().enumerate() {
        let subject = row(store, "subject", &[("code", Param::from(format!("s{n}")))]);
        let study = row(
            store,
            "study",
            &[
                ("subject_id", Param::Int(subject)),
                ("study_instance_uid", Param::from(format!("{uid}.0"))),
            ],
        );
        let series = row(
            store,
            "series",
            &[
                ("subject_id", Param::Int(subject)),
                ("study_id", Param::Int(study)),
                ("series_instance_uid", Param::from(*uid)),
            ],
        );
        for i in 0..per {
            out.push(row(
                store,
                "stack",
                &[
                    ("series_id", Param::Int(series)),
                    ("stack_index", Param::Int(i as i64)),
                    ("stack_key", Param::from(format!("{uid}#{i}"))),
                ],
            ));
        }
    }
    out
}

fn count(reg: &mut Registry, t: &str, filter: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {}{filter}", reg.store().qualified(t));
    reg.store().query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

fn digest(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

/// A stand-in for a file's sha256: the registry keeps the digest the
/// runner computed and reads no bytes.
fn sha(bytes: &[u8], salt: u8) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ u64::from(salt);
    for b in bytes {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}").repeat(4)
}

/// What an embed pipeline does for the stacks it is given: one file per
/// stack, written into the working place's derivative tree, and a row
/// registered for each. Answers how many stacks it embedded.
fn embed_run(
    reg: &mut Registry,
    dir: &Path,
    given: &[i64],
    encoder_id: i64,
    encoder: &str,
    version: &str,
    run: i64,
) -> usize {
    for (i, stack) in given.iter().enumerate() {
        let e = Embedding {
            header: Header {
                stack_id: *stack,
                encoder: encoder.to_string(),
                preprocess_version: version.to_string(),
                slices: vec![10, 11, 12],
                dim: 4,
            },
            matrix: (0..12).map(|v| v as f32 * 0.5 + i as f32).collect(),
        };
        let bytes = embedding::encode(&e).unwrap();
        let sha256 = sha(&bytes, run as u8);
        let path = PathBuf::from("derivatives/embedding")
            .join(&sha256[..2])
            .join(format!("{sha256}{}", embedding::EXTENSION));
        std::fs::create_dir_all(dir.join(&path).parent().unwrap()).unwrap();
        std::fs::write(dir.join(&path), &bytes).unwrap();
        // the file reads back as what was embedded
        let back = embedding::decode(&std::fs::read(dir.join(&path)).unwrap()).unwrap();
        assert_eq!(back.header.stack_id, *stack);
        let got = embedding::register(
            reg,
            &embedding::New {
                stack_id: *stack,
                encoder_id,
                preprocess_version: version,
                place_id: 1,
                path: path.to_str().unwrap(),
                bytes: bytes.len() as i64,
                sha256: &sha256,
                registered_by: "runner@lab",
                actor: None,
                run_id: Some(run),
                created_at: "2026-09-24T10:00:00Z",
            },
        )
        .unwrap();
        assert!(matches!(got, Registered::New(_)), "{got:?}");
    }
    given.len()
}

/// S4's proof: a first run embeds every stack, a second embeds none, a new
/// preprocessing version embeds all of them again and keeps the old rows,
/// and a new encoder is a new key too. The key is the index's as well as
/// the code's: a second live row under it is refused by the store.
#[test]
fn a_second_embedding_run_embeds_nothing_and_a_new_preprocessing_embeds_everything() {
    for mut l in labs() {
        let name = l.name;
        let dir = l.dir.path().join("working");
        let reg = &mut l.registry;
        let ids = stacks(reg, 3);
        let biomed = Encoder {
            name: "biomedclip",
            version: "9f2c1e",
            weights_digest: &digest('b'),
            image_digest: Some(&digest('i')),
        };
        let enc = embedding::register_encoder(reg, &biomed, "runner@lab").unwrap();
        assert_eq!(enc.kind, "encoder", "{name}");
        assert_eq!(enc.task, embedding::TASK, "{name}");
        assert_eq!(enc.card["image_digest"], digest('i'), "{name}");
        // registered once, whoever registers it again
        let again = embedding::register_encoder(reg, &biomed, "runner@lab").unwrap();
        assert_eq!(again.id, enc.id, "{name}");
        assert_eq!(count(reg, "model", ""), 1, "{name}");

        // run 1: nothing is cached, every stack is embedded
        let todo = embedding::missing(reg.store(), &ids, enc.id, "v1").unwrap();
        assert_eq!(todo, ids, "{name}");
        let n = embed_run(reg, &dir, &todo, enc.id, &enc.digest, "v1", 1);
        assert_eq!(n, 6, "{name}");
        let first = embedding::existing(reg.store(), &ids, enc.id, "v1").unwrap();
        assert_eq!(first.len(), 6, "{name}");
        for (stack, d) in &first {
            assert_eq!(d.stack_id, Some(*stack), "{name}");
            assert_eq!(d.kind, "embedding", "{name}");
            assert_eq!(d.model_id, Some(enc.id), "{name}");
            assert_eq!(d.run_id, Some(1), "{name}");
            assert_eq!(d.preprocess_version.as_deref(), Some("v1"), "{name}");
            assert_eq!(d.media_type, embedding::MEDIA_TYPE, "{name}");
        }

        // run 2: the same key, nothing to embed
        let todo = embedding::missing(reg.store(), &ids, enc.id, "v1").unwrap();
        assert!(todo.is_empty(), "{name}: {todo:?}");
        assert_eq!(
            embed_run(reg, &dir, &todo, enc.id, &enc.digest, "v1", 2),
            0,
            "{name}"
        );
        // an output for a stack the key holds is not registered: the row
        // that was there stays, whether or not the bytes agree
        let held = &first[&ids[0]];
        fn kept_as<'a>(stack: i64, enc: i64, sha256: &'a str) -> embedding::New<'a> {
            embedding::New {
                stack_id: stack,
                encoder_id: enc,
                preprocess_version: "v1",
                place_id: 1,
                path: "derivatives/embedding/xx/other.emb",
                bytes: 10,
                sha256,
                registered_by: "runner@lab",
                actor: None,
                run_id: Some(2),
                created_at: "2026-09-24T11:00:00Z",
            }
        }
        let (s0, e0) = (ids[0], enc.id);
        let (zeros, ones) = ("0".repeat(64), "1".repeat(64));
        let kept = |sha256| kept_as(s0, e0, sha256);
        assert_eq!(
            embedding::register(reg, &kept(&held.sha256)).unwrap(),
            Registered::Kept {
                id: held.id,
                same: true
            },
            "{name}"
        );
        assert_eq!(
            embedding::register(reg, &kept(&zeros)).unwrap(),
            Registered::Kept {
                id: held.id,
                same: false
            },
            "{name}"
        );
        assert_eq!(count(reg, "derivative", ""), 6, "{name}");

        // the store holds the key too: a second live row is refused, and a
        // withdrawn row frees its key
        let belongs =
            nils_registry::derivative::belongs(reg.store(), Some(ids[0]), None, None, None)
                .unwrap();
        let raw = nils_registry::derivative::New {
            kind: "embedding",
            belongs: &belongs,
            place_id: 1,
            path: "derivatives/embedding/yy/raw.emb",
            bytes: 1,
            sha256: "ab",
            media_type: embedding::MEDIA_TYPE,
            registered_by: "runner@lab",
            actor: None,
            model_id: Some(enc.id),
            run_id: None,
            preprocess_version: Some("v1"),
            supersedes_id: None,
            created_at: "2026-09-24T12:00:00Z",
        };
        assert!(
            nils_registry::derivative::insert(reg.store(), &raw).is_err(),
            "{name}: the unique index holds the key"
        );
        // a row with no preprocessing, as the door registers one, is
        // outside the key
        nils_registry::derivative::insert(
            reg.store(),
            &nils_registry::derivative::New {
                preprocess_version: None,
                ..raw.clone()
            },
        )
        .unwrap();
        let withdraw = format!(
            "UPDATE {} SET withdrawn_at = '2026-09-24T12:30:00Z' WHERE id = {}",
            reg.store().qualified("derivative"),
            held.id
        );
        reg.store().execute(&withdraw, &[]).unwrap();
        assert_eq!(
            embedding::missing(reg.store(), &ids, enc.id, "v1").unwrap(),
            [ids[0]],
            "{name}: a withdrawn embedding is computed again"
        );
        let replaced = nils_registry::derivative::insert(reg.store(), &raw).unwrap();
        assert!(replaced > held.id, "{name}");

        // a new preprocessing version: every stack again, the old rows kept
        let todo = embedding::missing(reg.store(), &ids, enc.id, "v2").unwrap();
        assert_eq!(todo, ids, "{name}");
        assert_eq!(
            embed_run(reg, &dir, &todo, enc.id, &enc.digest, "v2", 3),
            6,
            "{name}"
        );
        let old = embedding::existing(reg.store(), &ids, enc.id, "v1").unwrap();
        assert_eq!(old.len(), 6, "{name}: the old version's rows stay");
        for (stack, d) in &first {
            if *stack != ids[0] {
                assert_eq!(old[stack].id, d.id, "{name}");
            }
        }
        assert_eq!(
            embedding::existing(reg.store(), &ids, enc.id, "v2")
                .unwrap()
                .len(),
            6,
            "{name}"
        );
        assert_eq!(
            count(reg, "derivative", " WHERE kind = 'embedding'"),
            6 + 1 + 1 + 6,
            "{name}"
        );

        // a second encoder is a key of its own
        let siglip = embedding::register_encoder(
            reg,
            &Encoder {
                name: "siglip2",
                version: "b16-224",
                weights_digest: &digest('c'),
                image_digest: None,
            },
            "runner@lab",
        )
        .unwrap();
        assert_eq!(
            embedding::missing(reg.store(), &ids, siglip.id, "v2")
                .unwrap()
                .len(),
            6,
            "{name}"
        );

        // what is not an encoder has no embeddings, and a digest held by
        // another kind of model is not registered as an encoder
        let head = nils_registry::model::register(
            reg,
            &json!({"name": "bp-head", "version": "1", "kind": "head", "digest": digest('d'),
                    "task": "axis:body_part", "encoder": {"digest": enc.digest}}),
            "runner@lab",
        )
        .unwrap();
        let e = embedding::existing(reg.store(), &ids, head.id, "v1").unwrap_err();
        assert!(e.to_string().contains("not an encoder"), "{name}: {e}");
        let e = embedding::register_encoder(
            reg,
            &Encoder {
                name: "x",
                version: "1",
                weights_digest: &digest('d'),
                image_digest: None,
            },
            "runner@lab",
        )
        .unwrap_err();
        assert!(e.to_string().contains("not an encoder"), "{name}: {e}");
        let e = embedding::register(
            reg,
            &embedding::New {
                stack_id: 9999,
                ..kept(&ones)
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("no stack 9999"), "{name}: {e}");
        let e = embedding::register(
            reg,
            &embedding::New {
                preprocess_version: "has space",
                ..kept(&ones)
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("preprocessing"), "{name}: {e}");
    }
}

/// The synthetic results file of a body-part inference run: ten stacks,
/// an admitted head, a head nobody admitted, and a stack a person decided.
fn results(stacks: &[i64], head: i64, head_digest: &str, unadmitted: i64) -> serde_json::Value {
    let p = |stack: i64, value: &str, conf: f64, model: serde_json::Value| {
        let others = ["brain", "spine", "neck", "brain-neck"]
            .into_iter()
            .filter(|v| *v != value)
            .collect::<Vec<_>>();
        let rest = (1.0 - conf) / others.len() as f64;
        let mut probabilities = serde_json::Map::new();
        probabilities.insert(value.to_string(), json!(conf));
        for o in others {
            probabilities.insert(o.to_string(), json!(rest));
        }
        let mut out = json!({"stack_id": stack, "axis": "body_part", "value": value,
                             "probabilities": probabilities});
        for (k, v) in model.as_object().unwrap() {
            out[k] = v.clone();
        }
        out
    };
    let by = json!({"model_id": head});
    json!({
        "units": [{"unit": "all", "status": "ok"}],
        "proposals": [
            p(stacks[0], "brain", 0.97, by.clone()),
            p(stacks[1], "brain", 0.96, by.clone()),
            p(stacks[2], "brain", 0.995, by.clone()),
            p(stacks[3], "spine", 0.85, by.clone()),
            p(stacks[4], "spine", 0.6, by.clone()),
            p(stacks[5], "brain", 0.5, by.clone()),
            p(stacks[6], "neck", 0.55, by.clone()),
            p(stacks[7], "brain", 0.93, json!({"model_id": unadmitted})),
            p(stacks[8], "brain", 0.99, json!({"model_digest": head_digest})),
            p(stacks[9], "spine", 0.99, by),
        ]
    })
}

/// S6's proof: a synthetic results file gives the right groups and the
/// right staged rows, and nothing is in force before a person commits.
#[test]
fn a_results_file_s_proposals_become_groups_and_staged_rows_and_nothing_in_force() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 5);
        let enc = embedding::register_encoder(
            reg,
            &Encoder {
                name: "biomedclip",
                version: "9f2c1e",
                weights_digest: &digest('b'),
                image_digest: None,
            },
            "runner@lab",
        )
        .unwrap();
        let card = |v: &str, d: &str| {
            json!({"name": "bp-head", "version": v, "kind": "head", "digest": d,
                   "task": "axis:body_part", "encoder": {"digest": enc.digest},
                   "threshold": 0.8})
        };
        let head =
            nils_registry::model::register(reg, &card("1", &digest('1')), "anna@lab").unwrap();
        let passed = json!({"suite": "heldout", "passed": true, "checks": [{"name": "ece", "passed": true}]});
        nils_registry::model::admit(reg, head.id, &passed, "anna@lab").unwrap();
        let unadmitted =
            nils_registry::model::register(reg, &card("2", &digest('2')), "anna@lab").unwrap();

        // a person decided the last stack's body part, in force
        let asked = row(
            reg.store(),
            "review_item",
            &[
                ("kind", Param::from("body_part:low_confidence")),
                ("scope", Param::from("stack")),
                ("ref", Param::from(json!({"stack_id": ids[9]}).to_string())),
                (
                    "evidence",
                    Param::from(json!({"axis": "body_part", "value": "spine"}).to_string()),
                ),
                ("status", Param::from("open")),
            ],
        );
        review::apply(
            reg,
            &Apply {
                item: asked,
                member: None,
                scope: "stack",
                value: Some("spine"),
                author: Author {
                    who: "anna@lab",
                    kind: "person",
                    version: None,
                    model: None,
                },
                stage: false,
                why: None,
                campaign: None,
            },
        )
        .unwrap();

        // the results file, as the runner reads it after the container exits
        let file = l.dir.path().join("results.json");
        std::fs::write(
            &file,
            results(&ids, head.id, &head.digest, unadmitted.id).to_string(),
        )
        .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        let proposals = proposals::parse(&serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(proposals.len(), 10, "{name}");
        let run = Run {
            id: 7,
            job_id: None,
            principal: "runner@lab",
        };

        // what is wrong is refused before anything is written
        let items_before = count(reg, "review_item", "");
        let mut bad = proposals.clone();
        bad[0].probabilities.insert("chest".into(), 0.5);
        let e = proposals::ingest(reg, &run, &bad, None).unwrap_err();
        assert!(e.to_string().contains("sum to"), "{name}: {e}");
        let mut bad = proposals.clone();
        bad[0].stack_id = 9999;
        let e = proposals::ingest(reg, &run, &bad, None).unwrap_err();
        assert!(e.to_string().contains("stack 9999"), "{name}: {e}");
        let mut bad = proposals.clone();
        bad[0].model_id = Some(9999);
        let e = proposals::ingest(reg, &run, &bad, None).unwrap_err();
        assert!(e.to_string().contains("9999"), "{name}: {e}");
        let mut bad = proposals.clone();
        bad[1].model_digest = Some(digest('2'));
        let e = proposals::ingest(reg, &run, &bad, None).unwrap_err();
        assert!(e.to_string().contains("not sha256"), "{name}: {e}");
        let e = proposals::ingest(reg, &run, &proposals, Some(1.5)).unwrap_err();
        assert!(e.to_string().contains("threshold"), "{name}: {e}");
        assert_eq!(count(reg, "review_item", ""), items_before, "{name}");

        let done = proposals::ingest(reg, &run, &proposals, None).unwrap();
        assert_eq!(done.decided, 1, "{name}: the person's stack is not asked");
        assert_eq!(done.members, 9, "{name}");
        let mut got: Vec<(String, i64, String, i64, bool)> = done
            .groups
            .iter()
            .map(|g| {
                (
                    g.value.clone(),
                    g.model_id,
                    g.band.clone(),
                    g.members,
                    g.staged.is_some(),
                )
            })
            .collect();
        got.sort();
        let h = head.id;
        let u = unadmitted.id;
        let mut want = vec![
            ("brain".to_string(), h, "p>=0.95".to_string(), 2, true),
            ("brain".to_string(), h, "p>=0.99".to_string(), 2, true),
            ("spine".to_string(), h, "p>=0.8".to_string(), 1, true),
            ("spine".to_string(), h, "below".to_string(), 1, false),
            ("brain".to_string(), h, "below".to_string(), 1, false),
            ("neck".to_string(), h, "below".to_string(), 1, false),
            ("brain".to_string(), u, "p>=0.9".to_string(), 1, false),
        ];
        want.sort();
        assert_eq!(got, want, "{name}");
        assert_eq!(done.staged_members, 5, "{name}");
        assert_eq!(done.not_staged.len(), 1, "{name}: {:?}", done.not_staged);
        assert!(
            done.not_staged[0].contains("registered"),
            "{name}: {:?}",
            done.not_staged
        );

        // the items: one kind, grouped, staged at or above the threshold
        // and open below it, each member with its probabilities
        assert_eq!(
            count(reg, "review_item", " WHERE kind = 'body_part:model'"),
            7,
            "{name}"
        );
        assert_eq!(
            count(
                reg,
                "review_item",
                " WHERE kind = 'body_part:model' AND scope = 'group' AND status = 'staged'"
            ),
            3,
            "{name}"
        );
        assert_eq!(
            count(
                reg,
                "review_item",
                " WHERE kind = 'body_part:model' AND status = 'open'"
            ),
            4,
            "{name}"
        );
        let top = done
            .groups
            .iter()
            .find(|g| g.band == "p>=0.95")
            .unwrap()
            .clone();
        assert_eq!(top.confidence, 0.96, "{name}: the lowest member's");
        let item = review::item(reg.store(), top.item).unwrap().unwrap();
        assert_eq!(item.evidence["axis"], "body_part", "{name}");
        assert_eq!(item.evidence["tier"], "p>=0.95", "{name}");
        assert_eq!(item.evidence["model"]["digest"], head.digest, "{name}");
        assert_eq!(item.evidence["run_id"], 7, "{name}");
        let members = review::members(reg.store(), top.item).unwrap();
        assert_eq!(
            members.iter().map(|m| m.stack_id).collect::<Vec<_>>(),
            [ids[0], ids[1]],
            "{name}"
        );
        assert_eq!(
            members[0].evidence["probabilities"]["brain"], 0.97,
            "{name}"
        );
        assert_eq!(
            members[0].evidence["probabilities"]
                .as_object()
                .unwrap()
                .len(),
            4,
            "{name}: every class is kept"
        );

        // the staged rows: the model is the author, nobody committed them
        let sql = format!(
            "SELECT author_kind, model_id, value, staged_at IS NOT NULL, committed_at IS NULL, scope \
             FROM {} WHERE author_kind = 'model' ORDER BY id",
            reg.store().qualified("decision")
        );
        let rows = reg.store().query(&sql, &[]).unwrap();
        assert_eq!(rows.len(), 3, "{name}");
        for r in &rows {
            assert_eq!(r.opt_int(1).unwrap(), Some(head.id), "{name}");
            assert_eq!(r.int(3).unwrap(), 1, "{name}: staged");
            assert_eq!(r.int(4).unwrap(), 1, "{name}: not committed");
            assert_eq!(r.text(5).unwrap(), "group", "{name}");
        }

        // nothing is in force: the labels in force are the person's alone
        let in_force = |reg: &mut Registry, staged_too: bool| {
            labels::decision_labels(
                reg.store(),
                &DecisionQuery {
                    axis: "body_part",
                    stacks: None,
                    authors: &["model".to_string()],
                    campaign: None,
                    staged_too,
                },
            )
            .unwrap()
        };
        assert!(in_force(reg, false).is_empty(), "{name}");
        assert_eq!(in_force(reg, true).len(), 5, "{name}");

        // a run goes in once
        let e = proposals::ingest(reg, &run, &proposals, None).unwrap_err();
        assert!(e.to_string().contains("in already"), "{name}: {e}");

        // R6: an agent or a model does not commit them; a person commits
        // the surest part by filter, and the rest stays staged
        let filter = CommitFilter {
            min_confidence: Some(0.96),
            campaign: None,
        };
        for kind in ["agent", "model"] {
            let e = review::commit_where(reg, &filter, false, "bot@lab", kind).unwrap_err();
            assert!(e.to_string().contains("R6"), "{name}: {e}");
        }
        assert!(in_force(reg, false).is_empty(), "{name}");
        let part = review::commit_where(reg, &filter, false, "anna@lab", "person").unwrap();
        assert_eq!(part.decisions.len(), 2, "{name}: {part:?}");
        assert_eq!(part.left, 1, "{name}");
        let now: Vec<(i64, String)> = in_force(reg, false)
            .into_iter()
            .map(|l| (l.stack_id.unwrap(), l.value.unwrap()))
            .collect();
        assert_eq!(
            now,
            [
                (ids[0], "brain".to_string()),
                (ids[1], "brain".to_string()),
                (ids[2], "brain".to_string()),
                (ids[8], "brain".to_string()),
            ],
            "{name}"
        );
        let sql = format!(
            "SELECT DISTINCT committed_by, author_kind FROM {} WHERE committed_at IS NOT NULL AND model_id IS NOT NULL",
            reg.store().qualified("decision")
        );
        let by = reg.store().query(&sql, &[]).unwrap();
        assert_eq!(by.len(), 1, "{name}");
        assert_eq!(by[0].text(0).unwrap(), "anna@lab", "{name}");
        assert_eq!(
            by[0].text(1).unwrap(),
            "model",
            "{name}: the author stays the model"
        );
    }
}

fn contracts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts")
}

/// The proposals schema (`contracts/job/v1/proposals.schema.json`) is what
/// [`proposals::parse`] reads, its example parses, and the review-item
/// contract admits the kind the proposals become.
#[test]
fn the_proposals_contract_is_what_the_engine_reads() {
    let text = std::fs::read_to_string(contracts().join("job/v1/proposals.schema.json")).unwrap();
    let schema: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    let p = &schema["$defs"]["proposal"];
    assert_eq!(p["additionalProperties"], false);
    assert_eq!(
        p["required"],
        json!(["stack_id", "axis", "value", "probabilities"])
    );
    let props: Vec<&str> = p["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        props,
        [
            "stack_id",
            "axis",
            "value",
            "probabilities",
            "model_id",
            "model_digest",
            "note"
        ]
    );
    for example in schema["examples"].as_array().unwrap() {
        let parsed = proposals::parse(&json!({ "proposals": example })).unwrap();
        assert_eq!(parsed.len(), example.as_array().unwrap().len());
    }
    let review = std::fs::read_to_string(contracts().join("review-item/VERSION")).unwrap();
    let item: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(contracts().join(format!(
            "review-item/v{}/review-item.schema.json",
            review.trim()
        )))
        .unwrap(),
    )
    .unwrap();
    let pattern =
        regex::Regex::new(item["properties"]["kind"]["pattern"].as_str().unwrap()).unwrap();
    assert!(pattern.is_match("body_part:model"));
    let emb = std::fs::read_to_string(contracts().join("job/v1/embedding.md")).unwrap();
    assert!(emb.contains(embedding::MEDIA_TYPE));
    assert!(emb.contains(std::str::from_utf8(embedding::MAGIC).unwrap()));
}

/// A registered encoder and an admitted head of `axis:body_part` whose card
/// says `threshold`, or nothing.
fn head(
    reg: &mut Registry,
    version: &str,
    digest_char: char,
    threshold: Option<f64>,
) -> nils_registry::model::Model {
    let enc = embedding::register_encoder(
        reg,
        &Encoder {
            name: "biomedclip",
            version: "9f2c1e",
            weights_digest: &digest('b'),
            image_digest: None,
        },
        "runner@lab",
    )
    .unwrap();
    let mut card = json!({"name": "bp-head", "version": version, "kind": "head",
        "digest": digest(digest_char), "task": "axis:body_part",
        "encoder": {"digest": enc.digest}});
    if let Some(t) = threshold {
        card["threshold"] = json!(t);
    }
    let m = nils_registry::model::register(reg, &card, "anna@lab").unwrap();
    let passed =
        json!({"suite": "heldout", "passed": true, "checks": [{"name": "ece", "passed": true}]});
    nils_registry::model::admit(reg, m.id, &passed, "anna@lab").unwrap()
}

fn proposal(stack: i64, value: &str, p: f64, model: i64) -> proposals::Proposal {
    let other = if value == "brain" { "spine" } else { "brain" };
    proposals::Proposal {
        stack_id: stack,
        axis: "body_part".into(),
        value: value.into(),
        probabilities: [(value.to_string(), p), (other.to_string(), 1.0 - p)]
            .into_iter()
            .collect(),
        model_id: Some(model),
        model_digest: None,
        note: None,
    }
}

/// Record 43's ruling: the threshold a model's proposals are staged at is
/// its card's; whoever runs it may raise it for a run, never lower it, and
/// a model whose card names none has every proposal asked, none staged.
#[test]
fn a_model_s_card_sets_its_threshold_and_a_run_may_only_raise_it() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 2);
        let a = head(reg, "1", '1', Some(0.9));
        let b = head(reg, "2", '2', None);
        let given = [
            proposal(ids[0], "brain", 0.95, a.id),
            proposal(ids[1], "brain", 0.85, a.id),
            proposal(ids[2], "brain", 0.99, b.id),
        ];
        let run = |id| Run {
            id,
            job_id: None,
            principal: "runner@lab",
        };
        // lowering the card's threshold is refused, and nothing is written
        let e = proposals::ingest(reg, &run(1), &given, Some(0.8)).unwrap_err();
        assert!(e.to_string().contains("not lower it"), "{name}: {e}");
        assert_eq!(count(reg, "review_item", ""), 0, "{name}");
        // the card's threshold
        let done = proposals::ingest(reg, &run(1), &given, None).unwrap();
        let mut got: Vec<(i64, String, bool)> = done
            .groups
            .iter()
            .map(|g| (g.model_id, g.band.clone(), g.staged.is_some()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                (a.id, "below".to_string(), false),
                (a.id, "p>=0.95".to_string(), true),
                (b.id, "below".to_string(), false),
            ],
            "{name}"
        );
        assert!(
            done.not_staged
                .iter()
                .any(|w| w.contains("names no threshold")),
            "{name}: {:?}",
            done.not_staged
        );
        let item = review::item(reg.store(), done.groups[0].item)
            .unwrap()
            .unwrap();
        assert!(item.evidence["threshold"].is_number() || item.evidence["threshold"].is_null());
        // a caller raises it: 0.95 is under 0.97, so it is asked, not staged
        let raised = proposals::ingest(
            reg,
            &run(2),
            &[proposal(ids[0], "brain", 0.95, a.id)],
            Some(0.97),
        )
        .unwrap();
        assert_eq!(raised.groups.len(), 1, "{name}");
        assert_eq!(raised.groups[0].band, "below", "{name}");
        assert!(raised.groups[0].staged.is_none(), "{name}");
        let item = review::item(reg.store(), raised.groups[0].item)
            .unwrap()
            .unwrap();
        assert_eq!(item.evidence["threshold"], 0.97, "{name}");
        // a card's threshold is a probability
        let mut card = a.card.clone();
        card["version"] = json!("9");
        card["digest"] = json!(digest('9'));
        card["threshold"] = json!(1.5);
        let e = nils_registry::model::register(reg, &card, "anna@lab").unwrap_err();
        assert!(e.to_string().contains("threshold"), "{name}: {e}");
    }
}

/// Record 43's ruling: a newer run of a model withdraws what its earlier
/// runs staged on the axis and nobody committed, and closes their items,
/// so stale proposals do not pile up. What a person committed stays, and
/// so does another model's.
#[test]
fn a_newer_run_of_a_model_supersedes_what_its_earlier_runs_left_untaken() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let ids = stacks(reg, 3);
        let a = head(reg, "1", '1', Some(0.8));
        let c = head(reg, "2", '2', Some(0.8));
        let run = |id| Run {
            id,
            job_id: None,
            principal: "runner@lab",
        };
        let first = proposals::ingest(
            reg,
            &run(1),
            &[
                proposal(ids[0], "brain", 0.95, a.id),
                proposal(ids[1], "brain", 0.96, a.id),
                proposal(ids[2], "spine", 0.5, a.id),
                proposal(ids[3], "spine", 0.995, a.id),
            ],
            None,
        )
        .unwrap();
        assert_eq!(first.superseded, 0, "{name}");
        let other =
            proposals::ingest(reg, &run(3), &[proposal(ids[4], "brain", 0.9, c.id)], None).unwrap();
        assert!(other.groups[0].staged.is_some(), "{name}");
        // a person commits the surest group of run 1
        let filter = CommitFilter {
            min_confidence: Some(0.99),
            campaign: None,
        };
        let committed = review::commit_where(reg, &filter, false, "anna@lab", "person").unwrap();
        assert_eq!(committed.decisions.len(), 1, "{name}: {committed:?}");
        let status = |reg: &mut Registry, item: i64| {
            review::item(reg.store(), item).unwrap().unwrap().status
        };
        let of = |g: &str| first.groups.iter().find(|x| x.band == g).unwrap().clone();
        let (brain, below, sure) = (of("p>=0.95"), of("below"), of("p>=0.99"));
        assert_eq!(status(reg, brain.item), "staged", "{name}");
        assert_eq!(status(reg, below.item), "open", "{name}");
        assert_eq!(status(reg, sure.item), "accepted", "{name}");

        // run 2 of the same model on the same axis
        let second =
            proposals::ingest(reg, &run(2), &[proposal(ids[0], "brain", 0.97, a.id)], None)
                .unwrap();
        assert_eq!(second.superseded, 2, "{name}: {second:?}");
        assert_eq!(second.withdrawn, 1, "{name}");
        assert_eq!(status(reg, brain.item), "superseded", "{name}");
        assert_eq!(status(reg, below.item), "superseded", "{name}");
        assert_eq!(
            status(reg, sure.item),
            "accepted",
            "{name}: a person took it"
        );
        assert_eq!(
            status(reg, other.groups[0].item),
            "staged",
            "{name}: another model's"
        );
        let withdrawn = |reg: &mut Registry, id: i64| -> bool {
            let sql = format!(
                "SELECT withdrawn_at IS NOT NULL FROM {} WHERE id = {id}",
                reg.store().qualified("decision")
            );
            reg.store().query(&sql, &[]).unwrap()[0].int(0).unwrap() == 1
        };
        assert!(withdrawn(reg, brain.staged.unwrap()), "{name}");
        assert!(!withdrawn(reg, sure.staged.unwrap()), "{name}");
        assert!(!withdrawn(reg, other.groups[0].staged.unwrap()), "{name}");
        assert!(!withdrawn(reg, second.groups[0].staged.unwrap()), "{name}");
        // what is staged of model a now is run 2's alone
        let staged: Vec<i64> = labels::decision_labels(
            reg.store(),
            &DecisionQuery {
                axis: "body_part",
                stacks: None,
                authors: &["model".to_string()],
                campaign: None,
                staged_too: true,
            },
        )
        .unwrap()
        .into_iter()
        .filter_map(|l| l.stack_id)
        .collect();
        let mut staged = staged;
        staged.sort();
        let mut want = vec![ids[0], ids[3], ids[4]];
        want.sort();
        assert_eq!(staged, want, "{name}");
    }
}

/// Record 43's ruling: a head reads several encoders, in order, each
/// registered first; the row keeps the first and `model_encoder` the list.
#[test]
fn a_head_reads_several_encoders_in_the_order_its_card_lists_them() {
    for mut l in labs() {
        let name = l.name;
        let reg = &mut l.registry;
        let enc = |reg: &mut Registry, n: &str, c: char| {
            embedding::register_encoder(
                reg,
                &Encoder {
                    name: n,
                    version: "1",
                    weights_digest: &digest(c),
                    image_digest: None,
                },
                "runner@lab",
            )
            .unwrap()
        };
        let (bc, sg) = (enc(reg, "biomedclip", 'b'), enc(reg, "siglip2", 'c'));
        let card = |v: &str, d: char, extra: serde_json::Value| {
            let mut c = json!({"name": "bp-head", "version": v, "kind": "head",
                "digest": digest(d), "task": "axis:body_part"});
            for (k, x) in extra.as_object().unwrap() {
                c[k] = x.clone();
            }
            c
        };
        let two = json!({"encoders": [{"digest": sg.digest}, {"digest": bc.digest}]});
        let m = nils_registry::model::register(reg, &card("1", '1', two), "anna@lab").unwrap();
        assert_eq!(m.encoder_model_ids, [sg.id, bc.id], "{name}");
        assert_eq!(m.encoder_model_id, Some(sg.id), "{name}");
        let listed = nils_registry::model::list(reg.store(), &Default::default()).unwrap();
        let again = listed.iter().find(|x| x.id == m.id).unwrap();
        assert_eq!(again.encoder_model_ids, [sg.id, bc.id], "{name}");
        // one encoder alone is a list of one
        let one = json!({"encoder": {"digest": bc.digest}});
        let m1 = nils_registry::model::register(reg, &card("2", '2', one), "anna@lab").unwrap();
        assert_eq!(m1.encoder_model_ids, [bc.id], "{name}");
        // refused: none, an encoder outside the list, one unregistered, one twice
        for (extra, words) in [
            (json!({}), "names the encoders"),
            (
                json!({"encoder": {"digest": digest('e')}, "encoders": [{"digest": bc.digest}]}),
                "not one of",
            ),
            (
                json!({"encoders": [{"digest": digest('e')}]}),
                "not registered",
            ),
            (
                json!({"encoders": [{"digest": bc.digest}, {"digest": bc.digest}]}),
                "twice",
            ),
        ] {
            let e = nils_registry::model::register(reg, &card("3", '3', extra), "anna@lab")
                .unwrap_err();
            assert!(e.to_string().contains(words), "{name}: {e}");
        }
    }
}
