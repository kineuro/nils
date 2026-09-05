#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
#
# Wave 3's first two slices against the awkward corpus (spec §12, bar 1).
#
#     gate.sh WORKDIR [NILS] [AWKWARD]
#
# Writes the corpus, digests it, and checks the result against its manifest.
#
# Each scenario is digested with the identity rule its manifest declares,
# because one rule cannot read every layout: an archive whose subject folder
# sits one level down needs to be told so, and an archive whose tag is good
# needs no path source at all. Pretending one rule serves all of them hides a
# real setting and lets a scenario pass for the wrong reason.
set -eu

work=${1:?usage: gate.sh WORKDIR [NILS] [AWKWARD]}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
nils=${2:-$repo/engine/target/release/nils}
awkward=${3:-$repo/engine/target/release/examples/awkward}

# A run never writes into another run's directory. Re-running a gate over a
# registry that already passed destroys the evidence of what it passed with,
# which is how one earlier failure became impossible to diff against.
if [ -e "$work" ]; then
    echo "gate.sh: $work exists; give a run its own directory" >&2
    exit 2
fi
corpus="$work/corpus"
manifest="$work/manifest.json"
mkdir -p "$work"

echo "corpus:" >&2
"$awkward" --out "$corpus" > "$manifest"

# One registry per scenario, because the rule differs per scenario.
rules=$(python3 - "$manifest" <<'PY'
import json, sys
for s in json.load(open(sys.argv[1]))["scenarios"]:
    print(s["name"], s["rule"])
PY
)

merged="$work/merged.db"
first=1
echo "$rules" | while read -r name rule; do
    reg="$work/reg-$name"
    key="$work/key"
    head -c 32 /dev/urandom | base64 | tr -d '\n' > "$key"
    "$nils" key add k --registry "$reg" --from-file "$key" >/dev/null
    rm -f "$key"
    "$nils" init --registry "$reg" --backend sqlite --scheme blake2b-8 --key k >/dev/null

    file="$work/rule-$name.yml"
    case "$rule" in
    default)
        # The tag is the identity, which is v1's default rule.
        "$nils" digest "$corpus/$name" --registry "$reg" \
            --files dcm,no-ext,DCM,IMA --name "$name" --json > "$work/report-$name.json"
        ;;
    name)
        # The code hides in PatientName next to a date, so the pattern is what
        # takes the code and leaves the date.
        "$nils" linkage id-type add study-code --registry "$reg" >/dev/null
        printf '%s\n' \
            'identity:' \
            '  id_type: study-code' \
            '  code: verbatim' \
            '  from:' \
            '    - field: PatientName' \
            "      pattern: '^(?<id>[A-Za-z]+[0-9]+)\\^'" \
            '  fallback: StudyInstanceUID' > "$file"
        "$nils" digest "$corpus/$name" --registry "$reg" \
            --files dcm,no-ext,DCM,IMA --identity-rule "$file" --name "$name" --json > "$work/report-$name.json"
        ;;
    path:*)
        # The code is in the path. The pattern on the tag is what refuses a
        # placeholder, so the folder answers instead of it.
        "$nils" linkage id-type add study-code --registry "$reg" >/dev/null
        printf '%s\n' \
            'identity:' \
            '  id_type: study-code' \
            '  code: verbatim' \
            '  from:' \
            '    - field: PatientID' \
            "      pattern: '^(?<id>[A-Za-z][A-Za-z0-9]{2,}[0-9]{2,})$'" \
            '    - path:' \
            "        segment: ${rule#path:}" \
            "        pattern: '^(?<id>.+)$'" \
            '  fallback: StudyInstanceUID' > "$file"
        "$nils" digest "$corpus/$name" --registry "$reg" \
            --files dcm,no-ext,DCM,IMA --identity-rule "$file" --name "$name" --json > "$work/report-$name.json"
        ;;
    esac

    # The scenarios are digested apart and checked together, so the paths in
    # each registry are re-prefixed with the scenario they came from.
    python3 "$here/collect.py" "$reg/registry.db" "$name" "$merged" "$first" \
        "$work/report-$name.json"

    # Sessions are derived, never stored, so they are checked by asking for
    # them rather than by reading a column. A scenario may declare more than
    # one scheme: the point of most of them is that the same studies label
    # differently depending on what the scheme says.
    python3 "$here/schemes.py" "$manifest" "$name" "$work" | while read -r n; do
        "$nils" session list --registry "$reg" --scheme "$work/scheme-$name-$n.yml" \
            --json > "$work/sessions-$name-$n.json"
    done
    first=0
done

python3 "$here/check.py" "$merged" "$manifest" "$work" "${VERBOSE:+--verbose}"
