# SPDX-License-Identifier: AGPL-3.0-only
"""System 1 served as a NILS pipeline (record 55 D3, spec 11.1).

System 1 is the rules' learned partner: a fast model over a stack's header
that gives, for each axis, its own answer and its calibrated probability,
and a reliability score for the rules on that header. The engine keeps
them as a second opinion beside the rules' answers, never as a decision.

The model is a site model and never part of this image: it comes as the
run's model input, a bundle (one tar file) whose ``system1.json`` names
every file it holds by sha256 and the predictor that reads them. This image
checks the bundle, runs the predictor over the run's stacks, measures which
fields mattered by leaving each out, and writes ``results.json`` with the
opinions (``contracts/job/v1/opinions.schema.json``).
"""

__version__ = "0.1.0"
