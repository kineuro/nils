# SPDX-License-Identifier: AGPL-3.0-only
"""What model r1's fusion reads beside the image, made from the manifest as
model r1's frame made it from the registry (the study's ``w_base.py``,
``m_table.py`` and ``pcr4.py table3``):

- **who is a candidate:** an acquired (disposition acquisition or
  working_scan, not DERIVED), non-localizer MR stack of family T1-type
  (base T1w, technique not TOF or PC, not a DSC, DCE or contrast MR
  angiography) whose body_part says brain;
- **the header columns:** the contrast tag (ContrastBolusAgent filled and
  not a word for none), the rules' positive words, the rules' answer, and
  over the session's acquired non-localizer stacks: any and all of them
  tagged, and, where the session's clock is trusted, whether the stack was
  acquired at or after the first tagged one, or before it;
- **the session's time trust:** every series has a time, none at exactly
  midnight, a span of 2 minutes to 4 hours, at least half the times
  distinct, time and series number in order (Spearman 0.8 or more where it
  can be computed), at least two series, and not a cohort whose clock the
  fusion's file names untrusted;
- **the site:** the calibration's ``<cohort> <make>``.
"""

from __future__ import annotations

import re

import numpy as np

AGENT_NONE = re.compile(r"^\s*(none|no|nej|ingen|0|-+|n/?a|no contrast|non)\s*$", re.IGNORECASE)
ANGIO = re.compile(r"(angio|\bmra\b|ce.?mra|care.?bolus|bolus.?track|\btricks\b|\btwist\b|contrast.?enhanced.?mr.?a|fl3d.?ce|keyhole|\bmip\b)", re.IGNORECASE)
TOFPC = re.compile(r"(\btof\b|time.?of.?flight|inflow|\bpca?\b|phase.?contrast|qflow)", re.IGNORECASE)
GRE = {"FLASH", "GRE", "VIBE", "FISP", "TurboFLASH", "Unknown"}
# arm T's domain (model r1's m_table.py): 3D gradient-echo T1 by technique
T_DOMAIN = {"MPRAGE", "MEMPRAGE", "MP2RAGE", "FLASH", "TurboFLASH", "VIBE", "GRE"}


def _values(e, axis: str) -> list:
    rows = (e.header.get("classification") or {}).get(axis) or []
    return [r.get("value") for r in rows if isinstance(r, dict) and r.get("value") is not None]


def axis_max(e, axis: str):
    """The axis as the study's extract read it: max(value)."""
    v = _values(e, axis)
    return max(v) if v else None


def axis_agg(e, axis: str) -> str:
    """A many-valued axis as the study's extract read it: sorted, joined by '+'."""
    return "+".join(sorted(_values(e, axis)))


def contrast(e) -> dict:
    return e.contrast if isinstance(e.contrast, dict) else {}


def make_of(e) -> str:
    mk = str((e.header.get("fingerprint") or {}).get("manufacturer") or "").lower()
    if "siemens" in mk:
        return "Siemens"
    if "ge " in mk or mk.startswith("ge") or "general electric" in mk:
        return "GE"
    if "philips" in mk:
        return "Philips"
    if re.search("toshiba|canon", mk):
        return "Canon/Toshiba"
    return "other"


def agent(e) -> bool:
    a = contrast(e).get("contrast_bolus_agent")
    a = "" if a is None else str(a)
    return a.strip() != "" and not AGENT_NONE.match(a)


def tsec(e):
    """Seconds since midnight of the stack's acquisition time, else None."""
    t = contrast(e).get("acquisition_time")
    if not t:
        return None
    m = re.match(r"^\s*(\d{1,2}):(\d{2}):(\d{2})(\.\d+)?\s*$", str(t))
    if not m:
        m = re.match(r"^\s*(\d{2})(\d{2})(\d{2})(\.\d+)?\s*$", str(t))
        if not m:
            return None
    return int(m.group(1)) * 3600 + int(m.group(2)) * 60 + int(m.group(3)) + (float(m.group(4)) if m.group(4) else 0.0)


def acquired(e) -> bool:
    """MR, acquired, not DERIVED, not a localizer (the study's frame)."""
    mod = e.modality or (e.header.get("fingerprint") or {}).get("modality")
    if str(mod or "").upper() != "MR":
        return False
    if axis_max(e, "disposition") not in ("acquisition", "working_scan"):
        return False
    if contrast(e).get("derived") is True:
        return False
    return axis_max(e, "directory_type") != "localizer"


def family(e) -> str:
    """Model r1's family (w_base.py): DSC/DCE, CE-MRA, T1-type, FLAIR or other."""
    c = contrast(e)
    tech = axis_max(e, "technique") or ""
    base = axis_max(e, "base")
    mod = axis_agg(e, "modifier")
    rules = set(c.get("post_contrast_rules") or [])
    r_perf = "perfusion" in rules
    text = str(c.get("text_all_ci") or "")
    angio = bool(ANGIO.search(text))
    tofpc = bool(TOFPC.search(text))
    flag = str(c.get("angio_flag") or "").upper() == "Y"
    acq = str(c.get("mr_acquisition_type") or "")
    is_dsc = tech == "Perfusion" or (r_perf and tech not in ("ASL", "DCE"))
    is_dce = tech == "DCE"
    is_cemra = (tech not in ("TOF", "PC") and not tofpc and acq == "3D" and tech in GRE and (angio or flag)
                and base not in ("T2w", "DWI", "SWI", "T2*w", "PDw", "PWI") and not is_dce and not is_dsc)
    if is_dsc or is_dce:
        return "DSC/DCE"
    if is_cemra:
        return "CE-MRA"
    if base == "T1w" and tech not in ("TOF", "PC"):
        return "T1-type"
    if "FLAIR" in mod and tech not in ("TOF", "PC"):
        return "FLAIR"
    return "other"


def brain(e) -> bool:
    bp = axis_max(e, "body_part")
    return bp is not None and "brain" in str(bp)


def _cohort_candidates(e) -> list:
    """The names a stack's calibration cohort is looked for under, each
    with where it comes from, in the order ``cohort_of`` tries them: the
    datasets holding its files, its first ingest batch, the batch's
    dash-suffixes, its subject's open cohorts."""
    cands = [(str(d), "dataset") for d in (e.header.get("datasets") or [])]
    b = e.header.get("batch")
    if isinstance(b, str) and b:
        cands.append((b, "batch"))
        parts = b.split("-")
        cands += [("-".join(parts[i:]), "batch_part") for i in range(1, len(parts))]
    cands += [(c, "cohort") for c in sorted(str(c) for c in e.header.get("cohorts") or [])]
    return cands


def cohort_candidates(e) -> list:
    """The names of ``_cohort_candidates``, each once, in the order
    ``cohort_of`` tries them: what a site builds its key from
    (``<name> <make>``) when the fusion knows none of them."""
    out: list = []
    for c, _ in _cohort_candidates(e):
        if c not in out:
            out.append(c)
    return out


def cohort_of(e, fusion) -> tuple:
    """The stack's calibration cohort and where it was found: a dataset
    holding its files (through the fusion's aliases), its first ingest
    batch, the batch's dash-suffixes, its subject's open cohorts."""
    known = {k.rsplit(" ", 1)[0] for k in fusion.sites()} | set(fusion.cohort_aliases.values())
    al = fusion.cohort_aliases
    for c, where in _cohort_candidates(e):
        c2 = al.get(c, c)
        if c2 in known:
            return c2, where
    return None, None


def raw_columns(rows: dict, by_id: dict, sid: int) -> dict:
    """The raw-score mode's site columns of a stack's table (0.3.0), the
    same in the cascade's and the full pipeline's: its site key as the
    fusion sees it (null where its cohort is none the fusion knows), its
    make as the key spells it, and the cohort candidates the key could be
    built from, space separated, in the order ``cohort_of`` tries them."""
    e = by_id[sid]
    return {"site": rows[sid]["site"], "make": make_of(e), "cohort_candidates": " ".join(cohort_candidates(e)) or None}


def _spearman(x, y) -> float:
    def rank(a):
        a = np.asarray(a, float)
        order = np.argsort(a, kind="mergesort")
        r = np.empty(len(a))
        sa = a[order]
        i = 0
        while i < len(a):
            j = i
            while j + 1 < len(a) and sa[j + 1] == sa[i]:
                j += 1
            r[order[i : j + 1]] = (i + j) / 2.0 + 1
            i = j + 1
        return r

    rx, ry = rank(x), rank(y)
    rx, ry = rx - rx.mean(), ry - ry.mean()
    d = np.sqrt((rx * rx).sum() * (ry * ry).sum())
    return float((rx * ry).sum() / d) if d > 0 else float("nan")


def time_trusted(series: list, cohort, untrusted: list) -> bool:
    """``series`` is [(time or None, series number or None)] of the session's
    acquired non-localizer series, each its stacks' earliest time."""
    k = len(series)
    tt = [t for t, _ in series if t is not None]
    full = len(tt) == k
    midnight = any(t % 86400 == 0 for t in tt)
    span = (max(tt) - min(tt)) if tt else 0
    distinct = len(set(tt)) / k if k else 0
    ok = [(t, n) for t, n in series if t is not None and n is not None]
    rho = float("nan")
    if len(ok) >= 3 and len({t for t, _ in ok}) > 1 and len({n for _, n in ok}) > 1:
        rho = _spearman([t for t, _ in ok], [n for _, n in ok])
    return bool(full and not midnight and 120 <= span <= 4 * 3600 and distinct >= 0.5 and (rho != rho or rho >= 0.8) and k >= 2
                and cohort not in untrusted)


def rows_for(stacks: list, context: list, fusion) -> dict:
    """Each scored stack's fusion row: the header columns, the site, brain,
    the (absent) pair tier, and the facts behind them, by stack id."""
    by_study: dict = {}
    for e in list(stacks) + list(context):
        if e.study is None or e.sealed:
            continue
        by_study.setdefault(e.study, {})[e.stack_id] = e
    sess: dict = {}
    for study, members in by_study.items():
        B = [e for e in members.values() if acquired(e)]
        ag = [agent(e) for e in B]
        times = [tsec(e) for e in B]
        tagged = [t for t, a in zip(times, ag) if a and t is not None]
        first = min(tagged) if tagged else None
        ser: dict = {}
        for e, t in zip(B, times):
            key = e.series_id if e.series_id is not None else ("stack", e.stack_id)
            n = contrast(e).get("series_number")
            if key not in ser:
                ser[key] = [t, n]
            else:
                if t is not None and (ser[key][0] is None or t < ser[key][0]):
                    ser[key][0] = t
                if ser[key][1] is None:
                    ser[key][1] = n
        coh = None
        for e in B:
            coh, _ = cohort_of(e, fusion)
            break
        trusted = time_trusted([tuple(v) for v in ser.values()], coh, fusion.untrusted_time_cohorts) if B else False
        sess[study] = {"any": any(ag) if B else False, "all": all(ag) if B else False, "first": first, "trusted": trusted, "n": len(B)}
    out = {}
    for e in stacks:
        s = sess.get(e.study, {"any": False, "all": False, "first": None, "trusted": False, "n": 0})
        t = tsec(e)
        tr = bool(s["trusted"])
        coh, where = cohort_of(e, fusion)
        pc = axis_max(e, "post_contrast")
        rules = set(contrast(e).get("post_contrast_rules") or [])
        out[e.stack_id] = {
            "h_agent": float(agent(e)),
            "h_words": float("positive" in rules),
            "h_rules_given": float(str(pc) in ("1", "t", "given")),
            "h_ses_any_agent": float(s["any"]),
            "h_ses_all_agent": float(s["all"]),
            "h_at_or_after_first_tag": float(tr and s["first"] is not None and t is not None and t >= s["first"]),
            "h_before_first_tag": float(tr and s["first"] is not None and t is not None and t < s["first"]),
            "h_time_trusted": float(tr),
            "has_pair": 0.0,
            "site": f"{coh} {make_of(e)}" if coh else None,
            "cohort_from": where,
            "brain": brain(e),
            "family": family(e),
            "acquired": acquired(e),
            "acq3d": float(str(contrast(e).get("mr_acquisition_type") or "") == "3D"),
            "t_domain": bool(str(contrast(e).get("mr_acquisition_type") or "") == "3D" and axis_max(e, "technique") in T_DOMAIN),
            "thick": (e.header.get("fingerprint") or {}).get("slice_thickness"),
        }
    return out
