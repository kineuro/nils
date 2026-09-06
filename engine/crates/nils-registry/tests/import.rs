// SPDX-License-Identifier: AGPL-3.0-only

//! The one declarative importer (`docs/specs/wave4a-engine-completes.md`,
//! §7.2), on both backends: six targets, a preview that writes nothing, an
//! apply that is idempotent on the key, and the three answers to a row that
//! exists.

use std::env;
use std::sync::{Mutex, MutexGuard};

use nils_dicom::synth::TempDir;
use nils_registry::clinical::{self, Vocabulary};
use nils_registry::home::{Home, InitOptions};
use nils_registry::import::{self, Mapping, Verdict};
use nils_registry::linkage::{self, NewIdentity, Subkeys};
use nils_registry::schema::table;
use nils_registry::{Backend, Insert, Param, Registry, Scheme, Store};

static POSTGRES: Mutex<()> = Mutex::new(());
const SCHEMA: &str = "nils_import_test";

fn postgres_dsn() -> Option<String> {
    env::var("NILS_TEST_POSTGRES_DSN")
        .ok()
        .filter(|d| !d.is_empty())
}

struct Lab {
    name: &'static str,
    registry: Registry,
    _dir: TempDir,
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
    let dir = TempDir::new("import-home");
    let home = Home::new(dir.path());
    home.keys(None).add("k", b"nils-import-test-key").unwrap();
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
        _dir: dir,
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

/// Two subjects, the vocabulary, and a hospital identifier for the first.
fn seed(reg: &mut Registry) -> (i64, i64) {
    let now = nils_registry::time::now_iso();
    let rows = reg
        .store()
        .insert(
            &Insert::new(table("subject"), &["code", "created_at"]).returning(&["id"]),
            &[
                vec![Param::from("s-one"), Param::from(now.as_str())],
                vec![Param::from("s-two"), Param::from(now.as_str())],
            ],
        )
        .unwrap();
    let one = rows[0].int(0).unwrap();
    let two = rows[1].int(0).unwrap();
    let v = Vocabulary::parse(
        "vocabulary:\n  diseases:\n    - name: Multiple Sclerosis\n      code: G35\n      types: [{name: RRMS}, {name: SPMS}]\n  observation_types:\n    - {name: EDSS, category: scale, value_type: numeric, unit: points, min: 0, max: 10, primary: true}\n    - {name: Treatment, category: intervention, value_type: text}\n    - {name: Diagnosis, category: assessment}\n    - {name: Disease Onset, category: assessment}\n",
    )
    .unwrap();
    clinical::load(reg.store(), &v).unwrap();
    // The first subject is known to the hospital as H-001.
    let key = reg.pseudonym_key().unwrap();
    let keys = Subkeys::derive(&key);
    let mut link = reg.open_linkage().unwrap();
    let hospital =
        linkage::add_id_type(&mut link, "hospital", Some("the hospital's number")).unwrap();
    linkage::insert_identities(
        &mut link,
        &[NewIdentity {
            subject_id: one,
            id_type_id: hospital.id,
            lookup: keys.lookup("hospital", "H-001"),
            ciphertext: keys.seal("H-001"),
            source: "csv",
            first_batch_id: None,
        }],
    )
    .unwrap();
    (one, two)
}

fn count(reg: &mut Registry, sql: &str) -> i64 {
    let sql = sql.replace("{event}", &reg.store().qualified("event"));
    reg.store().query(&sql, &[]).unwrap()[0].int(0).unwrap()
}

const EDSS: &str = "import:\n  target: event\n  subject: {column: code}\n  observation_type: EDSS\n  source: the clinic's file\n  columns:\n    event_date: {column: date, parser: date, format: '%d/%m/%Y'}\n    value: {column: edss, parser: float}\n";

#[test]
fn a_preview_writes_nothing_an_apply_writes_once_and_a_rerun_changes_nothing() {
    for mut l in labs() {
        let name = l.name;
        seed(&mut l.registry);
        let mapping = Mapping::parse(EDSS).unwrap();
        let csv = "code,date,edss\ns-one,15/01/2022,2.5\ns-two,16/01/2022,6\nnobody,17/01/2022,1\ns-one,not a date,3\n";

        let preview = import::preview(&mut l.registry, &mapping, csv, "tester").unwrap();
        assert!(!preview.applied, "{name}");
        assert_eq!(preview.rows, 4, "{name}");
        assert_eq!(preview.added, 2, "{name}: {preview:?}");
        assert_eq!(preview.refused_total(), 2, "{name}: {preview:?}");
        assert_eq!(
            count(&mut l.registry, "SELECT COUNT(*) FROM {event}"),
            0,
            "{name}: a preview writes nothing"
        );

        let applied = import::apply(&mut l.registry, &mapping, csv, "tester").unwrap();
        assert!(applied.applied, "{name}");
        assert_eq!(applied.added, 2, "{name}");
        assert_eq!(
            count(&mut l.registry, "SELECT COUNT(*) FROM {event}"),
            2,
            "{name}"
        );
        assert_eq!(
            count(
                &mut l.registry,
                "SELECT COUNT(*) FROM {event} WHERE number = 2.5 AND source = 'the clinic''s file' AND actor = 'tester'"
            ),
            1,
            "{name}: the number, the source and the principal are on the row"
        );

        let again = import::apply(&mut l.registry, &mapping, csv, "tester").unwrap();
        assert_eq!(again.skipped, 2, "{name}: idempotent on the key");
        assert_eq!(again.changes(), 0, "{name}");
        assert_eq!(
            count(&mut l.registry, "SELECT COUNT(*) FROM {event}"),
            2,
            "{name}"
        );
    }
}

#[test]
fn a_row_that_exists_is_skipped_updated_or_superseded_as_the_mapping_says() {
    for mut l in labs() {
        let name = l.name;
        seed(&mut l.registry);
        let csv = "code,date,edss\ns-one,15/01/2022,2.5\n";
        let corrected = "code,date,edss\ns-one,15/01/2022,3.0\n";
        import::apply(
            &mut l.registry,
            &Mapping::parse(EDSS).unwrap(),
            csv,
            "tester",
        )
        .unwrap();

        let update = Mapping::parse(&format!("{EDSS}  on_existing: update\n")).unwrap();
        let r = import::apply(&mut l.registry, &update, corrected, "fixer").unwrap();
        assert_eq!(r.updated, 1, "{name}");
        assert_eq!(
            count(&mut l.registry, "SELECT COUNT(*) FROM {event}"),
            1,
            "{name}: in place"
        );
        assert_eq!(
            count(
                &mut l.registry,
                "SELECT COUNT(*) FROM {event} WHERE number = 3.0 AND actor = 'fixer'"
            ),
            1,
            "{name}"
        );

        let supersede = Mapping::parse(&format!("{EDSS}  on_existing: supersede\n")).unwrap();
        let r = import::apply(
            &mut l.registry,
            &supersede,
            "code,date,edss\ns-one,15/01/2022,3.5\n",
            "fixer",
        )
        .unwrap();
        assert_eq!(r.superseded, 1, "{name}");
        assert_eq!(
            count(&mut l.registry, "SELECT COUNT(*) FROM {event}"),
            2,
            "{name}: the old row stays"
        );
        assert_eq!(
            count(
                &mut l.registry,
                "SELECT COUNT(*) FROM {event} WHERE superseded_by IS NOT NULL AND number = 3.0"
            ),
            1,
            "{name}"
        );
        assert_eq!(
            count(
                &mut l.registry,
                "SELECT COUNT(*) FROM {event} WHERE superseded_by IS NULL AND number = 3.5"
            ),
            1,
            "{name}"
        );
        // And a re-run under skip sees the current row, not the superseded one.
        let r = import::apply(
            &mut l.registry,
            &Mapping::parse(EDSS).unwrap(),
            "code,date,edss\ns-one,15/01/2022,9\n",
            "tester",
        )
        .unwrap();
        assert_eq!(r.skipped, 1, "{name}");
    }
}

#[test]
fn a_subject_is_named_by_an_identifier_through_the_linkage_store() {
    for mut l in labs() {
        let name = l.name;
        seed(&mut l.registry);
        let mapping = Mapping::parse(
            "import:\n  target: event\n  subject: {column: hospital_no, by: identifier, id_type: hospital}\n  observation_type: {column: kind}\n  columns:\n    event_date: {column: date, parser: date, format: '%Y-%m-%d'}\n    value: {column: what}\n",
        )
        .unwrap();
        let csv = "hospital_no,kind,date,what\nH-001,Treatment,2021-03-01,natalizumab\nH-999,Treatment,2021-03-01,nothing\nH-001,Nothing,2021-03-01,x\n";
        let r = import::apply(&mut l.registry, &mapping, csv, "tester").unwrap();
        assert_eq!(r.added, 1, "{name}: {r:?}");
        assert_eq!(r.samples[0].subject.as_deref(), Some("s-one"), "{name}");
        assert_eq!(r.refused_total(), 2, "{name}");
        assert!(
            r.refused.keys().any(|k| k.contains("does not know")),
            "{name}: {r:?}"
        );
        assert!(
            r.refused.keys().any(|k| k.contains("no observation kind")),
            "{name}: {r:?}"
        );
    }
}

#[test]
fn demographics_fill_a_blank_and_a_disagreement_is_a_review_item() {
    for mut l in labs() {
        let name = l.name;
        let (one, _) = seed(&mut l.registry);
        let mapping = Mapping::parse(
            "import:\n  target: subject\n  subject: {column: code}\n  columns:\n    birth_date: {column: born, parser: date, format: '%Y-%m-%d'}\n    sex: {column: sex}\n",
        )
        .unwrap();
        let r = import::apply(
            &mut l.registry,
            &mapping,
            "code,born,sex\ns-one,1970-05-05,f\n",
            "tester",
        )
        .unwrap();
        assert_eq!(r.added, 2, "{name}: {r:?}");
        let sql = format!(
            "SELECT sex FROM {} WHERE id = {one}",
            l.registry.store().qualified("subject")
        );
        assert_eq!(
            l.registry.store().query(&sql, &[]).unwrap()[0]
                .text(0)
                .unwrap(),
            "F",
            "{name}"
        );

        // The same again is a skip; a different sex is a review item and the
        // registry's value stands.
        let r = import::apply(
            &mut l.registry,
            &mapping,
            "code,born,sex\ns-one,1970-05-05,F\n",
            "tester",
        )
        .unwrap();
        assert_eq!(r.skipped, 2, "{name}: {r:?}");
        let r = import::apply(
            &mut l.registry,
            &mapping,
            "code,born,sex\ns-one,1970-05-05,M\n",
            "tester",
        )
        .unwrap();
        assert_eq!(r.reviewed, 1, "{name}: {r:?}");
        assert!(
            matches!(&r.samples.iter().find(|o| o.verdict.name() == "reviewed").unwrap().verdict, Verdict::Reviewed { field } if field == "sex"),
            "{name}"
        );
        assert_eq!(
            l.registry.store().query(&sql, &[]).unwrap()[0]
                .text(0)
                .unwrap(),
            "F",
            "{name}: stands"
        );
        let items = format!(
            "SELECT COUNT(*) FROM {} WHERE kind = 'subject.demographics' AND status = 'open'",
            l.registry.store().qualified("review_item")
        );
        assert_eq!(count(&mut l.registry, &items), 1, "{name}");
    }
}

#[test]
fn cohorts_members_diseases_and_their_types_go_through_the_same_door() {
    for mut l in labs() {
        let name = l.name;
        seed(&mut l.registry);
        let cohort = Mapping::parse(
            "import:\n  target: cohort\n  columns:\n    name: {column: cohort}\n    owner: {value: the group}\n    description: {column: about}\n",
        )
        .unwrap();
        let r = import::apply(
            &mut l.registry,
            &cohort,
            "cohort,about\nnmosd,the NMOSD cohort\n",
            "tester",
        )
        .unwrap();
        assert_eq!(r.added, 1, "{name}");
        let r = import::apply(
            &mut l.registry,
            &cohort,
            "cohort,about\nnmosd,the NMOSD cohort\n",
            "tester",
        )
        .unwrap();
        assert_eq!(r.skipped, 1, "{name}");

        let member = Mapping::parse(
            "import:\n  target: cohort_member\n  subject: {column: code}\n  cohort: nmosd\n",
        )
        .unwrap();
        let r = import::apply(
            &mut l.registry,
            &member,
            "code\ns-one\ns-two\ns-one\n",
            "tester",
        )
        .unwrap();
        assert_eq!(r.added, 2, "{name}: {r:?}");
        assert_eq!(r.skipped, 1, "{name}: the third row is the first again");

        let disease = Mapping::parse(
            "import:\n  target: subject_disease\n  subject: {column: code}\n  disease: Multiple Sclerosis\n  columns:\n    onset_date: {column: onset, parser: date, format: '%Y-%m-%d'}\n    diagnosis_date: {column: dx, parser: date, format: '%Y-%m-%d'}\n",
        )
        .unwrap();
        let r = import::apply(
            &mut l.registry,
            &disease,
            "code,onset,dx\ns-one,2015-06-01,2016-02-10\n",
            "tester",
        )
        .unwrap();
        assert_eq!(r.added, 1, "{name}: {r:?}");
        // The onset and the diagnosis are events of their kinds, made by the
        // import, and the disease row points at them.
        assert_eq!(
            count(&mut l.registry, "SELECT COUNT(*) FROM {event}"),
            2,
            "{name}"
        );
        let linked = format!(
            "SELECT COUNT(*) FROM {} WHERE onset_event_id IS NOT NULL AND diagnosis_event_id IS NOT NULL",
            l.registry.store().qualified("subject_disease")
        );
        assert_eq!(count(&mut l.registry, &linked), 1, "{name}");

        let dtype = Mapping::parse(
            "import:\n  target: subject_disease_type\n  subject: {column: code}\n  disease: Multiple Sclerosis\n  disease_type: {column: course}\n  columns:\n    assigned_on: {column: since, parser: date, format: '%Y-%m-%d'}\n",
        )
        .unwrap();
        let r = import::apply(&mut l.registry, &dtype, "code,course,since\ns-one,RRMS,2016-02-10\ns-two,SPMS,2020-01-01\ns-one,PPMS,2016-02-10\n", "tester").unwrap();
        assert_eq!(r.added, 2, "{name}: {r:?}");
        assert_eq!(
            r.refused_total(),
            1,
            "{name}: no type PPMS in this vocabulary"
        );
        // The second subject had no disease row; the type import made one.
        let rows = format!(
            "SELECT COUNT(*) FROM {}",
            l.registry.store().qualified("subject_disease")
        );
        assert_eq!(count(&mut l.registry, &rows), 2, "{name}");
        let r = import::apply(
            &mut l.registry,
            &dtype,
            "code,course,since\ns-one,RRMS,2016-02-10\n",
            "tester",
        )
        .unwrap();
        assert_eq!(r.skipped, 1, "{name}");
    }
}
