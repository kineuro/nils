# nils-bodypart

v0's body-part detector as a NILS pipeline image: one image, four entry points, each with its descriptor (`bodypart-<entry>/nils.job.yml`). The engine runs them with `nils run`; the loop is in `docs/guides/pipelines.md`.

| entry | reads | writes |
|---|---|---|
| `embed` | the stacks (`/input/stacks.json`), the embeddings the cache holds already | one embedding per stack and encoder (BiomedCLIP, SigLIP2), only what is missing |
| `seed` | BiomedCLIP embeddings, each stack's rule answer | seeds per body-part value and a selection a curation campaign starts from; never proposals |
| `train` | a label set, the embeddings | a calibrated head (JSON for logistic regression, a joblib pickle for `rf` and `svm`) with its model card, and cross-validated accuracy, ECE and Brier |
| `infer` | a registered head (`--model`), the embeddings | a `body_part` proposal per stack with every class's probability; a pickled head only with `allow_pickle=true` and a card that checks |

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

The tests use stand-in encoders and need neither torch nor the weights.
