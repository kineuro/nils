# SPDX-License-Identifier: AGPL-3.0-only
#
# nils-bodypart: v0's body-part detector as a NILS pipeline image (record 43,
# S5). One image, four entry points (embed, seed, train, infer), each with its
# descriptor beside this file.
#
# CPU by default, which is how it runs until the laptop has a rootless GPU
# path (record 43 R5). A CUDA build takes the wheel index of a CUDA torch; the
# wheels carry the CUDA runtime, so the base stays the same:
#
#   docker build -t nils-bodypart pipelines/nils-bodypart
#   docker build --build-arg TORCH_INDEX=https://download.pytorch.org/whl/cu121 \
#       -t nils-bodypart:cuda pipelines/nils-bodypart
#
# The weights are baked in at build time at pinned Hugging Face commits
# (src/nils_bodypart/encoders.py, PINS), and the image runs with
# HF_HUB_OFFLINE=1: the build ends by loading both encoders with the network
# off, so an image that would reach for the network is never made.

ARG BASE=python:3.12-slim-bookworm@sha256:392307d22300de8b5986851a12d9176dfc0fc073e65bf6523ebd7dcbeb23564e
FROM ${BASE}

ARG TORCH_INDEX=https://download.pytorch.org/whl/cpu
ARG TORCH_VERSION=2.5.1
ARG TORCHVISION_VERSION=0.20.1

ENV PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    NILS_BODYPART_HOME=/opt/nils-bodypart \
    HF_HOME=/opt/nils-bodypart/hf

# torch first, in its own layer, from the index the build asks for.
RUN pip install --index-url "${TORCH_INDEX}" "torch==${TORCH_VERSION}" "torchvision==${TORCHVISION_VERSION}"

# Every other dependency at its pinned version.
COPY requirements.txt /tmp/requirements.txt
RUN pip install -r /tmp/requirements.txt && rm /tmp/requirements.txt

# The weights, in a layer of their own that a code change does not rebuild.
COPY src/nils_bodypart/__init__.py src/nils_bodypart/encoders.py src/nils_bodypart/bake.py /opt/bake/nils_bodypart/
RUN PYTHONPATH=/opt/bake python -m nils_bodypart.bake download \
    && rm -rf /opt/bake \
    && chmod -R a+rX /opt/nils-bodypart

# From here on the hub clients never touch the network.
ENV HF_HUB_OFFLINE=1 \
    TRANSFORMERS_OFFLINE=1 \
    HF_HUB_DISABLE_TELEMETRY=1

COPY pyproject.toml /src/pyproject.toml
COPY src /src/src
RUN pip install --no-deps /src \
    && rm -rf /src \
    && pip freeze > /opt/nils-bodypart/pip-freeze.txt \
    && python -m nils_bodypart.bake verify

# Not root: a runner maps its own user in (podman --userns keep-id), and
# anything else runs as nobody. Nothing in the image is written at run time.
USER 65534:65534
WORKDIR /tmp
ENTRYPOINT ["nils-bodypart"]
CMD ["--help"]
