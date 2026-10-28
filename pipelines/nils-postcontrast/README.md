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

## Raw scores and a site's calibration

The fusion's score is a log-odds per stack, calibrated per site (the key is
`<cohort> <make>`: the dataset or cohort holding the stack and the scanner's
make as `make` spells it) by a Platt pair the frozen fusion carries. A site it
has no pair for gets `no-calibration` and no answer. Since 0.3.0 such a site
fits its own pair, with no new model bundle or image:

1. Run with the `raw_scores` switch (`--raw-scores`, off by default; both
   pipelines). A T1-type brain stack at a site with no pair is scored too
   and its table carries `score_raw`, the fused log-odds before any
   calibration (the cascade adds each seed's in `score_raw_seeds`; a
   unit's metrics carry `score_raw` too), with its
   `site` (empty where the fusion knows none of its cohorts), its `make` as
   the key spells it and `cohort_candidates`, a JSON list (in the text
   column, so a name with a space survives) of the dataset, batch and cohort
   names the stack belongs to, through the fusion's cohort aliases, in the
   order the site key is looked for, so the site builds its key as `<one of
   them> <make>`; `p_given` and `value` stay empty, so nothing becomes a
   proposal. In the full pipeline a stack whose conversion or tools failed
   keeps that reason (`no-segmentation`, `worker-died`, ...) with no raw
   score, and one with no files is `no-files`; in the cascade a stack at
   such a site that the student could not read keeps `no-calibration` with
   no raw score and `stage1` saying why (`no-geometry`, `multi-frame`,
   `no-slice`), never `deferred`, so the run's deferral list, which the
   engine runs the full pipeline over, keeps its meaning. The run's
   metrics count the stacks the rule accepted as `candidates`, as before,
   and the raw-scored stacks as `raw_scored`. These columns, and the unit
   metric `score_raw`, are on every row and unit of every run, so a
   table's shape never changes: without the switch they are null and every
   stack is as before.
2. Label those stacks in a tab-separated file with a header, `stack`,
   `label` (`given` or `not_given`) and `source` (`header`, `name` or
   `manual`), and fit the pair:

   ```sh
   nils-postcontrast calibrate --labels labels.tsv --scores <run output>... --site "<cohort> <make>" --out pair.json
   ```

   It joins on the stack id and takes only the key's rows: the key is
   trimmed, its make compared without case and written as the tables spell
   it (a make word the fusion does not know is refused); rows of another
   make are counted
   as `n_other_make`, rows whose table names another site as
   `n_other_site`, and rows whose `cohort_candidates` do not name the
   key's cohort as `n_other_cohort` (a row whose table names the key
   itself as its site needs no such check; a row from an older table with
   no such column is taken and counted as `n_without_candidates`). A table
   that is not what a run writes, or a pair file that cannot be written, is
   refused by one line naming the path, never a value. It needs
   at least 20 labelled stacks of each class with a score, refuses a bad
   row by its line number and never by its content, and writes `{site, a,
   b, n, n_given, n_not_given, n_without_score, n_other_site,
   n_other_make, n_other_cohort, n_without_candidates, fit, per_source,
   overall, model, fitted_at}` with the log loss and accuracy overall and
   per source, printed as a short report. `fit` says how many
   Newton iterations the pair took and whether it converged (a step is
   adopted only when it lowers the loss; where none does, the fit stops and
   says so); `model.fusion_digest` names the fusion the scores came from.
   The file is written as one compact line, on purpose: an engine
   released before `sweep: false` (before 1.0.0-beta.3) ignores the flag
   and sweeps the site file as a secret, each of its lines a needle, and
   would remove every table echoing one (they echo the fusion digest and
   the site); a whole document on one line is a needle no table holds. A
   run reads either layout, but a hand-edited or pretty-printed pair file
   must not be given to such an engine.
3. Give the pair to every run: the descriptor's optional site file
   `post_contrast_calibration` (`nils pipeline secret set
   post_contrast_calibration --file /absolute/path/pair.json`; the engine
   takes an absolute path, since it reads the file where it runs), which
   the engine mounts and names through `NILS_PC_CALIBRATION`; outside the
   engine, `--calibration FILE`. The file is one pair file or a map `{site:
   [a, b]}` (a value may be a pair file's object, whose own `site`, where
   it is a string, must be the key's); every key is read as `calibrate`
   reads `--site` (trimmed, the make as the tables spell it) and its cohort
   goes through the fusion's cohort aliases, so the key the run looks a
   stack up by is the one the file set. Its pairs are
   added beside the fusion's before any stack is judged. A pair file whose
   `model.fusion_digest` names another fusion than the run's is refused, and
   so is a file naming a site the frozen (certified) fusion has a pair for,
   however it spells or aliases it: no site file replaces a certified pair
   (the run fails before any stack is read, naming the key). The run's
   `results.json`
   records under `calibration` the sites the file set, each with the number
   of stacks its pair calibrated (`sites: {key: n}`), `unchecked: true`
   where the file named no fusion to check against, and `matched_none:
   [keys]` where no key was any stack's site (the run still completes);
   never the pairs. The file is kept where secrets are kept and reaches the
   container as one, but it is a site file, not a secret: the descriptor
   declares it `sweep: false`, so the engine does not sweep the run's
   outputs for its lines (the tables echo the fusion digest and site it
   names). Engines released before that flag (before 1.0.0-beta.3) sweep
   it as any secret, which the one-line pair file survives (step 2) and a
   multi-line file does not.

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
