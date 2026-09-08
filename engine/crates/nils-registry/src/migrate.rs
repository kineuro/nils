// SPDX-License-Identifier: AGPL-3.0-only

//! Migrations (`docs/specs/wave1-parse-and-digest.md`, §4.1): numbered steps,
//! applied in order inside a transaction each, the version kept in the meta
//! table of each store. `nils init` runs them all; opening a store behind the
//! binary runs the missing ones; a store ahead of the binary is refused.

use std::fmt;

use crate::schema::{self, ID_TYPES, Table, linkage_tables, registry_tables};
use crate::store::{Error, Param, Store};

/// The version this binary writes.
pub const SCHEMA_VERSION: i64 = 35;

/// Which of the two stores a migration runs against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Registry,
    Linkage,
}

impl Kind {
    /// The meta table of the store.
    pub fn meta_table(self) -> &'static str {
        match self {
            Kind::Registry => "registry_meta",
            Kind::Linkage => "linkage_meta",
        }
    }

    pub fn tables(self) -> &'static [Table] {
        match self {
            Kind::Registry => registry_tables(),
            Kind::Linkage => linkage_tables(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Registry => "registry",
            Kind::Linkage => "linkage store",
        }
    }
}

/// One migration.
pub struct Migration {
    pub version: i64,
    pub apply: fn(&mut Store, Kind) -> Result<(), Error>,
}

/// Every migration, in order. The first creates the schema as declared; the
/// rest add what a later wave declared, so that a registry written by an older
/// binary opens under a newer one without being rebuilt.
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        apply: create_declared_tables,
    },
    Migration {
        version: 2,
        apply: create_stack_fingerprint,
    },
    Migration {
        version: 3,
        apply: create_classification,
    },
    Migration {
        version: 4,
        apply: evidence_says_which_pass,
    },
    Migration {
        version: 5,
        apply: study_carries_the_date_it_was_given,
    },
    Migration {
        version: 6,
        apply: create_session_scheme,
    },
    Migration {
        version: 7,
        apply: fingerprint_carries_what_it_worked_out,
    },
    Migration {
        version: 8,
        apply: study_says_whether_it_holds_a_primary,
    },
    Migration {
        version: 9,
        apply: diffusion_is_recorded_per_image,
    },
    Migration {
        version: 10,
        apply: a_decision_says_who_made_it,
    },
    Migration {
        version: 11,
        apply: create_pick,
    },
    Migration {
        version: 12,
        apply: create_release,
    },
    Migration {
        version: 13,
        apply: create_release_change,
    },
    Migration {
        version: 14,
        apply: series_says_what_is_in_its_pixels,
    },
    Migration {
        version: 15,
        apply: a_release_has_a_version,
    },
    Migration {
        version: 16,
        apply: a_release_has_a_layout,
    },
    Migration {
        version: 17,
        apply: a_release_can_be_handed_over,
    },
    Migration {
        version: 18,
        apply: a_release_is_a_current_state_and_a_log,
    },
    Migration {
        version: 19,
        apply: a_series_carries_its_private_elements,
    },
    Migration {
        version: 20,
        apply: an_axis_value_is_a_row,
    },
    Migration {
        version: 21,
        apply: the_registry_holds_the_clinical_layer,
    },
    Migration {
        version: 22,
        apply: a_kind_can_be_marked_sensitive,
    },
    Migration {
        version: 23,
        apply: the_registry_keeps_an_audit_log,
    },
    Migration {
        version: 24,
        apply: the_review_spine,
    },
    Migration {
        version: 25,
        apply: a_release_may_be_withdrawn,
    },
    Migration {
        version: 26,
        apply: a_stack_is_found_by_study_and_by_subject,
    },
    Migration {
        version: 27,
        apply: a_date_knows_its_precision,
    },
    Migration {
        version: 28,
        apply: a_membership_is_an_interval,
    },
    Migration {
        version: 29,
        apply: a_session_has_a_key,
    },
    Migration {
        version: 30,
        apply: a_question_leaves_a_handle,
    },
    Migration {
        version: 31,
        apply: a_document_has_a_handle,
    },
    Migration {
        version: 32,
        apply: a_job_carries_its_result,
    },
    Migration {
        version: 33,
        apply: an_act_names_its_actor,
    },
    Migration {
        version: 34,
        apply: a_writing_door_takes_an_idempotency_key,
    },
    Migration {
        version: 35,
        apply: an_overlay_is_a_registry_object,
    },
];

/// Wave 4b §11.3 and §11.4: the case folded companions of the fingerprint's
/// eight text columns and the two numbers of the spacing string, filled for
/// the rows a fingerprint job already wrote, and the two indexes without
/// which a session-to-stack set is not measurable at the reference scale.
fn a_stack_is_found_by_study_and_by_subject(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    const CI: &[(&str, &str)] = &[
        ("text_series_description", "text_series_description_ci"),
        ("text_protocol_name", "text_protocol_name_ci"),
        ("text_sequence_name", "text_sequence_name_ci"),
        ("text_body_part", "text_body_part_ci"),
        ("text_series_comments", "text_series_comments_ci"),
        ("text_image_comments", "text_image_comments_ci"),
        ("text_all", "text_all_ci"),
        ("text_contrast", "text_contrast_ci"),
    ];
    let fresh = !column_exists(store, "stack_fingerprint", "text_all_ci")?;
    let mut columns: Vec<&str> = CI.iter().map(|(_, ci)| *ci).collect();
    columns.extend(["pixel_spacing_row", "pixel_spacing_col"]);
    add_columns(store, "stack_fingerprint", &columns)?;
    add_indexes(store, "stack_fingerprint")?;
    if !fresh || !table_exists(store, "stack_fingerprint")? {
        return Ok(());
    }
    // The fill, in Rust and not in SQL, because SQLite's LOWER folds ASCII
    // only and the writer folds Unicode: a row filled here must read the
    // same as a row the next fingerprint job writes.
    let t = schema::table("stack_fingerprint");
    let sources: Vec<&str> = CI.iter().map(|(raw, _)| *raw).collect();
    let select = format!(
        "SELECT id, {}, pixel_spacing FROM {} WHERE id > ? AND id <= ? ORDER BY id",
        sources.join(", "),
        store.qualified("stack_fingerprint")
    );
    let select = match store.dialect() {
        crate::dialect::Dialect::Sqlite => select,
        crate::dialect::Dialect::Postgres => select.replacen('?', "$1", 1).replacen('?', "$2", 1),
    };
    let top = store
        .query_opt(
            &format!(
                "SELECT MAX(id) FROM {}",
                store.qualified("stack_fingerprint")
            ),
            &[],
        )?
        .and_then(|r| r.opt_int(0).ok().flatten())
        .unwrap_or(0);
    let mut low = 0i64;
    const WINDOW: i64 = 5_000;
    while low < top {
        let high = low + WINDOW;
        let rows = store.query(&select, &[Param::Int(low), Param::Int(high)])?;
        for r in &rows {
            let id = r.int(0)?;
            let mut sets: Vec<(&str, Param)> = Vec::with_capacity(CI.len() + 2);
            for (i, (_, ci)) in CI.iter().enumerate() {
                sets.push((
                    *ci,
                    match r.opt_text(i + 1)? {
                        Some(v) => Param::from(v.to_lowercase()),
                        None => Param::Null,
                    },
                ));
            }
            let spacing = r.opt_text(CI.len() + 1)?.unwrap_or("");
            let mut it = spacing.split('\\');
            let row_sp: Option<f64> = it.next().and_then(|v| v.trim().parse().ok());
            let col_sp: Option<f64> = it.next().and_then(|v| v.trim().parse().ok());
            sets.push((
                "pixel_spacing_row",
                row_sp.map_or(Param::Null, Param::Double),
            ));
            sets.push((
                "pixel_spacing_col",
                col_sp.map_or(Param::Null, Param::Double),
            ));
            store.update_by_id(t, &sets, "id", id)?;
        }
        low = high;
    }
    Ok(())
}

/// Wave 4b §5.2: every stored date carries its precision, row by row. What
/// is there reads as `day`; a kind's declared precision comes with the next
/// vocabulary load, which is where the year rule for a placeholder date is
/// applied (`clinical::load`).
fn a_date_knows_its_precision(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "observation_type", &["precision"])?;
    add_columns(store, "event", &["event_date_precision"])?;
    add_columns(store, "subject_disease_type", &["assigned_on_precision"])?;
    if table_exists(store, "event")? {
        store.execute(
            &format!(
                "UPDATE {} SET event_date_precision = 'day' WHERE event_date_precision IS NULL",
                store.qualified("event")
            ),
            &[],
        )?;
    }
    if table_exists(store, "subject_disease_type")? {
        store.execute(
            &format!(
                "UPDATE {} SET assigned_on_precision = 'day' WHERE assigned_on IS NOT NULL AND assigned_on_precision IS NULL",
                store.qualified("subject_disease_type")
            ),
            &[],
        )?;
    }
    Ok(())
}

/// Wave 4b §8.1: a membership is an interval in a log, not a row per pair.
/// The unique key gains `joined_at`, which no `ALTER` does on both
/// backends, so the table is rebuilt and its rows copied.
fn a_membership_is_an_interval(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    if !table_exists(store, "cohort_member")? {
        return add_tables(store, kind, &["cohort_member"]);
    }
    if column_exists(store, "cohort_member", "source")? {
        return Ok(());
    }
    let dialect = store.dialect();
    let schema_name = store.schema().map(str::to_string);
    let old = store.qualified("cohort_member");
    let mut rebuilt = schema::table("cohort_member").clone();
    rebuilt.name = "cohort_member_rebuilt";
    store.batch(&dialect.create_table(schema_name.as_deref(), &rebuilt))?;
    let new = store.qualified("cohort_member_rebuilt");
    store.batch(&format!(
        "INSERT INTO {new} (cohort_id, subject_id, joined_at, left_at, notes, source) \
         SELECT cohort_id, subject_id, joined_at, left_at, notes, 'import' FROM {old} ORDER BY id"
    ))?;
    store.batch(&format!("DROP TABLE {old}"))?;
    store.batch(&format!("ALTER TABLE {new} RENAME TO cohort_member"))?;
    for ix in dialect.create_indexes(schema_name.as_deref(), schema::table("cohort_member")) {
        store.batch(&ix)?;
    }
    Ok(())
}

/// Wave 4b §7: the session cache and its labels, the scheme's digest, and
/// the digest a pick was made under. Digests of the schemes already kept
/// are computed here; a pick names its scheme's digest by that name.
fn a_session_has_a_key(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(
        store,
        kind,
        &["session_cache", "session_cache_study", "session_label"],
    )?;
    add_columns(store, "session_scheme", &["digest"])?;
    add_columns(store, "pick", &["scheme_digest"])?;
    if !table_exists(store, "session_scheme")? {
        return Ok(());
    }
    let t = schema::table("session_scheme");
    let definition = store
        .dialect()
        .text_of(t.column("definition").expect("definition"));
    let rows = store.query(
        &format!(
            "SELECT id, {definition} FROM {} WHERE digest IS NULL",
            store.qualified("session_scheme")
        ),
        &[],
    )?;
    for r in &rows {
        let id = r.int(0)?;
        let scheme = crate::session::Scheme::from_json(r.text(1)?)
            .map_err(|e| Error::Message(format!("session_scheme {id} will not parse: {e}")))?;
        store.update_by_id(t, &[("digest", Param::from(scheme.digest()))], "id", id)?;
    }
    if table_exists(store, "pick")? {
        store.execute(
            &format!(
                "UPDATE {pick} SET scheme_digest = (SELECT s.digest FROM {scheme} s WHERE s.name = {pick}.scheme) \
                 WHERE scheme_digest IS NULL",
                pick = store.qualified("pick"),
                scheme = store.qualified("session_scheme")
            ),
            &[],
        )?;
    }
    Ok(())
}

/// Wave 4b §8: what a question leaves behind: the handle, its members and
/// pages, an uploaded list by reference, the saved ask with its versions,
/// the curation of the catalog, and the identifier read audit.
fn a_question_leaves_a_handle(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(
        store,
        kind,
        &[
            "handle",
            "handle_member",
            "handle_page",
            "values_source",
            "values_member",
            "selection",
            "selection_version",
            "catalog_curation",
            "handle_read_audit",
        ],
    )
}

/// Wave 4b §10: a document under authoring is addressed by handle.
fn a_document_has_a_handle(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["ask_document"])
}

/// Wave 4c §6.1: a queued job records what it produced, so a caller who
/// polled it to `done` has a supported way to find its answer.
fn a_job_carries_its_result(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "job", &["result"])
}

/// Wave 4c §5.5: an audit row, a handle and a decision each record who
/// acted for the principal, with "absent" as its own value.
fn an_act_names_its_actor(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "audit", &["actor"])?;
    add_columns(store, "handle", &["actor"])?;
    add_columns(store, "decision", &["actor_detail"])
}

/// Wave 4c §6.3: a writing door remembers what it answered under a key.
fn a_writing_door_takes_an_idempotency_key(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["idempotency"])
}

/// Wave 4c §6.6: an overlay is proposed, rehearsed and adopted as a row.
fn an_overlay_is_a_registry_object(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["overlay"])
}

/// Wave 4a §13.1: a release is the history of what left and is never
/// removed; it may be withdrawn, with a reason.
fn a_release_may_be_withdrawn(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "release",
        &["withdrawn_at", "withdrawn_by", "withdrawn_why"],
    )
}

/// Wave 4a §10.2: grouped items with members, and staged decisions.
fn the_review_spine(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["review_member"])?;
    add_columns(
        store,
        "review_item",
        &["job_id", "members", "group_key", "decision_id"],
    )?;
    add_columns(
        store,
        "decision",
        &["staged_at", "committed_at", "epoch_staged"],
    )
}

/// Wave 4a §9.2: the audit log as a table, and the acknowledgement on a
/// review item, which is its own home and not a decision.
fn the_registry_keeps_an_audit_log(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["audit"])?;
    add_columns(store, "review_item", &["accepted_by", "accepted_at"])
}

/// Wave 4a §7.4: the pack marks an observation kind sensitive, and the
/// release never writes one, named or not.
fn a_kind_can_be_marked_sensitive(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "observation_type", &["is_sensitive"])
}

/// Wave 4a §7.1: the clinical layer in the one registry: cohorts and their
/// members, the vocabulary of diseases and observation kinds, a subject's
/// diseases, the events, and the subject's demographics.
fn the_registry_holds_the_clinical_layer(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(
        store,
        kind,
        &[
            "cohort",
            "cohort_member",
            "disease",
            "disease_type",
            "observation_type",
            "subject_disease",
            "subject_disease_type",
            "event",
        ],
    )?;
    add_columns(store, "subject", &["deceased_at"])
}

/// Wave 4a §6.1, fault 4: a multi-valued axis stops being a comma-joined
/// string. The table is rebuilt because its unique key changes from
/// `(stack_id, axis)` to `(stack_id, axis, value)`, and every row whose value
/// held several is split into one row per value, in Rust, since neither
/// backend splits a string the same way.
fn an_axis_value_is_a_row(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    if !table_exists(store, "classification_axis")? {
        return add_tables(store, kind, &["classification_axis"]);
    }
    let dialect = store.dialect();
    let schema_name = store.schema().map(str::to_string);
    let old = store.qualified("classification_axis");
    // A registry made at this version has the shape already and no row with
    // a comma in it; rebuilding it again is a copy of every row, which is
    // cheap once and harmless.
    let mut rebuilt = schema::table("classification_axis").clone();
    rebuilt.name = "classification_axis_rebuilt";
    store.batch(&dialect.create_table(schema_name.as_deref(), &rebuilt))?;
    let new = store.qualified("classification_axis_rebuilt");
    let rows = store.query(
        &format!("SELECT stack_id, axis, value, confidence, tier FROM {old} ORDER BY id"),
        &[],
    )?;
    let mut batch: Vec<Vec<Param>> = Vec::new();
    let insert = crate::store::Insert::new(
        &rebuilt,
        &["stack_id", "axis", "value", "confidence", "tier"],
    );
    for r in &rows {
        let stack = r.int(0)?;
        let axis = r.text(1)?.to_string();
        let confidence = r.double(3)?;
        let tier = r.text(4)?.to_string();
        let values: Vec<String> = r
            .opt_text(2)?
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if values.is_empty() {
            batch.push(vec![
                Param::Int(stack),
                Param::from(axis.as_str()),
                Param::Null,
                Param::Double(confidence),
                Param::from(tier.as_str()),
            ]);
        }
        for v in values {
            batch.push(vec![
                Param::Int(stack),
                Param::from(axis.as_str()),
                Param::from(v),
                Param::Double(confidence),
                Param::from(tier.as_str()),
            ]);
        }
        if batch.len() >= 5_000 {
            store.insert(&insert, &batch)?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        store.insert(&insert, &batch)?;
    }
    store.batch(&format!("DROP TABLE {old}"))?;
    store.batch(&format!("ALTER TABLE {new} RENAME TO classification_axis"))?;
    for ix in dialect.create_indexes(schema_name.as_deref(), schema::table("classification_axis")) {
        store.batch(&ix)?;
    }
    Ok(())
}

/// Wave 4a §5.2: the private elements a pack names are read at digest time
/// and kept per series, keyed by address, so that a classifier can read a
/// vendor's parameter the way it reads a standard one.
fn a_series_carries_its_private_elements(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["series_private"])
}

/// Wave 4a §4: the release's bookkeeping moves from a row per stack per
/// version and a row per file per version to a current state per stack and a
/// change log.
///
/// `release_stack` is rebuilt, because its key changes from the version to the
/// dataset: for every `(name, root)` a `dataset` row is made, and of the rows a
/// stack had across versions the newest survives, marked with the version that
/// wrote it. `release_file` is dropped; its per-file digests are folded into
/// nothing, because a digest of digests cannot be computed in SQL, so a
/// migrated stack carries an empty digest until a version rewrites it, and a
/// handover treats an empty digest as "not recorded". Bytes and counts are
/// kept, because those can be summed.
fn a_release_is_a_current_state_and_a_log(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["dataset", "release_plan"])?;
    add_columns(store, "release", &["dataset_id"])?;
    if !table_exists(store, "release_stack")? {
        return add_tables(store, kind, &["release_stack"]);
    }
    if column_exists(store, "release_stack", "dataset_id")? {
        return Ok(());
    }
    let release = store.qualified("release");
    let dataset = store.qualified("dataset");
    let d = store.dialect();
    let now = d.text_of(
        schema::table("release")
            .column("started_at")
            .expect("release.started_at is a column"),
    );
    // A dataset per distinct name and root the releases have used.
    store.batch(&format!(
        "INSERT INTO {dataset} (name, root, created_at) \
         SELECT r.name, r.root, MIN({now}) FROM {release} r \
         GROUP BY r.name, r.root"
    ))?;
    store.batch(&format!(
        "UPDATE {release} SET dataset_id = (SELECT d.id FROM {dataset} d \
           WHERE d.name = {release}.name AND d.root = {release}.root) \
         WHERE dataset_id IS NULL"
    ))?;

    let dialect = store.dialect();
    let schema_name = store.schema().map(str::to_string);
    let mut rebuilt = schema::table("release_stack").clone();
    rebuilt.name = "release_stack_rebuilt";
    store.batch(&dialect.create_table(schema_name.as_deref(), &rebuilt))?;
    let old = store.qualified("release_stack");
    let new = store.qualified("release_stack_rebuilt");
    // The newest row per (dataset, stack), with the bytes summed from the
    // manifest while it still exists and an empty digest.
    let files = match table_exists(store, "release_file")? {
        true => format!(
            "COALESCE((SELECT SUM(f.bytes) FROM {} f WHERE f.release_id = s.release_id \
               AND f.stack_id = s.stack_id), 0)",
            store.qualified("release_file")
        ),
        false => "0".to_string(),
    };
    store.batch(&format!(
        "INSERT INTO {new} (dataset_id, stack_id, release_id, content, dir, stem, route, \
                            files, bytes, digest, extensions) \
         SELECT r.dataset_id, s.stack_id, s.release_id, s.content, s.dir, s.stem, \
                COALESCE(s.route, 'raw'), s.files, {files}, '', NULL \
         FROM {old} s JOIN {release} r ON r.id = s.release_id \
         WHERE s.release_id = (SELECT MAX(s2.release_id) FROM {old} s2 \
                               JOIN {release} r2 ON r2.id = s2.release_id \
                               WHERE r2.dataset_id = r.dataset_id AND s2.stack_id = s.stack_id)"
    ))?;
    store.batch(&format!("DROP TABLE {old}"))?;
    store.batch(&format!("ALTER TABLE {new} RENAME TO release_stack"))?;
    for ix in dialect.create_indexes(schema_name.as_deref(), schema::table("release_stack")) {
        store.batch(&ix)?;
    }
    if table_exists(store, "release_file")? {
        store.batch(&format!("DROP TABLE {}", store.qualified("release_file")))?;
    }
    Ok(())
}

/// Wave 3 §11: how a dataset physically left, as part of the release record.
fn a_release_can_be_handed_over(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(
        store,
        kind,
        &["handover", "handover_archive", "handover_subject"],
    )
}

/// Wave 3 §9: a release has a layout, and a BIDS one writes files that are not
/// one instance written out.
///
/// `release_file` is rebuilt rather than extended, because its `instance_id`
/// has to become optional and no `ALTER` relaxes a NOT NULL on both backends.
/// A NIfTI is a whole stack, and its sidecar, `.bval` and `.bvec` are the
/// stack's too.
fn a_release_has_a_layout(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "release", &["layout", "placements", "converter"])?;
    add_columns(store, "release_stack", &["stem", "route"])?;
    add_tables(store, kind, &["release_absent"])?;
    let sql = format!(
        "UPDATE {} SET layout = 'descriptive', placements = '{{}}' WHERE layout IS NULL",
        store.qualified("release")
    );
    store.execute(&sql, &[])?;
    let sql = format!(
        "UPDATE {} SET route = 'descriptive' WHERE route IS NULL",
        store.qualified("release_stack")
    );
    store.execute(&sql, &[])?;

    // A registry made after migration 18 has no manifest to rebuild, and one
    // made between 16 and 18 has rebuilt it already.
    if !table_exists(store, "release_file")? || column_exists(store, "release_file", "stack_id")? {
        return Ok(());
    }
    let dialect = store.dialect();
    let schema = store.schema().map(str::to_string);
    let mut rebuilt = schema::historical("release_file").clone();
    rebuilt.name = "release_file_rebuilt";
    store.batch(&dialect.create_table(schema.as_deref(), &rebuilt))?;
    let old = store.qualified("release_file");
    let new = store.qualified("release_file_rebuilt");
    let instance = store.qualified("instance");
    let stack = store.qualified("stack");
    // The stack of each file, which the old shape only knew through its
    // instance. A row whose instance is gone keeps its path and loses its
    // stack, which is why the copy is a join and not an update.
    store.batch(&format!(
        "INSERT INTO {new} (release_id, stack_id, instance_id, path, digest, bytes) \
         SELECT f.release_id, k.id, f.instance_id, f.path, f.digest, f.bytes \
         FROM {old} f JOIN {instance} i ON i.id = f.instance_id \
         JOIN {stack} k ON k.id = i.stack_id"
    ))?;
    store.batch(&format!("DROP TABLE {old}"))?;
    store.batch(&format!("ALTER TABLE {new} RENAME TO release_file"))?;
    for ix in dialect.create_indexes(schema.as_deref(), schema::historical("release_file")) {
        store.batch(&ix)?;
    }
    Ok(())
}

/// Wave 3 §8.6: a release is versioned, and a re-run pays only for what
/// changed. The release row gains the version and the counts; two tables carry
/// the state the next version compares against and what it decided.
///
/// A release made before this has no version. It is given the day it started
/// as its first, because a version that reads as a date is more use than a
/// null, and because the release before the first comparison wrote everything
/// either way.
fn a_release_has_a_version(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "release",
        &[
            "version",
            "previous_id",
            "unchanged",
            "moved",
            "rewritten",
            "added",
            "removed",
        ],
    )?;
    add_tables(store, kind, &["release_stack", "release_move"])?;
    // Read through the dialect's own rendering of a timestamp, or Postgres is
    // handed `substr(timestamptz, ...)` and refuses the whole migration. Both
    // renderings begin `YYYY-MM-DD`, so the offsets are the same.
    let started = store.dialect().text_of(
        schema::table("release")
            .column("started_at")
            .expect("release.started_at is a column"),
    );
    let sql = format!(
        "UPDATE {} SET version = SUBSTR({started}, 1, 4) || '.' || SUBSTR({started}, 6, 2) \
           || '.' || SUBSTR({started}, 9, 2) || '.1' WHERE version IS NULL",
        store.qualified("release")
    );
    store.execute(&sql, &[])?;
    for column in ["unchanged", "moved", "rewritten", "added", "removed"] {
        let sql = format!(
            "UPDATE {} SET {column} = 0 WHERE {column} IS NULL",
            store.qualified("release")
        );
        store.execute(&sql, &[])?;
    }
    Ok(())
}

/// Wave 3 §8.4: `BurnedInAnnotation`, which is what a release asks instead of
/// looking at pixels. v0 never reads it.
fn series_says_what_is_in_its_pixels(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "series", &["burned_in_annotation"])
}

/// Wave 3 §8.5: what a release changed, by tag and action, with no old value.
fn create_release_change(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["release_change"])
}

/// Wave 3 §8.5: what a release did, as rows rather than as a workbook.
fn create_release(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    // `release_file` was created here too, until migration 18 folded it into
    // `release_stack`; a registry that skips straight past has nothing to fold.
    add_tables(store, kind, &["release"])
}

/// Wave 3 §10: which stack stands for a session's role, with the evidence and
/// the population it was chosen against.
fn create_pick(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["pick", "pick_stack"])
}

/// Wave 3 §10.1: a decision records whether a person, an agent or a model made
/// it, and the evidence a decision writes says so too.
///
/// A registry from before this has decisions with no kind. They are people's:
/// nothing else could have written one, because nothing else could reach the
/// verb. So the column is backfilled rather than left null, which is the one
/// case where a default is a fact and not a guess.
fn a_decision_says_who_made_it(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "decision", &["author_kind", "author_version"])?;
    add_columns(store, "classification_evidence", &["author", "author_kind"])?;
    let sql = format!(
        "UPDATE {} SET author_kind = 'person' WHERE author_kind IS NULL",
        store.qualified("decision")
    );
    store.execute(&sql, &[])?;
    Ok(())
}

/// Wave 3 §6: the seven diffusion values that vary from one image of a series
/// to the next move to the instance, and the fingerprint gains what it works
/// out from them.
///
/// A b value, a gradient orientation and a directionality are per image by
/// design: that is what a multi-shell, multi-direction acquisition is. Keeping
/// one per series records such a series as its smallest shell and its gradient
/// count as one. The columns on `series_mr` are left where they are in a
/// registry that already has them, unread, because a migration adds and does
/// not take away.
fn diffusion_is_recorded_per_image(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "instance",
        &[
            "diffusion_b_value",
            "diffusion_gradient_orientation",
            "diffusion_directionality",
            "dwi_siemens_b_value",
            "dwi_siemens_directionality",
            "dwi_ge_b_value",
            "dwi_philips_b_value",
        ],
    )?;
    add_columns(
        store,
        "stack_fingerprint",
        &[
            "dwi_b_value",
            "dwi_b_values",
            "dwi_b_value_source",
            "dwi_pe_direction",
            "dwi_pe_direction_source",
            "dwi_directions",
            "dwi_directions_source",
        ],
    )
}

/// Wave 3 §6: a study says whether any of its stacks is one the scanner called
/// its output, which is half of the session rescue. The other half is the
/// scheme, and it is applied on read.
fn study_says_whether_it_holds_a_primary(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "study", &["has_original_primary"])
}

/// Wave 3 §6: the fields the fingerprint derives rather than reads, each beside
/// the measured column it came from. A registry from Wave 2 gains six columns;
/// one created now has them already.
fn fingerprint_carries_what_it_worked_out(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "stack_fingerprint",
        &[
            "field_strength_tesla",
            "field_strength_normalized",
            "field_strength_unit",
            "acquisition_type_filled",
            "acquisition_type_source",
            "image_role",
        ],
    )
}

/// Wave 3 §5: the registry keeps the schemes it derives sessions with, so a
/// labelling can be reproduced from the registry alone. A registry created at
/// version 1 gains the table; one created now already has it.
fn create_session_scheme(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["session_scheme"])
}

/// Wave 3 §4: a study whose `StudyDate` said nothing carries the day the vote
/// found, the source that carried the most weight for it, and how close the
/// vote was. Never over the measured column: a registry that has the table
/// from version 1 gains four, one created now has them already.
fn study_carries_the_date_it_was_given(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "study",
        &[
            "date_filled",
            "date_source",
            "date_weight",
            "date_runner_up",
        ],
    )
}

/// Wave 2 §7: a pass writes evidence like a rule does, and says which pass it
/// was and which named reference it voted against. A registry that has the
/// table from version 3 gains the two columns; one created now has them
/// already.
fn evidence_says_which_pass(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "classification_evidence", &["pass", "reference"])
}

/// Wave 2 §8: what a pack decided, what made it decide, and the decisions
/// that outrank it.
fn create_classification(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(
        store,
        kind,
        &[
            "classification",
            "classification_axis",
            "classification_evidence",
            "decision",
        ],
    )
}

/// Wave 2 §4.2. A registry created at version 1 gains the table; one created
/// now already has it from [`create_declared_tables`], so this is a no-op
/// there and the two paths reach the same schema.
fn create_stack_fingerprint(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["stack_fingerprint"])
}

/// Create the named declared tables if they are not there yet.
fn add_tables(store: &mut Store, kind: Kind, names: &[&str]) -> Result<(), Error> {
    let dialect = store.dialect();
    let schema = store.schema().map(str::to_string);
    for t in kind.tables().iter().filter(|t| names.contains(&t.name)) {
        if table_exists(store, t.name)? {
            continue;
        }
        store.batch(&dialect.create_table(schema.as_deref(), t))?;
        for ix in dialect.create_indexes(schema.as_deref(), t) {
            store.batch(&ix)?;
        }
    }
    Ok(())
}

/// Create a table's declared indexes that are not there yet.
fn add_indexes(store: &mut Store, table: &str) -> Result<(), Error> {
    if !table_exists(store, table)? {
        return Ok(());
    }
    let dialect = store.dialect();
    let schema = store.schema().map(str::to_string);
    for ix in dialect.create_indexes(schema.as_deref(), schema::table(table)) {
        store.batch(&ix)?;
    }
    Ok(())
}

/// Add declared columns a table has not got yet. A column is added, never
/// changed: what an older binary wrote stays readable.
fn add_columns(store: &mut Store, table: &str, names: &[&str]) -> Result<(), Error> {
    if !table_exists(store, table)? {
        return Ok(());
    }
    let dialect = store.dialect();
    let qualified = store.qualified(table);
    let declared = schema::table(table);
    for name in names {
        if column_exists(store, table, name)? {
            continue;
        }
        let column = declared
            .column(name)
            .unwrap_or_else(|| panic!("{table}.{name} is not a declared column"));
        store.batch(&format!(
            "ALTER TABLE {qualified} ADD COLUMN {name} {}",
            dialect.type_name(column.ty)
        ))?;
    }
    Ok(())
}

fn column_exists(store: &mut Store, table: &str, column: &str) -> Result<bool, Error> {
    Ok(match store {
        Store::Sqlite(_) => store
            .query(&format!("PRAGMA table_info({table})"), &[])?
            .iter()
            .any(|r| r.text(1).map(|n| n == column).unwrap_or(false)),
        Store::Postgres { .. } => {
            let schema = store.schema().unwrap_or("public").to_string();
            store
                .query_opt(
                    "SELECT 1 FROM information_schema.columns WHERE table_schema = $1 AND table_name = $2 AND column_name = $3",
                    &[Param::from(schema), Param::from(table), Param::from(column)],
                )?
                .is_some()
        }
    })
}

fn table_exists(store: &mut Store, name: &str) -> Result<bool, Error> {
    Ok(match store {
        Store::Sqlite(_) => store
            .query_opt(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
                &[Param::from(name)],
            )?
            .is_some(),
        Store::Postgres { .. } => {
            let schema = store.schema().unwrap_or("public").to_string();
            store
                .query_opt(
                    "SELECT 1 FROM information_schema.tables WHERE table_schema = $1 AND table_name = $2",
                    &[Param::from(schema), Param::from(name)],
                )?
                .is_some()
        }
    })
}

fn create_declared_tables(store: &mut Store, kind: Kind) -> Result<(), Error> {
    let dialect = store.dialect();
    let schema = store.schema().map(str::to_string);
    for t in kind.tables() {
        store.batch(&dialect.create_table(schema.as_deref(), t))?;
        for ix in dialect.create_indexes(schema.as_deref(), t) {
            store.batch(&ix)?;
        }
    }
    if kind == Kind::Linkage {
        let table = store.qualified("id_type");
        let sql = format!(
            "INSERT INTO {table} (name, description) VALUES ({}, {})",
            dialect.param(1, crate::schema::Type::Text),
            dialect.param(2, crate::schema::Type::Text)
        );
        for (name, description) in ID_TYPES {
            store.execute(&sql, &[Param::from(name), Param::from(description)])?;
        }
    }
    Ok(())
}

/// The store's version: `None` when it has no meta table at all.
pub fn version_of(store: &mut Store, kind: Kind) -> Result<Option<i64>, Error> {
    let exists = match store {
        Store::Sqlite(_) => store
            .query_opt(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
                &[Param::from(kind.meta_table())],
            )?
            .is_some(),
        Store::Postgres { .. } => {
            let schema = store.schema().unwrap_or("public").to_string();
            store
                .query_opt(
                    "SELECT 1 FROM information_schema.tables WHERE table_schema = $1 AND table_name = $2",
                    &[Param::from(schema), Param::from(kind.meta_table())],
                )?
                .is_some()
        }
    };
    if !exists {
        return Ok(None);
    }
    let table = store.qualified(kind.meta_table());
    let sql = format!(
        "SELECT value FROM {table} WHERE key = {}",
        store.dialect().param(1, crate::schema::Type::Text)
    );
    match store.query_opt(&sql, &[Param::from("schema_version")])? {
        Some(row) => {
            let v = row.text(0)?.parse::<i64>().map_err(|_| {
                Error::Message(format!("{}: schema_version is not a number", kind.name()))
            })?;
            Ok(Some(v))
        }
        None => Ok(Some(0)),
    }
}

/// Where a store stands against the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Nothing there yet.
    Empty,
    /// The binary's version.
    Current,
    /// Migrations are pending.
    Behind(i64),
    /// Written by a newer binary.
    Ahead(i64),
}

impl fmt::Display for Standing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Standing::Empty => f.write_str("empty"),
            Standing::Current => write!(f, "schema version {SCHEMA_VERSION}"),
            Standing::Behind(v) => write!(
                f,
                "schema version {v}, behind this binary's {SCHEMA_VERSION}"
            ),
            Standing::Ahead(v) => write!(
                f,
                "schema version {v}, ahead of this binary's {SCHEMA_VERSION}"
            ),
        }
    }
}

pub fn standing(store: &mut Store, kind: Kind) -> Result<Standing, Error> {
    Ok(match version_of(store, kind)? {
        None => Standing::Empty,
        Some(v) if v == SCHEMA_VERSION => Standing::Current,
        Some(v) if v < SCHEMA_VERSION => Standing::Behind(v),
        Some(v) => Standing::Ahead(v),
    })
}

/// Apply every migration after the store's version, each in its own
/// transaction, and record the version. Returns the versions applied.
pub fn migrate(store: &mut Store, kind: Kind) -> Result<Vec<i64>, Error> {
    let from = match standing(store, kind)? {
        Standing::Empty => 0,
        Standing::Current => return Ok(Vec::new()),
        Standing::Behind(v) => v,
        Standing::Ahead(v) => {
            return Err(Error::Message(format!(
                "the {} has schema version {v}, ahead of this binary's {SCHEMA_VERSION}; use a newer nils",
                kind.name()
            )));
        }
    };
    let mut applied = Vec::new();
    for m in MIGRATIONS.iter().filter(|m| m.version > from) {
        store.begin()?;
        let result = (m.apply)(store, kind).and_then(|()| set_version(store, kind, m.version));
        match result {
            Ok(()) => store.commit()?,
            Err(e) => {
                let _ = store.rollback();
                return Err(e);
            }
        }
        applied.push(m.version);
    }
    Ok(applied)
}

fn set_version(store: &mut Store, kind: Kind, version: i64) -> Result<(), Error> {
    let table = store.qualified(kind.meta_table());
    let d = store.dialect();
    let sql = format!(
        "INSERT INTO {table} (key, value) VALUES ({}, {}) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        d.param(1, crate::schema::Type::Text),
        d.param(2, crate::schema::Type::Text)
    );
    store.execute(
        &sql,
        &[
            Param::from("schema_version"),
            Param::from(version.to_string()),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_numbered_from_one_without_gaps() {
        for (i, m) in MIGRATIONS.iter().enumerate() {
            assert_eq!(m.version, i as i64 + 1);
        }
        assert_eq!(MIGRATIONS.last().unwrap().version, SCHEMA_VERSION);
    }

    #[test]
    fn an_empty_sqlite_store_is_created_and_then_current() {
        let mut store = Store::sqlite_in_memory().unwrap();
        assert_eq!(
            standing(&mut store, Kind::Registry).unwrap(),
            Standing::Empty
        );
        // Every migration runs on a new store; the later ones are no-ops
        // there, since migration 1 creates every declared table.
        let applied: Vec<i64> = MIGRATIONS.iter().map(|m| m.version).collect();
        assert_eq!(migrate(&mut store, Kind::Registry).unwrap(), applied);
        assert_eq!(
            standing(&mut store, Kind::Registry).unwrap(),
            Standing::Current
        );
        assert!(migrate(&mut store, Kind::Registry).unwrap().is_empty());
        let tables = store
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
                &[],
            )
            .unwrap();
        let names: Vec<&str> = tables.iter().map(|r| r.text(0).unwrap()).collect();
        for t in registry_tables() {
            assert!(names.contains(&t.name), "{} missing", t.name);
        }
        // a store from the future is refused
        store
            .execute(
                "UPDATE registry_meta SET value = '99' WHERE key = 'schema_version'",
                &[],
            )
            .unwrap();
        assert_eq!(
            standing(&mut store, Kind::Registry).unwrap(),
            Standing::Ahead(99)
        );
        let err = migrate(&mut store, Kind::Registry).unwrap_err().to_string();
        assert!(err.contains("ahead of this binary"), "{err}");
    }

    #[test]
    fn the_linkage_store_is_seeded_with_its_id_types() {
        let mut store = Store::sqlite_in_memory().unwrap();
        migrate(&mut store, Kind::Linkage).unwrap();
        let rows = store
            .query("SELECT name FROM id_type ORDER BY id", &[])
            .unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.text(0).unwrap()).collect();
        assert_eq!(names, vec!["patient-id", "study-instance-uid"]);
        assert_eq!(
            standing(&mut store, Kind::Linkage).unwrap(),
            Standing::Current
        );
    }
}

#[cfg(test)]
mod column_migration {
    use super::*;

    /// A registry written before Wave 2's passes opens under this binary with
    /// the two columns added rather than being rebuilt.
    #[test]
    fn a_column_a_later_wave_declared_is_added_to_an_existing_table() {
        let mut store = Store::sqlite_in_memory().unwrap();
        // Everything up to the version that created the table, and no further.
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 3) {
            (m.apply)(&mut store, Kind::Registry).unwrap();
        }
        store
            .batch("ALTER TABLE classification_evidence DROP COLUMN pass")
            .unwrap();
        store
            .batch("ALTER TABLE classification_evidence DROP COLUMN reference")
            .unwrap();
        assert!(!column_exists(&mut store, "classification_evidence", "pass").unwrap());

        evidence_says_which_pass(&mut store, Kind::Registry).unwrap();
        assert!(column_exists(&mut store, "classification_evidence", "pass").unwrap());
        assert!(column_exists(&mut store, "classification_evidence", "reference").unwrap());
        // and again, because a migration that has run must be safe to run
        evidence_says_which_pass(&mut store, Kind::Registry).unwrap();
    }
}
