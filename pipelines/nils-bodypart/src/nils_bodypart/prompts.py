# SPDX-License-Identifier: AGPL-3.0-only
"""Zero-shot text prompts for seeding, v0's catalogue (``prompts.py``).

The catalogue is keyed by the pack's values instead of v0's category names
(Brain, Brain-Neck, Spine, Chest become brain, brain-neck, spine, chest); the
prompts are v0's word for word. A value without its own prompts, such as the
pack's ``neck``, takes v0's fallback for a custom category: three generic
positives and the union of every default negative.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class CategoryPrompts:
    positive: tuple[str, ...]
    negative: tuple[str, ...]


_GENERIC_NEGATIVES: tuple[str, ...] = (
    "MRI of the abdomen",
    "MRI of the pelvis",
    "MRI of the knee",
    "MRI of the breast",
    "x-ray scout image",
    "localizer image",
)

DEFAULT_PROMPTS: dict[str, CategoryPrompts] = {
    "brain": CategoryPrompts(
        positive=(
            "axial MRI scan of the brain",
            "sagittal MRI scan of the brain",
            "coronal MRI scan of the brain",
            "MRI showing brain parenchyma and skull",
            "head MRI showing the cerebrum",
        ),
        negative=(
            "MRI of the cervical spine",
            "MRI of the thoracic spine",
            "MRI of the lumbar spine",
            "MRI of the chest",
            "MRI showing the neck and trachea",
        )
        + _GENERIC_NEGATIVES,
    ),
    "brain-neck": CategoryPrompts(
        positive=(
            "sagittal MRI showing the brain and the cervical spine",
            "MRI showing the brain stem and the upper neck",
            "MRI showing the head and the cervical region",
            "MRI of the head and neck",
        ),
        negative=(
            "axial MRI of the brain only",
            "MRI of the lumbar spine",
            "MRI of the thoracic spine",
            "MRI of the chest",
        )
        + _GENERIC_NEGATIVES,
    ),
    "spine": CategoryPrompts(
        positive=(
            "sagittal MRI of the spine",
            "MRI of the cervical spine",
            "MRI of the thoracic spine",
            "MRI of the lumbar spine",
            "MRI showing vertebrae and spinal cord",
        ),
        negative=(
            "axial MRI of the brain",
            "MRI of the head only",
            "MRI of the chest",
        )
        + _GENERIC_NEGATIVES,
    ),
    "chest": CategoryPrompts(
        positive=(
            "MRI of the chest",
            "thoracic MRI showing the heart and lungs",
            "axial MRI of the thorax",
        ),
        negative=(
            "MRI of the brain",
            "MRI of the cervical spine",
            "MRI of the lumbar spine",
            "MRI of the abdomen",
        )
        + _GENERIC_NEGATIVES,
    ),
}


def prompts_for_category(category: str) -> CategoryPrompts:
    """The prompts of a value, or v0's generic pack for one it has none for."""
    if category in DEFAULT_PROMPTS:
        return DEFAULT_PROMPTS[category]
    cat = category.strip()
    pos = (f"MRI of the {cat}", f"axial MRI scan of the {cat}", f"MRI showing the {cat}")
    seen: set[str] = set()
    neg: list[str] = []
    for pack in DEFAULT_PROMPTS.values():
        for n in pack.negative:
            if n not in seen:
                seen.add(n)
                neg.append(n)
    return CategoryPrompts(positive=pos, negative=tuple(neg))
