# SPDX-License-Identifier: AGPL-3.0-only
"""How v0's tables map onto the catalogue's levels: the v0 table of each
level, its join key, and the v0 column of every v1 column where the names
differ. v0 named its columns as v1 does with a few exceptions, all listed
here, so a column absent from this mapping is the same name on both sides."""

from __future__ import annotations

from dataclasses import dataclass, field

from .catalogue import Field


@dataclass(frozen=True)
class Level:
    name: str
    #: the v0 table
    table: str
    #: v1 column -> v0 column, where the names differ
    renames: dict[str, str] = field(default_factory=dict)
    #: v1 columns v0 has no counterpart for
    absent: frozenset[str] = frozenset()
    #: v1 columns v0 stored rounded: column -> decimals
    decimals: dict[str, int] = field(default_factory=dict)

    def v0_column(self, column: str) -> str | None:
        if column in self.absent:
            return None
        return self.renames.get(column, column)


LEVELS: dict[str, Level] = {
    "subject": Level("subject", "subject", renames={"birth_date": "patient_birth_date", "sex": "patient_sex"}),
    # The three dates Wave 3 added for the date vote (spec §4.2) are v1's
    # alone: v0 never read them, so there is nothing to compare against.
    "study": Level(
        "study",
        "study",
        renames={"modalities_in_study": "modality"},
        absent=frozenset({"pps_start_date", "pps_end_date", "issue_date"}),
    ),
    # `burned_in_annotation` is Wave 3 §8.4's addition: what the file says
    # about text in its own pixels. v0 never reads it, so there is nothing to
    # compare against.
    "series": Level(
        "series", "series", absent=frozenset({"burned_in_annotation"})
    ),
    "series_mr": Level("series_mr", "mri_series_details"),
    "series_ct": Level("series_ct", "ct_series_details"),
    "series_pet": Level("series_pet", "pet_series_details"),
    # v0's series_stack prefixes every value with `stack_`; its
    # `stack_image_orientation` is the derived orientation class (v1's
    # `orientation`), and the raw orientation v1 keeps has no v0 column.
    "stack": Level(
        "stack",
        "series_stack",
        renames={
            "inversion_time": "stack_inversion_time",
            "echo_time": "stack_echo_time",
            "echo_numbers": "stack_echo_numbers",
            "echo_train_length": "stack_echo_train_length",
            "repetition_time": "stack_repetition_time",
            "flip_angle": "stack_flip_angle",
            "receive_coil_name": "stack_receive_coil_name",
            "image_type": "stack_image_type",
            "xray_exposure": "stack_xray_exposure",
            "kvp": "stack_kvp",
            "tube_current": "stack_tube_current",
            "pet_bed_index": "stack_pet_bed_index",
            "pet_frame_type": "stack_pet_frame_type",
            "orientation": "stack_image_orientation",
        },
        absent=frozenset({"image_orientation_patient"}),
        decimals={
            "echo_time": 2,
            "inversion_time": 1,
            "repetition_time": 1,
            "flip_angle": 1,
            "kvp": 0,
            "tube_current": 0,
        },
    ),
    # Wave 3 §6 moved the seven diffusion values that vary from one image of a
    # series to the next onto the instance. v0 keeps them on
    # `mri_series_details`, which is keyed by series, so there is one v0 value
    # for a whole series and nothing to compare an image against. That is not a
    # gap in the tool: it is the finding. v0's own enrichment joins that table
    # to `instance` and walks one row repeated once per image, so its list of
    # shells holds one value and its gradient count is one.
    "instance": Level(
        "instance",
        "instance",
        absent=frozenset(
            {
                "charset",
                "instance_creation_date",
                "presentation_creation_date",
                "diffusion_b_value",
                "diffusion_gradient_orientation",
                "diffusion_directionality",
                "dwi_siemens_b_value",
                "dwi_siemens_directionality",
                "dwi_ge_b_value",
                "dwi_philips_b_value",
            }
        ),
    ),
}

#: The stack columns compared besides the catalogue's: the orientation class.
STACK_EXTRA: tuple[Field, ...] = (Field("stack", "orientation", "text", "technical", "the derived class"),)

#: v0 wrote only these SOP classes (`extract/worker.py`, ALLOWED_SOP_CLASS_UIDS).
V0_SOP_CLASSES: frozenset[str] = frozenset(
    {
        "1.2.840.10008.5.1.4.1.1.2",
        "1.2.840.10008.5.1.4.1.1.2.1",
        "1.2.840.10008.5.1.4.1.1.2.2",
        "1.2.840.10008.5.1.4.1.1.4",
        "1.2.840.10008.5.1.4.1.1.4.1",
        "1.2.840.10008.5.1.4.1.1.4.2",
        "1.2.840.10008.5.1.4.1.1.4.4",
        "1.2.840.10008.5.1.4.1.1.128",
        "1.2.840.10008.5.1.4.1.1.128.1",
    }
)

#: v0 wrote only these modalities.
V0_MODALITIES: frozenset[str] = frozenset({"MR", "CT", "PT", "PET"})

#: The fields §12.3 wants identical on every compared row; the rest may
#: differ on one row in a thousand.
EXACT_FIELDS: frozenset[tuple[str, str]] = frozenset(
    {
        ("series", "modality"),
        ("series", "sop_class_uid"),
        ("instance", "instance_number"),
        ("instance", "rows"),
        ("instance", "columns"),
        ("instance", "number_of_frames"),
        ("stack", "orientation"),
    }
    | {("stack", c) for c in LEVELS["stack"].renames if c != "orientation"}
)

#: Series columns that carry the first instance's value on both sides
#: (the engine's `VARIES_PER_INSTANCE`; v0 kept them on the series row the
#: same way). Which instance is first depends on the walk order, so the two
#: registries disagree on them by construction: the tool compares and
#: reports them, and classes their divergences `accepted` unless the
#: adjudication file says otherwise.
ORDER_DEPENDENT: dict[tuple[str, str], str] = {
    ("series", "media_storage_sop_instance_uid"): "the first instance's file meta; which instance is first follows the walk order on both sides",
    ("series", "image_position_patient"): "the first instance's position; which instance is first follows the walk order on both sides",
}

#: The suffix a divergence pattern carries when the series holds several
#: stacks on either side and the field is one a stack signature is made of
#: (`catalogue.stack_defining`): the series row carries the first instance's
#: value on both sides (§8), the instances differ on it by definition of the
#: stacks, and which one is first follows the walk order. The tool classes
#: such a group `accepted` unless the adjudication file says otherwise; the
#: same divergence in a single-stack series keeps its plain pattern and its
#: standing.
MULTI_STACK = " (multi-stack)"
MULTI_STACK_NOTE = (
    "a stack-signature column of a series with several stacks: the series row carries the first "
    "instance's value on both sides, and which instance is first follows the walk order"
)

#: The v0 file-name modes (`extract/worker.py`, `_matches_extension`) and the
#: v1 `files` knob that selects the same names.
V0_FILE_MODES: dict[str, str] = {
    "all": "dcm,no-ext",
    "dcm": "*.dcm",
    "DCM": "*.DCM",
    "all_dcm": "dcm",
    "no_ext": "no-ext",
}
