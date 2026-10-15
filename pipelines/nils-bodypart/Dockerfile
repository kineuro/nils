# SPDX-License-Identifier: AGPL-3.0-only
#
# nils-bodypart: v0's body-part detector as a NILS pipeline image (record 43,
# S5). One image, five entry points (embed, seed, train, infer, and
# infer-fusion, the certified body-part model of record 50), each with its
# descriptor beside this file.
#
# One image for the CPU and for a card (record 55 A5). torch comes from the
# wheel index of a CUDA torch, whose wheels carry the CUDA runtime, so the
# base stays the same. Where the runner gives the container a card (apptainer
# --nv, a CDI device under podman, --gpus under docker), infer-fusion decodes
# lossless JPEG 2000 on it with the nvImageCodec wheels in requirements.txt
# and runs the cascade's student on it, and embed and seed run their encoders
# on it. Where it gives none, torch finds no card and every step runs on the
# CPU, as the CPU build does: the release publishes this build.
#
#   docker build -t nils-bodypart pipelines/nils-bodypart
#
# A site that never has a card may build the smaller CPU-only image from the
# same torch version; its answers are the same (record 55 E2, A5.2.3a):
#
#   docker build --build-arg TORCH_INDEX=https://download.pytorch.org/whl/cpu \
#       -t nils-bodypart:cpu pipelines/nils-bodypart
#
# Both builds take one torch version, 2.13.0, the one the cascade was graded
# with: its CPU build gave the certified model's answers exactly, and its
# CUDA 12.9 build on a card the CPU build's answers (class I). It is the newest
# with a CUDA 12 build (the nvImageCodec wheels are CUDA 12 ones), and its CUDA
# wheels carry kernels for Blackwell cards (sm_120), which torch 2.5.1 lacked.
#
# The weights are baked in at build time at pinned Hugging Face commits
# (src/nils_bodypart/encoders.py, PINS), and the image runs with
# HF_HUB_OFFLINE=1: the build ends by loading both encoders with the network
# off, so an image that would reach for the network is never made. Given
# ENCODER_DIGESTS (a comma list, as bodypart-embed's descriptor pins them),
# the build also fails where the weights it baked are not those, which is
# how a release checks the descriptors it ships against the image it built.

ARG BASE=python:3.12-slim-bookworm@sha256:392307d22300de8b5986851a12d9176dfc0fc073e65bf6523ebd7dcbeb23564e
FROM ${BASE}

ARG TORCH_INDEX=https://download.pytorch.org/whl/cu129
ARG TORCH_VERSION=2.13.0
ARG TORCHVISION_VERSION=0.28.0

ENV PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    NILS_BODYPART_HOME=/opt/nils-bodypart \
    HF_HOME=/opt/nils-bodypart/hf

# LightGBM's OpenMP runtime, the one system library the image adds.
RUN apt-get update \
    && apt-get install -y --no-install-recommends libgomp1 \
    && rm -rf /var/lib/apt/lists/*

# The standard library's bytecode. The base image ships it for some modules
# only, and the image runs as a user who cannot write it, so every process
# (a run's main process and each of its workers) compiled the rest again at
# every start. pip compiles what it installs below.
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

# The weights, in a layer of their own that a code change does not rebuild.
COPY src/nils_bodypart/__init__.py src/nils_bodypart/encoders.py src/nils_bodypart/bake.py /opt/bake/nils_bodypart/
# The hub client's download cache and log (xet) go: the log names the
# building machine's memory, and nothing reads either offline.
RUN PYTHONPATH=/opt/bake python -m nils_bodypart.bake download \
    && rm -rf /opt/bake "$HF_HOME/xet" \
    && test ! -e "$HF_HOME/xet" \
    && chmod -R a+rX /opt/nils-bodypart

# From here on the hub clients never touch the network.
ENV HF_HUB_OFFLINE=1 \
    TRANSFORMERS_OFFLINE=1 \
    HF_HUB_DISABLE_TELEMETRY=1

COPY pyproject.toml /src/pyproject.toml
COPY src /src/src
ARG ENCODER_DIGESTS=""
RUN pip install --no-deps /src \
    && rm -rf /src \
    && pip freeze > /opt/nils-bodypart/pip-freeze.txt \
    && NILS_BODYPART_EXPECT_ENCODERS="${ENCODER_DIGESTS}" python -m nils_bodypart.bake verify

# Not root: anything run by hand runs as nobody. The engine's runner names
# its own user with --user (with podman's --userns keep-id beside it), which
# overrides this line, so a run's outputs belong to the engine's user.
# Nothing in the image is written at run time.
USER 65534:65534
WORKDIR /tmp
# No ENTRYPOINT: each descriptor's command line names the program
# (`nils-bodypart embed ...`) and replaces this CMD whole.
CMD ["nils-bodypart", "--help"]
