# nils-bodypart

v0's body-part detector and the certified body-part model as a NILS pipeline image: one image, five entry points, each with its descriptor (`bodypart-<entry>/nils.job.yml`). The engine runs them with `nils run`; the loop is in `docs/guides/pipelines.md`.

| entry | reads | writes |
|---|---|---|
| `embed` | the stacks (`/input/stacks.json`), the embeddings the cache holds already | one embedding per stack and encoder (BiomedCLIP, SigLIP2), only what is missing |
| `seed` | BiomedCLIP embeddings, each stack's rule answer | seeds per body-part value and a selection a curation campaign starts from; never proposals |
| `train` | a label set, the embeddings | a calibrated head (JSON for logistic regression, a joblib pickle for `rf` and `svm`) with its model card, and cross-validated accuracy, ECE and Brier |
| `infer` | a registered head (`--model`), the embeddings | a `body_part` proposal per stack with every class's probability; a pickled head only with `allow_pickle=true` and a card that checks |
| `infer-fusion` | the stacks' files and headers, the certified model's encoder, head and coarse mode file (models) | per stack a table of its scores in both modes, a `body_part` proposal (fine) and a `body_region` proposal (coarse) |

`infer-fusion` runs the certified body-part model of record 50: one frozen model with two modes. It reads the stack's files its training listed (96 positions evenly, and v0's slices) into an 8 mm volume in patient axes, scores it with the image encoder (three seeds, run in numpy), and gives the encoder's answer and 44 features of the stack's header to a LightGBM head. Fine mode calibrates the head's answer to the six body_part values at the temperature of the stack's cohort (from its first ingest batch, then its subject's cohorts), or the global one; coarse mode sums it into four regions (head, spine, chest, other) and calibrates those. A mode answers at or above its threshold and abstains below it. A stack with no fingerprint row or no geometry gets no answer and is skipped. The run is refused when an artifact is not the one its digest names, or the parts do not name each other.

A stack's time goes into reading its files and decoding its frames, lossless JPEG 2000 most of all, not into the network. `infer-fusion` scores stacks in `--threads` worker processes (16 in the descriptor), each with one BLAS thread, and asks the kernel to read the next stack's files ahead. Every path gives the same answers byte for byte: one worker or many, and with `--device cuda` (or `auto`, which takes a card where one is found; the default is `cpu`), which decodes lossless JPEG 2000 on the card through nvImageCodec in `--gpu-workers` of the workers (2 by default) and hands pydicom the same samples. `--encoder-device cuda` also runs the image encoder on the card in batches of `--batch` stacks; that one agrees with the numpy encoder to float32 precision, not bit for bit, and is off by default. The descriptor asks for no card.

Every entry writes `/output/results.json` (`contracts/job/v1/results.schema.json`). An embedding file is the engine's `.emb` format, `contracts/job/v1/embedding.md`.

The encoders are pinned to Hugging Face commits in `src/nils_bodypart/encoders.py` (`PINS`), baked into the image at build time, and verified by their weights' sha256 (`encoders.json` in the image). The image runs offline.

## Build

```sh
podman build -t nils-bodypart pipelines/nils-bodypart
podman build --build-arg TORCH_INDEX=https://download.pytorch.org/whl/cu121 -t nils-bodypart:cuda pipelines/nils-bodypart
```

The image has no entry point: each descriptor's command line names the program, `nils-bodypart <entry> ...`. A descriptor is pinned to the image's registry manifest digest before `nils pipeline add`.

## Test

```sh
pip install -c requirements.txt -e ".[test]"
python -m pytest -q
```

The tests use stand-in encoders and synthetic models, and need neither torch nor the weights; where torch is installed, one more test checks the numpy encoder against the same network in torch.
