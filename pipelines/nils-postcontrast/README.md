# nils-postcontrast

The certified post-contrast model of NILS (model r1) served through its
three-slice student, as a pipeline image. It answers `post_contrast` only on
T1-type brain, where the model is certified, and says "not sure" everywhere
else.

Per stack it reads three native slices chosen from the registry's geometry,
scores them with the student's three seeds, and passes each seed's arms and
their mean through model r1's frozen fusion with the stack's header evidence
(the contrast tag, the session's clock and tags, the rules' answer and
words), calibrated per site. A stack is answered where the seeds agree with
the mean and its log-odds margin from the nearer threshold is at least 4;
the rest of T1-type brain is deferred to the full pipeline.

- Descriptor: `postcontrast-cascade/nils.job.yml` (stacks layout with
  `x-nils.input.header`, `geometry` and `contrast`; model inputs `student`,
  `fusion`, `cascade`).
- Build: `docker build -t nils-postcontrast pipelines/nils-postcontrast`, or
  with `--build-arg TORCH_INDEX=https://download.pytorch.org/whl/cu128` for a
  card. Each site builds it and repins the descriptor; no release publishes
  it.
- Tests: `pip install -e .[student,test] && pytest` (the student's tests are
  skipped without torch).

The weights are a site's model inputs and are never part of the image.

## The full pipeline

`nils-postcontrast full` (descriptor `postcontrast-full/nils.job.yml`) is
model r1 itself on the stacks the cascade defers: dcm2niix, SynthMorph,
SynthSeg and SynthStrip, the regions', structures' and tail features, the
three frozen arms (model input `arms`, with their atlases) and the fusion.
Its image is `Dockerfile.full`, FreeSurfer 8.2.0's image with the card's
runtimes beside it. FreeSurfer's image carries third-party components whose
terms are not ours to pass on, so each site builds it and repins the
descriptor; it is never published. Tests: `pip install -e .[full,test] &&
pytest tests/test_full.py`.

AGPL-3.0-only.
