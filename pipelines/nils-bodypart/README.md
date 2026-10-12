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

A stack's time goes into reading its files and decoding its frames, lossless JPEG 2000 most of all, not into the network. Since 0.4.0 `infer-fusion` reads, by default, only the frames the encoder's six planes interpolate from (`--reader reduced:touched`, about half the full reader's files), chosen from each file's geometry in the manifest, which the descriptor asks the engine for (`x-nils.input.geometry`); `--reader full` reads every file as the model was certified, and `reduced:full` reads every kept frame from the geometry with the full reader's planes bit for bit. A file without geometry, a multi-frame file and a stack of mixed orientations have their headers read; a file decoded on the card has its header read too, so the card's answers are the CPU's. `infer-fusion` scores stacks in `--threads` worker processes (16 in the descriptor), each with one BLAS thread, and asks the kernel to read the next stack's files ahead. Every path gives the same answers byte for byte: one worker or many, and with `--device cuda` (or `auto`, which takes a card where one is found; the default is `cpu`), which decodes lossless JPEG 2000 on the card through nvImageCodec in `--gpu-workers` of the workers (2 by default) and hands pydicom the same samples. `--encoder-device cuda` also runs the image encoder on the card in batches of `--batch` stacks; that one agrees with the numpy encoder to float32 precision, not bit for bit, and is off by default. The descriptor asks for no card.

Since 0.5.0 `infer-fusion` runs body part's cascade where the run is given one (record 55 E2): the cascade is one registered model whose file names its parts by digest, and the run takes them as the model inputs `student`, `deferral` and `cascade` beside the encoder, the head and the coarse mode file. Each stack is answered by the first stage that may: `s1`, a few-slice student over one native slice (the slot-0 rule of the few-slice study, 192 x 192 at 1.5 mm, chosen and described from the manifest alone, with the stack's height against its session's top), accepted where the deferral model is at least its threshold; then the planes of at most 24 frames, accepted at a log-odds margin from both thresholds; then the touched frames, reusing what the 24-frame stage read. Each table names the stage, the files read, every part's digest and, where the stack has no body region, the reason (`sealed`, `not-mr`, `no-geometry`, `not-sure` or `unreadable`); each `body_part` decision names the cascade, each `body_region` decision the coarse part the cascade names. A stack's height is read against its whole session: the engine lists the rest of each session a run holds part of as `stacks.json`'s `context`. `--cache <folder>` keeps the inputs the stages read per session (`bodypart-slice-192:v1`, `bodypart-planes-b24:v1`, `bodypart-planes-touched:v1`), keyed by each file's SOP Instance UID and the sha256 of its bytes, and `--cache-fill touched` also builds the touched planes of stacks an earlier stage answered. The student runs in float32 on the CPU (or on the card with `--device cuda`, where `--gpu-workers` still says how many of the planes stages' workers decode on the card, and 0 keeps every decode on the CPU); torch and timm are in the image, and the weights are registered model inputs, never in the image.

Every entry writes `/output/results.json` (`contracts/job/v1/results.schema.json`). An embedding file is the engine's `.emb` format, `contracts/job/v1/embedding.md`.

The encoders are pinned to Hugging Face commits in `src/nils_bodypart/encoders.py` (`PINS`), baked into the image at build time, and verified by their weights' sha256 (`encoders.json` in the image). The image runs offline.

## Build

```sh
podman build -t nils-bodypart pipelines/nils-bodypart
podman build --build-arg TORCH_INDEX=https://download.pytorch.org/whl/cu129 -t nils-bodypart:cuda pipelines/nils-bodypart
```

Both builds take torch 2.13.0 and torchvision 0.28.0 (the Dockerfile's `TORCH_VERSION` and `TORCHVISION_VERSION`): the newest torch with a CUDA 12 build, since the nvImageCodec wheels are CUDA 12 ones. Its CUDA build has kernels for Blackwell cards (sm_120), which torch 2.5.1's had not.

The image has no entry point: each descriptor's command line names the program, `nils-bodypart <entry> ...`. A descriptor is pinned to the image's registry manifest digest before `nils pipeline add`.

## Test

```sh
pip install -c requirements.txt -e ".[test]"
python -m pytest -q
```

The tests use stand-in encoders, a stand-in student and synthetic models, and need neither torch nor the weights; where torch and timm are installed, more tests check the numpy encoder against the same network in torch and the student's network. With `NILS_BODYPART_FROZEN` naming a site's frozen cascade and its registered parts (outside the repository), one more test runs them end to end on synthetic stacks.
