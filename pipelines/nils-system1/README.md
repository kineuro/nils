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
  `trust_code` true, after its bytes match.
- Build: `docker build -t nils-system1 pipelines/nils-system1`. Each site
  builds it and repins the descriptor; no release publishes it.
- Tests: `pip install -e .[test] && pytest`.

AGPL-3.0-only.
