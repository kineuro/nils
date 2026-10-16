# SPDX-License-Identifier: AGPL-3.0-only
#
# nils-lst-ai: LST-AI as NILS runs it (the descriptor is nils.job.yml in this
# folder; record 55, the thin image its notes promised). LST-AI's own image,
# jqmcginnis/lst-ai v1.2.0 by LST-AI's authors, pinned below by its manifest
# digest, with one thin layer on top.
#
#   docker build -f pipelines/lst-ai/Dockerfile -t nils-lst-ai pipelines/lst-ai
#
# Published to ghcr.io/kineuro/nils-lst-ai by scripts/publish-lst-ai.sh, whose
# --pin writes the pushed digest into the descriptor.
#
# What the layer changes, and nothing else (LST-AI's code, weights and atlas,
# greedy, HD-BET and every library are the upstream image's, byte for byte):
#
# - No ENTRYPOINT. Job contract v1 runs the descriptor's command line as the
#   container's command; upstream's ENTRYPOINT `lst` would take its words as
#   its own arguments and refuse them.
# - HD-BET's parameters are in /opt/hd-bet_params, readable by every account.
#   Upstream keeps them in /root/hd-bet_params as files of mode 0600 under a
#   /root of mode 0700, and HD-BET looks for them in ~/hd-bet_params, so the
#   account a run has (never root, no passwd entry, a home that is missing or
#   a root that is read-only) found none and HD-BET went to download them,
#   where a run has no network. Now one line of HD_BET/paths.py names the
#   folder, whoever runs it and whatever its home is.
# - A working folder and a user that need no /root (upstream's WORKDIR was
#   /root/hd-bet_params).
# - The notices of what the image carries (NOTICE) and the licence texts that
#   ride with it, in /usr/local/share/licenses/nils-lst-ai.
#
# The image runs offline: LST-AI's weights, atlas and binaries are in it, so
# its own download step finds them and fetches nothing.

ARG LST_NAME=docker.io/jqmcginnis/lst-ai
ARG LST_DIGEST=sha256:2b2a0d6454e6efff60a350df3c650ae9584f1926338dec7842f9e71912cc6c85

FROM ${LST_NAME}@${LST_DIGEST}

ARG LST_NAME
ARG LST_DIGEST
LABEL org.opencontainers.image.title="nils-lst-ai" \
      org.opencontainers.image.description="LST-AI 1.2.0 (white-matter lesions of multiple sclerosis from a T1w and a FLAIR) as NILS runs it: LST-AI's own image without its ENTRYPOINT, HD-BET's parameters readable by every account. Research use only." \
      org.opencontainers.image.base.name="${LST_NAME}" \
      org.opencontainers.image.base.digest="${LST_DIGEST}"

# Before anything is removed: no RUN below starts in the upstream WORKDIR,
# which is the folder the parameters leave
WORKDIR /tmp

# HD-BET's five parameter files (Zenodo record 2540695, CC-BY-4.0), copied
# unchanged to a folder every account reads, and the upstream folder gone. The
# sums are those of the upstream image's own files, whose MD5 sums are the
# record's.
RUN set -eux; \
    install -d -m 0755 /opt/hd-bet_params; \
    for i in 0 1 2 3 4; do install -m 0644 "/root/hd-bet_params/$i.model" "/opt/hd-bet_params/$i.model"; done; \
    cd /opt/hd-bet_params; \
    printf '%s\n' \
      '6f75233753c4750672815e2b7a86db754995ae44b8f1cd77bccfc37becd2d83c  0.model' \
      '5fcd430c271bb5e13ea8d44f3d303b2dfafa4d0845c1aec6c5a7375f6d727189  1.model' \
      '558ede4acb35537585219ece9c6b4b627dd3c019c6865daed0dfc80781dd94e9  2.model' \
      '57bf67a58c9e925b13d5b9ae6d6bb648deaf759f6d0af2ccad67651d7a2a93db  3.model' \
      '567689e4dff87debe357b30e4414c412485207657d1e99c33bf0bf587d28bf32  4.model' \
      | sha256sum -c -; \
    rm -rf /root/hd-bet_params

# HD-BET looks for them in ~/hd-bet_params. One line of its paths.py names the
# folder instead (Apache-2.0 asks a changed file to say so, and the file does).
RUN set -eux; f=/custom_apps/lst_directory/HD-BET/HD_BET/paths.py; \
    grep -qxF "folder_with_parameter_files = os.path.join(os.path.expanduser('~'), 'hd-bet_params')" "$f"; \
    sed -i "s|^folder_with_parameter_files = .*\$|# Changed by NILS (kineuro/nils, pipelines/lst-ai/Dockerfile): the parameters are at one place in this image,\n# whoever runs it and wherever its home is. HD-BET is under the Apache License 2.0 (see its LICENSE).\nfolder_with_parameter_files = '/opt/hd-bet_params'|" "$f"; \
    find /custom_apps/lst_directory/HD-BET -name 'paths.cpython-*.pyc' -delete; \
    [ "$(grep -c '^folder_with_parameter_files' "$f")" = 1 ]; \
    HOME=/nonexistent python3 -c "from HD_BET.paths import folder_with_parameter_files as p; assert p == '/opt/hd-bet_params', p"

# The licence texts that ride with the code they cover, from the upstream
# image's own trees: LST-AI's MIT, HD-BET's Apache-2.0, and greedy's GPL-3.0
# (the greedy built into this image predates its change to Apache-2.0, and its
# source is in the image, in /greedy).
RUN set -eux; L=/usr/local/share/licenses/nils-lst-ai; mkdir -p "$L"; \
    cp /custom_apps/lst_directory/LST-AI/LICENSE "$L/LST-AI-LICENSE.txt"; \
    cp /custom_apps/lst_directory/HD-BET/LICENSE "$L/HD-BET-LICENSE.txt"; \
    cp /greedy/COPYING.txt "$L/greedy-COPYING.txt"; \
    chmod 0644 "$L"/*
COPY --chmod=0644 NOTICE /usr/local/share/licenses/nils-lst-ai/NOTICE

# What the layer promises, checked as the account a run has: no passwd entry,
# no home, nothing of root's to read. The parameters are all there and read,
# HD-BET names them, and the notices are in place.
RUN set -eux; \
    test ! -e /root/hd-bet_params; \
    setpriv --reuid 65534 --regid 65534 --clear-groups env -i PATH="$PATH" HOME=/nonexistent sh -ec ' \
      for i in 0 1 2 3 4; do head -c 16 "/opt/hd-bet_params/$i.model" > /dev/null; done; \
      python3 -c "from HD_BET.utils import get_params_fname; assert get_params_fname(0) == \"/opt/hd-bet_params/0.model\"" ; \
      test -s /usr/local/share/licenses/nils-lst-ai/NOTICE; \
      test -s /usr/local/share/licenses/nils-lst-ai/LST-AI-LICENSE.txt; \
      test -s /usr/local/share/licenses/nils-lst-ai/HD-BET-LICENSE.txt; \
      test -s /usr/local/share/licenses/nils-lst-ai/greedy-COPYING.txt; \
      command -v lst > /dev/null; command -v hd-bet > /dev/null; command -v greedy > /dev/null'

# No ENTRYPOINT: the descriptor's command line is the container's command. A
# plain `docker run` prints lst's help and is not root.
ENTRYPOINT []
USER 65534:65534
CMD ["lst", "--help"]
