# SPDX-License-Identifier: AGPL-3.0-only
"""§12.4, the subjects and their sessions: every subject code in v0 is a
subject code in v1, every study both sides hold hangs off the same code,
and v1's studies grouped under the default session scheme (one session per
subject and study date) meet v0's events one for one, except where v0
opened an event per modality. With `--key-file`, how each v0 subject got its
code: the keyed hash of one of its identifiers, v0's fallback hash under the
cohort name, or neither (a CSV-mapped code, or an identifier v0 overwrote).
Counts only; no code, identifier or date leaves the process."""

from __future__ import annotations

import sys
from dataclasses import dataclass, field

import duckdb

from . import keys


@dataclass
class SubjectReport:
    v0_subjects: int = 0
    #: v0 codes that are a subject code somewhere in v1
    codes_in_v1: int = 0
    #: v0 codes that are a subject code under the compared root
    codes_in_scope: int = 0
    #: v0 subjects with no instance in v1 at all (explains an absent code)
    without_common_instance: int = 0
    #: studies both sides hold
    studies: int = 0
    studies_same_code: int = 0
    #: how the studies with another code split: v1 code is/isn't a v0 code
    studies_other_code: dict[str, int] = field(default_factory=dict)
    #: subjects touched by a study under another code, per side
    v0_subjects_touched: int = 0
    v1_subjects_touched: int = 0
    #: subjects whose v0 studies land on more than one v1 subject
    v0_subjects_split: int = 0
    #: v1 subjects holding studies of more than one v0 subject
    v1_subjects_merged: int = 0
    #: `keys.classify_code` class -> subjects
    code_classes: dict[str, int] = field(default_factory=dict)
    # sessions
    v1_sessions: int = 0
    v0_events: int = 0
    sessions_matched: int = 0
    v0_extra_events: int = 0
    v1_extra_sessions: int = 0
    #: v0 (subject, day) pairs with more than one event, and the events
    #: beyond the first on those days
    v0_days_with_several_events: int = 0
    v0_events_surplus: int = 0
    #: v0 events whose date is not the study's date
    v0_event_date_differs: int = 0
    v0_studies_without_event: int = 0
    v1_studies_without_date: int = 0


def _log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def compare(
    con: duckdb.DuckDBPyConnection, cohort: str | None, key: str | None, classify: bool
) -> SubjectReport:
    rep = SubjectReport()
    rep.v0_subjects = con.execute("SELECT count(*) FROM w.v0_subject").fetchone()[0]
    rep.codes_in_v1 = con.execute(
        "SELECT count(*) FROM w.v0_subject WHERE subject_code IN (SELECT code FROM w.subject_all)"
    ).fetchone()[0]
    rep.codes_in_scope = con.execute(
        "SELECT count(*) FROM w.v0_subject WHERE subject_code IN (SELECT code FROM w.subject)"
    ).fetchone()[0]
    rep.without_common_instance = con.execute(
        "SELECT count(*) FROM w.v0_subject WHERE subject_id NOT IN "
        "(SELECT DISTINCT v.subject_id FROM w.v0_instance v "
        " JOIN w.instance b ON b.sop_instance_uid = v.sop_instance_uid)"
    ).fetchone()[0]

    # studies both sides hold, with the code on each side
    con.execute(
        "CREATE OR REPLACE TABLE w.study_pair AS "
        "SELECT a.study_instance_uid, a.study_id AS v0_study_id, b.id AS v1_study_id, "
        "  ua.subject_id AS v0_subject_id, ua.subject_code AS v0_code, ub.id AS v1_subject_id, ub.code AS v1_code, "
        "  a.event_id, norm_date(b.study_date) AS v1_date "
        "FROM v0db.v0.study a "
        "JOIN w.v0_subject ua ON ua.subject_id = a.subject_id "
        "JOIN w.study b ON b.study_instance_uid = a.study_instance_uid "
        "JOIN w.subject ub ON ub.id = b.subject_id"
    )
    rep.studies = con.execute("SELECT count(*) FROM w.study_pair").fetchone()[0]
    rep.studies_same_code = con.execute("SELECT count(*) FROM w.study_pair WHERE v0_code = v1_code").fetchone()[0]
    for known, n in con.execute(
        "SELECT v1_code IN (SELECT subject_code FROM v0db.v0.subject), count(*) FROM w.study_pair "
        "WHERE v0_code <> v1_code GROUP BY 1"
    ).fetchall():
        rep.studies_other_code["v1 code is another v0 subject's" if known else "v1 code unknown to v0"] = n
    rep.v0_subjects_touched = con.execute(
        "SELECT count(DISTINCT v0_subject_id) FROM w.study_pair WHERE v0_code <> v1_code"
    ).fetchone()[0]
    rep.v1_subjects_touched = con.execute(
        "SELECT count(DISTINCT v1_subject_id) FROM w.study_pair WHERE v0_code <> v1_code"
    ).fetchone()[0]
    rep.v0_subjects_split = con.execute(
        "SELECT count(*) FROM (SELECT v0_subject_id FROM w.study_pair GROUP BY 1 HAVING count(DISTINCT v1_subject_id) > 1)"
    ).fetchone()[0]
    rep.v1_subjects_merged = con.execute(
        "SELECT count(*) FROM (SELECT v1_subject_id FROM w.study_pair GROUP BY 1 HAVING count(DISTINCT v0_subject_id) > 1)"
    ).fetchone()[0]

    if classify:
        rows = con.execute(
            "SELECT u.subject_code, list(o.other_identifier) FILTER (WHERE o.other_identifier IS NOT NULL) "
            "FROM w.v0_subject u "
            "LEFT JOIN v0db.v0.subject_other_identifiers o ON o.subject_id = u.subject_id GROUP BY 1"
        ).fetchall()
        for code, identifiers in rows:
            cls = keys.classify_code(code, list(identifiers or []), key, cohort)
            rep.code_classes[cls] = rep.code_classes.get(cls, 0) + 1
        del rows

    # sessions: v1 groups (code, study date) against v0 events, over the
    # studies both sides hold
    con.execute(
        "CREATE OR REPLACE TABLE w.v1_session AS "
        "SELECT DISTINCT v1_code AS code, v1_date AS day FROM w.study_pair WHERE v1_date IS NOT NULL"
    )
    con.execute(
        "CREATE OR REPLACE TABLE w.v0_event AS "
        "SELECT DISTINCT p.v0_code AS code, e.event_id, norm_date(e.event_date) AS day "
        "FROM w.study_pair p JOIN v0db.v0.event e ON e.event_id = p.event_id"
    )
    rep.v1_sessions = con.execute("SELECT count(*) FROM w.v1_session").fetchone()[0]
    rep.v0_events = con.execute("SELECT count(*) FROM w.v0_event").fetchone()[0]
    rep.v1_studies_without_date = con.execute(
        "SELECT count(*) FROM w.study_pair WHERE v1_date IS NULL"
    ).fetchone()[0]
    rep.v0_studies_without_event = con.execute(
        "SELECT count(*) FROM w.study_pair WHERE event_id IS NULL OR event_id NOT IN "
        "(SELECT event_id FROM v0db.v0.event)"
    ).fetchone()[0]
    several = con.execute(
        "SELECT count(*), coalesce(sum(n - 1), 0) FROM "
        "(SELECT code, day, count(*) AS n FROM w.v0_event GROUP BY 1, 2 HAVING count(*) > 1)"
    ).fetchone()
    rep.v0_days_with_several_events, rep.v0_events_surplus = several[0], int(several[1])
    rep.v0_event_date_differs = con.execute(
        "SELECT count(*) FROM w.study_pair p JOIN v0db.v0.event e ON e.event_id = p.event_id "
        "WHERE norm_date(e.event_date) IS DISTINCT FROM p.v1_date"
    ).fetchone()[0]
    rep.sessions_matched = con.execute(
        "SELECT count(*) FROM w.v1_session s WHERE EXISTS "
        "(SELECT 1 FROM w.v0_event e WHERE e.code = s.code AND e.day = s.day)"
    ).fetchone()[0]
    # one for one: a matched session accounts for one event; the rest of
    # the events are v0's extra ones (a second event on a day, or an event
    # under another code or date)
    rep.v1_extra_sessions = rep.v1_sessions - rep.sessions_matched
    rep.v0_extra_events = rep.v0_events - rep.sessions_matched
    _log(
        f"subjects: {rep.v0_subjects:,} in v0, {rep.codes_in_v1:,} codes in v1; "
        f"studies {rep.studies:,} common, {rep.studies_same_code:,} same code; "
        f"sessions v1 {rep.v1_sessions:,}, v0 events {rep.v0_events:,}"
    )
    return rep
