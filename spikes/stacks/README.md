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

**Verdict.** Pending.
