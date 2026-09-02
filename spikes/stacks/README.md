# Stacks: does v1 partition a series the way v0 does?

**Question.** Wave 1 slice 5 (`docs/specs/wave1-parse-and-digest.md`, §8 and §14
item 5) is done when the stack partitions on the spike's corpora equal v0's for
the series both hold. v1 computes the signature in Rust from its own extraction;
v0 computed it in Python from pydicom's. Do the two split the same series into
the same stacks?

**Criteria, written before the run.** Over every series both registries hold,
restricted to the instances both hold: the share of series whose partitions are
equal, and for the rest, what differs, classified by the signature field that
caused it. Equality is the partition, not the index: `stack_index` is the order
of first appearance and the walk order differs (§8). A difference is accepted
only if it traces to a divergence the specification already lists (trailing
spaces, empty text, an integer with a decimal point) or to a v0 reading the
specification names as wrong; anything else is a bug in slice 5.

**Method.** `v0_partition.py` reads every file of a tree with pydicom the way
v0's worker did and writes each instance's v0 signature to a file that stays on
the host. `compare.py` joins it with v1's registry on the SOP instance UID and
prints counts. The two v0 modules the first script imports (`stack_utils.py`,
`dicom_mappings.py` of v0 0.5.0, MIT) are placed beside it as `v0/` and are not
part of this repository.

**Verdict, nmosd (2026-09-02).** Equal. The two hold 2,165 series in common
and v1's partition equals v0's on every one of them, over the 493,708 instances
both hold: 2,534 v0 groups, 2,534 v1 stacks, 212 series with more than one
stack (at most five), no label. v1 holds no instance v0 does not. v0's reader
(pydicom, `force=True`) opened all 508,045 files; 134 had no series or SOP
instance UID (the 124 that are not DICOM and the 10 without UIDs, the same 134
v1 refuses) and the other 507,911 files are 497,017 distinct instances. v0's
`kept` flag (the four UIDs, modality MR, CT, PT or PET) marks 496,888 of them:
the 493,708 both hold, plus 3,180 only v0 holds, which are the non-image SOP
classes §5.3 refuses that carry an MR modality (Secondary Capture, nearly all).
The 129 instances v0 would not keep (modality SR, DOC and PR) are the Enhanced
SR, Encapsulated PDF and presentation state files §5.3 refuses too. Of v1's
2,534 stacks 34 are oblique (confidence 0.81 to 0.89) and none has an unknown
orientation.

**Mix.** Pending the copy of the corpus; the same two scripts run on it as they
ran here, and the numbers go below this line.
