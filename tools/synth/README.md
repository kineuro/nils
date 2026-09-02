# Synthetic corpora

The engine's tests and benchmarks run on data that never existed: files written here by a generator, from a fixed seed, with invented values and UIDs under the example root `1.2.826.0.1.3680043.8.498`. Nothing derived from a real registry enters this directory (design record, C10).

`synthetic.py` is the seed, carried over from the language spike: nine files that cover the shapes a walker meets (a Part 10 file, the same without preamble, a raw data set, a truncated file, a file without SOP Instance UID, a Finder file, an empty file, a text file, a copy in a subdirectory). It grows, in slice 8 of Wave 1, to the one-million-instance corpus that the CI benchmark digests end to end with a hard regression gate (`docs/specs/wave1-parse-and-digest.md`, §12.6).

```sh
python3 tools/synth/synthetic.py /tmp/corpus
```
