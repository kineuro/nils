# SPDX-License-Identifier: AGPL-3.0-only
"""nils-bodypart: v0's body-part detector as a NILS pipeline image.

Four entry points share this package (record 43, S5):

- ``embed``: v0's preparation of a slice, then BiomedCLIP and SigLIP2;
- ``seed``: v0's two-pool seeding (keyword prior and zero-shot margin, each
  picked by farthest-point sampling);
- ``train``: scaler, PCA chosen by cross-validation, a classifier, and a
  calibration on every head;
- ``infer``: slice probabilities, v0's axial Brain-Neck rule and per-stack
  aggregation.

Everything that reads pixels or runs an encoder is imported lazily, so the
rules can be tested without torch.
"""

__version__ = "0.1.0"

# The preparation's version. It keys an embedding (record 43, S4): a change to
# anything in ``preprocess.PreprocessConfig`` or to which slices are embedded
# bumps it, and embeddings made under another version are not mixed.
PREPROCESS_VERSION = "bodypart-v1"

# The body-part values of the MRI pack this image was built for
# (packs/mri/axes/body_part.yml at mri@0.4.0).
PACK_VERSION = "mri@0.4.0"
PACK_VALUES = ("neck", "spine", "brain", "brain-neck", "chest")
