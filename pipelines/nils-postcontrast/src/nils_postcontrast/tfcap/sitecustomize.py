# SPDX-License-Identifier: AGPL-3.0-only
"""A share of the card for each SynthSeg process: with NILS_PC_TF_MEM_MB
set, the first card is given one logical device of that many MiB before any
model is built, so several processes on one card cannot fill it (memory
growth alone lets each keep what it once used)."""
import os

_mb = os.environ.get("NILS_PC_TF_MEM_MB")
if _mb:
    import tensorflow as tf

    _g = tf.config.list_physical_devices("GPU")
    if _g:
        tf.config.set_logical_device_configuration(_g[0], [tf.config.LogicalDeviceConfiguration(memory_limit=int(_mb))])
