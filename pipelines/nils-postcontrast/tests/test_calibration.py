# SPDX-License-Identifier: AGPL-3.0-only
"""A site's calibration (0.3.0): the pair file's shapes, the labels file's
refusals by line number, the Platt fit recovering a known pair from the raw
scores a run wrote, the floor of 20 per class, the report per label source,
and the console command end to end."""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
import pytest

from nils_postcontrast import calibration, cli
from nils_postcontrast.calibration import CalibrationError, fit_platt, load_pairs, read_labels
from nils_postcontrast.fusion import Fusion

A, B = 0.8, -0.5
FUSION = "sha256:" + "f" * 64


def fusion() -> Fusion:
    return Fusion(cols=["x"], mean=np.zeros(1), scale=np.ones(1), coef=np.ones(1), intercept=0.0, cal={"cohort-a Siemens": (1.0, 0.0)}, t=0.9,
                  use_V=True, cohort_aliases={}, untrusted_time_cohorts=[])


def write_run(tmp: Path, n: int, seed: int = 0, site=None, start: int = 1) -> dict:
    """A raw-score run's output folder: n tables with z drawn wide and the
    label each z would get under the known pair, by stack id."""
    rng = np.random.default_rng(seed)
    z = rng.normal(0.0, 3.0, n)
    y = (rng.random(n) < 1 / (1 + np.exp(-(A * z + B)))).astype(int)
    folder = tmp / "postcontrast"
    folder.mkdir(parents=True, exist_ok=True)
    labels = {}
    for i in range(n):
        sid = start + i
        (folder / f"{sid}.json").write_text(json.dumps({"stack_id": sid, "p_given": None, "value": None, "reason": "no-calibration",
                                                         "score_raw": round(float(z[i]), 6), "site": site, "fusion_digest": FUSION,
                                                         "cascade_digest": "sha256:" + "c" * 64}))
        labels[sid] = int(y[i])
    # a table with no raw score, as a calibrated stack's, is passed over
    (folder / f"{start + n}.json").write_text(json.dumps({"stack_id": start + n, "p_given": 0.99, "value": "given", "score_raw": None}))
    return labels


def write_labels(path: Path, labels: dict, sources=("header", "name", "manual"), extra_rows=()) -> None:
    lines = ["stack\tlabel\tsource"]
    for i, (sid, y) in enumerate(sorted(labels.items())):
        lines.append(f"{sid}\t{'given' if y else 'not_given'}\t{sources[i % len(sources)]}")
    lines += list(extra_rows)
    path.write_text("\n".join(lines) + "\n")


# ---------------------------------------------------------------- the pairs

def test_a_pair_file_and_a_map_of_pairs_both_load_and_merge_over_the_fusion(tmp_path):
    one = tmp_path / "one.json"
    one.write_text(json.dumps({"site": "cohort-c Siemens", "a": 0.7, "b": -0.2, "n": 40}))
    assert load_pairs(one) == {"cohort-c Siemens": (0.7, -0.2)}
    many = tmp_path / "many.json"
    many.write_text(json.dumps({"cohort-c Siemens": [0.7, -0.2], "cohort-a Siemens": {"a": 2.0, "b": 0.5}}))
    G, sites = calibration.apply(fusion(), many)
    assert sites == ["cohort-a Siemens", "cohort-c Siemens"]
    assert G.cal == {"cohort-a Siemens": (2.0, 0.5), "cohort-c Siemens": (0.7, -0.2)} and G.sites() == set(G.cal)
    for bad in ("not json", "{}", json.dumps({"cohort-c Siemens": [1.0]}), json.dumps({"cohort-c Siemens": ["a", 0.0]}),
                json.dumps({"cohort-c Siemens": [float("nan"), 0.0]}).replace("NaN", "1e999")):
        (tmp_path / "bad.json").write_text(bad)
        with pytest.raises(CalibrationError):
            load_pairs(tmp_path / "bad.json")


def test_the_run_reads_the_site_file_the_engine_mounted(tmp_path, monkeypatch):
    f = tmp_path / "pairs.json"
    f.write_text(json.dumps({"cohort-c Siemens": [1.0, 0.0]}))
    extra: dict = {}
    assert calibration.for_run(fusion(), None, extra).cal == {"cohort-a Siemens": (1.0, 0.0)} and extra == {}
    monkeypatch.setenv(calibration.ENV, str(f))
    G = calibration.for_run(fusion(), None, extra)
    assert "cohort-c Siemens" in G.cal and extra == {"calibration": {"sites": ["cohort-c Siemens"]}}
    with pytest.raises(CalibrationError):
        calibration.for_run(fusion(), str(tmp_path / "missing.json"), {})


# ---------------------------------------------------------------- the labels

def test_a_bad_row_is_refused_by_its_line_number_and_never_by_its_content(tmp_path):
    p = tmp_path / "labels.tsv"
    p.write_text("stack\tlabel\tsource\n1\tgiven\theader\n2\tnot_given\tname\n3\tgiven\tmanual\n4\tgiven\tguessword\n")
    with pytest.raises(CalibrationError, match="line 5") as e:
        read_labels(p)
    assert "guessword" not in str(e.value) and "source" in str(e.value)
    p.write_text("stack\tlabel\tsource\n1\tgiven\theader\n2\tmaybe\tname\n")
    with pytest.raises(CalibrationError, match="line 3"):
        read_labels(p)
    p.write_text("stack\tlabel\tsource\n1\tgiven\theader\n1\tgiven\theader\n")
    with pytest.raises(CalibrationError, match="line 3.*repeats"):
        read_labels(p)
    p.write_text("stack\tlabel\tsource\nx\tgiven\theader\n")
    with pytest.raises(CalibrationError, match="line 2"):
        read_labels(p)
    p.write_text("stack\tlabel\n1\tgiven\n")
    with pytest.raises(CalibrationError, match="lacks the column source"):
        read_labels(p)
    p.write_text("source\tstack\tlabel\textra\nheader\t7\tnot_given\tx\n\n")
    assert read_labels(p) == {7: (0, "header")}


# ---------------------------------------------------------------- the fit

def test_the_fit_recovers_a_known_pair_and_stays_finite_on_a_separable_set():
    rng = np.random.default_rng(1)
    z = rng.normal(0.0, 3.0, 4000)
    y = (rng.random(4000) < 1 / (1 + np.exp(-(A * z + B)))).astype(float)
    a, b = fit_platt(z, y)
    assert abs(a - A) < 0.08 and abs(b - B) < 0.1, (a, b)
    a, b = fit_platt(np.array([-3.0, -2.0, -1.0, 1.0, 2.0, 3.0]), np.array([0, 0, 0, 1, 1, 1.0]))
    assert math.isfinite(a) and math.isfinite(b) and a > 0


def test_calibrate_fits_the_sites_pair_from_a_runs_raw_scores_and_reports_per_source(tmp_path, capsys):
    labels = write_run(tmp_path / "run", 2000)
    # three labelled stacks no run scored
    write_labels(tmp_path / "labels.tsv", labels, extra_rows=["9001\tgiven\theader", "9002\tnot_given\tname", "9003\tgiven\tmanual"])
    out = tmp_path / "pair.json"
    rc = cli.main(["calibrate", "--labels", str(tmp_path / "labels.tsv"), "--scores", str(tmp_path / "run"), "--site", "cohort-c Siemens",
                   "--out", str(out)])
    assert rc == 0
    d = json.loads(out.read_text())
    assert d["site"] == "cohort-c Siemens" and abs(d["a"] - A) < 0.15 and abs(d["b"] - B) < 0.2, (d["a"], d["b"])
    assert d["n"] == 2000 and d["n_given"] + d["n_not_given"] == 2000 and d["n_without_score"] == 3 and d["n_other_site"] == 0
    assert set(d["per_source"]) == {"header", "name", "manual"} and sum(m["n"] for m in d["per_source"].values()) == 2000
    for m in list(d["per_source"].values()) + [d["overall"]]:
        assert 0 < m["log_loss"] < math.log(2) and 0.5 < m["accuracy"] <= 1
    assert d["model"]["fusion_digest"] == FUSION and d["model"]["version"] == cli.__version__ and d["fitted_at"].endswith("Z")
    assert set(d) == {"site", "a", "b", "n", "n_given", "n_not_given", "n_without_score", "n_other_site", "per_source", "overall", "model", "fitted_at"}
    text = capsys.readouterr().out
    assert "site cohort-c Siemens" in text and "3 labelled with no score" in text and "header:" in text and "manual:" in text
    # the pair file is a calibration file a run takes as it is
    assert load_pairs(out) == {"cohort-c Siemens": (d["a"], d["b"])}


def test_calibrate_refuses_under_twenty_of_a_class_and_a_bad_source_by_line(tmp_path, capsys):
    labels = write_run(tmp_path / "run", 60, seed=3)
    few = {sid: y for sid, y in labels.items()}
    given = [sid for sid, y in few.items() if y]
    for sid in given[19:]:
        few[sid] = 0  # 19 given at most
    write_labels(tmp_path / "labels.tsv", few)
    rc = cli.main(["calibrate", "--labels", str(tmp_path / "labels.tsv"), "--scores", str(tmp_path / "run"), "--site", "cohort-c Siemens",
                   "--out", str(tmp_path / "pair.json")])
    err = capsys.readouterr().err
    assert rc == 1 and "at least 20" in err and "19 given" in err and not (tmp_path / "pair.json").exists()
    write_labels(tmp_path / "labels.tsv", labels, extra_rows=["77\tgiven\tsomeword"])
    rc = cli.main(["calibrate", "--labels", str(tmp_path / "labels.tsv"), "--scores", str(tmp_path / "run"), "--site", "cohort-c Siemens",
                   "--out", str(tmp_path / "pair.json")])
    err = capsys.readouterr().err
    assert rc == 1 and f"line {len(labels) + 2}" in err and "someword" not in err


def test_calibrate_takes_several_runs_and_keeps_another_sites_rows_out(tmp_path):
    la = write_run(tmp_path / "run-a", 80, seed=5, start=1)
    lb = write_run(tmp_path / "run-b", 80, seed=6, start=1000)
    lc = write_run(tmp_path / "run-c", 30, seed=7, site="cohort-a Siemens", start=5000)
    labels = {**la, **lb, **lc}
    write_labels(tmp_path / "labels.tsv", labels)
    scores = calibration.read_scores([tmp_path / "run-a", tmp_path / "run-b", tmp_path / "run-c" / "postcontrast" / "5000.json"])
    assert len(scores) == 161
    d = calibration.calibrate(read_labels(tmp_path / "labels.tsv"), scores, "cohort-c Siemens")
    assert d["n"] == 160 and d["n_other_site"] == 1 and d["n_without_score"] == 29
    with pytest.raises(CalibrationError, match="does not exist"):
        calibration.read_scores([tmp_path / "nowhere"])
