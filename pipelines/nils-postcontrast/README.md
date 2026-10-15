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
- Build: `docker build -t nils-postcontrast pipelines/nils-postcontrast`.
  One image runs on the CPU and on a card: torch 2.8.0 from its CUDA 12.8
  wheels, which run on the CPU where the container is given no card
  (`--build-arg TORCH_INDEX=https://download.pytorch.org/whl/cpu` builds a
  smaller CPU-only image). The release publishes it, and ships the
  descriptor with the digest of the image it pushed in its
  `pipelines.tar.gz` (`nils pipeline add --release postcontrast-cascade`).
- Tests: `pip install -e .[student,test] && pytest` (the student's tests are
  skipped without torch).

The weights are a site's model inputs and are never part of the image.

## The full pipeline

`nils-postcontrast full` (descriptor `postcontrast-full/nils.job.yml`) is
model r1 itself on the stacks the cascade defers: dcm2niix, SynthMorph,
SynthSeg and SynthStrip, the regions', structures' and tail features, the
three frozen arms (model input `arms`, with their atlases) and the fusion. A
stack left with no image arm, for any reason, is "not sure"
(`no-usable-volume`, `no-segmentation`, `no-image-arm`, or `worker-died` and
`step-failed` where the run itself failed), where
model r1 would answer from its header evidence alone. On a card it holds
at most the memory the engine leases it (`gpu-memory-gb`, 60 GB: one full
wave of two SynthSeg processes beside SynthMorph and SynthStrip); a smaller
budget runs fewer SynthSeg processes, then the CPU, and the Frangi filter
goes on the card only where every worker's share fits beside a full wave.
Its image is `Dockerfile.full`: of FreeSurfer 8.2.0 only the parts the
Synth tools run on, copied unchanged from FreeSurfer's pinned image, with the
card's runtimes beside them, and its notices (`NOTICE.full`) and licences in
`/usr/local/share/licenses/nils-postcontrast-full`. No MATLAB Runtime, FSL or
CorTechs tools and no licence key are in it. Tests: `pip install -e .[full,test] &&
pytest tests/test_full.py`.

AGPL-3.0-only.
