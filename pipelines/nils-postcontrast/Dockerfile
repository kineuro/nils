# SPDX-License-Identifier: AGPL-3.0-only
#
# nils-postcontrast: the certified post-contrast model (model r1) served
# through its three-slice student (record 55 E3), one entry point
# (`nils-postcontrast cascade`), its descriptor beside this file.
#
# CPU by default. A CUDA build of torch, for the student on a card, takes
# the wheel index of a CUDA torch; the wheels carry the CUDA runtime, so the
# base stays the same:
#
#   docker build -t nils-postcontrast pipelines/nils-postcontrast
#   docker build --build-arg TORCH_INDEX=https://download.pytorch.org/whl/cu128 \
#       -t nils-postcontrast:cuda pipelines/nils-postcontrast
#
# torch 2.8.0 is the version the student was trained and graded with; its
# CUDA 12.8 wheels carry kernels for Blackwell cards (sm_120).
#
# Nothing is downloaded at run time: the student's network is built with no
# pretrained weights and filled from the run's model input, and the image
# runs offline.

ARG BASE=python:3.12-slim-bookworm@sha256:392307d22300de8b5986851a12d9176dfc0fc073e65bf6523ebd7dcbeb23564e
FROM ${BASE}

ARG TORCH_INDEX=https://download.pytorch.org/whl/cpu
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

RUN pip install --index-url "${TORCH_INDEX}" "torch==${TORCH_VERSION}" "torchvision==${TORCHVISION_VERSION}"

COPY requirements.txt /tmp/requirements.txt
RUN pip install -r /tmp/requirements.txt && rm /tmp/requirements.txt

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
