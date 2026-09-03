#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Export the tables the compare tool reads from a v0 (0.x) metadata database,
# read-only, one zstd-compressed CSV per table.
#
#   export.sh OUT_DIR [CONTAINER] [DB] [USER]
#
# Runs psql inside the database's container (v0 publishes no port), with
# default_transaction_read_only forced on for the session, so nothing here can
# write. The subject table is exported without names: v1 does not store them,
# so the comparison needs only whether v0 held one. Everything else is copied
# as stored, identifiers included, which makes OUT_DIR as sensitive as the
# database itself: keep it on the private host, delete it after the run.
set -eu

out=${1:?usage: export.sh OUT_DIR [CONTAINER] [DB] [USER]}
container=${2:-neuroimaging_sorting_toolkit-metadata-db-1}
db=${3:-neurotoolkit_metadata}
user=${4:-postgres}

mkdir -p "$out"

# One COPY per table, streamed through zstd. `docker exec -i` so that psql reads
# nothing from a terminal; PGOPTIONS makes every transaction of the session
# read-only before the first statement runs.
copy() {
    name=$1
    query=$2
    printf '%s ' "$name" >&2
    docker exec -i -e PGOPTIONS='-c default_transaction_read_only=on' "$container" \
        psql -U "$user" -d "$db" -q -X -v ON_ERROR_STOP=1 \
        -c "COPY ($query) TO STDOUT WITH (FORMAT csv, HEADER true)" \
        | zstd -q -T0 -o "$out/$name.csv.zst" -f
    printf '%s rows\n' "$(zstd -dc "$out/$name.csv.zst" | wc -l | awk '{print $1 - 1}')" >&2
}

copy schema_version "SELECT id, version, applied_at FROM schema_version"
copy subject "SELECT subject_id, subject_code, patient_birth_date, patient_sex,
    (patient_name IS NOT NULL AND patient_name <> '') AS has_patient_name, is_active
    FROM subject"
copy cohort "SELECT cohort_id, name, path, is_active FROM cohort"
copy subject_cohorts "SELECT subject_id, cohort_id FROM subject_cohorts"
copy id_types "SELECT id_type_id, id_type_name, description FROM id_types"
copy subject_other_identifiers "SELECT subject_other_identifier_id, subject_id, id_type_id, other_identifier
    FROM subject_other_identifiers"
copy observation_types "SELECT observation_type_id, name FROM observation_types"
copy event "SELECT event_id, subject_id, observation_type_id, event_date, event_time FROM event"
copy study "SELECT study_id, study_instance_uid, study_date, study_time, study_description,
    study_comments, modality, manufacturer, manufacturer_model_name, station_name,
    institution_name, subject_id, event_id FROM study"
copy series "SELECT series_id, series_instance_uid, frame_of_reference_uid, implementation_class_uid,
    media_storage_sop_instance_uid, sop_class_uid, implementation_version_name, series_date,
    series_time, modality, image_type, sequence_name, protocol_name, series_description,
    body_part_examined, scanning_sequence, sequence_variant, scan_options, series_comments,
    slice_thickness, spacing_between_slices, images_in_acquisition, image_orientation_patient,
    image_position_patient, patient_position, contrast_bolus_agent, contrast_bolus_route,
    contrast_bolus_total_dose, contrast_bolus_start_time, contrast_bolus_volume,
    contrast_flow_rate, contrast_flow_duration, study_id, subject_id FROM series"
copy mri_series_details "SELECT * FROM mri_series_details"
copy ct_series_details "SELECT * FROM ct_series_details"
copy pet_series_details "SELECT * FROM pet_series_details"
copy series_stack "SELECT * FROM series_stack"
copy instance "SELECT instance_id, series_id, series_instance_uid, sop_instance_uid, instance_number,
    acquisition_number, acquisition_date, acquisition_time, content_date, content_time,
    slice_location, pixel_spacing, rows, columns, bits_allocated, bits_stored, high_bit,
    pixel_representation, window_center, window_width, rescale_intercept, rescale_slope,
    number_of_frames, lossy_image_compression, derivation_description, image_comments,
    transfer_syntax_uid, dicom_file_path, series_stack_id FROM instance"

printf 'exported to %s\n' "$out" >&2
