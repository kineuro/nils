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

from nils_postcontrast import calibration, cli, evidence, manifest
from nils_postcontrast.calibration import CalibrationError, fit_platt, load_pairs, read_labels
from nils_postcontrast.fusion import Fusion

A, B = 0.8, -0.5
FUSION = "sha256:" + "f" * 64


def fusion() -> Fusion:
    return Fusion(cols=["x"], mean=np.zeros(1), scale=np.ones(1), coef=np.ones(1), intercept=0.0, cal={"cohort-a Siemens": (1.0, 0.0)}, t=0.9,
                  use_V=True, cohort_aliases={}, untrusted_time_cohorts=[])


def write_run(tmp: Path, n: int, seed: int = 0, site=None, start: int = 1, make: str = "Siemens", candidates=("cohort-c",)) -> dict:
    """A raw-score run's output folder: n tables with z drawn wide and the
    label each z would get under the known pair, by stack id; ``candidates``
    is written as the run writes the column, a JSON list in the text column,
    and None writes an older table with no cohort_candidates column."""
    rng = np.random.default_rng(seed)
    z = rng.normal(0.0, 3.0, n)
    y = (rng.random(n) < 1 / (1 + np.exp(-(A * z + B)))).astype(int)
    folder = tmp / "postcontrast"
    folder.mkdir(parents=True, exist_ok=True)
    labels = {}
    for i in range(n):
        sid = start + i
        doc = {"stack_id": sid, "p_given": None, "value": None, "reason": "no-calibration", "score_raw": round(float(z[i]), 6), "site": site,
               "make": make, "cohort_candidates": json.dumps(list(candidates)) if candidates is not None else None, "fusion_digest": FUSION,
               "cascade_digest": "sha256:" + "c" * 64}
        if candidates is None:
            del doc["cohort_candidates"]
        (folder / f"{sid}.json").write_text(json.dumps(doc))
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
    assert load_pairs(one) == ({"cohort-c Siemens": (0.7, -0.2)}, False)
    many = tmp_path / "many.json"
    many.write_text(json.dumps({"cohort-c Siemens": [0.7, -0.2], "cohort-d GE": {"a": 2.0, "b": 0.5}}))
    G, sites, checked = calibration.apply(fusion(), many)
    assert sites == ["cohort-c Siemens", "cohort-d GE"] and checked is False  # a map names no fusion
    assert G.cal == {"cohort-a Siemens": (1.0, 0.0), "cohort-c Siemens": (0.7, -0.2), "cohort-d GE": (2.0, 0.5)} and G.sites() == set(G.cal)
    # a site the frozen fusion is certified for is never replaced: the file is refused by one line naming the key
    many.write_text(json.dumps({"cohort-c Siemens": [0.7, -0.2], "cohort-a Siemens": {"a": 2.0, "b": 0.5}}))
    with pytest.raises(CalibrationError, match="no site file replaces: cohort-a Siemens") as e:
        calibration.apply(fusion(), many)
    assert "\n" not in str(e.value)
    one.write_text(json.dumps({"site": "cohort-a Siemens", "a": 0.7, "b": -0.2}))
    with pytest.raises(CalibrationError, match="cohort-a Siemens"):
        calibration.for_run(fusion(), str(one), {})
    # a map's value is [a, b], two numbers, or a pair file's object; a string
    # of two digits, three numbers, booleans or a bare number are refused by
    # one line naming the key
    for bad in ("not json", "{}", json.dumps({"cohort-c Siemens": [1.0]}), json.dumps({"cohort-c Siemens": ["a", 0.0]}),
                json.dumps({"cohort-c Siemens": [float("nan"), 0.0]}).replace("NaN", "1e999"),
                json.dumps({"cohort-c Siemens": "12"}), json.dumps({"cohort-c Siemens": [1.0, 0.0, 2.0]}), json.dumps({"cohort-c Siemens": [True, 0.0]}),
                json.dumps({"cohort-c Siemens": 1.0}), json.dumps({"cohort-c Siemens": {"a": 1.0}}),
                json.dumps({"site": ["cohort-c", "Siemens"], "a": 0.7, "b": -0.2}), json.dumps({"site": 7, "a": 0.7, "b": -0.2})):
        (tmp_path / "bad.json").write_text(bad)
        with pytest.raises(CalibrationError) as e:
            load_pairs(tmp_path / "bad.json")
        assert "\n" not in str(e.value)
        if "cohort-c Siemens" in bad and "site" not in bad:
            assert "cohort-c Siemens" in str(e.value)


def test_a_pair_file_fitted_on_another_fusion_is_refused_and_one_naming_none_is_unchecked(tmp_path):
    fitted = tmp_path / "fitted.json"
    fitted.write_text(json.dumps({"site": "cohort-c Siemens", "a": 0.7, "b": -0.2, "model": {"fusion_digest": FUSION}}))
    assert load_pairs(fitted) == ({"cohort-c Siemens": (0.7, -0.2)}, False)  # with no digest to hold it to
    assert load_pairs(fitted, FUSION) == ({"cohort-c Siemens": (0.7, -0.2)}, True)
    G, sites, checked = calibration.apply(fusion(), fitted, FUSION)
    assert sites == ["cohort-c Siemens"] and checked is True
    other = "sha256:" + "e" * 64
    with pytest.raises(CalibrationError, match="another fusion"):
        calibration.apply(fusion(), fitted, other)
    with pytest.raises(CalibrationError, match="another fusion"):
        load_pairs(fitted, other)
    # a map whose values are pair files is held to its digests too
    many = tmp_path / "many.json"
    many.write_text(json.dumps({"cohort-c Siemens": {"a": 0.7, "b": -0.2, "model": {"fusion_digest": FUSION}}, "cohort-d GE": [1.0, 0.0]}))
    assert calibration.apply(fusion(), many, FUSION)[2] is True
    with pytest.raises(CalibrationError, match="another fusion"):
        calibration.apply(fusion(), many, other)
    extra: dict = {}
    calibration.for_run(fusion(), str(fitted), extra, FUSION)
    assert extra == {"calibration": {"sites": {"cohort-c Siemens": 0}}}
    extra = {}
    calibration.for_run(fusion(), str(many), extra)
    assert extra == {"calibration": {"sites": {"cohort-c Siemens": 0, "cohort-d GE": 0}, "unchecked": True}}


def entry(sid, datasets=(), batch=None):
    return manifest.Entry(sid, f"stack-{sid}", sid + 10, "MR", {"study": 7, "sealed": False},
                          {"fingerprint": {"manufacturer": "SIEMENS"}, "classification": {}, "datasets": list(datasets), "batch": batch}, {})


def test_a_merged_file_never_reroutes_a_stack_the_frozen_fusion_calibrates(tmp_path):
    F = fusion()
    many = tmp_path / "many.json"
    many.write_text(json.dumps({"cohort-c Siemens": [1.0, 0.0]}))
    G, _, _ = calibration.apply(F, many)
    assert G.frozen_sites() == {"cohort-a Siemens"} and G.sites() == {"cohort-a Siemens", "cohort-c Siemens"} and F.frozen_sites() == F.sites()
    # a stack held by a dataset named as the file's cohort, ingested in a batch of a fusion cohort: the fusion's key, as before the file
    e = entry(1, datasets=("cohort-c",), batch="reread-cohort-a")
    assert evidence.cohort_of(e, F) == ("cohort-a", "batch_part") and evidence.cohort_of(e, G) == ("cohort-a", "batch_part")
    assert evidence.rows_for([e], [], G)[1]["site"] == "cohort-a Siemens"
    # a stack the frozen fusion names nothing of is the file's
    e = entry(2, datasets=("cohort-c",), batch="dcm-raw-2026")
    assert evidence.cohort_of(e, F) == (None, None) and evidence.cohort_of(e, G) == ("cohort-c", "dataset")
    # merging a second file keeps the frozen sites the frozen fusion's
    H, _, _ = calibration.apply(G, many)
    assert H.frozen_sites() == {"cohort-a Siemens"}
    # the candidates a run writes are alias-resolved, as the site key is, and the pools are computed once per run
    A = Fusion(cols=["x"], mean=np.zeros(1), scale=np.ones(1), coef=np.ones(1), intercept=0.0, cal={"cohort-a Siemens": (1.0, 0.0)}, t=0.9,
               use_V=True, cohort_aliases={"old-name": "cohort-a"}, untrusted_time_cohorts=[])
    e = entry(3, datasets=("old-name", "cohort-c"), batch="dcm-raw-2026")
    row = evidence.rows_for([e], [], A)[3]
    assert row["site"] == "cohort-a Siemens" and row["cohort_candidates"] == ["cohort-a", "cohort-c", "dcm-raw-2026", "raw-2026", "2026"]
    assert evidence.cohort_candidates(e) == ["old-name", "cohort-c", "dcm-raw-2026", "raw-2026", "2026"]
    assert evidence.cohort_pools(A) == ({"cohort-a"}, set()) and evidence.cohort_of(e, A, evidence.cohort_pools(A)) == ("cohort-a", "dataset")


def test_the_results_count_the_stacks_each_pair_calibrated_and_name_keys_that_matched_no_stack():
    extra = {"calibration": {"sites": {"cohort-c Siemens": 0, "cohort-d GE": 0}}}
    sites_of = {1: "cohort-a Siemens", 2: "cohort-c Siemens", 3: "cohort-c Siemens", 4: "cohort-c Siemens", 5: None}
    tables = {1: {"p_given": 0.9}, 2: {"p_given": 0.8}, 3: {"p_given": 0.2}, 4: {"p_given": None, "reason": "deferred"}, 5: {"p_given": None}}
    calibration.record(extra, sites_of, tables)
    assert extra == {"calibration": {"sites": {"cohort-c Siemens": 2, "cohort-d GE": 0}}}
    extra = {"calibration": {"sites": {"cohort-z Siemens": 0}, "unchecked": True}}
    calibration.record(extra, sites_of, tables)
    assert extra == {"calibration": {"sites": {"cohort-z Siemens": 0}, "unchecked": True, "matched_none": ["cohort-z Siemens"]}}
    extra = {}
    calibration.record(extra, sites_of, tables)
    assert extra == {}


def test_the_run_reads_the_site_file_the_engine_mounted(tmp_path, monkeypatch):
    f = tmp_path / "pairs.json"
    f.write_text(json.dumps({"cohort-c Siemens": [1.0, 0.0]}))
    extra: dict = {}
    assert calibration.for_run(fusion(), None, extra).cal == {"cohort-a Siemens": (1.0, 0.0)} and extra == {}
    monkeypatch.setenv(calibration.ENV, str(f))
    G = calibration.for_run(fusion(), None, extra)
    assert "cohort-c Siemens" in G.cal and extra == {"calibration": {"sites": {"cohort-c Siemens": 0}, "unchecked": True}}
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
    a, b, fit = fit_platt(z, y)
    assert abs(a - A) < 0.08 and abs(b - B) < 0.1, (a, b)
    assert fit["converged"] is True and 0 < fit["iterations"] <= 200
    sep_z, sep_y = np.array([-3.0, -2.0, -1.0, 1.0, 2.0, 3.0]), np.array([0, 0, 0, 1, 1, 1.0])
    a, b, fit = fit_platt(sep_z, sep_y)
    assert math.isfinite(a) and math.isfinite(b) and a > 0 and isinstance(fit["converged"], bool)
    # the fit never adopts a step that raises the loss: every budget of
    # iterations ends at or below the loss of the pair before it
    def loss(a, b):
        p = 1 / (1 + np.exp(-(a * z + b)))
        npos, nneg = y.sum(), (1 - y).sum()
        t = np.where(y > 0.5, (npos + 1) / (npos + 2), 1 / (nneg + 2))
        return float(np.mean(-(t * np.log(p) + (1 - t) * np.log(1 - p))))
    losses = [loss(*fit_platt(z, y, iters=k)[:2]) for k in range(0, 8)]
    assert all(l2 <= l1 + 1e-12 for l1, l2 in zip(losses, losses[1:])), losses
    assert fit_platt(z, y, iters=0) == (0.0, 0.0, {"iterations": 0, "converged": False})


def test_calibrate_fits_the_sites_pair_from_a_runs_raw_scores_and_reports_per_source(tmp_path, capsys):
    labels = write_run(tmp_path / "run", 2000)
    # three labelled stacks no run scored
    write_labels(tmp_path / "labels.tsv", labels, extra_rows=["9001\tgiven\theader", "9002\tnot_given\tname", "9003\tgiven\tmanual"])
    out = tmp_path / "pair.json"
    # the make word is taken without case and written as the tables spell it
    rc = cli.main(["calibrate", "--labels", str(tmp_path / "labels.tsv"), "--scores", str(tmp_path / "run"), "--site", "cohort-c siemens",
                   "--out", str(out)])
    assert rc == 0
    d = json.loads(out.read_text())
    assert d["site"] == "cohort-c Siemens" and abs(d["a"] - A) < 0.15 and abs(d["b"] - B) < 0.2, (d["a"], d["b"])
    assert d["n"] == 2000 and d["n_given"] + d["n_not_given"] == 2000 and d["n_without_score"] == 3 and d["n_other_site"] == 0
    assert d["n_other_make"] == 0 and d["n_other_cohort"] == 0 and d["n_without_candidates"] == 0
    assert d["fit"]["converged"] is True and d["fit"]["iterations"] > 0
    assert set(d["per_source"]) == {"header", "name", "manual"} and sum(m["n"] for m in d["per_source"].values()) == 2000
    for m in list(d["per_source"].values()) + [d["overall"]]:
        assert 0 < m["log_loss"] < math.log(2) and 0.5 < m["accuracy"] <= 1
    assert d["model"]["fusion_digest"] == FUSION and d["model"]["version"] == cli.__version__ and d["fitted_at"].endswith("Z")
    assert set(d) == {"site", "a", "b", "n", "n_given", "n_not_given", "n_without_score", "n_other_site", "n_other_make", "n_other_cohort",
                      "n_without_candidates", "fit", "per_source", "overall", "model", "fitted_at"}
    text = capsys.readouterr().out
    assert "site cohort-c Siemens" in text and "3 labelled with no score" in text and "header:" in text and "manual:" in text
    assert "0 of another make, 0 of another cohort, 0 from a table naming no cohort candidates" in text and "converged" in text
    # the pair file is a calibration file a run takes as it is
    assert load_pairs(out) == ({"cohort-c Siemens": (d["a"], d["b"])}, False)
    # a make word the fusion does not know is refused, and nothing is written
    rc = cli.main(["calibrate", "--labels", str(tmp_path / "labels.tsv"), "--scores", str(tmp_path / "run"), "--site", "cohort-c Bruker",
                   "--out", str(tmp_path / "none.json")])
    assert rc == 1 and "none the fusion knows" in capsys.readouterr().err and not (tmp_path / "none.json").exists()


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


def test_calibrate_takes_several_runs_and_keeps_another_sites_makes_and_cohorts_rows_out(tmp_path):
    la = write_run(tmp_path / "run-a", 80, seed=5, start=1)
    lb = write_run(tmp_path / "run-b", 80, seed=6, start=1000)
    lc = write_run(tmp_path / "run-c", 30, seed=7, site="cohort-a Siemens", start=5000)
    # the same cohort on another make: its rows are not this key's
    ld = write_run(tmp_path / "run-d", 10, seed=8, start=7000, make="GE")
    # stacks of another cohort (their candidates never name the key's), and an older run's tables with no candidates column
    le = write_run(tmp_path / "run-e", 12, seed=9, start=8000, candidates=("cohort-d", "batch-2026-cohort-d"))
    lf = write_run(tmp_path / "run-f", 8, seed=10, start=9000, candidates=None)
    # stacks whose table names the key as its site (a cohort the fusion reached through an alias, say): the site proves the
    # cohort, whatever the candidates say
    lg = write_run(tmp_path / "run-g", 6, seed=11, start=9500, site="cohort-c Siemens", candidates=("other-name",))
    labels = {**la, **lb, **lc, **ld, **le, **lf, **lg}
    write_labels(tmp_path / "labels.tsv", labels)
    scores = calibration.read_scores([tmp_path / "run-a", tmp_path / "run-b", tmp_path / "run-c" / "postcontrast" / "5000.json", tmp_path / "run-d",
                                      tmp_path / "run-e", tmp_path / "run-f", tmp_path / "run-g"])
    assert len(scores) == 197 and scores[7000]["make"] == "GE" and scores[1]["make"] == "Siemens"
    assert scores[1]["cohort_candidates"] == ["cohort-c"] and scores[8000]["cohort_candidates"] == ["cohort-d", "batch-2026-cohort-d"]
    assert scores[9000]["cohort_candidates"] is None
    d = calibration.calibrate(read_labels(tmp_path / "labels.tsv"), scores, "cohort-c Siemens")
    assert d["n"] == 174 and d["n_other_site"] == 1 and d["n_without_score"] == 29 and d["n_other_make"] == 10
    assert d["n_other_cohort"] == 12 and d["n_without_candidates"] == 8
    assert "12 of another cohort, 8 from a table naming no cohort candidates" in calibration.report(d)
    # the make is compared without case and written as the tables spell it; a key without a make, or with a make the fusion
    # does not know, is refused
    d2 = calibration.calibrate(read_labels(tmp_path / "labels.tsv"), scores, "cohort-c siemens")
    assert d2["n"] == 174 and d2["site"] == "cohort-c Siemens"
    assert calibration.site_key(" cohort-d canon/toshiba ") == "cohort-d Canon/Toshiba" and calibration.site_key("cohort-d OTHER") == "cohort-d other"
    with pytest.raises(CalibrationError, match="<cohort> <make>"):
        calibration.calibrate(read_labels(tmp_path / "labels.tsv"), scores, "cohort-c")
    with pytest.raises(CalibrationError, match="none the fusion knows"):
        calibration.calibrate(read_labels(tmp_path / "labels.tsv"), scores, "cohort-c Bruker")
    with pytest.raises(CalibrationError, match="does not exist"):
        calibration.read_scores([tmp_path / "nowhere"])


def test_a_cohort_name_with_a_space_survives_the_candidates_column(tmp_path):
    labels = write_run(tmp_path / "run", 120, seed=12, candidates=("my cohort", "batch 2026"))
    write_labels(tmp_path / "labels.tsv", labels)
    scores = calibration.read_scores([tmp_path / "run"])
    assert scores[1]["cohort_candidates"] == ["my cohort", "batch 2026"]
    d = calibration.calibrate(read_labels(tmp_path / "labels.tsv"), scores, "my cohort Siemens")
    assert d["site"] == "my cohort Siemens" and d["n"] == 120 and d["n_other_cohort"] == 0
    with pytest.raises(CalibrationError, match="none the fusion knows"):
        calibration.calibrate(read_labels(tmp_path / "labels.tsv"), scores, "my cohort")  # the last word is the make


def test_a_table_that_is_not_a_runs_is_refused_by_its_path_and_an_unwritable_pair_file_by_one_line(tmp_path, capsys):
    labels = write_run(tmp_path / "run", 60, seed=13)
    write_labels(tmp_path / "labels.tsv", labels)
    folder = tmp_path / "run" / "postcontrast"
    for bad in ({"stack_id": "seven", "score_raw": 1.0}, {"stack_id": None, "score_raw": 1.0}, {"stack_id": 7.0, "score_raw": 1.0},
                {"stack_id": True, "score_raw": 1.0}, {"stack_id": 9, "score_raw": "high"}, {"stack_id": 9, "score_raw": 1.0, "cohort_candidates": "a b"},
                {"stack_id": 9, "score_raw": 1.0, "cohort_candidates": "[1, 2]"}):
        (folder / "9.json").write_text(json.dumps(bad))
        with pytest.raises(CalibrationError) as e:
            calibration.read_scores([tmp_path / "run"])
        msg = str(e.value)
        assert "\n" not in msg and str(folder / "9.json") in msg and "seven" not in msg and "high" not in msg, msg
    (folder / "9.json").write_text(json.dumps({"stack_id": 9, "score_raw": 1.0, "cohort_candidates": None}))
    assert calibration.read_scores([tmp_path / "run"])[9]["cohort_candidates"] is None
    (folder / "9.json").unlink()
    # the command: a refused table is one line on stderr, exit 1, no traceback
    (folder / "9.json").write_text(json.dumps({"stack_id": "seven", "score_raw": 1.0}))
    rc = cli.main(["calibrate", "--labels", str(tmp_path / "labels.tsv"), "--scores", str(tmp_path / "run"), "--site", "cohort-c Siemens",
                   "--out", str(tmp_path / "pair.json")])
    err = capsys.readouterr().err
    assert rc == 1 and err.count("\n") == 1 and "no integer stack_id" in err and "Traceback" not in err
    (folder / "9.json").unlink()
    # an --out under a file is one line naming the path
    (tmp_path / "wall").write_text("")
    out = tmp_path / "wall" / "pair.json"
    rc = cli.main(["calibrate", "--labels", str(tmp_path / "labels.tsv"), "--scores", str(tmp_path / "run"), "--site", "cohort-c Siemens",
                   "--out", str(out)])
    err = capsys.readouterr().err
    assert rc == 1 and err.count("\n") == 1 and str(out) in err and "cannot be written" in err
