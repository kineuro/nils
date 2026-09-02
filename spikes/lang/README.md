# The language spike (C1, D2)

**Question.** Does the engine use Rust, the prior of D2, or Go, the group's prior art (Bifrost)?

**Criteria, written before the work.** On the baseline host of C6 (an 8 vCPU, 64 GB virtual machine over network storage, the machine the performance budget of D6 is stated on), with one million DICOM instances from the private corpus:

1. **Throughput and memory:** files per second and resident memory when parsing the headers with `dicom-rs` and with a Go DICOM library.
2. **Vendor files:** how many files from each scanner vendor each library fails to parse.
3. **Static binaries:** whether one binary that embeds SQLite and DuckDB cross-compiles from CI for the six release targets (Linux, macOS and Windows on x86-64 and arm64).
4. **Maintainability:** how much code the same functionality takes, how readable it is, and how many people in the field could work on it.

Go is chosen only if it matches Rust's throughput within 20 percent, fails no more vendor files, clears the cross-compile test, and is at least as maintainable; otherwise Rust. Ten working days from 2026-09-02; the report goes in this directory and closes D2 either way.

**Rules.** The corpus never leaves the private host; the spike publishes counts, rates and failure classes, never a file, a path or an identifier. Code under `rust/` and `go/` is throwaway and carries the AGPL-3.0-only header like everything else outside `contracts/`.

## The harness

Two programs with the same contract, `rust/parse` on `dicom-object` 0.10.0 and `go/cmd/parse` on `suyashkumar/dicom` v1.1.0. Each one:

- walks every regular file below `--root` (no extension filter, no symlinks), in the order the file system gives;
- sniffs the first 132 bytes itself: `DICM` at offset 128 or 0 means a Part 10 file, group `0008` at offset 0 means a raw data set without a meta group, anything else is `not_dicom` and the library is never called;
- hands the file to `--workers` parsing threads (goroutines) which read the header up to, and not including, Pixel Data and keep 22 technical tags (SOP class and instance, study and series instance UIDs, modality, manufacturer and model, series description, protocol name, series and instance numbers, image type, echo, repetition and inversion times, flip angle, slice thickness, pixel spacing, orientation, position, rows, columns); no patient-level tag is ever read into the output;
- sends every record to one writer, the way v0 has a single database writer, which produces `index.tsv` (one row per parsed file), `paths.tsv` (sequence number to path, so that the index carries no path), `failures.tsv` (class and library message, with the path replaced by `<path>`) and `summary.json` (counts per class and transfer syntax, wall time, files and megabytes per second, user and system CPU, peak resident memory from `getrusage`).

Failure classes: `not_dicom`, `parse_error`, `truncated`, `unsupported_ts`, `missing_sop` (parsed, but no SOP Instance UID), `io_error`. Megabytes per second is the corpus size divided by wall time, so it is the rate at which a corpus is worked through, not the bytes a library actually reads (the Rust side reads headers only, the Go side has to read the pixel data too, see the findings).

`synthetic.py` writes a nine-file corpus with no library and no real data (a Part 10 file, the same without preamble, a raw data set, a truncated file, a file without SOP Instance UID, a Finder file, an empty file, a text file, a copy in a subdirectory); `smoke.sh` runs both harnesses over it and checks the class counts and that the rows both sides produce are identical. `referee.py` is the judge for the real corpora: for every file either side failed, pydicom (`force=True`, `stop_before_pixels=True`) says whether it can read a SOP Instance UID out of it; a file pydicom reads and a library rejects is that library's miss, a file nobody reads is bad on disk. It then compares the rows both sides produced tag by tag (numbers as numbers, multi-values element-wise). `run.sh` is the measurement protocol: for each worker count, Rust then Go, twice each, into `/scratch/nils/spike/<label>/`, then the referee on the last pair and a `results.json` of counts and rates.

`rust/dbcheck` and `go/cmd/dbcheck` are criterion 3: one binary that opens an in-memory SQLite and an in-memory DuckDB, runs a query on each and prints the versions. `.github/workflows/spike-lang.yml` builds and runs both on the six release targets, natively on each runner, and checks with `ldd` and `otool` that no database library is linked dynamically.

## Hosts and toolchains

The first runs (2026-09-02) were made on CT 110 `nils` on Asgard: an LXC container with 64 cores and 256 GB, the corpus on the NVMe pool `fast` (RAIDZ1, lz4, record size 1M) mounted at `/scratch/nils`. The baseline host of C6 is CT 111 `baseline` on the same pool: 8 cores, 64 GB, an LXC container rather than a virtual machine, which I note as a deviation from the criteria; the network-storage case of the criteria is the same corpus read from the tank over NFS (`/data/source`), and the spike runs both.

Toolchains on the host: rustc 1.98.0 (stable, 2026-08-18), go 1.26.8, pydicom 3.0.2 as the referee. Both harnesses are built on the host from the same commit (`cargo build --release`, `go build`).

## Corpora

**nmosd** (2026-09-02): the raw DICOM tree of one study copied from the tank to `/scratch/nils/source/nmosd`: 44 subject folders, 508,045 files, 64.1 GB as the sum of file sizes (about 32 GB on disk after lz4). Every DICOM file in it is Part 10, explicit VR little endian, from one vendor and one decade; 124 files are Finder metadata (`.DS_Store`), 10 are DICOM files without a SOP Instance UID. Clean and homogeneous, so it measures throughput, not vendor tolerance.

**mix** (planned 2026-09-02, Nima's suggestion): a small, deliberately diverse sample of the live v0 archive on fg, which holds 37.5 million instances from 16 manufacturer labels and 86 scanner models, study years 2001 to 2026, four transfer syntaxes (JPEG 2000 lossless for 85 percent of the instances, explicit VR little endian, JPEG lossless in two flavours), 1,567 enhanced multi-frame MR series and a handful of CT series. The selection is made in SQL against the v0 metadata database, read-only, whole series only: up to two series per (manufacturer, model, study year, SOP class, transfer syntax, multi-frame or not, implementation version) stratum, every series of the manufacturers with fewer than 200 series in total, every CT series, three multi-stack series per manufacturer, and the three largest series of the archive (`helpers/spike-mix-select.sh` in the server design record). It travels fg to Asgard over Bifrost like every other transfer and lands at `/scratch/nils/source/mix` with a `MANIFEST.tsv` of strata and counts. Together with nmosd, and ctrl or longtbi from the tank when the one-million-instance run is due, it gives criterion 2 the vendor spread nmosd lacks.

## Results so far

### nmosd, CT 110, 2026-09-02

Warm cache (the corpus had been written an hour earlier, the host has 3 TB of RAM), 8 workers, two runs per side:

| side | wall | files/s | CPU (user+sys) | peak RSS | parsed | failed |
|---|---|---|---|---|---|---|
| Rust, dicom-object 0.10.0 | 8.0 s, 8.3 s | 63,900, 60,900 | 69 s, 71 s | 277 MB, 239 MB | 507,911 | 134 |
| Go, suyashkumar/dicom v1.1.0 | 22.5 s, 22.2 s | 22,600, 22,900 | 218 s, 217 s | 53 MB, 65 MB | 506,140 | 1,905 |

Worker scaling, one run per point, same corpus and cache state:

| workers | Rust wall | Rust files/s | Rust CPU | Go wall | Go files/s | Go CPU |
|---|---|---|---|---|---|---|
| 2 | 28.1 s | 18,100 | 65 s | 55.7 s | 9,100 | 162 s |
| 4 | 15.3 s | 33,200 | 71 s | 34.1 s | 14,900 | 189 s |
| 8 | 8.0 s | 63,900 | 69 s | 22.2 s | 22,900 | 217 s |
| 16 | 7.1 s | 71,700 | 75 s | 17.4 s | 29,300 | 268 s |
| 32 | 7.3 s | 70,000 | 79 s | 17.1 s | 29,700 | 360 s |

Both sides stop scaling at 16 workers, where the single writer and the walk become the limit (the shape v0 has too); Rust's CPU stays flat while Go's grows with the worker count. Per file, Rust spends about 130 microseconds of CPU on a header, Go about 320 at two workers and 700 at 32.

Failures: both sides reject the same 134 files (124 `not_dicom` Finder files, 10 `missing_sop`), and pydicom reads none of them either. Go alone fails 1,771 more files, all with `ParseSpecificCharacterSet: Unknown character set 'ISO IR 100'`, a Specific Character Set written without the underscore by one scanner's software; pydicom reads all 1,771 with a warning, dicom-object reads them silently. The referee compared 50,000 of the 506,140 rows both sides produced: no disagreement on any tag or on the transfer syntax.

So on this corpus Go runs at 36 percent of Rust's throughput, uses three times the CPU, a quarter of the memory, and misses 1,771 files Rust reads. Criterion 1 and criterion 2 both point to Rust; the numbers on the 8-core baseline host and on the mix corpus follow.

### Static binaries, the CI matrix, 2026-09-02

Workflow run 33623854968 on the pull request, GitHub's hosted runners, one job per target, both sides built and run natively (no cross-compilation), `ldd` or `otool -L` on the result:

| target (runner) | Rust: build, binary, runs | Go: build, runs |
|---|---|---|
| linux-x86_64 (ubuntu-latest) | 12m23s, 67.1 MB, yes | 0m28s, yes |
| linux-arm64 (ubuntu-24.04-arm) | 8m39s, 63.2 MB, yes | 0m21s, yes |
| macos-x86_64 (macos-15-intel) | 14m51s, 44.9 MB, yes | 0m45s, yes |
| macos-arm64 (macos-latest) | 10m34s, 44.0 MB, yes | 0m36s, yes |
| windows-x86_64 (windows-latest) | 15m16s, 29.1 MB, yes | 1m11s, yes |
| windows-arm64 (windows-11-arm) | 12m24s, 34.3 MB, yes | build fails | 

Every binary that built ran its query on SQLite 3.53 and DuckDB 1.5.5 from inside itself, and none links a database library (Linux: libstdc++, libgcc_s, libm, libc only; macOS: the system libraries only). The Rust side compiles the DuckDB amalgamation on every target, which is the 8 to 15 minutes; the Go side links the static libraries that `duckdb-go` ships, hence the seconds, and has none for Windows on arm64: the build stops in `runtime/cgo` itself (`gcc_arm64.S: no such instruction: stp x29,x30`), because the C toolchain on that runner cannot assemble arm64, so no cgo program builds there without a toolchain of our own. Criterion 3 holds for Rust on six of six targets and for Go on five, at a cost of build time on the Rust side that a cache pays once.

## Findings about the libraries

- `suyashkumar/dicom` cannot stop in front of Pixel Data: `SkipPixelData()` skips the element by reading and discarding its bytes, so every file is read to the end. On nmosd that is 64 GB of reads for headers that add up to a few hundred megabytes. There is no public API to end the parse at a tag.
- `suyashkumar/dicom` requires `DICM` at offset 128; a Part 10 file without the preamble is misread (its "compatibility mode" rewinds to offset 0 and guesses the transfer syntax from the next 100 bytes), which the synthetic corpus shows as `missing_sop`. `dicom-object` handles both with `ReadPreamble::Auto`.
- `suyashkumar/dicom` returns an error for the character set `ISO IR 100` (1,771 files in nmosd); pydicom and dicom-object accept it.
- `dicom-object` reads raw data sets without a meta group through `DicomCollectorOptions` with an expected transfer syntax; the Go side needs `SkipMetadataReadOnNewParserInit()` and infers the syntax.
- `dicom-object`'s errors are `snafu` enums that capture a backtrace on every failure; on a corpus with many bad files that is a per-file cost worth measuring (it did not show on nmosd, where 134 files fail).
- The DuckDB amalgamation compiled by `libduckdb-sys` needs the per-package profile override in `rust/Cargo.toml` (`debug = false`), otherwise every translation unit carries debug info and a 16 GB machine runs out of memory during the build. `duckdb-go` ships prebuilt static libraries and needs cgo; a Windows arm64 library is not among them, and the CI matrix showed the build failing there before it got that far, in Go's own cgo runtime.
- Both `dbcheck` binaries link only the C and C++ runtimes on Linux (`ldd`: libstdc++, libgcc_s, libm, libc); Rust's is 68 MB, Go's about the same order.

## Open

- The 8-core baseline host (CT 111) and the NFS case.
- The mix corpus (transfer pending), then the one-million-instance run.
- Criterion 4: the two harnesses are the same size (449 lines of Rust, 433 of Go for `parse`; 36 and 50 for `dbcheck`); the judgment is written in the report.
