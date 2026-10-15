# SPDX-License-Identifier: AGPL-3.0-only
#
# nils-postcontrast: the certified post-contrast model (model r1) served
# through its three-slice student (record 55 E3), one entry point
# (`nils-postcontrast cascade`), its descriptor beside this file.
#
# One image for the CPU and for a card (record 55 A5). torch comes from the
# wheel index of a CUDA torch, whose wheels carry the CUDA runtime, so the
# base stays the same. Where the runner gives the container a card (apptainer
# --nv, a CDI device under podman, --gpus under docker) the student runs on
# it under bfloat16 autocast, as the study ran it; where it gives none, torch
# finds no card and the student runs on the CPU in float32, as the CPU build
# does. The release publishes this build:
#
#   docker build -t nils-postcontrast pipelines/nils-postcontrast
#
# A site that never has a card may build the smaller CPU-only image of the
# same torch:
#
#   docker build --build-arg TORCH_INDEX=https://download.pytorch.org/whl/cpu \
#       -t nils-postcontrast:cpu pipelines/nils-postcontrast
#
# torch 2.8.0 is the version the student was trained and graded with; its
# CUDA 12.8 wheels carry kernels for Blackwell cards (sm_120).
#
# Nothing is downloaded at run time: the student's network is built with no
# pretrained weights and filled from the run's model input, and the image
# runs offline.

ARG BASE=python:3.12-slim-bookworm@sha256:392307d22300de8b5986851a12d9176dfc0fc073e65bf6523ebd7dcbeb23564e
FROM ${BASE}

ARG TORCH_INDEX=https://download.pytorch.org/whl/cu128
ARG TORCH_VERSION=2.8.0
ARG TORCHVISION_VERSION=0.23.0

ENV PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    HF_HUB_OFFLINE=1 \
    HF_HUB_DISABLE_TELEMETRY=1

# The standard library's bytecode, compiled once (the image runs as a user
# who cannot write it).
RUN python -m compileall -q -j 0 -x '/site-packages/' "$(python -c 'import sysconfig; print(sysconfig.get_path("stdlib"))')"

# torch from the index the build asks for, then every other dependency at
# its pinned version, in one layer. torch's CUDA wheels pull Triton, which
# bundles NVIDIA's ptxas, cuobjdump and nvdisasm: no NVIDIA licence names
# those as redistributable, and nothing here compiles a kernel (no
# torch.compile), so it is removed in the layer that installed it and no
# layer of the image carries it.
COPY requirements.txt /tmp/requirements.txt
RUN pip install --index-url "${TORCH_INDEX}" "torch==${TORCH_VERSION}" "torchvision==${TORCHVISION_VERSION}" \
    && pip install -r /tmp/requirements.txt && rm /tmp/requirements.txt \
    && pip uninstall -y triton \
    && python -c "import importlib.util, sys; sys.exit(importlib.util.find_spec('triton') is not None)"

COPY pyproject.toml /src/pyproject.toml
COPY src /src/src
RUN pip install --no-deps /src \
    && rm -rf /src \
    && mkdir -p /opt/nils-postcontrast \
    && pip freeze > /opt/nils-postcontrast/pip-freeze.txt \
    && python -c "from nils_postcontrast.student import net; net()"

USER 65534:65534
WORKDIR /tmp
CMD ["nils-postcontrast", "--help"]
