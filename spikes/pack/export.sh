#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Export the two tables the pack spike reads from a v0 (0.x) metadata database,
# read-only, one zstd-compressed CSV per table, on stdout of the remote host.
#
#   export.sh OUT_DIR [CONTAINER] [DB] [USER]
#
# The same shape as tools/v0-compare/export.sh: psql inside the database's
# container with default_transaction_read_only forced on for the session, so
# nothing here can write. `stack_fingerprint` is what v0's classifier reads and
# `series_classification_cache` is what it wrote; together they are the spike's
# input and its ground truth. Both carry the site's own protocol text, so
# OUT_DIR is as sensitive as the database: keep it on a private host, delete it
# after the run.
set -eu

out=${1:?usage: export.sh OUT_DIR [CONTAINER] [DB] [USER]}
container=${2:-neuroimaging_sorting_toolkit-metadata-db-1}
db=${3:-neurotoolkit_metadata}
user=${4:-postgres}

mkdir -p "$out"

copy() {
    name=$1
    query=$2
    printf '%s ' "$name" >&2
    docker exec -i -e PGOPTIONS='-c default_transaction_read_only=on' "$container" \
        psql -U "$user" -d "$db" -q -X -v ON_ERROR_STOP=1 \
        -c "COPY ($query) TO STDOUT WITH (FORMAT csv, HEADER true)" \
        | zstd -q -T0 -o "$out/$name.csv.zst" -f
    printf '%s lines\n' "$(zstd -dc "$out/$name.csv.zst" | wc -l | awk '{print $1 - 1}')" >&2
}

# Every column v0's ClassificationContext.from_fingerprint reads, plus the
# modality that routes a stack to a pack and the instance count the physics
# vote bins on.
copy stack_fingerprint "SELECT series_stack_id, modality, manufacturer, manufacturer_model,
    stack_sequence_name, text_search_blob, contrast_search_blob, stack_orientation,
    fov_x, fov_y, aspect_ratio, image_type, scanning_sequence, sequence_variant, scan_options,
    mr_te, mr_tr, mr_ti, mr_flip_angle, mr_echo_train_length, mr_echo_number,
    mr_acquisition_type, mr_diffusion_b_value, stack_n_instances
    FROM stack_fingerprint ORDER BY series_stack_id"

# v0's verdict. The reference pool of the physics vote is built from base,
# technique and directory_type; the rest is what the wave's gate will diff.
copy series_classification_cache "SELECT series_stack_id, directory_type, base, technique,
    modifier_csv, construct_csv, provenance, acceleration_csv, body_part,
    post_contrast, localizer, spinal_cord, manual_review_required, manual_review_reasons_csv,
    dicom_origin_cohort
    FROM series_classification_cache ORDER BY series_stack_id"

printf 'exported to %s\n' "$out" >&2
