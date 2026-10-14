# SPDX-License-Identifier: AGPL-3.0-only
#
# nils-system1: System 1 served as a pipeline (record 55 D3, spec 11.1), one
# entry point (`nils-system1 score`), its descriptor beside this file.
#
#   docker build -t nils-system1 pipelines/nils-system1
#
# The model is a site model, never part of the image: a bundle (one tar
# file) the run is given as its model input, whose files are checked
# against the sha256 its system1.json names before anything is loaded. CPU
# only; System 1 answers in milliseconds a stack. The image runs offline.

ARG BASE=python:3.12-slim-bookworm@sha256:392307d22300de8b5986851a12d9176dfc0fc073e65bf6523ebd7dcbeb23564e
FROM ${BASE}

ENV PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1

RUN python -m compileall -q -j 0 -x '/site-packages/' "$(python -c 'import sysconfig; print(sysconfig.get_path("stdlib"))')"

COPY requirements.txt /tmp/requirements.txt
RUN apt-get update && apt-get install -y --no-install-recommends libgomp1 \
    && rm -rf /var/lib/apt/lists/* \
    && pip install -r /tmp/requirements.txt && rm /tmp/requirements.txt

COPY pyproject.toml /src/pyproject.toml
COPY src /src/src
RUN pip install --no-deps /src \
    && rm -rf /src \
    && mkdir -p /opt/nils-system1 \
    && pip freeze > /opt/nils-system1/pip-freeze.txt

USER 65534:65534
WORKDIR /tmp
CMD ["nils-system1", "--help"]
