# SPDX-License-Identifier: AGPL-3.0-only
"""The report: counts and shapes only. `report.json` for the record's
tooling, `report.md` for the reader; the verdict against §12.2 to §12.4's
bars comes first, the detail after. No value of any field, no code, no
identifier, no path and no UID appears in either."""

from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone

from . import __version__
from .classify import Adjudication
from .fields import FieldStat, Group
from .instances import InstanceReport
from .mapping import EXACT_FIELDS, MULTI_STACK, MULTI_STACK_NOTE, ORDER_DEPENDENT
from .stacks import StackReport
from .subjects import SubjectReport

#: The floor for a field outside `EXACT_FIELDS` and for the multi-stack
#: partitions (§12.3).
FLOOR = 0.999

#: A divergence in one of these classes does not count against v1: the
#: difference is accepted, or v0 is the side that is wrong. `v1-bug` and
#: an unclassified group count in full.
EXCUSED = ("accepted", "v0-bug")


@dataclass
class Bar:
    name: str
    passed: bool
    detail: str


@dataclass
class Report:
    tool: str = f"v0-compare {__version__}"
    when: str = field(default_factory=lambda: datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"))
    v0_origin: str = ""
    v1_backend: str = ""
    cohort: str | None = None
    v0_files: str = ""
    root_given: bool = False
    v1_counts: dict[str, int] = field(default_factory=dict)
    pairs: dict[str, int] = field(default_factory=dict)
    instances: InstanceReport = field(default_factory=InstanceReport)
    stacks: StackReport = field(default_factory=StackReport)
    subjects: SubjectReport = field(default_factory=SubjectReport)
    fields: list[FieldStat] = field(default_factory=list)
    #: the adjudication of the partition and instance classes
    partition_classes: dict[str, str | None] = field(default_factory=dict)
    instance_classes: dict[str, dict[str, str | None]] = field(default_factory=dict)
    bars: list[Bar] = field(default_factory=list)
    unclassified: int = 0

    @property
    def passed(self) -> bool:
        return all(b.passed for b in self.bars)


def adjudicate(rep: Report, adj: Adjudication) -> None:
    """Assign classes to every group; count what stays unclassified; excuse
    what the classes excuse. The file's rule wins; without one, a group of
    an order-dependent column, or of a stack-signature column in
    multi-stack series, is `accepted` with the built-in note."""
    rep.unclassified = 0
    for stat in rep.fields:
        order_dependent = ORDER_DEPENDENT.get((stat.level, stat.field))
        for g in stat.groups:
            rule = adj.divergence(g.level, g.field, g.pattern)
            if rule is not None:
                g.classification, g.note = rule.classification, rule.note
            elif order_dependent is not None:
                g.classification, g.note = "accepted", order_dependent
            elif g.pattern.endswith(MULTI_STACK):
                g.classification, g.note = "accepted", MULTI_STACK_NOTE
            else:
                rep.unclassified += 1
        stat.excuse(EXCUSED)
    rep.stacks.excused = 0
    for pattern, n in rep.stacks.divergent.items():
        rule = adj.partition(pattern)
        rep.partition_classes[pattern] = rule.classification if rule else None
        if rule is None:
            rep.unclassified += 1
        elif rule.classification in EXCUSED:
            rep.stacks.excused += n
    rep.instance_classes = {"v0-only": {}, "v1-only": {}}
    for side, classes in (("v0-only", rep.instances.v0_only), ("v1-only", rep.instances.v1_only)):
        for pattern in classes:
            rule = adj.instance(side, pattern)
            rep.instance_classes[side][pattern] = rule.classification if rule else None
            if rule is None:
                rep.unclassified += 1


def _pct(x: float | None) -> str:
    return "n/a" if x is None else f"{100 * x:.3f}%"


def _instance_bar(rep: Report, side: str, counts: dict[str, int], fails: Callable[[str], bool]) -> tuple[int, int]:
    """The instances of `side` that fail the bar, and how many of those the
    adjudication excuses."""
    failing = {c: n for c, n in counts.items() if fails(c)}
    excused = sum(n for c, n in failing.items() if rep.instance_classes.get(side, {}).get(c) in EXCUSED)
    return sum(failing.values()), excused


def verdict(rep: Report) -> None:
    """The bars of §12.2 to §12.4, each passed or not, with the number
    behind it. A divergence the adjudication classes as accepted or as
    v0's bug is excused from the bar it would fail; a v1 bug and an
    unclassified group are not."""
    bars: list[Bar] = []
    inst = rep.instances
    v0_missing, v0_excused = _instance_bar(rep, "v0-only", inst.v0_only, lambda c: not c.startswith("path in v1, "))
    bars.append(
        Bar(
            "12.2 every v0 instance is in v1 or refused by name",
            v0_missing == v0_excused,
            f"{inst.common:,} of {inst.v0_total:,} matched; {sum(inst.v0_only.values()):,} not in v1, "
            f"{v0_missing:,} of them without a refusal in v1"
            + (f", {v0_excused:,} excused" if v0_excused else ""),
        )
    )
    v1_unexplained, v1_excused = _instance_bar(rep, "v1-only", inst.v1_only, lambda c: c.startswith("unexplained"))
    bars.append(
        Bar(
            "12.2 every v1 instance is in v0 or explained",
            v1_unexplained == v1_excused,
            f"{sum(inst.v1_only.values()):,} v1 instance(s) not in v0, {v1_unexplained:,} unexplained"
            + (f", {v1_excused:,} excused" if v1_excused else ""),
        )
    )
    exact_failed = []
    floor_failed = []
    excused_fields = 0
    for stat in rep.fields:
        a = stat.agreement
        if a is None:
            continue
        if stat.excused:
            excused_fields += 1
        if (stat.level, stat.field) in EXACT_FIELDS:
            if a < 1.0:
                exact_failed.append(f"{stat.level}.{stat.field} {_pct(a)}")
        elif a < FLOOR:
            floor_failed.append(f"{stat.level}.{stat.field} {_pct(a)}")
    excused_note = f" ({excused_fields:,} field(s) with excused rows)" if excused_fields else ""
    bars.append(
        Bar(
            "12.3 the exact fields agree on every row",
            not exact_failed,
            ("all agree" if not exact_failed else "; ".join(exact_failed)) + excused_note,
        )
    )
    bars.append(
        Bar(
            "12.3 every other field agrees on 99.9% of rows",
            not floor_failed,
            ("all above the floor" if not floor_failed else "; ".join(floor_failed)) + excused_note,
        )
    )
    st = rep.stacks
    bars.append(
        Bar(
            "12.3 the stack partition is identical for 99.9% of multi-stack series",
            st.multi == 0 or (st.multi_agreement or 0) >= FLOOR,
            f"{st.multi_identical:,} of {st.multi:,} ({_pct(st.multi_agreement)})"
            + (f", {st.excused:,} excused" if st.excused else ""),
        )
    )
    su = rep.subjects
    bars.append(
        Bar(
            "12.4 every v0 subject code is a v1 subject code",
            su.codes_in_v1 == su.v0_subjects,
            f"{su.codes_in_v1:,} of {su.v0_subjects:,}"
            + (f" ({su.without_common_instance:,} v0 subject(s) without any instance in v1)" if su.without_common_instance else ""),
        )
    )
    bars.append(
        Bar(
            "12.4 every common study hangs off the same code",
            su.studies_same_code == su.studies,
            f"{su.studies_same_code:,} of {su.studies:,}",
        )
    )
    bars.append(
        Bar(
            "12.4 sessions meet v0's events one for one, per-modality events aside",
            su.v1_extra_sessions == 0 and su.v0_extra_events == su.v0_events_surplus,
            f"{su.sessions_matched:,} matched of {su.v1_sessions:,} v1 sessions and {su.v0_events:,} v0 events; "
            f"{su.v0_days_with_several_events:,} v0 day(s) with several events",
        )
    )
    bars.append(
        Bar(
            "12.3 every divergence is classified",
            rep.unclassified == 0,
            "all classified" if rep.unclassified == 0 else f"{rep.unclassified} group(s) unclassified",
        )
    )
    rep.bars = bars


def to_json(rep: Report) -> str:
    return json.dumps({**asdict(rep), "passed": rep.passed}, indent=2, ensure_ascii=False)


def _table(rows: list[list[str]], header: list[str]) -> list[str]:
    out = ["| " + " | ".join(header) + " |", "|" + "|".join("---" for _ in header) + "|"]
    out += ["| " + " | ".join(r) + " |" for r in rows]
    return out


def _cls(c: str | None) -> str:
    return c or "**unclassified**"


def to_markdown(rep: Report) -> str:
    lines: list[str] = []
    lines.append(f"# v0 compare report")
    lines.append("")
    lines.append(f"{rep.tool}, {rep.when}. v0: {rep.v0_origin}; v1: {rep.v1_backend}; "
                 f"cohort: {rep.cohort or 'all'}; v0 file mode: {rep.v0_files}; "
                 f"root: {'given' if rep.root_given else 'every source'}.")
    lines.append("")
    lines.append("## Verdict: " + ("PASS" if rep.passed else "FAIL"))
    lines.append("")
    lines += _table([[("pass" if b.passed else "**fail**"), b.name, b.detail] for b in rep.bars], ["", "bar", "detail"])
    lines.append("")

    inst = rep.instances
    lines.append("## Instances (§12.2)")
    lines.append("")
    lines.append(f"v0 holds {inst.v0_total:,} instance(s) in scope, v1 {inst.v1_total:,}; {inst.common:,} in both.")
    if inst.v0_subjects_in_several_cohorts:
        lines.append(f"{inst.v0_subjects_in_several_cohorts:,} v0 subject(s) in scope are listed under more than one cohort.")
    if inst.fs_checked:
        lines.append(f"{inst.fs_checked:,} path(s) checked on disk.")
    lines.append("")
    if inst.v0_only:
        lines.append("v0 instances not in v1:")
        lines.append("")
        lines += _table(
            [[c, f"{n:,}", _cls(rep.instance_classes.get("v0-only", {}).get(c))] for c, n in inst.v0_only.items()],
            ["class", "instances", "adjudication"],
        )
        lines.append("")
    if inst.v1_only:
        lines.append("v1 instances not in v0:")
        lines.append("")
        lines += _table(
            [[c, f"{n:,}", _cls(rep.instance_classes.get("v1-only", {}).get(c))] for c, n in inst.v1_only.items()],
            ["class", "instances", "adjudication"],
        )
        lines.append("")

    lines.append("## Fields (§12.3)")
    lines.append("")
    lines.append("Per field, over the rows both sides hold: agreement = equal + both null + excused over compared, "
                 "where the excused rows follow a pattern the adjudication classes as accepted or as v0's bug. "
                 "Exact fields are marked with a star.")
    lines.append("")
    level = None
    for stat in rep.fields:
        if stat.level != level:
            if level is not None:
                lines.append("")
            level = stat.level
            lines.append(f"### {level} ({rep.pairs.get(level, 0):,} pairs)")
            lines.append("")
            lines += _table(
                [], ["field", "kind", "compared", "equal", "both null", "one null", "differ", "excused", "agreement"]
            )
        star = "*" if (stat.level, stat.field) in EXACT_FIELDS else ""
        lines.append(
            f"| {stat.field}{star} | {stat.kind} | {stat.compared:,} | {stat.equal:,} | {stat.both_null:,} | "
            f"{stat.one_null:,} | {stat.differ:,} | {stat.excused:,} | {_pct(stat.agreement)} |"
        )
    lines.append("")
    groups: list[Group] = [g for stat in rep.fields for g in stat.groups]
    if groups:
        lines.append("### Divergences by pattern")
        lines.append("")
        lines.append("v0 shape on the left of the arrow, v1 on the right; classed fields show no shape. "
                     "A field whose residual was sampled says so.")
        lines.append("")
        sampled = {(s.level, s.field): s.sampled for s in rep.fields if s.sampled}
        rows = []
        for g in sorted(groups, key=lambda g: (g.level, g.field, -g.count)):
            samples = "; ".join(f"`{a}` ↔ `{b}`" for a, b in g.samples)
            note = f" (sample of {sampled[(g.level, g.field)]:,})" if (g.level, g.field) in sampled else ""
            rows.append([g.level, g.field, g.pattern, f"{g.count:,}{note}", samples, _cls(g.classification), g.note or ""])
        lines += _table(rows, ["level", "field", "pattern", "count", "shapes", "adjudication", "note"])
        lines.append("")

    st = rep.stacks
    lines.append("## Stacks (§12.3)")
    lines.append("")
    lines.append(
        f"{st.series:,} series with common instances; v0 {st.v0_stacks:,} stack(s), v1 {st.v1_stacks:,}, "
        f"{st.pairs:,} matched by membership. Multi-stack series: {st.multi:,}, identical partition: "
        f"{st.multi_identical:,}, excused: {st.excused:,} ({_pct(st.multi_agreement)} together), numbered in the "
        f"same order: {st.multi_same_order:,}. Single-stack series identical: {st.single_identical:,}."
    )
    if st.v0_unstacked or st.v1_unstacked:
        lines.append(f"Common instances without a stack: v0 {st.v0_unstacked:,}, v1 {st.v1_unstacked:,}.")
    lines.append("")
    if st.divergent:
        lines += _table(
            [[p, f"{n:,}", _cls(rep.partition_classes.get(p))] for p, n in st.divergent.items()],
            ["partition", "series", "adjudication"],
        )
        lines.append("")

    su = rep.subjects
    lines.append("## Subjects and sessions (§12.4)")
    lines.append("")
    lines.append(
        f"v0 subjects in scope: {su.v0_subjects:,}; codes present in v1: {su.codes_in_v1:,} "
        f"({su.codes_in_scope:,} under the compared root); v0 subjects without any instance in v1: "
        f"{su.without_common_instance:,}."
    )
    lines.append(
        f"Common studies: {su.studies:,}; same code on both sides: {su.studies_same_code:,}"
        + (
            "; other code: " + ", ".join(f"{n:,} {c}" for c, n in su.studies_other_code.items())
            + f" (touching {su.v0_subjects_touched:,} v0 and {su.v1_subjects_touched:,} v1 subjects; "
            f"{su.v0_subjects_split:,} v0 subject(s) split over several v1 subjects, "
            f"{su.v1_subjects_merged:,} v1 subject(s) holding several v0 subjects)"
            if su.studies_other_code
            else "."
        )
    )
    if su.code_classes:
        lines.append(
            "How v0 derived its codes: "
            + ", ".join(f"{n:,} {c}" for c, n in sorted(su.code_classes.items(), key=lambda x: -x[1]))
            + "."
        )
    lines.append(
        f"Sessions: v1 groups {su.v1_sessions:,} (subject, study date) session(s) over the common studies, "
        f"v0 has {su.v0_events:,} event(s); {su.sessions_matched:,} meet one for one; v1 sessions without an "
        f"event: {su.v1_extra_sessions:,}; v0 events beyond one per session: {su.v0_extra_events:,}, of which "
        f"{su.v0_events_surplus:,} are further events on {su.v0_days_with_several_events:,} day(s) that carry "
        f"several (an event per modality, accepted). "
        f"Events dated unlike the study: {su.v0_event_date_differs:,}; v0 studies without an event: "
        f"{su.v0_studies_without_event:,}; v1 studies without a date: {su.v1_studies_without_date:,}."
    )
    lines.append("")
    return "\n".join(lines) + "\n"
