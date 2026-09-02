# Synthetic corpora

The engine's tests and benchmarks run on data that never existed: files written by a generator, from a fixed seed, with invented values and UIDs under the example root `1.2.826.0.1.3680043.8.498`. Nothing derived from a real registry enters this directory or the generator (design record, C10).

## The generator

`corpus` is an example of the `nils-dicom` crate; it writes a whole tree from a seed, with exactly the number of accepted instances asked for:

```sh
cargo run --release -p nils-dicom --example corpus -- \
    --out /scratch/nils/synth --instances 1000000 --seed 1 > synth-manifest.json
```

| Flag | Default | What it sets |
| --- | --- | --- |
| `--out DIR` | required | the root of the tree |
| `--instances N` | required | accepted instance files, exactly |
| `--seed S` | 1 | the seed; the same seed writes the same tree |
| `--pixel-bytes B` | 4096 | Pixel Data appended to every accepted file, so the reader's stop before Pixel Data is exercised |
| `--duplicate-percent P` | 1 | share of instances copied a second time under `dup/` |
| `--refused-every K` | 500 | one refused file per K instances |

The tree is `sub-NNNNNN/st-N/se-NN-<MOD>/IM_NNNN` (some series with a `.dcm` suffix), with the duplicates under `dup/` on the same relative paths and the refused files next to the instances they follow. What it holds, so that a digest of it walks every path the spec names:

- MR, CT and PT series in v0's proportions (70/20/10), a tenth of the MR series as Enhanced MR: one multi-frame file per series with its parameters in the shared functional groups and a position per frame;
- subjects with one to three studies, studies with one to eight series, series of 20 to 400 instances; the catalogue's subject, study, series, modality and instance fields filled with plausible values, private DWI blocks on diffusion series;
- Part 10 files with the preamble (85%), without it (10%) and bare data sets (5%, explicit VR); implicit VR little endian in 5% of the Part 10 files;
- ISO_IR 100 (85%), ISO_IR 192 with non-ASCII descriptions (10%) and no declared character set (5%), chosen per study;
- 2% of subjects without a PatientID, so identity falls to the study;
- refused files in turn: an empty file, a truncated instance, a text file, a junk `.DS_Store`, a file without SOP Instance UID, an ultrasound file (an unsupported SOP class).

The manifest on stdout carries the counts a digest report is checked against: `files` is the report's `seen`, `instances + duplicates` its `parsed`, `refused` its `quarantined`, and `studies` and `series` its own. The report's `subjects` runs a little above the manifest's, by design: a subject without a PatientID is one registry subject per study.

Throughput on a laptop, single-threaded, to tmpfs: about 40,000 files per second at 256 bytes of Pixel Data. A million instances with the default 4 KB of Pixel Data take about 5.7 GB.

## The seed files

`synthetic.py` is the seed carried over from the language spike: nine files that cover the shapes a walker meets (a Part 10 file, the same without preamble, a raw data set, a truncated file, a file without SOP Instance UID, a Finder file, an empty file, a text file, a copy in a subdirectory). The CI benchmark of slice 8 digests the generator's million end to end with a hard regression gate (`docs/specs/wave1-parse-and-digest.md`, §12.6).

```sh
python3 tools/synth/synthetic.py /tmp/corpus
```
