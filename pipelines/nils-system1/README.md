# nils-system1

System 1, the rules' learned partner, as a NILS pipeline image. For each
stack and axis it writes System 1's answer, its calibrated probability, a
reliability score for the rules on that header and the fields that mattered
most. The engine keeps them as a second opinion beside the rules' answers,
never as a decision.

- Descriptor: `system1-score/nils.job.yml` (stacks layout with
  `x-nils.input.header` and `x-nils.input.rows`; one model input, `model`;
  opinions on provenance, technique, modifier, construct and base).
- The model is a site model, the one chosen for the task `system1`, kept as
  one bundle file whose `system1.json` names every file by sha256
  (`src/nils_system1/bundle.py`). A bundle that brings code runs only with
  `trust_code` true, after its bytes match; a run given an admitted model
  is given it by the admission (`trusted-by-admission`).
- Speed: a part of 5,000 stacks spends about 150 s in the bundle's per-stack
  featurising on one core. The engine runs parts several at once; the next
  retrain builds its bundle with the featurising batched over a part's
  stacks (record 55, ruling 28).
- Build: `docker build -t nils-system1 pipelines/nils-system1`. CPU only:
  System 1 is arithmetic per stack. The release publishes it, and ships the
  descriptor with the digest of the image it pushed in its
  `pipelines.tar.gz` (`nils pipeline add --release system1-score`).
- Tests: `pip install -e .[test] && pytest`.

AGPL-3.0-only.
