// SPDX-License-Identifier: AGPL-3.0-only

//! Migrations (`docs/specs/wave1-parse-and-digest.md`, §4.1): numbered steps,
//! applied in order inside a transaction each, the version kept in the meta
//! table of each store. `nils init` runs them all; opening a store behind the
//! binary runs the missing ones; a store ahead of the binary is refused.

use std::fmt;

use crate::schema::{self, ID_TYPES, Table, linkage_tables, registry_tables};
use crate::store::{Error, Param, Store};

/// The version this binary writes.
pub const SCHEMA_VERSION: i64 = 110;

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
    Migration {
        version: 36,
        apply: a_handle_may_be_invalidated,
    },
    Migration {
        version: 37,
        apply: a_place_is_a_registry_object,
    },
    Migration {
        version: 38,
        apply: a_place_declares_how_it_is_handled,
    },
    Migration {
        version: 39,
        apply: a_source_place_is_a_dataset,
    },
    Migration {
        version: 40,
        apply: a_batch_has_a_kind_and_a_subject_may_be_provisional,
    },
    Migration {
        version: 41,
        apply: a_cohort_is_fed_retired_and_released_under_its_policy,
    },
    Migration {
        version: 42,
        apply: a_subject_may_be_merged,
    },
    Migration {
        version: 43,
        apply: a_release_says_why_its_sessions_were_numbered,
    },
    Migration {
        version: 44,
        apply: an_original_is_proved_by_its_own_digest,
    },
    Migration {
        version: 45,
        apply: a_release_says_how_many_stacks_it_could_not_judge,
    },
    Migration {
        version: 46,
        apply: a_stack_says_what_it_covers,
    },
    Migration {
        version: 47,
        apply: a_stack_says_which_coil_received_it,
    },
    Migration {
        version: 48,
        apply: a_release_says_which_naming_mode_it_wrote,
    },
    Migration {
        version: 49,
        apply: an_instance_may_hold_frames_of_more_than_one_stack,
    },
    Migration {
        version: 50,
        apply: the_date_is_the_date,
    },
    Migration {
        version: 51,
        apply: a_stack_says_where_it_sits_and_when_it_was_made,
    },
    Migration {
        version: 52,
        apply: every_rule_votes,
    },
    Migration {
        version: 53,
        apply: an_answer_names_its_model_and_its_committer,
    },
    Migration {
        version: 54,
        apply: a_model_has_a_home,
    },
    Migration {
        version: 55,
        apply: a_person_may_pick,
    },
    Migration {
        version: 56,
        apply: a_derivative_has_a_home,
    },
    Migration {
        version: 57,
        apply: a_campaign_asks_one_question_of_many,
    },
    Migration {
        version: 58,
        apply: labels_leave_with_their_provenance,
    },
    Migration {
        version: 59,
        apply: a_pipeline_has_a_catalog_and_its_runs,
    },
    Migration {
        version: 60,
        apply: an_embedding_is_kept_once_per_encoder_and_preprocessing,
    },
    Migration {
        version: 61,
        apply: a_head_reads_its_encoders_in_order,
    },
    Migration {
        version: 62,
        apply: a_run_s_input_release_says_so,
    },
    Migration {
        version: 63,
        apply: a_reading_is_timed_and_a_certificate_unseals,
    },
    Migration {
        version: 64,
        apply: a_run_s_units_are_scheduled_and_resumed,
    },
    Migration {
        version: 65,
        apply: a_run_s_numbers_are_measures_and_a_starter_says_so,
    },
    Migration {
        version: 66,
        apply: an_answer_may_be_unsure,
    },
    Migration {
        version: 67,
        apply: a_run_s_scratch_is_apart_from_its_output,
    },
    Migration {
        version: 68,
        apply: an_answer_keeps_what_it_derived,
    },
    Migration {
        version: 69,
        apply: a_campaign_carries_suggestions_from_outside,
    },
    Migration {
        version: 70,
        apply: an_answer_may_be_corrected,
    },
    Migration {
        version: 71,
        apply: a_series_says_its_pulse_sequence,
    },
    Migration {
        version: 72,
        apply: a_campaign_says_what_it_suggests,
    },
    Migration {
        version: 73,
        apply: a_campaign_settles_candidates_blind,
    },
    Migration {
        version: 74,
        apply: a_campaign_shows_two_stacks_side_by_side,
    },
    Migration {
        version: 75,
        apply: a_campaign_may_show_the_pictures_alone,
    },
    Migration {
        version: 76,
        apply: a_campaign_reads_a_stack_beside_two_anchors,
    },
    Migration {
        version: 77,
        apply: a_series_says_how_it_was_acquired,
    },
    Migration {
        version: 78,
        apply: a_stack_keeps_the_philips_frame_image_type,
    },
    Migration {
        version: 79,
        apply: a_fingerprint_reads_the_2026_10_03_fields,
    },
    Migration {
        version: 80,
        apply: a_source_place_nobody_declared_is_undeclared,
    },
    Migration {
        version: 81,
        apply: a_held_file_may_wait_for_its_subject_s_id_type,
    },
    Migration {
        version: 82,
        apply: a_personnummer_is_never_kept,
    },
    Migration {
        version: 83,
        apply: a_dataset_s_declaration_is_its_structure,
    },
    Migration {
        version: 84,
        apply: a_root_s_folder_is_a_dataset_only_when_added,
    },
    Migration {
        version: 85,
        apply: a_classification_notes_what_was_not_asked,
    },
    Migration {
        version: 86,
        apply: the_same_everywhere_id_type_is_built_in,
    },
    Migration {
        version: 87,
        apply: a_scan_is_held_wherever_its_files_are,
    },
    Migration {
        version: 88,
        apply: a_scope_keeps_its_own_main_scan_rules,
    },
    Migration {
        version: 89,
        apply: a_file_set_aside_by_policy_asks_nobody,
    },
    Migration {
        version: 90,
        apply: a_result_can_be_checked,
    },
    Migration {
        version: 91,
        apply: a_check_is_a_campaign,
    },
    Migration {
        version: 92,
        apply: a_refused_item_says_why,
    },
    Migration {
        version: 93,
        apply: a_failure_says_its_kind,
    },
    Migration {
        version: 94,
        apply: a_release_keeps_the_picks_it_read,
    },
    Migration {
        version: 95,
        apply: a_stack_carries_its_contrast_label,
    },
    Migration {
        version: 96,
        apply: a_site_chooses_a_model_per_task,
    },
    Migration {
        version: 97,
        apply: a_file_s_content_is_learnt_when_it_is_read,
    },
    Migration {
        version: 98,
        apply: system_one_beside_the_rules,
    },
    Migration {
        version: 99,
        apply: a_certificate_may_cite_its_graded_run,
    },
    Migration {
        version: 100,
        apply: a_rule_change_is_proposed_and_decided,
    },
    Migration {
        version: 101,
        apply: a_decision_keeps_the_rules_answer_in_view,
    },
    Migration {
        version: 102,
        apply: an_id_type_says_whether_it_identifies,
    },
    Migration {
        version: 103,
        apply: a_card_is_kept_by_a_person,
    },
    Migration {
        version: 104,
        apply: a_reserve_number_is_built_in_and_never_kept,
    },
    Migration {
        version: 105,
        apply: a_handover_has_its_own_password,
    },
    Migration {
        version: 106,
        apply: a_dataset_s_files_are_counted_from_the_indexes,
    },
    Migration {
        version: 107,
        apply: a_verify_campaign_plants_values,
    },
    Migration {
        version: 108,
        apply: a_tool_s_terms_are_accepted_as_events,
    },
    Migration {
        version: 109,
        apply: a_pipeline_version_is_the_pair_and_a_unit_records_its_pack,
    },
    Migration {
        version: 110,
        apply: an_audit_draws_by_count_and_is_settled,
    },
];

/// Record 58 P1 (Wave 7b spec §12): a verify campaign's planting
/// (`campaign.planted`), its planted items (`campaign_planted`) and the
/// names accounts go by (`principal_name`). Only a column and two tables
/// are added; no row is touched, and a campaign from before plants nothing.
/// Record 58 P2 (Wave 7b spec §13): an audit draw by count since a time,
/// the settle campaign and its seed and share, the rule review a round
/// over the rule opened, and each drawn stack's vendor and acquisition
/// year. Only columns are added; a draw from before is a draw by share,
/// unsettled, and its stacks' vendor and year are unknown.
fn an_audit_draws_by_count_and_is_settled(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "audit_draw",
        &[
            "count",
            "since",
            "settle_campaign_id",
            "settle_seed",
            "settle_share",
            "review_campaign_id",
        ],
    )?;
    add_columns(store, "audit_draw_stack", &["vendor", "year"])
}

fn a_verify_campaign_plants_values(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["campaign_planted", "principal_name"])?;
    add_columns(store, "campaign", &["planted"])
}

/// Record 57 P5 (Wave 7b, section 6.5): a tool's terms are accepted by an
/// administrator once per site, and the acceptance is an event the
/// registry keeps (who, when, which terms, the licence file's sha256; never
/// its content), which a run of the tool is refused without. The table is
/// added; nothing else changes.
fn a_tool_s_terms_are_accepted_as_events(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["software_acceptance"])
}

/// The Data page on a dataset of 30 million files (2026-10-17): its summary
/// counted the files by status, found the subjects of its set-aside files
/// and the held IDs door the subjects its copies carry by reading every row
/// of the two largest tables, on every request. `source_file` gains an
/// index on its source and status, so a dataset's files are counted by
/// status from the index and its refused and copied files are found among
/// few; `pseudonym_file`'s index on its place and state gains the subject,
/// so the subjects a place's copies carry are read from the index one
/// subject at a time, and the narrower index it replaces is dropped. Only
/// indexes change. Each is built once, over every row of its table: on a
/// registry of 35 million files, a minute or two each.
fn a_dataset_s_files_are_counted_from_the_indexes(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_indexes(store, "source_file")?;
    if table_exists(store, "pseudonym_file")? {
        add_indexes(store, "pseudonym_file")?;
        let narrower = store.qualified("ix_pseudonym_file_place_id_state");
        store.batch(&format!("DROP INDEX IF EXISTS {narrower}"))?;
    }
    Ok(())
}

/// Record 57 P1 to P4 (Wave 7b A1, 2026-10-18): a pipeline version is the
/// pair of descriptor and pack (`pipeline.pack`, `pack_digest`,
/// `version_digest`), and a unit records what the pack decided of it and
/// the two keys its results carry (`pipeline_unit.pack`,
/// `comparability_key`, `result_key`). The columns are added and left
/// empty: a version from before is one with no pack, and a unit from before
/// recorded no knobs and has no key.
fn a_pipeline_version_is_the_pair_and_a_unit_records_its_pack(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "pipeline",
        &["pack", "pack_digest", "version_digest"],
    )?;
    add_columns(
        store,
        "pipeline_unit",
        &["pack", "comparability_key", "result_key"],
    )?;
    add_columns(store, "pipeline_run", &["person"])
}

/// Record 55, ruling 35 (c), Nima 2026-10-16: each handover has its own
/// password, made or typed, or is not encrypted with a reason
/// (`handover.password`, `password_sealed`, `unencrypted_why`). The columns
/// are added and left empty: a handover from before keeps the password its
/// key gave every handover under it, and says so.
fn a_handover_has_its_own_password(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "handover",
        &["password", "password_sealed", "unencrypted_why"],
    )
}

/// Record 55 K6 (2026-10-16): a person keeps a card, and an assistant's
/// draft is no card until they do (`ask_document.kept_by`, `kept_at`). The
/// two columns are added, and every line of versions the registry holds is
/// marked kept by whoever made it, when it was made. A draft stored before
/// cannot be told from a card made by hand, and no card leaves a person's
/// list unasked. Only the first document of a line is marked: the one with
/// no parent, or whose parent is no longer stored.
fn a_card_is_kept_by_a_person(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "ask_document", &["kept_by", "kept_at"])?;
    let table = store.qualified("ask_document");
    store.execute(
        &format!(
            "UPDATE {table} SET kept_by = principal, kept_at = created_at \
             WHERE kept_at IS NULL AND (parent_id IS NULL OR parent_id NOT IN (SELECT id FROM {table}))"
        ),
        &[],
    )?;
    Ok(())
}

/// Record 55 ruling 22 (b): a certificate may cite its graded run (the
/// run, its manifest's sha256, the plan's commit and the model's artifact
/// digest) instead of a sample sealed in this registry, and names its one
/// model, a run certifying a model once (a unique key on the sample and
/// the model where it names one). Only columns and that key are added; a
/// certificate from before measured a sealed sample.
fn a_certificate_may_cite_its_graded_run(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "certificate", &["evidence", "model_id"])?;
    add_indexes(store, "certificate")
}

/// Record 56 (sections 6.4 and 6.5): a proposed rule change as a registry
/// object (`rule_proposal`), decided by a person and applied by a re-sort.
/// Every overlay adopted before becomes the scoped change it is, so it stays
/// in its scope and every sort applies it, where its adoption sorted the
/// whole registry under it once and the next sort dropped it.
fn a_rule_change_is_proposed_and_decided(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["rule_proposal"])?;
    if table_exists(store, "overlay")? {
        crate::rule_proposal::adopt_overlays(store)?;
    }
    Ok(())
}

/// Record 56 §5.2 (2026-10-15): a decision keeps the rules' answer the
/// person saw when they made it (`decision.rules_in_view`), so a sort asks
/// about it again only when a newer rules version moves that answer. Only
/// a column is added; no row is touched.
fn a_decision_keeps_the_rules_answer_in_view(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "decision", &["rules_in_view"])
}

/// Record 55, ruling 24: an ID type says whether its IDs identify a person
/// wherever they are met, which makes resolving one a re-identification.
/// The built-in identifying types are marked; a type a person made is a
/// study's own until they say otherwise.
fn an_id_type_says_whether_it_identifies(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Linkage {
        return Ok(());
    }
    add_columns(store, "id_type", &["identifying"])?;
    mark_identifying(store)
}

/// The built-in identifying types marked so.
fn mark_identifying(store: &mut Store) -> Result<(), Error> {
    let table = store.qualified("id_type");
    let d = store.dialect();
    let sql = format!(
        "UPDATE {table} SET identifying = 1 WHERE name = {}",
        d.param(1, crate::schema::Type::Text)
    );
    for name in schema::IDENTIFYING_ID_TYPES {
        store.execute(&sql, &[Param::from(name)])?;
    }
    Ok(())
}

/// Record 55 F2 and F5 (spec §12.3): the post-contrast label of each stack,
/// the time rule's and the subject step's (`contrast_label`). Only a table
/// is added; a registry from before has no labels until its sessions are
/// labelled (`nils contrast label`, or the next classification).
fn a_stack_carries_its_contrast_label(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["contrast_label"])
}

/// Record 55 D4: several admitted models per task and slot, and the one a
/// site chose (`model_choice`). Only a table is added; a registry from
/// before has no choice, and each task answers with its default.
/// Record 55 D3, D6 and D7 with record 56 (spec §11): System 1's answers
/// as a second opinion (`opinion`), the rounds of a rules version and a
/// System 1 version and the gate's outcomes on flagged datasets, a
/// question's mode and the provisional answers and reads of blind-first
/// review, the blind mark and the seconds on a decision and on a
/// campaign's answer, a campaign's purpose and its reveals, a dataset's
/// damaged-header flag, and the audit draws. Only tables and columns are
/// added; no row is touched.
fn system_one_beside_the_rules(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(
        store,
        kind,
        &[
            "opinion",
            "review_round",
            "gate_outcome",
            "review_provisional",
            "review_read",
            "campaign_reveal",
            "audit_draw",
            "audit_draw_stack",
        ],
    )?;
    add_columns(store, "review_item", &["mode"])?;
    add_columns(store, "decision", &["blind", "seconds", "mode"])?;
    add_columns(store, "campaign", &["purpose"])?;
    add_columns(store, "campaign_answer", &["blind"])?;
    add_columns(store, "place", &["headers"])
}

fn a_site_chooses_a_model_per_task(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["model_choice"])
}

/// Record 55 E5: the sha256 of a source file's bytes, learnt when a
/// pipeline read it whole (`file_content`). Only a table is added; a
/// registry from before knows no file's content until a run reads it.
fn a_file_s_content_is_learnt_when_it_is_read(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["file_content"])
}

/// After type 3's pipe test (record 55 H3): an item its close could not
/// write is kept as refused, with when and why, rather than left looking
/// agreed. Only columns are added; no row is touched.
fn a_refused_item_says_why(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "campaign_item", &["refused_at", "refused_why"])
}

/// Record 55 (rulings of 2026-10-12, item 6, carried to releases): a
/// release keeps the main scans it read, frozen when it is made
/// (`release_pick`), since a scope's newer rules replace the picks of the
/// version before. Only a table is added; a release from before has none,
/// and its readers read the live picks of what it recorded while they are
/// there.
fn a_release_keeps_the_picks_it_read(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["release_pick"])
}

/// 2026-10-12: a run's failure is recorded with its kind (dependency,
/// resource, input or the pipeline's own), its cause and what to do, beside
/// the error it keeps. Only a column is added; a run from before reads its
/// failure from its error.
fn a_failure_says_its_kind(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "pipeline_run", &["failure"])
}

/// Job type 3, check a result (record 55 H3, the build spec's section 7,
/// the campaign's half): a campaign item names the one result it asks
/// about, the part set it is read as and, at part grain, the part; a check
/// item's panels are frozen in `campaign_check_panel`. Only columns and a
/// table are added; no row is touched.
fn a_check_is_a_campaign(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "campaign_item",
        &["derivative_id", "part_set", "part_label"],
    )?;
    add_indexes(store, "campaign_item")?;
    add_tables(store, kind, &["campaign_check_panel"])
}

/// Job type 3, check a result (record 55 H3, the build spec's section 7,
/// the result's half): a derivative gains its grid, the scan whose grid it
/// is on and how the two are tied, its part sets, and the status a check
/// writes; each part of a mask is a row of `derivative_part`. Only columns
/// and a table are added: a mask registered before gains its grid and its
/// parts from `nils derivative parts` the first time a check needs them,
/// and no row is touched here.
fn a_result_can_be_checked(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "derivative",
        &[
            "grid",
            "grid_stack_id",
            "grid_link",
            "part_sets",
            "check_status",
            "check_reasons",
            "check_decision_id",
            "checked_at",
        ],
    )?;
    add_indexes(store, "derivative")?;
    add_tables(store, kind, &["derivative_part"])
}

/// Record 56 (the rules decide; Review asks only what matters), 2026-10-11:
/// a file the read set aside by policy, a SOP class or a modality outside
/// the batch's knobs, files no review item any more. The open
/// `ingest.quarantine` items an older digest filed for those classes are
/// superseded, by `nils` and with the record as the decision, so the queue
/// shows none of them; the files stay quarantined rows as they were, and an
/// item of a class the reader decides stays open.
fn a_file_set_aside_by_policy_asks_nobody(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry || !table_exists(store, "review_item")? {
        return Ok(());
    }
    let rows = store.query(
        &format!(
            "SELECT id, CAST(ref AS TEXT) FROM {} WHERE kind = 'ingest.quarantine' AND status = 'open'",
            store.qualified("review_item")
        ),
        &[],
    )?;
    let mut ids = Vec::new();
    for r in &rows {
        let reference: serde_json::Value =
            serde_json::from_str(r.opt_text(1)?.unwrap_or("null")).unwrap_or_default();
        let policy = reference["class"]
            .as_str()
            .and_then(nils_dicom::QuarantineClass::from_name)
            .is_some_and(|c| c.is_policy_class());
        if policy {
            ids.push(r.int(0)?.to_string());
        }
    }
    if ids.is_empty() {
        return Ok(());
    }
    let d = store.dialect();
    let decision = serde_json::json!({
        "decision": "set_aside",
        "why": "record 56: a file set aside by policy is counted, not asked about",
    });
    store.execute(
        &format!(
            "UPDATE {} SET status = 'superseded', decided_at = {}, actor = 'nils', decision = {} \
             WHERE status = 'open' AND id IN ({})",
            store.qualified("review_item"),
            d.param(1, schema::Type::Timestamp),
            d.param(2, schema::Type::Json),
            ids.join(", ")
        ),
        &[
            Param::from(crate::time::now_iso()),
            Param::from(decision.to_string()),
        ],
    )?;
    Ok(())
}

/// Record 55, decision 6 (the main-scans study, section 4): a dataset or a
/// cohort may keep main-scan rules of its own, versioned in `pick_rules`; a
/// pick names the scope it belongs to and the version of the rules it was
/// made with; and a release names the rules whose picks it read. A registry
/// from before gains the table empty and the columns null: every pick it
/// holds is the pack's.
fn a_scope_keeps_its_own_main_scan_rules(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["pick_rules", "pick_rules_run"])?;
    add_columns(
        store,
        "pick",
        &["rules_scope_kind", "rules_scope_id", "rules_version"],
    )?;
    add_columns(store, "release", &["pick_rules"])
}

/// Wave 7a (2026-10-10, found trying the desk): a dataset whose PatientID
/// held an identifying ID that is the same everywhere failed at its first
/// run, because the registry's linkage store had no id type for it. Every linkage store has
/// it now: a fresh one from the seed ([`ID_TYPES`]), one from before from
/// here, once. A store where a person made the type already keeps theirs.
fn the_same_everywhere_id_type_is_built_in(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Linkage {
        return Ok(());
    }
    if crate::linkage::id_type_id(store, schema::GENERATOR_ID_TYPE)?.is_some() {
        return Ok(());
    }
    let table = store.qualified("id_type");
    let d = store.dialect();
    let sql = format!(
        "INSERT INTO {table} (name, description) VALUES ({}, {})",
        d.param(1, crate::schema::Type::Text),
        d.param(2, crate::schema::Type::Text)
    );
    store.execute(
        &sql,
        &[
            Param::from(schema::GENERATOR_ID_TYPE),
            Param::from(schema::GENERATOR_ID_DESCRIPTION),
        ],
    )?;
    Ok(())
}

/// Record 55 (Nima's duplicate policy, 2026-10-10): a file whose subject,
/// study, series and instance UIDs are an instance the registry holds is a
/// location of that instance, and a dataset holds every scan its tree has a
/// file of, whoever read the scan first. `source_file` gains when a file was
/// first seen, filled from when it was last seen on a row from before;
/// `source_stack`, which sources hold a file of each stack, is filled from
/// the instances' own files a registry from before recorded; and a copy it
/// filed by its instance UID alone is marked `unchecked`, so that the next
/// read of its source compares its subject, study and series and records it
/// as a copy or holds it. No file is touched.
fn a_scan_is_held_wherever_its_files_are(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry || !table_exists(store, "source_file")? {
        return Ok(());
    }
    add_columns(store, "source_file", &["first_seen_at"])?;
    store.execute(
        &format!(
            "UPDATE {} SET first_seen_at = seen_at WHERE first_seen_at IS NULL",
            store.qualified("source_file")
        ),
        &[],
    )?;
    let fresh = !table_exists(store, "source_stack")?;
    add_tables(store, kind, &["source_stack"])?;
    if fresh && table_exists(store, "instance")? && table_exists(store, "instance_frame")? {
        crate::location::backfill(store)?;
    }
    // a copy filed by its instance UID alone is compared on its next read
    store.execute(
        &format!(
            "UPDATE {} SET reason = 'unchecked' WHERE status = 'duplicate' AND reason IS NULL",
            store.qualified("source_file")
        ),
        &[],
    )?;
    Ok(())
}

/// Record 55 H3 (Nima's ruling of 2026-10-09): what the sort decided without
/// asking anybody is kept on the stack's classification. `notes` holds who
/// beat whom where the pack's ranking decided between two rules, the answers
/// below the pack's threshold, the axes no rule answered, the split note and
/// the equal-rank disagreements; `disagreements` counts the last, so that a
/// door finds the stacks a pack cannot rank without reading every note. A
/// registry from before gains both empty, and its next classify fills them.
fn a_classification_notes_what_was_not_asked(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "classification", &["notes", "disagreements"])
}

/// Wave 7a (Nima, 2026-10-08: "on starting page we just need to add a
/// root/s. then on data page when we add the folder as data"): a folder
/// under a root is a dataset only when a person adds it. The datasets a
/// development build made of every folder under a root, and that nothing
/// has read since (no digest of its tree, nothing pseudonymised), are
/// removed, their folders plain folders again; one that was read stays a
/// dataset. No file is touched.
fn a_root_s_folder_is_a_dataset_only_when_added(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry || !table_exists(store, "place")? {
        return Ok(());
    }
    let t = schema::table("place");
    let dataset = store
        .dialect()
        .text_of(t.column("dataset").expect("place.dataset"));
    let rows = store.query(
        &format!(
            "SELECT id, {dataset}, path FROM {} WHERE role = 'source' ORDER BY id",
            store.qualified("place")
        ),
        &[],
    )?;
    // Each root a digest read, as it was given and as it was resolved, each
    // also resolved again now: a folder or tree behind a symbolic link has
    // its resolved root outside the place's path (#457).
    let read: Vec<std::path::PathBuf> = if table_exists(store, "source")? {
        let mut out = Vec::new();
        for r in store.query(
            &format!(
                "SELECT root, root_canonical FROM {}",
                store.qualified("source")
            ),
            &[],
        )? {
            for i in 0..2 {
                if let Some(t) = r.opt_text(i)? {
                    let p = std::path::PathBuf::from(t);
                    out.push(crate::place::canonical_prefix(&p));
                    out.push(p);
                }
            }
        }
        out
    } else {
        Vec::new()
    };
    let held = table_exists(store, "pseudonym_file")?;
    for r in &rows {
        let doc: serde_json::Value = r
            .opt_text(1)?
            .and_then(|d| serde_json::from_str(d).ok())
            .unwrap_or(serde_json::Value::Null);
        if doc["root"].is_null() {
            continue;
        }
        let id = r.int(0)?;
        let path = std::path::Path::new(r.text(2)?);
        if was_read(path, &doc, &read) {
            continue;
        }
        if held {
            let n = store
                .query(
                    &format!(
                        "SELECT COUNT(*) FROM {} WHERE place_id = {}",
                        store.qualified("pseudonym_file"),
                        store.dialect().param(1, schema::Type::Int)
                    ),
                    &[Param::Int(id)],
                )?
                .first()
                .map(|r| r.int(0))
                .transpose()?
                .unwrap_or(0);
            if n > 0 {
                continue;
            }
        }
        store.execute(
            &format!(
                "DELETE FROM {} WHERE id = {}",
                store.qualified("place"),
                store.dialect().param(1, schema::Type::Int)
            ),
            &[Param::Int(id)],
        )?;
    }
    Ok(())
}

/// Whether a digest read a dataset's folder (schema 84's step): a root it
/// read lies under the folder, or under one of its trees (those its
/// declaration names, and the two a dataset's structure gives), each side
/// compared as stored and resolved, as [`crate::place::Place::holds_path`]
/// resolves both. A folder or a tree that is a symbolic link whose target
/// cannot be resolved now (storage not mounted) cannot be shown unread, and
/// counts as read: a migration never removes what may have been read.
fn was_read(path: &std::path::Path, doc: &serde_json::Value, read: &[std::path::PathBuf]) -> bool {
    let mut mine = vec![path.to_path_buf()];
    let mut rels: Vec<String> = vec![
        crate::place::ORIGINALS_TREE.to_string(),
        crate::place::ANON_TREE.to_string(),
    ];
    if let Some(trees) = doc["trees"].as_object() {
        rels.extend(trees.values().filter_map(|v| v.as_str()).map(String::from));
    }
    for rel in rels {
        if rel != "." && !rel.is_empty() {
            mine.push(path.join(rel));
        }
    }
    let mut prefixes = Vec::new();
    for m in &mine {
        let dangling = m.ancestors().any(|a| {
            std::fs::symlink_metadata(a).is_ok_and(|md| md.file_type().is_symlink())
                && std::fs::canonicalize(a).is_err()
        });
        if dangling {
            return true;
        }
        prefixes.push(crate::place::canonical_prefix(m));
        prefixes.push(m.clone());
    }
    read.iter()
        .any(|s| prefixes.iter().any(|p| s.starts_with(p)))
}

/// Wave 7a: schema 80's step again, for a store a development build
/// migrated under its earlier form, before a dataset's state was read from
/// its structure: every source place without a whole, current declaration
/// undeclared, a place on a dataset's pseudonymised tree legacy. On a store
/// schema 80 already brought there it changes nothing.
fn a_dataset_s_declaration_is_its_structure(store: &mut Store, kind: Kind) -> Result<(), Error> {
    a_source_place_nobody_declared_is_undeclared(store, kind)
}

/// Wave 7a §5.4: a dataset may write an id type's value into PatientID,
/// and a file whose subject has none is held with its subject and the type
/// it waits for. A registry from before gains both columns empty.
fn a_held_file_may_wait_for_its_subject_s_id_type(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "pseudonym_file", &["subject_id", "wants_type"])
}

/// Wave 7a §5.3 (Nima, 2026-10-08: "when we have production i add each
/// data one by one and decide how we treat them"): a source place without
/// a whole, current declaration is `undeclared`, and nothing in it is read
/// until a person says how its files arrive. That is a place `nils setup`
/// made with no dataset, which every reader took as a de-identified folder
/// read whole; one declared only by the old default to read its folder
/// itself; and one arriving de-identified or coded that does not say what
/// PatientID holds and how its subjects are found. The rest of each
/// dataset (its rule, cohort and tag lists, what became of its originals)
/// is kept. A whole declaration is kept as it is. A place that names a
/// dataset's pseudonymised tree itself (a path ending in
/// `derivatives/dcm-raw` or `derivatives/dcm-anon`) is a `legacy` place:
/// it keeps reading that tree as an anonymised dataset, once it says what
/// PatientID holds and how its subjects are found. Which of the rest are
/// roots is the folder's to say, and the next exploration of the place
/// says it. No file is touched.
fn a_source_place_nobody_declared_is_undeclared(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry || !table_exists(store, "place")? {
        return Ok(());
    }
    let t = schema::table("place");
    let dataset = store
        .dialect()
        .text_of(t.column("dataset").expect("place.dataset"));
    let rows = store.query(
        &format!(
            "SELECT id, {dataset}, path FROM {} WHERE role = 'source' ORDER BY id",
            store.qualified("place")
        ),
        &[],
    )?;
    for r in &rows {
        let current: serde_json::Value = r
            .opt_text(1)?
            .and_then(|d| serde_json::from_str(d).ok())
            .unwrap_or(serde_json::Value::Null);
        let path = std::path::Path::new(r.text(2)?);
        let names_a_tree = path
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|p| p == "derivatives")
            && path
                .file_name()
                .is_some_and(|n| n == "dcm-raw" || n == "dcm-anon");
        let kept = crate::place::dataset_of(&current, None).ok();
        if names_a_tree {
            let legacy = crate::place::dataset_of(
                &serde_json::json!({
                    "kind": "legacy",
                    "arrives": "deidentified",
                    "state": "anonymised",
                    "trees": {"originals": null, "anon": "."},
                }),
                kept.as_ref(),
            )
            .map_err(Error::Message)?;
            if Some(&legacy) != kept.as_ref() || current.is_null() {
                store.update_by_id(
                    t,
                    &[("dataset", Param::from(legacy.to_string()))],
                    "id",
                    r.int(0)?,
                )?;
            }
            continue;
        }
        if crate::place::incomplete(&current).is_none() {
            continue;
        }
        let undeclared = crate::place::dataset_of(
            &serde_json::json!({"arrives": crate::place::UNDECLARED}),
            kept.as_ref(),
        )
        .unwrap_or_else(|_| crate::place::default_dataset(None));
        if Some(&undeclared) == kept.as_ref() && !current.is_null() {
            continue;
        }
        store.update_by_id(
            t,
            &[("dataset", Param::from(undeclared.to_string()))],
            "id",
            r.int(0)?,
        )?;
    }
    Ok(())
}

/// Wave 7a (Nima, 2026-10-08: "we never save pn any where on registery or
/// audit"): no personnummer is kept, not even sealed. The linkage store
/// keeps a personnummer identity as its keyed lookup alone, and a held
/// file's row keeps its shape and lookup; what a store from before sealed
/// of one is dropped. The subject's code stands for the person.
///
/// A gap, written down after the review of Wave 7a's merge (2026-10-10):
/// before Wave 7a a personnummer was filed under the keyed lookup of the
/// number as written, and lookups now take its twelve digits, so a number
/// filed as `850101-2382` is not found again as `198501012382`, and with its
/// sealed value dropped here its lookup cannot be derived again. Deriving it
/// needs the registry's key, which a migration does not hold. Only a
/// registry that read personnummer before Wave 7a holds such identities:
/// the pre-production one is archived for research as migrated and never
/// read into again (record 55 B1, and the ruling of 2026-10-09), and
/// production starts empty. A store that must go on reading personnummer
/// would be keyed again with its key before it migrates; no tool does that
/// yet.
fn a_personnummer_is_never_kept(store: &mut Store, kind: Kind) -> Result<(), Error> {
    nothing_kept_of(store, kind, crate::personnummer::ID_TYPE)
}

/// What a store sealed of one type's identifiers dropped: in the linkage
/// store, every identity of the type keeps its keyed lookup alone; in the
/// registry, a held file of the type keeps its shape and its lookup.
fn nothing_kept_of(store: &mut Store, kind: Kind, id_type: &str) -> Result<(), Error> {
    let d = store.dialect();
    match kind {
        Kind::Linkage => {
            let Some(id) = crate::linkage::id_type_id(store, id_type)? else {
                return Ok(());
            };
            store.execute(
                &format!(
                    "UPDATE {} SET ciphertext = {} WHERE id_type_id = {}",
                    store.qualified("identity"),
                    d.param(1, schema::Type::Bytes),
                    d.param(2, schema::Type::Int)
                ),
                &[Param::Bytes(Vec::new()), Param::Int(id)],
            )?;
        }
        Kind::Registry => {
            if !table_exists(store, "pseudonym_file")? {
                return Ok(());
            }
            store.execute(
                &format!(
                    "UPDATE {} SET sealed = NULL WHERE id_type = {}",
                    store.qualified("pseudonym_file"),
                    d.param(1, schema::Type::Text)
                ),
                &[Param::from(id_type)],
            )?;
        }
    }
    Ok(())
}

/// Nima, 2026-10-16: a hospital's reserve number, which a dataset whose
/// PatientID holds an ID that is the same everywhere reads a PatientID that
/// is none as, identifies a person as a personnummer does without being one
/// ("just like pns however they are not real pns"), and has its protection.
/// Every linkage store has its type, built in and identifying: a fresh one
/// from the seed ([`ID_TYPES`]), one from before from here, once. The store
/// never keeps a value of it, not even sealed ([`crate::linkage::keeps`]):
/// what a type of that name a person made before sealed is dropped, in the
/// linkage store and in a held file's row, as migration 82 dropped what a
/// store from before sealed of a personnummer.
fn a_reserve_number_is_built_in_and_never_kept(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind == Kind::Linkage {
        if crate::linkage::id_type_id(store, schema::RESERVE_ID_TYPE)?.is_none() {
            let table = store.qualified("id_type");
            let d = store.dialect();
            store.execute(
                &format!(
                    "INSERT INTO {table} (name, description) VALUES ({}, {})",
                    d.param(1, crate::schema::Type::Text),
                    d.param(2, crate::schema::Type::Text)
                ),
                &[
                    Param::from(schema::RESERVE_ID_TYPE),
                    Param::from(schema::RESERVE_ID_DESCRIPTION),
                ],
            )?;
        }
        mark_identifying(store)?;
    }
    nothing_kept_of(store, kind, schema::RESERVE_ID_TYPE)
}

/// The 2026-10-03 fingerprint fields: a series gains how it stores its
/// pixels (PhotometricInterpretation and SamplesPerPixel), an MR series the
/// contrast an enhanced object says it was acquired for
/// (AcquisitionContrast), and the fingerprint those, the angiography flag,
/// the temporal resolution with where it came from, and the diffusion
/// directionality of the stack's images, for a pack to read. A registry from
/// before gains every column empty and no row is written. The fingerprint's
/// revision moved with them, so the next `nils fingerprint` derives every
/// stack again, and the angiography flag, the temporal resolution and the
/// directionality fill at once from what the registry holds. The series
/// columns fill when one file of each series is read again, which `nils
/// digest <root> --reread-every --reread-one --reread-missing` does.
fn a_fingerprint_reads_the_2026_10_03_fields(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "series",
        &["photometric_interpretation", "samples_per_pixel"],
    )?;
    add_columns(store, "series_mr", &["acquisition_contrast"])?;
    add_columns(store, "stack_fingerprint", FINGERPRINT_79)
}

/// The fingerprint's columns of schema 79.
pub const FINGERPRINT_79: &[&str] = &[
    "angio_flag",
    "acquisition_contrast",
    "temporal_resolution",
    "temporal_resolution_source",
    "diffusion_directionality",
    "photometric_interpretation",
    "samples_per_pixel",
];

/// A stack, and its fingerprint for a pack to read, gain the ImageType
/// Philips writes per frame of an enhanced MR object in its (2005,140F)
/// private sequence, whose third and fourth values name a Dixon part (W, F,
/// IP or OP) that the object's top-level ImageType does not. A registry from
/// before gains both columns empty and no row is written; a stack's column
/// fills when its files are read again, and the fingerprint's when it is
/// derived again, which its revision moving makes the next `nils fingerprint`
/// do. Only a Philips enhanced object has the value to give.
fn a_stack_keeps_the_philips_frame_image_type(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "stack", &["private_frame_image_type"])?;
    add_columns(store, "stack_fingerprint", &["private_frame_image_type"])
}

/// Record 53 S1: an MR series gains the MR Pulse Sequence module's mechanism
/// attributes (echo pulse sequence, multiple spin echo, echo planar, steady
/// state, time of flight and arterial spin labelling contrast, the geometry
/// and segmentation of k-space) and the MR Modifier group's spoiling and
/// inversion recovery; the fingerprint carries them, the phase contrast the
/// series already held, and the series' SOP class, for a pack to read. A
/// registry from before gains every column empty. The fingerprint's revision
/// moved with them, so the next `nils fingerprint` derives every stack again,
/// and the SOP class and the phase contrast fill at once from the series. The
/// other series columns fill when the files are read again, which `nils
/// digest --reread` does per manufacturer; only an enhanced or spectroscopy
/// object has them to give.
fn a_series_says_how_it_was_acquired(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "series_mr",
        &[
            "echo_pulse_sequence",
            "multiple_spin_echo",
            "echo_planar_pulse_sequence",
            "steady_state_pulse_sequence",
            "time_of_flight_contrast",
            "arterial_spin_labeling_contrast",
            "geometry_of_k_space_traversal",
            "segmented_k_space_traversal",
            "spoiling",
            "inversion_recovery",
        ],
    )?;
    add_columns(
        store,
        "stack_fingerprint",
        &[
            "sop_class_uid",
            "echo_pulse_sequence",
            "multiple_spin_echo",
            "echo_planar_pulse_sequence",
            "steady_state_pulse_sequence",
            "phase_contrast",
            "time_of_flight_contrast",
            "arterial_spin_labeling_contrast",
            "geometry_of_k_space_traversal",
            "segmented_k_space_traversal",
            "spoiling",
            "inversion_recovery",
        ],
    )
}

/// Post-contrast gold, anchored reading: an item of an anchored campaign
/// shows a candidate beside a pre and a post anchor, and which stack is in
/// which panel and plays which role is kept in a table of its own. A
/// registry from before gains it empty, since no campaign before asked of
/// anchored items.
fn a_campaign_reads_a_stack_beside_two_anchors(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["campaign_anchor_panel"])
}

/// The post-contrast study's single reads: a campaign may show its raters
/// the pictures alone, with no text of the file's header beside them. A
/// campaign from before gains the column empty, and shows the header as it
/// did.
fn a_campaign_may_show_the_pictures_alone(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry || !table_exists(store, "campaign")? {
        return Ok(());
    }
    add_columns(store, "campaign", &["hide_header"])
}

/// Post-contrast gold, pair mode: an item of a pair campaign shows two
/// stacks side by side, and which is on which side is kept in a table of
/// its own. A registry from before gains it empty, since no campaign before
/// asked of pairs.
fn a_campaign_shows_two_stacks_side_by_side(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["campaign_pair_side"])
}

/// Record 48, the reference read by judges: an A/B campaign's items carry
/// the voters' candidates, an answer on one says what it chose on each
/// axis, and the person who settled it gives a cause per axis. A registry
/// from before gains the three tables empty and the column empty, since no
/// campaign before settled candidates.
fn a_campaign_settles_candidates_blind(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "campaign_answer", &["choices"])?;
    add_tables(
        store,
        kind,
        &[
            "campaign_ab_item",
            "campaign_ab_candidate",
            "campaign_ab_cause",
        ],
    )
}

/// A campaign says what its reader shows beside each item as the answer
/// suggested: none, the rules, or suggestions brought in. A campaign from
/// before is written as what it showed: `imported` where suggestions were
/// brought into it, `rules` otherwise, since the engine then showed its
/// rules' answer on every item of a stack not sealed.
fn a_campaign_says_what_it_suggests(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry || !table_exists(store, "campaign")? {
        return Ok(());
    }
    add_columns(store, "campaign", &["suggest"])?;
    let c = store.qualified("campaign");
    if table_exists(store, "campaign_suggestion")? {
        let s = store.qualified("campaign_suggestion");
        store.execute(
            &format!(
                "UPDATE {c} SET suggest = 'imported' WHERE suggest IS NULL \
                 AND EXISTS (SELECT 1 FROM {s} s WHERE s.campaign_id = {c}.id)"
            ),
            &[],
        )?;
    }
    store.execute(
        &format!("UPDATE {c} SET suggest = 'rules' WHERE suggest IS NULL"),
        &[],
    )?;
    Ok(())
}

/// The 2026-09-28 sequence research: an MR series gains PulseSequenceName
/// (0018,9005), and the fingerprint carries it for a pack to read. A
/// registry from before gains both empty. The fingerprint's revision moved
/// with them, so the next `nils fingerprint` derives every stack again; the
/// series column fills when the files are read again, which `nils digest
/// --reread` does for the manufacturers that need it.
fn a_series_says_its_pulse_sequence(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "series_mr", &["pulse_sequence_name"])?;
    add_columns(store, "stack_fingerprint", &["pulse_sequence_name"])
}

/// Record 49 A1: pipeline runs have a lane of their own, their units run
/// side by side within a budget, and a run that stopped is taken up again
/// with the units it finished kept. A registry from before gains the unit
/// table empty and three columns on the run, empty on every run before,
/// which ran its units together, was never resumed and recorded no
/// threshold of its own.
fn a_run_s_units_are_scheduled_and_resumed(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "pipeline_run", &["units", "resumes", "threshold"])?;
    add_tables(store, kind, &["pipeline_unit"])
}

/// Record 49 A3 and A4: the measures a run's tables and declared metrics
/// hold, which the ask reads, and a catalog entry's origin, `starter` where
/// the engine seeded it. A registry from before gains the table empty, since
/// no run before wrote a table, and the column empty, since every entry
/// before was a person's.
fn a_run_s_numbers_are_measures_and_a_starter_says_so(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "pipeline", &["origin"])?;
    add_tables(store, kind, &["measure"])
}

/// Record 48, how the reference is read: an answer says whether the rater
/// marked the stack unsure. A registry from before gains the column empty,
/// which reads as not unsure, since no answer before could say it.
fn an_answer_may_be_unsure(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "campaign_answer", &["unsure"])
}

/// Record 49 R7: a run's output goes to the lane's output place and its
/// scratch to its scratch place. A registry from before gains the column
/// empty on every run, which read as scratch and output in the one place.
fn a_run_s_scratch_is_apart_from_its_output(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "pipeline_run", &["scratch_place_id"])
}

/// Record 48, after the first real read: an answer keeps the axes the
/// engine derived from it through the pack. A registry from before gains
/// the column empty, since no question before derived anything.
fn an_answer_keeps_what_it_derived(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "campaign_answer", &["derived"])
}

/// Record 50 R3: a campaign carries answers suggested from outside the
/// engine, each with its author and its confidences, and an answer says who
/// suggested what it was given beside. A registry from before gains the
/// table empty and the column empty, since no suggestion came from outside
/// before and no answer said whose its suggestion was.
fn a_campaign_carries_suggestions_from_outside(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "campaign_answer", &["suggested_by"])?;
    add_tables(store, kind, &["campaign_suggestion"])
}

/// Record 48, after the first gold campaign: a rater corrects their own
/// answer while the campaign is open, and the correction is a new answer
/// that supersedes the earlier, which is kept. The key of one answer per
/// item, rater and round becomes one first answer per item, rater and
/// round, and each answer is superseded once at most. Postgres drops the
/// old key by its name; SQLite cannot drop a key its table declared, so
/// the table is rebuilt with every row and its id as it was.
fn an_answer_may_be_corrected(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    if !table_exists(store, "campaign_answer")? {
        return add_tables(store, kind, &["campaign_answer"]);
    }
    add_columns(
        store,
        "campaign_answer",
        &["supersedes_id", "superseded_by"],
    )?;
    const OLD_KEY: &str = "uq_campaign_answer_item_id_principal_round";
    match store {
        Store::Postgres { .. } => {
            let t = store.qualified("campaign_answer");
            store.batch(&format!(
                "ALTER TABLE {t} DROP CONSTRAINT IF EXISTS {OLD_KEY}"
            ))?;
            add_indexes(store, "campaign_answer")
        }
        Store::Sqlite(_) => {
            let declared = store
                .query_opt(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'campaign_answer'",
                    &[],
                )?
                .and_then(|r| r.opt_text(0).ok().flatten().map(str::to_string))
                .unwrap_or_default();
            if !declared.contains(OLD_KEY) {
                return add_indexes(store, "campaign_answer");
            }
            let dialect = store.dialect();
            let schema_name = store.schema().map(str::to_string);
            let old = store.qualified("campaign_answer");
            let mut rebuilt = schema::table("campaign_answer").clone();
            rebuilt.name = "campaign_answer_rebuilt";
            // the columns a later migration adds are its own to add
            let mut had = Vec::new();
            for c in &rebuilt.columns {
                if column_exists(store, "campaign_answer", c.name)? {
                    had.push(c.name);
                }
            }
            rebuilt.columns.retain(|c| had.contains(&c.name));
            store.batch(&dialect.create_table(schema_name.as_deref(), &rebuilt))?;
            let new = store.qualified("campaign_answer_rebuilt");
            let cols: Vec<&str> = rebuilt.columns.iter().map(|c| c.name).collect();
            let cols = cols.join(", ");
            store.batch(&format!(
                "INSERT INTO {new} ({cols}) SELECT {cols} FROM {old} ORDER BY id"
            ))?;
            store.batch(&format!("DROP TABLE {old}"))?;
            store.batch(&format!("ALTER TABLE {new} RENAME TO campaign_answer"))?;
            for ix in
                dialect.create_indexes(schema_name.as_deref(), schema::table("campaign_answer"))
            {
                store.batch(&ix)?;
            }
            Ok(())
        }
    }
}

/// Record 48: an answer says how long it took, what the engine suggested
/// and whether the answer changed it, and how it came (a claim or a
/// batch); an item says a batch held it back to be read alone; a sealed stack may be unsealed by the certificate its sample
/// was drawn for; and the certificates have a table. A registry from
/// before gains the columns empty, since no answer before was timed and
/// nothing was ever unsealed, and the table empty.
fn a_reading_is_timed_and_a_certificate_unseals(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "campaign_answer",
        &["seconds", "suggested", "changed", "via"],
    )?;
    add_columns(store, "campaign_item", &["held_back"])?;
    add_columns(store, "campaign", &["hold_back", "hold_back_seed"])?;
    // every campaign from before draws a seed of its own now, a secret
    // nothing derives from its id
    if !table_exists(store, "campaign")? {
        return add_tables(store, kind, &["certificate"]);
    }
    let c = store.qualified("campaign");
    let ids: Vec<i64> = store
        .query(
            &format!("SELECT id FROM {c} WHERE hold_back_seed IS NULL"),
            &[],
        )?
        .iter()
        .map(|r| r.int(0))
        .collect::<Result<_, _>>()?;
    for id in ids {
        store.update_by_id(
            schema::table("campaign"),
            &[(
                "hold_back_seed",
                Param::from(uuid::Uuid::new_v4().to_string()),
            )],
            "id",
            id,
        )?;
    }
    add_columns(store, "campaign_assignment", &["leased_ms"])?;
    add_columns(
        store,
        "sealed_stack",
        &["unsealed_at", "unsealed_by", "certificate_id"],
    )?;
    add_tables(store, kind, &["certificate"])
}

/// Record 43: the release a pipeline run materialised its bids input by
/// says so, so the release history can leave it out. A registry from before
/// gains the column, and the releases its runs name are marked.
fn a_run_s_input_release_says_so(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "release", &["purpose"])?;
    let (r, pr) = (store.qualified("release"), store.qualified("pipeline_run"));
    store.batch(&format!(
        "UPDATE {r} SET purpose = '{}' WHERE purpose IS NULL AND id IN \
         (SELECT input_release_id FROM {pr} WHERE input_release_id IS NOT NULL)",
        crate::pipeline::RUN_INPUT
    ))?;
    Ok(())
}

/// Record 43: a head reads several encoders. A registry from before gains
/// the table with each head's one encoder as its first, so every reader of
/// the list sees what the row said.
fn a_head_reads_its_encoders_in_order(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["model_encoder"])?;
    let (me, m) = (store.qualified("model_encoder"), store.qualified("model"));
    store.batch(&format!(
        "INSERT INTO {me} (model_id, encoder_model_id, position) \
         SELECT id, encoder_model_id, 0 FROM {m} WHERE encoder_model_id IS NOT NULL \
         AND id NOT IN (SELECT model_id FROM {me})"
    ))?;
    Ok(())
}

/// Record 43 S1 and S2: the pipeline catalog and the runs. A registry from
/// before gains the two tables empty; with no pipeline added nothing runs,
/// and every derivative from before keeps its null run.
fn a_pipeline_has_a_catalog_and_its_runs(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["pipeline", "pipeline_run"])
}

/// Record 43 S4: the embedding cache's key, a unique index over the live
/// embeddings by stack, encoder and preprocessing version. The column
/// `preprocess_version` came with the table (migration 56), and every row
/// written before has it null, since nothing wrote one: a null is outside
/// the key, so no row of a registry from before stands in the index's way.
fn an_embedding_is_kept_once_per_encoder_and_preprocessing(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "derivative", &["preprocess_version"])?;
    add_indexes(store, "derivative")
}

/// Record 42 S2: the model registry. A registry from before gains the two
/// tables empty, since no model was registered before there was a place to
/// register one, and a release gains the models its tree's answers came
/// from, empty on every release before, which named none.
fn a_model_has_a_home(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["model", "model_event"])?;
    add_columns(store, "release", &["models"])
}

/// Record 42 S1: a decision, a piece of evidence and a pick name the
/// registered model a model's answer came from and the campaign it was given
/// in, and a decision names who put it in force. A registry from before
/// gains the columns empty: no answer before it came from a registered model
/// or a campaign, since neither existed. A decision written without staging
/// was put in force by its own author as it was written, so its committer is
/// its actor; one that was staged and committed later says nothing, because
/// who committed it was never recorded on the row, and guessing would be a
/// claim nobody made (the audit log holds the commit).
fn an_answer_names_its_model_and_its_committer(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "decision",
        &["model_id", "campaign_id", "committed_by"],
    )?;
    add_columns(
        store,
        "classification_evidence",
        &["model_id", "campaign_id"],
    )?;
    add_columns(store, "pick", &["model_id", "campaign_id"])?;
    if table_exists(store, "decision")? {
        store.execute(
            &format!(
                "UPDATE {} SET committed_by = actor WHERE committed_by IS NULL AND staged_at IS NULL",
                store.qualified("decision")
            ),
            &[],
        )?;
    }
    Ok(())
}

/// Record 42 S3: a person's pick. The pick table gains why a person picked,
/// who withdrew a pick, and which person's pick overruled a run's; every
/// pick written before is a run's, so the three are null on it.
fn a_person_may_pick(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "pick", &["why", "withdrawn_by", "overruled_by"])
}

/// Record 42 S4: the derivative table, empty. A registry from before has
/// no derivative, and the working place it would be kept in is unchanged.
fn a_derivative_has_a_home(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["derivative"])
}

/// Record 42 S5: campaigns, their items, the raters' assignments and their
/// answers. A registry from before gains the four tables empty; with no
/// campaign nothing changes and review items work as they did.
fn a_campaign_asks_one_question_of_many(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(
        store,
        kind,
        &[
            "campaign",
            "campaign_item",
            "campaign_assignment",
            "campaign_answer",
        ],
    )
}

/// Record 42 S7: the label sets, each an export with its digest and the
/// handle it pins, and the sealed samples of record 40 R3, whose stacks
/// make a set that holds any of them sealed. A registry from before gains
/// both tables empty: nothing was sealed before there was a seal.
fn labels_leave_with_their_provenance(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["label_set", "sealed_stack"])
}

/// Record 41 S2: every rule's vote. A registry from before gains the two
/// tables empty, and the next `nils classify` fills them; until then
/// `nils classify votes` has nothing to say about a stack, rather than
/// something wrong.
fn every_rule_votes(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(
        store,
        kind,
        &["classification_voter", "classification_vote"],
    )
}

/// Record 38 S3: a release keeps the real date, and the shift and year
/// policies are gone with everything that served them.
///
/// In the registry, a place's declared handling loses `on_release.dates`,
/// since the dates are no longer a choice and a desk showing "dates shifted"
/// on a dataset whose release keeps them would be wrong; and the release
/// plan loses the offset it carried per stack, which is scratch a version
/// clears when it closes. In the linkage store the per-subject offsets go:
/// they served only the shift, and each was derivable from the key anyway.
///
/// What a release from before recorded is not touched: its row keeps its
/// policy, its `session_naming` sentence and its `shifted` change counts,
/// because they say what that release did.
fn the_date_is_the_date(store: &mut Store, kind: Kind) -> Result<(), Error> {
    match kind {
        Kind::Linkage => {
            if table_exists(store, "date_shift")? {
                let t = store.qualified("date_shift");
                store.batch(&format!("DROP TABLE {t}"))?;
            }
            Ok(())
        }
        Kind::Registry => {
            if column_exists(store, "release_plan", "offset_days")? {
                let t = store.qualified("release_plan");
                store.batch(&format!("ALTER TABLE {t} DROP COLUMN offset_days"))?;
            }
            if !table_exists(store, "place")? {
                return Ok(());
            }
            let t = schema::table("place");
            let handling = store
                .dialect()
                .text_of(t.column("handling").expect("place.handling"));
            let rows = store.query(
                &format!(
                    "SELECT id, {handling} FROM {} ORDER BY id",
                    store.qualified("place")
                ),
                &[],
            )?;
            for r in &rows {
                let id = r.int(0)?;
                let Some(mut doc) = r
                    .opt_text(1)?
                    .and_then(|h| serde_json::from_str::<serde_json::Value>(h).ok())
                else {
                    continue;
                };
                let Some(release) = doc
                    .get_mut("on_release")
                    .and_then(serde_json::Value::as_object_mut)
                else {
                    continue;
                };
                if release.remove("dates").is_none() {
                    continue;
                }
                store.update_by_id(t, &[("handling", Param::from(doc.to_string()))], "id", id)?;
            }
            Ok(())
        }
    }
}

/// Record 38 S2: a repeat is the same stack in the same place, identical in
/// everything but its own acquisition time, and numbered in that order. An
/// instance gains the position it sits at in three dimensions. The
/// fingerprint gains the centre of a stack's slices, along the slice normal
/// and in three dimensions, the earliest acquisition
/// date and time of its images, its series number, the gradient directions it
/// played and its temporal position, and a series gains the number the
/// scanner gave it. A registry from before gains them empty; the next `nils
/// fingerprint` derives every stack again because the derivation's revision
/// moved with them, and a series keeps no number until its files are read
/// again, so a `run-` index there falls back to the stack's id after the
/// acquisition time.
fn a_stack_says_where_it_sits_and_when_it_was_made(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "stack_fingerprint",
        &[
            "slice_centre_mm",
            "centre_x_mm",
            "centre_y_mm",
            "centre_z_mm",
            "earliest_acquisition_date",
            "earliest_acquisition_time",
            "series_number",
            "dwi_gradients",
            "temporal_position",
            "temporal_positions",
        ],
    )?;
    add_columns(store, "series", &["series_number"])?;
    add_columns(store, "instance", &["image_position_patient"])
}

/// Record 37 S7: which naming mode a release's names were built under. A row
/// from before says nothing, which is right: it was written when there was
/// one mode, and calling it either now would be a claim nobody made.
fn a_release_says_which_naming_mode_it_wrote(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "release", &["naming"])
}

/// Record 37 S3: the receive coil, which a series is already split on. A
/// registry from before gains the column empty and fills it on the next
/// `nils fingerprint`, which derives every stack again because the
/// derivation's revision moved with it.
fn a_stack_says_which_coil_received_it(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "stack_fingerprint", &["receive_coil_name"])
}

/// Record 37 S1: the coverage facts, and the revision that says which
/// derivation wrote a fingerprint row. A registry from before gains five
/// columns, all empty; the next `nils fingerprint` sees a revision that is
/// not the current one and derives every stack again, without `--force`.
fn a_stack_says_what_it_covers(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(
        store,
        "stack_fingerprint",
        &[
            "n_slices",
            "slice_span_mm",
            "coverage_source",
            "acquisition_matrix",
            "fingerprint_revision",
        ],
    )
}

/// Record 26 §3, §4 and §14: a batch says which step it is, `digest` or
/// `pseudonymize`, every batch from before being a digest; a subject says
/// whether the pseudonymiser made it from an identifier no map named,
/// which no subject from before was; and the pseudonymiser's rows carry
/// their directory, read one directory at a time.
fn a_batch_has_a_kind_and_a_subject_may_be_provisional(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "ingest_batch", &["kind"])?;
    add_columns(store, "subject", &["provisional"])?;
    add_columns(store, "pseudonym_file", &["dir"])?;
    add_indexes(store, "pseudonym_file")?;
    if table_exists(store, "ingest_batch")? {
        store.execute(
            &format!(
                "UPDATE {} SET kind = 'digest' WHERE kind IS NULL",
                store.qualified("ingest_batch")
            ),
            &[],
        )?;
    }
    Ok(())
}

/// Record 26 §8, §9 and §13: a membership interval names the batch of the
/// digest that opened it, a cohort may be retired, and a release records the
/// leaving policy of each dataset it spanned.
fn a_cohort_is_fed_retired_and_released_under_its_policy(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "cohort", &["retired_at"])?;
    add_columns(store, "cohort_member", &["batch_id"])?;
    add_columns(store, "release", &["policies"])
}

/// Record 26 §1: a source place is a dataset, with what arrives, its two
/// trees, its identity rule, what an unmapped identifier does, the cohort it
/// feeds, its tag lists and what becomes of the originals; and the table the
/// pseudonymiser records every file of the originals in. A source place from
/// before reads the folder itself as its pseudonymised tree, arriving as its
/// handling said, or de-identified where none was declared, so every place
/// declared before this keeps working as it did.
fn a_source_place_is_a_dataset(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "place", &["dataset"])?;
    add_tables(store, kind, &["pseudonym_file"])?;
    if !table_exists(store, "place")? {
        return Ok(());
    }
    let t = schema::table("place");
    let handling = store
        .dialect()
        .text_of(t.column("handling").expect("place.handling"));
    let rows = store.query(
        &format!(
            "SELECT id, {handling} FROM {} WHERE role = 'source' AND dataset IS NULL ORDER BY id",
            store.qualified("place")
        ),
        &[],
    )?;
    for r in &rows {
        let id = r.int(0)?;
        let arrives = r
            .opt_text(1)?
            .and_then(|h| serde_json::from_str::<serde_json::Value>(h).ok())
            .and_then(|h| h["arrives"].as_str().map(str::to_string));
        // what a dataset was by default when this migration was written: the
        // arrival its handling named, else de-identified, reading its folder
        let dataset = crate::place::dataset_of(
            &serde_json::json!({
                "arrives": arrives.as_deref().filter(|a| *a != crate::place::UNDECLARED).unwrap_or("deidentified"),
                "trees": {"originals": null, "anon": "."},
            }),
            None,
        )
        .map_err(Error::Message)?;
        store.update_by_id(
            t,
            &[("dataset", Param::from(dataset.to_string()))],
            "id",
            id,
        )?;
    }
    Ok(())
}

/// Record 26 §6: a subject merged into another keeps its row, marked, and
/// the linkage store has the type its code is filed under on the canonical
/// subject. A store from before this gains the type; a fresh one has it
/// from the seed.
fn a_subject_may_be_merged(store: &mut Store, kind: Kind) -> Result<(), Error> {
    match kind {
        Kind::Registry => {
            add_columns(store, "subject", &["merged_into", "merged_at"])?;
            add_indexes(store, "subject")
        }
        Kind::Linkage => {
            let (name, description) = ID_TYPES[2];
            debug_assert_eq!(name, schema::SUBJECT_CODE_TYPE);
            if crate::linkage::id_type_id(store, name)?.is_some() {
                return Ok(());
            }
            let table = store.qualified("id_type");
            let d = store.dialect();
            let sql = format!(
                "INSERT INTO {table} (name, description) VALUES ({}, {})",
                d.param(1, crate::schema::Type::Text),
                d.param(2, crate::schema::Type::Text)
            );
            store.execute(&sql, &[Param::from(name), Param::from(description)])?;
            Ok(())
        }
    }
}

/// §4.3 with record 26 §13: a release says why its sessions were numbered in
/// date order, where a dataset's own declaration moved the dates under a
/// scheme that labels by them. A row written before this says nothing, which
/// is what a run whose scheme stood has to say for itself anyway.
fn a_release_says_why_its_sessions_were_numbered(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "release", &["session_naming"])
}

/// Lab 26d, finding 2: the pseudonymiser records the digest of the original
/// it read beside the digest of the copy it wrote, so that what a purge
/// destroys is proved by its content and not by a size and a modification
/// time anything may set. A row written before this has none, and no reading
/// of the file can supply it after the fact: it stays null until a run of the
/// pseudonymiser reads that original again, and until then a purge of the
/// dataset is refused in words naming that run.
fn an_original_is_proved_by_its_own_digest(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "pseudonym_file", &["original_digest"])
}

/// §8.4 with record 35, finding 3: a release's own row says how many stacks it
/// held because the file said their pixels carry text, and how many it could
/// not judge either way. A row written before this says neither, and no
/// reading of the tree supplies them after the fact: they stay null until that
/// dataset is released again.
fn a_release_says_how_many_stacks_it_could_not_judge(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "release", &["burned_in", "unjudged"])
}

/// Record 37 S8: an enhanced multi-frame instance whose frames hold more than
/// one stack says which frames are in which. A registry digested before this
/// gains the table empty, and stays short of those stacks until the tree is
/// digested again: nothing in a row can say what was never read.
fn an_instance_may_hold_frames_of_more_than_one_stack(
    store: &mut Store,
    kind: Kind,
) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["instance_frame"])
}

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

/// Wave 5 §12.8: the invalidation of a handle is a row.
fn a_handle_may_be_invalidated(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["handle_invalidation"])
}

/// Wave 5 §12.5: a place as a registry object, so that every path the
/// engine takes is bound to a role and the rules of §10.2 are checked at
/// the doors.
fn a_place_is_a_registry_object(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_tables(store, kind, &["place"])
}

/// A place declares how what comes in through it is handled: whether it
/// arrives identified, and what a release does to it on the way out.
fn a_place_declares_how_it_is_handled(store: &mut Store, kind: Kind) -> Result<(), Error> {
    if kind != Kind::Registry {
        return Ok(());
    }
    add_columns(store, "place", &["handling"])
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

/// Create a table's declared indexes that are not there yet, of those whose
/// columns the table has now. The declaration is the current schema's, so
/// a migration that runs before a later one added a column leaves that
/// column's index to the migration that adds it (the review of E1, item
/// 6: a registry at schema 56 to 59 failed at 60 on the indexes of check
/// and grid_stack_id, which migration 90 adds).
fn add_indexes(store: &mut Store, table: &str) -> Result<(), Error> {
    if !table_exists(store, table)? {
        return Ok(());
    }
    let dialect = store.dialect();
    let schema = store.schema().map(str::to_string);
    let mut now = schema::table(table).clone();
    let named: Vec<&'static str> = now
        .indexes
        .iter()
        .flatten()
        .chain(now.partial_uniques.iter().flat_map(|p| p.columns.iter()))
        .copied()
        .collect();
    let mut has: std::collections::BTreeMap<&'static str, bool> = std::collections::BTreeMap::new();
    for c in named {
        if !has.contains_key(c) {
            has.insert(c, column_exists(store, table, c)?);
        }
    }
    let there = |cols: &[&str]| cols.iter().all(|c| has.get(c).copied().unwrap_or(false));
    now.indexes.retain(|key| there(key));
    now.partial_uniques.retain(|p| there(&p.columns));
    for ix in dialect.create_indexes(schema.as_deref(), &now) {
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

pub fn column_exists(store: &mut Store, table: &str, column: &str) -> Result<bool, Error> {
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

/// Whether the store has a table of that name, declared or not: the held
/// doors read a table another slice declares, when it is there.
pub fn table_exists(store: &mut Store, name: &str) -> Result<bool, Error> {
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
        mark_identifying(store)?;
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
        let future = SCHEMA_VERSION + 1;
        store
            .execute(
                &format!(
                    "UPDATE registry_meta SET value = '{future}' WHERE key = 'schema_version'"
                ),
                &[],
            )
            .unwrap();
        assert_eq!(
            standing(&mut store, Kind::Registry).unwrap(),
            Standing::Ahead(future)
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
        assert_eq!(
            names,
            vec![
                "patient-id",
                "study-instance-uid",
                "subject-code",
                "personnummer",
                "reserve-number"
            ]
        );
        assert_eq!(
            standing(&mut store, Kind::Linkage).unwrap(),
            Standing::Current
        );
    }

    /// A linkage store from before migration 86 gains the type of an ID
    /// that is the same everywhere once; one where a person had made it
    /// keeps theirs (2026-10-10: a dataset declared one and its run failed).
    /// Record 55, ruling 24: a linkage store from before migration 102
    /// learns which of its types identify a person: the built-in ones, and
    /// none a person made.
    #[test]
    fn a_linkage_store_from_before_learns_its_identifying_id_types() {
        let mut store = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 98) {
            (m.apply)(&mut store, Kind::Linkage).unwrap();
            set_version(&mut store, Kind::Linkage, m.version).unwrap();
        }
        crate::linkage::add_id_type(&mut store, "ctrl-id", None).unwrap();
        store
            .execute("UPDATE id_type SET identifying = NULL", &[])
            .unwrap();
        migrate(&mut store, Kind::Linkage).unwrap();
        for name in schema::IDENTIFYING_ID_TYPES {
            assert!(
                crate::linkage::identifying_type(&mut store, name).unwrap(),
                "{name}"
            );
        }
        assert!(!crate::linkage::identifying_type(&mut store, "subject-code").unwrap());
        assert!(!crate::linkage::identifying_type(&mut store, "ctrl-id").unwrap());
        // the registry store has nothing to do
        let mut registry = Store::sqlite_in_memory().unwrap();
        an_id_type_says_whether_it_identifies(&mut registry, Kind::Registry).unwrap();
    }

    #[test]
    fn a_linkage_store_from_before_gains_the_same_everywhere_id_type() {
        let mut store = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 85) {
            (m.apply)(&mut store, Kind::Linkage).unwrap();
            set_version(&mut store, Kind::Linkage, m.version).unwrap();
        }
        store
            .execute("DELETE FROM id_type WHERE name = 'personnummer'", &[])
            .unwrap();
        assert_eq!(
            standing(&mut store, Kind::Linkage).unwrap(),
            Standing::Behind(85)
        );
        migrate(&mut store, Kind::Linkage).unwrap();
        let count = |store: &mut Store| {
            store
                .query(
                    "SELECT COUNT(*) FROM id_type WHERE name = 'personnummer'",
                    &[],
                )
                .unwrap()[0]
                .int(0)
                .unwrap()
        };
        assert_eq!(count(&mut store), 1);
        let described = store
            .query(
                "SELECT description FROM id_type WHERE name = 'personnummer'",
                &[],
            )
            .unwrap()[0]
            .text(0)
            .unwrap()
            .to_string();
        assert_eq!(described, schema::GENERATOR_ID_DESCRIPTION);
        // once: running it again adds nothing
        the_same_everywhere_id_type_is_built_in(&mut store, Kind::Linkage).unwrap();
        assert_eq!(count(&mut store), 1);
        // the registry store has nothing to do
        let mut registry = Store::sqlite_in_memory().unwrap();
        the_same_everywhere_id_type_is_built_in(&mut registry, Kind::Registry).unwrap();
    }

    /// A linkage store seeded before record 26 gains `subject-code` from
    /// migration 39, once.
    #[test]
    fn a_linkage_store_from_before_gains_the_subject_code_type() {
        let mut store = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 38) {
            (m.apply)(&mut store, Kind::Linkage).unwrap();
        }
        store
            .execute("DELETE FROM id_type WHERE name = 'subject-code'", &[])
            .unwrap();
        a_subject_may_be_merged(&mut store, Kind::Linkage).unwrap();
        a_subject_may_be_merged(&mut store, Kind::Linkage).unwrap();
        let rows = store
            .query(
                "SELECT COUNT(*) FROM id_type WHERE name = 'subject-code'",
                &[],
            )
            .unwrap();
        assert_eq!(rows[0].int(0).unwrap(), 1);
    }
}

#[cfg(test)]
mod column_migration {
    use super::*;

    /// Record 56 (2026-10-11): the open quarantine items of a policy class an
    /// older digest filed are superseded; a reader's class stays open, and an
    /// item a person decided keeps its decision.
    #[test]
    fn an_open_item_about_files_set_aside_by_policy_is_superseded() {
        let mut store = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 88) {
            (m.apply)(&mut store, Kind::Registry).unwrap();
        }
        for (id, class, status) in [
            (1, "unsupported_sop_class", "open"),
            (2, "unsupported_modality", "open"),
            (3, "not_dicom", "open"),
            (4, "missing_modality", "open"),
            (5, "unsupported_sop_class", "accepted"),
        ] {
            store
                .execute(
                    &format!(
                        "INSERT INTO review_item (id, kind, scope, ref, evidence, status, created_at) \
                         VALUES ({id}, 'ingest.quarantine', 'batch', '{{\"batch_id\": 2, \"class\": \"{class}\"}}', \
                         '{{\"count\": 3}}', '{status}', '2026-10-10T00:00:00Z')"
                    ),
                    &[],
                )
                .unwrap();
        }
        // another kind with a class of the same name is not the digest's
        store
            .execute(
                "INSERT INTO review_item (id, kind, scope, ref, status, created_at) \
                 VALUES (6, 'identity.unmapped', 'batch', '{\"class\": \"unsupported_sop_class\"}', 'open', 't')",
                &[],
            )
            .unwrap();
        a_file_set_aside_by_policy_asks_nobody(&mut store, Kind::Registry).unwrap();
        // and again, because a migration that has run must be safe to run
        a_file_set_aside_by_policy_asks_nobody(&mut store, Kind::Registry).unwrap();
        let rows = store
            .query(
                "SELECT id, status, actor, decided_at IS NOT NULL FROM review_item ORDER BY id",
                &[],
            )
            .unwrap();
        let got: Vec<(i64, String, Option<String>, i64)> = rows
            .iter()
            .map(|r| {
                (
                    r.int(0).unwrap(),
                    r.text(1).unwrap().to_string(),
                    r.opt_text(2).unwrap().map(str::to_string),
                    r.int(3).unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (1, "superseded".to_string(), Some("nils".to_string()), 1),
                (2, "superseded".to_string(), Some("nils".to_string()), 1),
                (3, "open".to_string(), None, 0),
                (4, "open".to_string(), None, 0),
                (5, "accepted".to_string(), None, 0),
                (6, "open".to_string(), None, 0),
            ]
        );
        // a linkage store has no review items
        let mut linkage = Store::sqlite_in_memory().unwrap();
        a_file_set_aside_by_policy_asks_nobody(&mut linkage, Kind::Linkage).unwrap();
    }

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

    /// A registry from before record 26's pseudonymiser opens with every
    /// batch a digest and no subject provisional, and a row written after
    /// keeps what it said.
    #[test]
    fn every_batch_from_before_is_a_digest_and_no_subject_is_provisional() {
        let mut store = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 39) {
            (m.apply)(&mut store, Kind::Registry).unwrap();
        }
        store
            .batch("ALTER TABLE ingest_batch DROP COLUMN kind")
            .unwrap();
        store
            .batch("ALTER TABLE subject DROP COLUMN provisional")
            .unwrap();
        store
            .execute(
                "INSERT INTO ingest_batch (id, source_id, job_id, name, config, started_at, state) VALUES (1, 1, 1, 'n', '{}', 't', 'done')",
                &[],
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO subject (id, code, created_at) VALUES (1, 'abc', 't')",
                &[],
            )
            .unwrap();

        a_batch_has_a_kind_and_a_subject_may_be_provisional(&mut store, Kind::Registry).unwrap();
        assert!(column_exists(&mut store, "ingest_batch", "kind").unwrap());
        assert!(column_exists(&mut store, "subject", "provisional").unwrap());
        let kind = store
            .query_opt("SELECT kind FROM ingest_batch WHERE id = 1", &[])
            .unwrap()
            .unwrap();
        assert_eq!(kind.text(0).unwrap(), "digest");
        let provisional = store
            .query_opt("SELECT provisional FROM subject WHERE id = 1", &[])
            .unwrap()
            .unwrap();
        assert_eq!(provisional.opt_int(0).unwrap(), None);
        // and again: a batch that says what it is keeps saying it
        store
            .execute(
                "INSERT INTO ingest_batch (id, source_id, job_id, name, config, started_at, state, kind) VALUES (2, 1, 2, 'n', '{}', 't', 'done', 'pseudonymize')",
                &[],
            )
            .unwrap();
        a_batch_has_a_kind_and_a_subject_may_be_provisional(&mut store, Kind::Registry).unwrap();
        let kind = store
            .query_opt("SELECT kind FROM ingest_batch WHERE id = 2", &[])
            .unwrap()
            .unwrap();
        assert_eq!(kind.text(0).unwrap(), "pseudonymize");
    }

    /// A registry from before record 26 opens with every source place a
    /// dataset reading its folder itself, arriving as its handling said and
    /// de-identified where none was declared; a place of another role has
    /// none; and the pseudonymiser's table is there, empty.
    #[test]
    fn a_source_place_from_before_becomes_a_dataset_reading_its_folder() {
        use crate::schema::table;
        use crate::store::Insert;
        let mut store = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 38) {
            (m.apply)(&mut store, Kind::Registry).unwrap();
        }
        store
            .batch("ALTER TABLE place DROP COLUMN dataset")
            .unwrap();
        store.batch("DROP TABLE pseudonym_file").unwrap();
        assert!(!column_exists(&mut store, "place", "dataset").unwrap());
        let row = |name: &str, role: &str, handling: Option<&str>| {
            vec![
                Param::from(name),
                Param::from(role),
                Param::from(format!("/data/{name}")),
                Param::from("{}"),
                Param::from("2026-09-01T00:00:00Z"),
                handling.map_or(Param::Null, Param::from),
            ]
        };
        let spec = Insert::new(
            table("place"),
            &[
                "name",
                "role",
                "path",
                "guarantees",
                "created_at",
                "handling",
            ],
        );
        store
            .insert(
                &spec,
                &[
                    row("plain", "source", None),
                    row("named", "source", Some(r#"{"arrives": "identified", "on_release": {"dates": "keep", "uids": "remap", "deface": false}}"#)),
                    row("out", "export", None),
                ],
            )
            .unwrap();

        a_source_place_is_a_dataset(&mut store, Kind::Registry).unwrap();
        assert!(column_exists(&mut store, "place", "dataset").unwrap());
        assert!(table_exists(&mut store, "pseudonym_file").unwrap());
        let places = crate::place::list(&mut store).unwrap();
        let of = |name: &str| {
            places
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .dataset
                .clone()
        };
        assert_eq!(of("plain")["arrives"], "deidentified");
        assert_eq!(
            of("plain")["trees"],
            serde_json::json!({"originals": null, "anon": "."})
        );
        assert_eq!(of("plain")["unmapped"], "code");
        assert_eq!(of("named")["arrives"], "identified");
        assert_eq!(of("named")["unmapped"], "hold");
        assert!(of("out").is_null());
        // and again, because a migration that has run must be safe to run,
        // and a dataset once filled is not filled over
        store
            .execute(
                "UPDATE place SET dataset = '{\"arrives\": \"coded\"}' WHERE name = 'plain'",
                &[],
            )
            .unwrap();
        a_source_place_is_a_dataset(&mut store, Kind::Registry).unwrap();
        let places = crate::place::list(&mut store).unwrap();
        let plain = places.iter().find(|p| p.name == "plain").unwrap();
        assert_eq!(plain.dataset["arrives"], "coded");
    }

    /// Wave 7a §5.3: a source place without a whole, current declaration
    /// becomes undeclared: one setup made with no dataset, one the old
    /// default had read its folder itself, and a de-identified one that does
    /// not say what PatientID holds and how its subjects are found. It keeps
    /// its cohort. A whole declaration stays; a place of another role keeps
    /// none. Twice is the same as once.
    #[test]
    fn a_source_place_nobody_declared_becomes_undeclared() {
        use crate::schema::table;
        use crate::store::Insert;
        let mut store = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 79) {
            (m.apply)(&mut store, Kind::Registry).unwrap();
        }
        let folder = r#"{"arrives": "deidentified", "trees": {"originals": null, "anon": "."}, "cohort": "ms"}"#;
        let half = r#"{"arrives": "deidentified", "trees": {"originals": null, "anon": "derivatives/dcm-anon"}}"#;
        let whole = r#"{"arrives": "deidentified", "trees": {"originals": null, "anon": "derivatives/dcm-anon"}, "patient_id": "id-type:site-id", "subjects": "map"}"#;
        let identified = r#"{"arrives": "identified", "trees": {"originals": "derivatives/dcm-original", "anon": "derivatives/dcm-anon"}}"#;
        let row = |name: &str, role: &str, dataset: Option<&str>| {
            vec![
                Param::from(name),
                Param::from(role),
                Param::from(if name == "raw" {
                    "/data/study/derivatives/dcm-raw".to_string()
                } else {
                    format!("/data/{name}")
                }),
                Param::from("{}"),
                Param::from("2026-10-01T00:00:00Z"),
                dataset.map_or(Param::Null, Param::from),
            ]
        };
        let spec = Insert::new(
            table("place"),
            &[
                "name",
                "role",
                "path",
                "guarantees",
                "created_at",
                "dataset",
            ],
        );
        store
            .insert(
                &spec,
                &[
                    row("setup", "source", None),
                    row("folder", "source", Some(folder)),
                    row("half", "source", Some(half)),
                    row("whole", "source", Some(whole)),
                    row("identified", "source", Some(identified)),
                    row(
                        "raw",
                        "source",
                        Some(r#"{"arrives": "deidentified", "cohort": "ms"}"#),
                    ),
                    row("out", "export", None),
                ],
            )
            .unwrap();
        for _ in 0..2 {
            a_source_place_nobody_declared_is_undeclared(&mut store, Kind::Registry).unwrap();
            let places = crate::place::list(&mut store).unwrap();
            let of = |name: &str| {
                places
                    .iter()
                    .find(|p| p.name == name)
                    .unwrap()
                    .dataset
                    .clone()
            };
            assert_eq!(of("setup")["arrives"], "undeclared");
            assert_eq!(
                of("setup")["trees"],
                serde_json::json!({"originals": null, "anon": null})
            );
            assert_eq!(of("folder")["arrives"], "undeclared");
            assert_eq!(of("folder")["trees"]["anon"], serde_json::Value::Null);
            assert_eq!(of("folder")["cohort"], "ms");
            assert_eq!(of("half")["arrives"], "undeclared");
            assert_eq!(of("whole")["arrives"], "deidentified");
            assert_eq!(of("whole")["subjects"], "map");
            assert_eq!(of("identified")["arrives"], "identified");
            // a place on a dataset's pseudonymised tree reads it, as legacy
            assert_eq!(of("raw")["kind"], "legacy");
            assert_eq!(of("raw")["state"], "anonymised");
            assert_eq!(of("raw")["trees"]["anon"], ".");
            assert_eq!(of("raw")["cohort"], "ms");
            assert!(of("out").is_null());
        }
    }

    /// Wave 7a: a dataset a development build made of a root's folder and
    /// that nothing read is removed; one a digest read, or the
    /// pseudonymiser wrote, stays; a dataset of no root and the root stay.
    #[test]
    fn a_root_s_unread_folders_are_plain_folders_again() {
        use crate::schema::table;
        use crate::store::Insert;
        let mut store = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 83) {
            (m.apply)(&mut store, Kind::Registry).unwrap();
        }
        let under = r#"{"kind": "dataset", "root": "src", "arrives": "deidentified", "state": "anonymised", "trees": {"originals": null, "anon": "derivatives/dcm-anon"}}"#;
        let alone = r#"{"kind": "dataset", "arrives": "deidentified", "state": "anonymised", "trees": {"originals": null, "anon": "derivatives/dcm-anon"}}"#;
        let root = r#"{"kind": "root", "arrives": "undeclared"}"#;
        let spec = Insert::new(
            table("place"),
            &[
                "name",
                "role",
                "path",
                "guarantees",
                "created_at",
                "dataset",
            ],
        );
        let row = |name: &str, path: &str, dataset: &str| {
            vec![
                Param::from(name),
                Param::from("source"),
                Param::from(path),
                Param::from("{}"),
                Param::from("2026-10-08T00:00:00Z"),
                Param::from(dataset),
            ]
        };
        store
            .insert(
                &spec,
                &[
                    row("src", "/data/src", root),
                    row("unread", "/data/src/unread", under),
                    row("digested", "/data/src/digested", under),
                    row("written", "/data/src/written", under),
                    row("alone", "/data/alone", alone),
                ],
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO source (root, root_canonical, first_seen_at) VALUES ('x', '/data/src/digested/derivatives/dcm-anon', '2026-10-08T00:00:00Z')",
                &[],
            )
            .unwrap();
        let written = store
            .query("SELECT id FROM place WHERE name = 'written'", &[])
            .unwrap()[0]
            .int(0)
            .unwrap();
        store
            .execute(
                &format!("INSERT INTO pseudonym_file (place_id, path, size, mtime, state, first_seen, code_anyway) VALUES ({written}, 'a', 1, 1, 'written', '2026-10-08T00:00:00Z', 0)"),
                &[],
            )
            .unwrap();
        for _ in 0..2 {
            a_root_s_folder_is_a_dataset_only_when_added(&mut store, Kind::Registry).unwrap();
        }
        let mut names: Vec<String> = crate::place::list(&mut store)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        names.sort();
        assert_eq!(names, ["alone", "digested", "src", "written"]);
    }

    /// #457: a dataset whose tree, or whose folder, is a symbolic link was
    /// read when a digest's resolved root lies behind that link; schema 84
    /// keeps it, as it keeps one whose link cannot be resolved now. A real
    /// folder nothing read still goes, links or not.
    #[cfg(unix)]
    #[test]
    fn a_dataset_read_through_a_symbolic_link_stays() {
        use crate::schema::table;
        use crate::store::Insert;
        use std::os::unix::fs::symlink;
        let dir = nils_dicom::synth::TempDir::new("migrate-84-links");
        let base = dir.path();
        let src = base.join("src");
        let elsewhere = base.join("elsewhere");
        for d in [
            "src",
            "elsewhere/anon",
            "elsewhere/original",
            "elsewhere/folder/derivatives/dcm-anon",
            "elsewhere/repointed",
            "elsewhere/unread-anon",
            "src/linked-tree/derivatives",
            "src/linked-original/derivatives",
            "src/dangling/derivatives",
            "src/repointed/derivatives",
            "src/unread/derivatives",
            "src/near",
            "src/near-other/derivatives/dcm-anon",
        ] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        symlink(
            elsewhere.join("anon"),
            src.join("linked-tree/derivatives/dcm-anon"),
        )
        .unwrap();
        symlink(
            elsewhere.join("original"),
            src.join("linked-original/derivatives/dcm-original"),
        )
        .unwrap();
        symlink(elsewhere.join("folder"), src.join("linked-folder")).unwrap();
        symlink(base.join("gone"), src.join("dangling/derivatives/dcm-anon")).unwrap();
        symlink(
            elsewhere.join("repointed"),
            src.join("repointed/derivatives/dcm-anon"),
        )
        .unwrap();
        symlink(
            elsewhere.join("unread-anon"),
            src.join("unread/derivatives/dcm-anon"),
        )
        .unwrap();
        let resolved = |p: std::path::PathBuf| {
            std::fs::canonicalize(p)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };

        let mut store = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 83) {
            (m.apply)(&mut store, Kind::Registry).unwrap();
        }
        let under = r#"{"kind": "dataset", "root": "src", "arrives": "deidentified", "state": "anonymised", "trees": {"originals": null, "anon": "derivatives/dcm-anon"}}"#;
        let root = r#"{"kind": "root", "arrives": "undeclared"}"#;
        let spec = Insert::new(
            table("place"),
            &[
                "name",
                "role",
                "path",
                "guarantees",
                "created_at",
                "dataset",
            ],
        );
        let at = |name: &str| src.join(name).to_string_lossy().into_owned();
        let row = |name: &str, path: String, dataset: &str| {
            vec![
                Param::from(name),
                Param::from("source"),
                Param::from(path.as_str()),
                Param::from("{}"),
                Param::from("2026-10-08T00:00:00Z"),
                Param::from(dataset),
            ]
        };
        let names = [
            "linked-tree",
            "linked-original",
            "linked-folder",
            "dangling",
            "repointed",
            "unread",
            "near",
        ];
        let mut rows = vec![row("src", src.to_string_lossy().into_owned(), root)];
        rows.extend(names.iter().map(|n| row(n, at(n), under)));
        store.insert(&spec, &rows).unwrap();
        // What the digests kept: the resolved roots (each given as the link,
        // or as its target), one root since repointed (its old target gone),
        // and a root of a sibling folder whose name starts like `near`.
        let read = [
            (
                at("linked-tree/derivatives/dcm-anon"),
                resolved(elsewhere.join("anon")),
            ),
            (
                resolved(elsewhere.join("original")),
                resolved(elsewhere.join("original")),
            ),
            (
                at("linked-folder/derivatives/dcm-anon"),
                resolved(elsewhere.join("folder/derivatives/dcm-anon")),
            ),
            (
                at("repointed/derivatives/dcm-anon"),
                base.join("old-target").to_string_lossy().into_owned(),
            ),
            (
                at("near-other/derivatives/dcm-anon"),
                resolved(src.join("near-other/derivatives/dcm-anon")),
            ),
        ];
        for (given, canonical) in &read {
            store
                .execute(
                    "INSERT INTO source (root, root_canonical, first_seen_at) VALUES (?1, ?2, '2026-10-08T00:00:00Z')",
                    &[Param::from(given.as_str()), Param::from(canonical.as_str())],
                )
                .unwrap();
        }
        for _ in 0..2 {
            a_root_s_folder_is_a_dataset_only_when_added(&mut store, Kind::Registry).unwrap();
        }
        let mut kept: Vec<String> = crate::place::list(&mut store)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        kept.sort();
        assert_eq!(
            kept,
            [
                "dangling",
                "linked-folder",
                "linked-original",
                "linked-tree",
                "repointed",
                "src"
            ]
        );
    }

    /// Wave 7a: what a store from before sealed of a personnummer is
    /// dropped, in the linkage store and in a held file's row; every other
    /// identifier stays sealed.
    #[test]
    fn a_personnummer_kept_before_is_dropped() {
        let keys = crate::linkage::Subkeys::derive(b"a test key");
        let mut linkage = Store::sqlite_in_memory().unwrap();
        migrate(&mut linkage, Kind::Linkage).unwrap();
        let pnr = crate::linkage::ensure_id_type(&mut linkage, "personnummer", None)
            .unwrap()
            .0
            .id;
        let other = crate::linkage::id_type_id(&mut linkage, "patient-id")
            .unwrap()
            .unwrap();
        // what a store from before sealed, filed past the rule the store
        // applies now (`linkage::keeps`)
        sealed_before(
            &mut linkage,
            &[(pnr, keys.seal("198501012382")), (other, keys.seal("P1"))],
        );
        for _ in 0..2 {
            a_personnummer_is_never_kept(&mut linkage, Kind::Linkage).unwrap();
        }
        let shown = crate::linkage::reveal(&mut linkage, &keys, 1, "tester", None).unwrap();
        let kept: Vec<(String, bool, String)> = shown
            .into_iter()
            .map(|r| (r.id_type, r.kept, r.value))
            .collect();
        assert_eq!(
            kept,
            [
                ("personnummer".to_string(), false, String::new()),
                ("patient-id".to_string(), true, "P1".to_string()),
            ]
        );
        let mut registry = Store::sqlite_in_memory().unwrap();
        migrate(&mut registry, Kind::Registry).unwrap();
        registry
            .execute(
                "INSERT INTO pseudonym_file (place_id, path, size, mtime, state, sealed, id_type, first_seen, code_anyway) \
                 VALUES (1, 'a', 1, 1, 'held', x'0102', 'personnummer', '2026-10-08T00:00:00Z', 0), \
                        (1, 'b', 1, 1, 'held', x'0102', 'study-id', '2026-10-08T00:00:00Z', 0)",
                &[],
            )
            .unwrap();
        a_personnummer_is_never_kept(&mut registry, Kind::Registry).unwrap();
        let rows = registry
            .query(
                "SELECT path FROM pseudonym_file WHERE sealed IS NOT NULL",
                &[],
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text(0).unwrap(), "b");
    }

    /// Identity rows of subject 1 with their ciphertext as given, filed
    /// past `linkage::insert_identities`, as a store from before kept them.
    fn sealed_before(linkage: &mut Store, rows: &[(i64, Vec<u8>)]) {
        let values: Vec<Vec<Param>> = rows
            .iter()
            .enumerate()
            .map(|(i, (id_type_id, ciphertext))| {
                vec![
                    Param::Int(1),
                    Param::Int(*id_type_id),
                    Param::Bytes(vec![i as u8 + 1]),
                    Param::Bytes(ciphertext.clone()),
                    Param::from("dicom"),
                    Param::Null,
                    Param::from("2026-10-08T00:00:00Z"),
                ]
            })
            .collect();
        linkage
            .insert(
                &crate::store::Insert::all(schema::table("identity")),
                &values,
            )
            .unwrap();
    }

    /// Nima, 2026-10-16: a linkage store from before migration 104 gains the
    /// reserve number's type once, identifying; one where a person had made a
    /// type of that name keeps it, and what it sealed under it is dropped, in
    /// the linkage store and in a held file's row, every other identifier
    /// staying sealed.
    #[test]
    fn migration_104_builds_the_reserve_number_in_and_keeps_none() {
        let count = |store: &mut Store| {
            store
                .query(
                    "SELECT COUNT(*) FROM id_type WHERE name = 'reserve-number'",
                    &[],
                )
                .unwrap()[0]
                .int(0)
                .unwrap()
        };
        let before = |store: &mut Store| {
            for m in MIGRATIONS.iter().take_while(|m| m.version <= 103) {
                (m.apply)(store, Kind::Linkage).unwrap();
                set_version(store, Kind::Linkage, m.version).unwrap();
            }
            store
                .execute("DELETE FROM id_type WHERE name = 'reserve-number'", &[])
                .unwrap();
        };
        // a store that never had it
        let mut store = Store::sqlite_in_memory().unwrap();
        before(&mut store);
        assert_eq!(count(&mut store), 0);
        assert_eq!(
            migrate(&mut store, Kind::Linkage).unwrap(),
            MIGRATIONS
                .iter()
                .filter(|m| m.version >= 104)
                .map(|m| m.version)
                .collect::<Vec<_>>()
        );
        assert_eq!(count(&mut store), 1);
        let t = crate::linkage::id_type_named(&mut store, "reserve-number")
            .unwrap()
            .unwrap();
        assert!(t.identifying);
        assert_eq!(
            t.description.as_deref(),
            Some(schema::RESERVE_ID_DESCRIPTION)
        );
        // once: running it again adds nothing
        a_reserve_number_is_built_in_and_never_kept(&mut store, Kind::Linkage).unwrap();
        assert_eq!(count(&mut store), 1);

        // a store where a person made a type of that name and sealed values
        let keys = crate::linkage::Subkeys::derive(b"a test key");
        let mut store = Store::sqlite_in_memory().unwrap();
        before(&mut store);
        let made = crate::linkage::add_id_type(&mut store, "reserve-number", Some("ours"))
            .unwrap()
            .id;
        let other = crate::linkage::id_type_id(&mut store, "patient-id")
            .unwrap()
            .unwrap();
        sealed_before(
            &mut store,
            &[(made, keys.seal("199913241234")), (other, keys.seal("P1"))],
        );
        migrate(&mut store, Kind::Linkage).unwrap();
        assert_eq!(count(&mut store), 1);
        let t = crate::linkage::id_type_named(&mut store, "reserve-number")
            .unwrap()
            .unwrap();
        assert_eq!((t.id, t.identifying), (made, true));
        let shown = crate::linkage::reveal(&mut store, &keys, 1, "tester", None).unwrap();
        let kept: Vec<(String, bool, String)> = shown
            .into_iter()
            .map(|r| (r.id_type, r.kept, r.value))
            .collect();
        assert_eq!(
            kept,
            [
                ("reserve-number".to_string(), false, String::new()),
                ("patient-id".to_string(), true, "P1".to_string()),
            ]
        );

        // a held file's row keeps its shape and lookup alone
        let mut registry = Store::sqlite_in_memory().unwrap();
        migrate(&mut registry, Kind::Registry).unwrap();
        registry
            .execute(
                "INSERT INTO pseudonym_file (place_id, path, size, mtime, state, sealed, id_type, first_seen, code_anyway) \
                 VALUES (1, 'a', 1, 1, 'held', x'0102', 'reserve-number', '2026-10-16T00:00:00Z', 0), \
                        (1, 'b', 1, 1, 'held', x'0102', 'patient-id', '2026-10-16T00:00:00Z', 0)",
                &[],
            )
            .unwrap();
        a_reserve_number_is_built_in_and_never_kept(&mut registry, Kind::Registry).unwrap();
        let rows = registry
            .query(
                "SELECT path FROM pseudonym_file WHERE sealed IS NOT NULL",
                &[],
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text(0).unwrap(), "b");
    }

    /// Record 38 S3: a registry and a linkage store from before open with
    /// the shift gone. A place's handling loses `on_release.dates` and keeps
    /// the rest; one that never said anything is left alone; the release
    /// plan loses its offset; the linkage store loses the offsets. Twice is
    /// the same as once.
    #[test]
    fn a_store_from_before_loses_the_shift_and_keeps_the_rest() {
        use crate::schema::table;
        use crate::store::Insert;
        let mut store = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 49) {
            (m.apply)(&mut store, Kind::Registry).unwrap();
        }
        store
            .batch("ALTER TABLE release_plan ADD COLUMN offset_days INTEGER NOT NULL DEFAULT 0")
            .unwrap();
        let row = |name: &str, handling: Option<&str>| {
            vec![
                Param::from(name),
                Param::from("source"),
                Param::from(format!("/data/{name}")),
                Param::from("{}"),
                Param::from("2026-09-01T00:00:00Z"),
                handling.map_or(Param::Null, Param::from),
            ]
        };
        let spec = Insert::new(
            table("place"),
            &[
                "name",
                "role",
                "path",
                "guarantees",
                "created_at",
                "handling",
            ],
        );
        store
            .insert(
                &spec,
                &[
                    row("shifted", Some(r#"{"arrives": "identified", "on_release": {"dates": "shift", "uids": "remap", "deface": true}}"#)),
                    row("kept", Some(r#"{"arrives": "coded", "on_release": {"dates": "keep", "uids": "preserve", "deface": false}}"#)),
                    row("plain", None),
                ],
            )
            .unwrap();

        let mut linkage = Store::sqlite_in_memory().unwrap();
        for m in MIGRATIONS.iter().take_while(|m| m.version <= 49) {
            (m.apply)(&mut linkage, Kind::Linkage).unwrap();
        }
        linkage
            .batch("CREATE TABLE date_shift (subject_id INTEGER PRIMARY KEY, offset_days INTEGER NOT NULL)")
            .unwrap();

        for _ in 0..2 {
            the_date_is_the_date(&mut store, Kind::Registry).unwrap();
            the_date_is_the_date(&mut linkage, Kind::Linkage).unwrap();
        }
        assert!(!column_exists(&mut store, "release_plan", "offset_days").unwrap());
        assert!(!table_exists(&mut linkage, "date_shift").unwrap());
        let places = crate::place::list(&mut store).unwrap();
        let of = |name: &str| {
            places
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .handling
                .clone()
        };
        assert_eq!(
            of("shifted"),
            serde_json::json!({"arrives": "identified", "on_release": {"uids": "remap", "deface": true}})
        );
        assert_eq!(
            of("kept"),
            serde_json::json!({"arrives": "coded", "on_release": {"uids": "preserve", "deface": false}})
        );
        assert!(of("plain").is_null() || of("plain")["on_release"].get("dates").is_none());
    }
}
