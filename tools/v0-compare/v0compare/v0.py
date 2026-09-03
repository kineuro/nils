# SPDX-License-Identifier: AGPL-3.0-only
"""The v0 side: its tables loaded into a DuckDB file, either from the
read-only export `export.sh` writes (one CSV per table, zstd or plain) or
straight from the v0 database through DuckDB's Postgres scanner. The column
lists here mirror the export's queries; the subject table never carries a
name, only whether v0 held one."""

from __future__ import annotations

import sys
from pathlib import Path

import duckdb

from .v1 import quote

# Every v0 table the tool reads, with the columns it keeps and their DuckDB
# types. Dates and times stay text: v1 stores them as text and the tool
# compares their normal forms (§6.3).
TABLES: dict[str, dict[str, str]] = {
    "schema_version": {"id": "BIGINT", "version": "VARCHAR", "applied_at": "VARCHAR"},
    "subject": {
        "subject_id": "BIGINT",
        "subject_code": "VARCHAR",
        "patient_birth_date": "VARCHAR",
        "patient_sex": "VARCHAR",
        "has_patient_name": "BOOLEAN",
        "is_active": "BOOLEAN",
    },
    "cohort": {"cohort_id": "BIGINT", "name": "VARCHAR", "path": "VARCHAR", "is_active": "BOOLEAN"},
    "subject_cohorts": {"subject_id": "BIGINT", "cohort_id": "BIGINT"},
    "id_types": {"id_type_id": "BIGINT", "id_type_name": "VARCHAR", "description": "VARCHAR"},
    "subject_other_identifiers": {
        "subject_other_identifier_id": "BIGINT",
        "subject_id": "BIGINT",
        "id_type_id": "BIGINT",
        "other_identifier": "VARCHAR",
    },
    "observation_types": {"observation_type_id": "BIGINT", "name": "VARCHAR"},
    "event": {
        "event_id": "BIGINT",
        "subject_id": "BIGINT",
        "observation_type_id": "BIGINT",
        "event_date": "VARCHAR",
        "event_time": "VARCHAR",
    },
    "study": {
        "study_id": "BIGINT",
        "study_instance_uid": "VARCHAR",
        "study_date": "VARCHAR",
        "study_time": "VARCHAR",
        "study_description": "VARCHAR",
        "study_comments": "VARCHAR",
        "modality": "VARCHAR",
        "manufacturer": "VARCHAR",
        "manufacturer_model_name": "VARCHAR",
        "station_name": "VARCHAR",
        "institution_name": "VARCHAR",
        "subject_id": "BIGINT",
        "event_id": "BIGINT",
    },
    "series": {
        "series_id": "BIGINT",
        "series_instance_uid": "VARCHAR",
        "frame_of_reference_uid": "VARCHAR",
        "implementation_class_uid": "VARCHAR",
        "media_storage_sop_instance_uid": "VARCHAR",
        "sop_class_uid": "VARCHAR",
        "implementation_version_name": "VARCHAR",
        "series_date": "VARCHAR",
        "series_time": "VARCHAR",
        "modality": "VARCHAR",
        "image_type": "VARCHAR",
        "sequence_name": "VARCHAR",
        "protocol_name": "VARCHAR",
        "series_description": "VARCHAR",
        "body_part_examined": "VARCHAR",
        "scanning_sequence": "VARCHAR",
        "sequence_variant": "VARCHAR",
        "scan_options": "VARCHAR",
        "series_comments": "VARCHAR",
        "slice_thickness": "DOUBLE",
        "spacing_between_slices": "DOUBLE",
        "images_in_acquisition": "VARCHAR",
        "image_orientation_patient": "VARCHAR",
        "image_position_patient": "VARCHAR",
        "patient_position": "VARCHAR",
        "contrast_bolus_agent": "VARCHAR",
        "contrast_bolus_route": "VARCHAR",
        "contrast_bolus_total_dose": "DOUBLE",
        "contrast_bolus_start_time": "VARCHAR",
        "contrast_bolus_volume": "DOUBLE",
        "contrast_flow_rate": "DOUBLE",
        "contrast_flow_duration": "DOUBLE",
        "study_id": "BIGINT",
        "subject_id": "BIGINT",
    },
    "mri_series_details": {
        "series_id": "BIGINT",
        "series_instance_uid": "VARCHAR",
        "mr_acquisition_type": "VARCHAR",
        "angio_flag": "VARCHAR",
        "repetition_time": "DOUBLE",
        "echo_time": "DOUBLE",
        "inversion_time": "DOUBLE",
        "inversion_times": "VARCHAR",
        "flip_angle": "DOUBLE",
        "phase_contrast": "VARCHAR",
        "number_of_averages": "DOUBLE",
        "imaging_frequency": "DOUBLE",
        "imaged_nucleus": "VARCHAR",
        "echo_numbers": "VARCHAR",
        "magnetic_field_strength": "DOUBLE",
        "number_of_phase_encoding_steps": "VARCHAR",
        "echo_train_length": "BIGINT",
        "percent_sampling": "DOUBLE",
        "percent_phase_field_of_view": "DOUBLE",
        "pixel_bandwidth": "VARCHAR",
        "receive_coil_name": "VARCHAR",
        "transmit_coil_name": "VARCHAR",
        "acquisition_matrix": "VARCHAR",
        "phase_encoding_direction": "VARCHAR",
        "sar": "DOUBLE",
        "dbdt": "VARCHAR",
        "b1rms": "VARCHAR",
        "temporal_position_identifier": "VARCHAR",
        "number_of_temporal_positions": "VARCHAR",
        "temporal_resolution": "VARCHAR",
        "diffusion_b_value": "VARCHAR",
        "diffusion_gradient_orientation": "VARCHAR",
        "diffusion_directionality": "VARCHAR",
        "parallel_acquisition_technique": "VARCHAR",
        "parallel_reduction_factor_in_plane": "VARCHAR",
        "dwi_siemens_b_value": "BIGINT",
        "dwi_siemens_directionality": "VARCHAR",
        "dwi_siemens_pe_dir_positive": "BIGINT",
        "dwi_ge_b_value": "BIGINT",
        "dwi_ge_n_directions": "BIGINT",
        "dwi_philips_b_value": "DOUBLE",
    },
    "ct_series_details": {
        "series_id": "BIGINT",
        "series_instance_uid": "VARCHAR",
        "kvp": "DOUBLE",
        "data_collection_diameter": "DOUBLE",
        "reconstruction_diameter": "DOUBLE",
        "gantry_detector_tilt": "DOUBLE",
        "table_height": "DOUBLE",
        "rotation_direction": "VARCHAR",
        "exposure_time": "DOUBLE",
        "x_ray_tube_current": "DOUBLE",
        "exposure": "DOUBLE",
        "filter_type": "VARCHAR",
        "generator_power": "DOUBLE",
        "focal_spots": "VARCHAR",
        "convolution_kernel": "VARCHAR",
        "revolution_time": "DOUBLE",
        "single_collimation_width": "DOUBLE",
        "total_collimation_width": "DOUBLE",
        "table_speed": "DOUBLE",
        "table_feed_per_rotation": "DOUBLE",
        "spiral_pitch_factor": "DOUBLE",
        "exposure_modulation_type": "VARCHAR",
        "ctdi_vol": "DOUBLE",
        "ctdi_phantom_type_code_sequence": "VARCHAR",
        "calcium_scoring_mass_factor_device": "DOUBLE",
        "calcium_scoring_mass_factor_patient": "DOUBLE",
    },
    "pet_series_details": {
        "series_id": "BIGINT",
        "series_instance_uid": "VARCHAR",
        "radiopharmaceutical": "VARCHAR",
        "radionuclide_total_dose": "DOUBLE",
        "radionuclide_half_life": "DOUBLE",
        "radionuclide_positron_fraction": "DOUBLE",
        "radiopharmaceutical_start_time": "VARCHAR",
        "radiopharmaceutical_stop_time": "VARCHAR",
        "radiopharmaceutical_volume": "DOUBLE",
        "radiopharmaceutical_route": "VARCHAR",
        "decay_correction": "VARCHAR",
        "decay_factor": "DOUBLE",
        "reconstruction_method": "VARCHAR",
        "scatter_correction_method": "VARCHAR",
        "attenuation_correction_method": "VARCHAR",
        "randoms_correction_method": "VARCHAR",
        "dose_calibration_factor": "DOUBLE",
        "activity_concentration_scale": "DOUBLE",
        "suv_type": "VARCHAR",
        "suvbw": "DOUBLE",
        "suvlbm": "DOUBLE",
        "suvbsa": "DOUBLE",
        "counts_source": "VARCHAR",
        "units": "VARCHAR",
        "frame_reference_time": "DOUBLE",
        "actual_frame_duration": "DOUBLE",
        "patient_gantry_relationship_code": "VARCHAR",
        "slice_progression_direction": "VARCHAR",
        "series_type": "VARCHAR",
        "units_type": "VARCHAR",
        "counts_included": "VARCHAR",
    },
    "series_stack": {
        "series_stack_id": "BIGINT",
        "series_id": "BIGINT",
        "stack_modality": "VARCHAR",
        "stack_index": "BIGINT",
        "stack_key": "VARCHAR",
        "stack_inversion_time": "DOUBLE",
        "stack_echo_time": "DOUBLE",
        "stack_echo_numbers": "VARCHAR",
        "stack_echo_train_length": "BIGINT",
        "stack_repetition_time": "DOUBLE",
        "stack_flip_angle": "DOUBLE",
        "stack_receive_coil_name": "VARCHAR",
        "stack_image_orientation": "VARCHAR",
        "stack_orientation_confidence": "DOUBLE",
        "stack_image_type": "VARCHAR",
        "stack_xray_exposure": "DOUBLE",
        "stack_kvp": "DOUBLE",
        "stack_tube_current": "DOUBLE",
        "stack_pet_bed_index": "BIGINT",
        "stack_pet_frame_type": "VARCHAR",
        "stack_n_instances": "BIGINT",
    },
    "instance": {
        "instance_id": "BIGINT",
        "series_id": "BIGINT",
        "series_instance_uid": "VARCHAR",
        "sop_instance_uid": "VARCHAR",
        "instance_number": "BIGINT",
        "acquisition_number": "BIGINT",
        "acquisition_date": "VARCHAR",
        "acquisition_time": "VARCHAR",
        "content_date": "VARCHAR",
        "content_time": "VARCHAR",
        "slice_location": "DOUBLE",
        "pixel_spacing": "VARCHAR",
        "rows": "BIGINT",
        "columns": "BIGINT",
        "bits_allocated": "BIGINT",
        "bits_stored": "BIGINT",
        "high_bit": "BIGINT",
        "pixel_representation": "BIGINT",
        "window_center": "VARCHAR",
        "window_width": "VARCHAR",
        "rescale_intercept": "DOUBLE",
        "rescale_slope": "DOUBLE",
        "number_of_frames": "BIGINT",
        "lossy_image_compression": "VARCHAR",
        "derivation_description": "VARCHAR",
        "image_comments": "VARCHAR",
        "transfer_syntax_uid": "VARCHAR",
        "dicom_file_path": "VARCHAR",
        "series_stack_id": "BIGINT",
    },
}

# Columns the export projects instead of copying; the same projection is
# applied when reading the database directly.
_PROJECTED: dict[str, dict[str, str]] = {
    "subject": {"has_patient_name": "(patient_name IS NOT NULL AND patient_name <> '')"},
}

# The primary key of the v0 tables that have one, for the index.
_KEYS: dict[str, str] = {
    "subject": "subject_id",
    "cohort": "cohort_id",
    "id_types": "id_type_id",
    "subject_other_identifiers": "subject_other_identifier_id",
    "observation_types": "observation_type_id",
    "event": "event_id",
    "study": "study_id",
    "series": "series_id",
    "mri_series_details": "series_id",
    "ct_series_details": "series_id",
    "pet_series_details": "series_id",
    "series_stack": "series_stack_id",
    "instance": "instance_id",
}

_UNIQUE: dict[str, str] = {
    "subject": "subject_code",
    "study": "study_instance_uid",
    "series": "series_instance_uid",
    "instance": "sop_instance_uid",
}


def _log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _typed_select(table: str, source: str) -> str:
    cols = ", ".join(f"CAST({name} AS {ty}) AS {name}" for name, ty in TABLES[table].items())
    return f"SELECT {cols} FROM {source}"


def _index(con: duckdb.DuckDBPyConnection, table: str) -> None:
    if table in _KEYS:
        con.execute(f"CREATE INDEX IF NOT EXISTS {table}_pk ON out.v0.{table} ({_KEYS[table]})")
    if table in _UNIQUE:
        con.execute(f"CREATE INDEX IF NOT EXISTS {table}_uid ON out.v0.{table} ({_UNIQUE[table]})")


def _open(out: Path, fresh: bool) -> duckdb.DuckDBPyConnection:
    """The file attached as catalog `out`, its tables under `out.v0`; the
    catalog is named here so the file's own name (v0.duckdb, say) never
    collides with the schema."""
    if fresh and out.exists():
        out.unlink()
    con = duckdb.connect()
    con.execute(f"ATTACH {quote(str(out))} AS out")
    con.execute("CREATE SCHEMA IF NOT EXISTS out.v0")
    return con


def _count(con: duckdb.DuckDBPyConnection, table: str) -> int:
    return con.execute(f"SELECT count(*) FROM out.v0.{table}").fetchone()[0]


def from_export(export_dir: Path, out: Path, *, threads: int | None = None) -> dict[str, int]:
    """Load the export at `export_dir` into `out`; the row count per table."""
    con = _open(out, fresh=True)
    if threads:
        con.execute(f"SET threads = {int(threads)}")
    counts: dict[str, int] = {}
    for table in TABLES:
        source = None
        for suffix in (".csv.zst", ".csv.gz", ".csv"):
            candidate = export_dir / f"{table}{suffix}"
            if candidate.exists():
                source = candidate
                break
        if source is None:
            raise FileNotFoundError(f"{export_dir}: no {table}.csv[.zst] in the export")
        _log(f"{table}: loading")
        # Everything read as text, then cast by name: the export's column
        # order is the database's and may carry columns the tool ignores.
        con.execute(
            f"CREATE OR REPLACE TABLE out.v0.{table} AS "
            + _typed_select(
                table,
                f"read_csv({str(source)!r}, header = true, all_varchar = true, "
                "quote = '\"', escape = '\"', nullstr = '')",
            )
        )
        _index(con, table)
        counts[table] = _count(con, table)
        _log(f"{table}: {counts[table]:,} rows")
    con.execute("CREATE OR REPLACE TABLE out.v0.origin AS SELECT 'export' AS kind, ? AS source", [str(export_dir)])
    con.close()
    return counts


def from_dsn(dsn: str, out: Path, *, threads: int | None = None) -> dict[str, int]:
    """Copy the tables straight from a v0 database, read-only, into `out`."""
    con = _open(out, fresh=True)
    if threads:
        con.execute(f"SET threads = {int(threads)}")
    con.execute("INSTALL postgres; LOAD postgres")
    con.execute(f"ATTACH {quote(dsn)} AS pg (TYPE postgres, READ_ONLY)")
    counts: dict[str, int] = {}
    for table in TABLES:
        _log(f"{table}: copying")
        projected = _PROJECTED.get(table, {})
        cols = ", ".join(
            f"{projected[name]} AS {name}" if name in projected else name for name in TABLES[table]
        )
        con.execute(
            f"CREATE OR REPLACE TABLE out.v0.{table} AS "
            + _typed_select(table, f"(SELECT {cols} FROM pg.public.{table}) AS t")
        )
        _index(con, table)
        counts[table] = _count(con, table)
        _log(f"{table}: {counts[table]:,} rows")
    con.execute("DETACH pg")
    con.execute("CREATE OR REPLACE TABLE out.v0.origin AS SELECT 'database' AS kind, ? AS source", ["<dsn>"])
    con.close()
    return counts


def open_readonly(path: Path) -> duckdb.DuckDBPyConnection:
    """The file attached read-only as catalog `v0db`: tables under `v0db.v0`."""
    con = duckdb.connect()
    con.execute(f"ATTACH {quote(str(path))} AS v0db (READ_ONLY)")
    return con
