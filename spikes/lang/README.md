# The language spike (C1, D2)

**Question.** Does the engine use Rust, the prior of D2, or Go, the group's prior art (Bifrost)?

**Criteria, written before the work.** On the baseline host of C6 (an 8 vCPU, 64 GB virtual machine over network storage, the machine the performance budget of D6 is stated on), with one million DICOM instances from the private corpus:

1. **Throughput and memory:** files per second and resident memory when parsing the headers with `dicom-rs` and with a Go DICOM library.
2. **Vendor files:** how many files from each scanner vendor each library fails to parse.
3. **Static binaries:** whether one binary that embeds SQLite and DuckDB cross-compiles from CI for the six release targets (Linux, macOS and Windows on x86-64 and arm64).
4. **Maintainability:** how much code the same functionality takes, how readable it is, and how many people in the field could work on it.

Go is chosen only if it matches Rust's throughput within 20 percent, fails no more vendor files, clears the cross-compile test, and is at least as maintainable; otherwise Rust. Ten working days from 2026-09-02; the report goes in this directory and closes D2 either way.

**Rules.** The corpus never leaves the private host; the spike publishes counts, rates and failure classes, never a file, a path or an identifier. Code under `rust/` and `go/` is throwaway and carries the AGPL-3.0-only header like everything else outside `contracts/`.
