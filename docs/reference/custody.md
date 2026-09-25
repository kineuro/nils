# Custody

Every store the registry at `<home>` keeps (backend sqlite), rendered by `nils custody --markdown`: where it lives, which classes of data it holds (§4.3 of the Wave 1 specification), how long it is kept, and the command that reads, changes, exports or deletes it. Every command named here exists, and nothing is retained that this page does not show. `nils custody` prints the same table with the files and counts of the moment; `--json` is the machine-readable form.

## configuration

| | |
|---|---|
| what | nils.toml: the backend, the Postgres dsn if written there, the schema, the key store path |
| where | `<home>/nils.toml` |
| holds | technical<br>secret: a password in the dsn, when one is written there instead of NILS_DSN |
| owner | the registry's operator |
| kept | until removed |
| read | `nils status` |
| change | edit the file |
| export | no command |
| delete | remove the file |

## registry

| | |
|---|---|
| what | the pseudonymous catalogue: subjects, studies, series, stacks, instances, source files, diagnostics, review items, jobs and batches |
| where | `<home>/registry.db`, mode 600 (SQLite keeps registry.db-wal and registry.db-shm beside it while a connection is open) |
| holds | quasi-identifying: birth dates, sex, study dates and times, station and institution names, descriptions and comments, source paths<br>technical: everything else the catalogue declares |
| owner | the registry's operator, for the research group that owns the archive |
| kept | until deleted; nothing expires on its own, and a run marks files that vanished as gone instead of deleting their rows; a stack or series that holds no instance, because every file of it was a duplicate, is removed |
| read | `nils status [--batch <id>]`<br>`nils quarantine list`<br>`nils review list` |
| change | `nils digest <root>`<br>`nils repair empty-stacks` |
| export | none in Wave 1: the file (or the schema) is the export |
| delete | remove `<home>/registry.db` (nils has no command for it) |

## linkage store

| | |
|---|---|
| what | the identifiers behind the codes, encrypted under the registry's key; the linkages between subjects; the audit of every read |
| where | `<home>/linkage.db`, mode 600 (SQLite keeps linkage.db-wal and linkage.db-shm beside it while a connection is open) |
| holds | identifying: the identifiers (encrypted) and their keyed lookups<br>technical: the linkages, the id types, the read audit (actor, time, why, identity id) |
| owner | the registry's operator; the identifiers are the clinic's |
| kept | until purged; a purged identifier is filed again only when its file is parsed again (changed, or new), not by a digest that finds the file unchanged |
| read | `nils linkage show <code> [--why <text>]` (every read is audited) |
| change | `nils digest <root>`<br>`nils linkage import <csv>`<br>`nils linkage link \| unlink \| merge`<br>`nils linkage id-type add` |
| export | none in Wave 1 |
| delete | `nils linkage purge --subject <code> \| --all` (the read audit and the id types stay) |

## key store

| | |
|---|---|
| what | the pseudonym key (k for this registry) and any other key added |
| where | `<home>/keys`, mode 700, one file per key, mode 600 |
| holds | secret: the key bytes; whoever holds the registry's key can derive its codes and read its linkage store |
| owner | the registry's operator |
| kept | until removed; the key the registry names cannot be removed while it names it |
| read | `nils key list` (names, lengths and fingerprints, never the bytes) |
| change | `nils key add <name>` |
| export | copy the file (that is the backup the key needs) |
| delete | `nils key remove <name>` |

## quarantine list

| | |
|---|---|
| what | the files a digest refused, each with its class and detail, and one review item per batch and class |
| where | rows of source_file (status quarantined) and review_item (kind ingest.quarantine) in the registry |
| holds | quasi-identifying: the file paths<br>technical: the class, the detail, the counts |
| owner | the registry's operator |
| kept | a file's row until the file changes or a run reads it again with --retry-quarantine; the review items until decided (review apply is Wave 4's) |
| read | `nils quarantine list [--batch <id>] [--class <c>]`<br>`nils review list [--kind ingest.quarantine]`<br>`nils review show <id>` |
| change | `nils digest <root> --retry-quarantine` |
| export | `nils quarantine list --json` |
| delete | with the registry |

## classifications

| | |
|---|---|
| what | what a pack decided about each stack, one row per axis, with the evidence that made it, every rule's vote on it and any decision a person recorded |
| where | rows of stack_fingerprint, classification, classification_axis, classification_evidence, classification_voter, classification_vote and decision in the registry |
| holds | technical: the fields a pack reads, the axes, the tiers and confidences, the rule that fired, every clause that held and the value its rule said<br>a person's words: the why on a decision |
| owner | the pack's author for the rules, the reviewers for the decisions |
| kept | until the next run of that job replaces it; a decision until withdrawn, and a withdrawn one for good |
| read | `nils explain <stack>`<br>`nils review list`<br>`nils pack show <name>` |
| change | `nils fingerprint`<br>`nils classify`<br>`nils classify --no-votes`<br>`nils review decide <id> --value <v>` |
| export | `nils explain <stack> --json`<br>`nils classify votes [--out <file>] [--axis <axis>]` |
| delete | with the registry |

## overlays

| | |
|---|---|
| what | a site's amendments to a pack as registry objects: the document, its scope, the rehearsal that justified it, and who proposed, adopted or refused it (Wave 4c section 6.6) |
| where | rows of overlay in the registry, with a review item beside each proposal |
| holds | a site's words for a pack's buckets and the cases that show what they change<br>who proposed and who adopted, with the actor |
| owner | the reviewer who proposed each, the operator who adopted it |
| kept | for good; an adopted overlay is what the rows it judged cite |
| read | `nils overlay list`<br>`nils overlay show <id>` |
| change | `nils overlay refuse <id>`<br>POST /api/overlays<br>POST /api/overlays/<id>/adopt |
| export | `nils overlay export <id> --to <dir>` |
| delete | with the registry |

## models

| | |
|---|---|
| what | the models whose answers become registry facts (record 42, D15): each by the digest of its artifact, with its card, the check that admitted it, its state and every transition; never the artifact itself |
| where | rows of model and model_event in the registry |
| holds | technical: names, versions, digests, tasks and slots, the metrics a card states, the checks<br>who registered, admitted, promoted and retired each |
| owner | the operator who registered, admitted and promoted each |
| kept | for good; a retired model is what the decisions it answered name |
| read | `nils model list`<br>`nils model show <model>` |
| change | `nils model register --card <file>`<br>`nils model admit <model> --check <file>`<br>`nils model promote <model>`<br>`nils model retire <model>` |
| export | `nils model show <model> --json` |
| delete | with the registry |

## clinical layer

| | |
|---|---|
| what | what v0 kept in a second database, in the one registry (Wave 4a section 7.1): cohorts and their members, the vocabulary of diseases and observation kinds, each subject's diseases, the events, and the subject's demographics |
| where | `<home>/registry.db`, mode 600 (SQLite keeps registry.db-wal and registry.db-shm beside it while a connection is open) |
| holds | quasi-identifying: birth dates, sex, dates of death, the dates of diagnoses, onsets, treatments and every observation<br>clinical: diagnoses and their types, the scales and their values, the treatments |
| owner | the research group that owns the cohort |
| kept | for ever; a correction supersedes the old row and the old row stays (section 13.2) |
| read | `nils clinical vocabulary list`<br>`nils clinical cohort list`<br>`nils clinical cohort show <name>`<br>`nils custody` |
| change | `nils clinical vocabulary load`<br>`nils clinical cohort make \| rename \| set \| retire \| add \| remove`<br>`nils ask promote`<br>a digest of a dataset that feeds a cohort |
| export | `nils release` |
| delete | remove `<home>/registry.db` (nils has no command for it) |

## campaigns and label sets

| | |
|---|---|
| what | campaigns (record 42): the question, the frozen item list, each item's review item, the raters' leases, every answer with who gave it, and what each item came to; and the label sets written out of the decisions or a campaign, with the digest of each and where it went; and the stacks of the samples an operator sealed for certification, which make a set that holds any of them sealed (record 40 R3) until the certificate recorded for the sample unseals it (record 48 R2); and how long each answer took, the suggestion it had and whether it changed it (record 48 R1); and the axes derived from each answer through the pack (record 48) |
| where | rows of campaign, campaign_item, campaign_assignment, campaign_answer, label_set, sealed_stack and certificate in the registry; a label set's labels.tsv and provenance.json in the export place it was written to |
| holds | quasi-identifying: the day a session opened, on a pick campaign's items and its labels<br>a person's words: the why and the form of an answer<br>technical: stack, subject and derivative ids, values, the principals, the times, the digests |
| owner | the research group that runs the campaign; each answer is its rater's |
| kept | for good: an answer is never deleted or overwritten, and a closed campaign keeps every answer; a label set's row stays after its files are removed; a seal's row stays when a certificate unseals it, naming the certificate |
| read | `nils campaign list`<br>`nils campaign show <campaign> [--answers]`<br>`nils campaign stats <campaign>`<br>`nils labels list`<br>`nils labels show <id>`<br>`nils labels certificates` |
| change | `nils campaign create \| claim \| answer \| release \| metric \| requestion \| close`<br>`nils labels import-v0 --tsv <file>`<br>`nils labels seal --select selection:<name>@<v>`<br>POST /api/certificates<br>POST /api/certificates/<id>/unseal |
| export | `nils campaign export <campaign> --to <dir> [--answers]`<br>`nils labels export --axis <axis> --to <dir>`<br>`nils labels export --for-training --to <dir>` |
| delete | with the registry |

## claims cache

| | |
|---|---|
| what | what nils serve keeps of a token it verified (Wave 4c section 5.9): the subject, the grants and the detail, the display name and the mail the token carried, and who acted for the subject; in memory, for the token's lifetime |
| where | the memory of nils serve; nothing on disk |
| holds | quasi-identifying: the subject, the display name, the mail<br>technical: the grants and the detail, the expiry, the actor |
| owner | the registry's operator |
| kept | until the token expires, at most its lifetime; gone at restart |
| read | GET /api/capabilities, for the caller's own entry |
| change | no command |
| export | no command |
| delete | restart nils serve |

## backups

| | |
|---|---|
| what | an archive of the registry and the linkage store with a manifest (Wave 4c section 6.5), written by nils backup or the backup job; the key store is never in one and is copied on its own |
| where | the directory nils backup --dir or nils serve --backup-dir names; <home>/backups by default; <home>/backups-before-restore before a restore |
| holds | everything the registry and the linkage store hold, at the moment of the archive<br>technical: the manifest, with sizes and digests, and the last check of the archive |
| owner | the registry's operator |
| kept | until removed; the newest N of the registry when a backup runs with --keep N, as a scheduled backup does |
| read | `nils verify <archive> [--rehearse]`<br>GET /api/backups |
| change | `nils backup [--dir <dir>] [--keep N] [--rehearse]`<br>PUT /api/backups/schedule<br>`nils restore <archive> --yes` (with nils serve stopped) |
| export | copy the archive directory |
| delete | remove the archive directory |

## job records

| | |
|---|---|
| what | every verb that runs longer than a second, and the queue (Wave 4a section 9.1): its command line and arguments, host and pid, heartbeat, progress, counts and outcome |
| where | rows of job and ingest_batch in the registry |
| holds | quasi-identifying: the root path in a run's arguments and command line<br>technical: the counts, the host name, the pid, the times, the outcome |
| owner | the registry's operator |
| kept | one year of finished jobs (Wave 4a section 13.1); a running or queued one until it is over |
| read | `nils status [--batch <id>]`<br>`nils jobs list [--all]`<br>`nils jobs show <id>` |
| change | `nils jobs cancel <id>`<br>`nils jobs enqueue -- <command>`<br>`nils jobs work`<br>`nils jobs resume <id>` |
| export | `nils status --json`<br>`nils status --batch <id> --json`<br>`nils jobs list --all --json` |
| delete | `nils jobs prune [--keep-days <n>]` |

## audit log

| | |
|---|---|
| what | who did what, to which scope, when, under which policy (Wave 4a section 9.2): every decision, acknowledgement, import, vocabulary load, linkage change, release and handover, as the principal user@node |
| where | rows of audit in the registry |
| holds | quasi-identifying: the principal, a release's root path<br>technical: the action, the scope's ids and counts, the policy, the time; never an identifier |
| owner | the registry's operator; read by whoever answers for the archive |
| kept | for ever (Wave 4a section 13.1); nobody deletes an audit row |
| read | `nils audit list [--principal <who>] [--action <action>] [--since <stamp>]` |
| change | no command |
| export | `nils audit list --json` |
| delete | with the registry |

## session cache

| | |
|---|---|
| what | each subject's sessions under each window, built by the one resolver over the subject's whole timeline (Wave 4b section 7), with the labels a scheme gives them beside the identity |
| where | rows of session_cache, session_cache_study and session_label in the registry |
| holds | quasi-identifying: the first and last study day of each session<br>technical: the surrogate id, the window, the timeline digest, the labels |
| owner | the registry's operator |
| kept | no retention: rebuildable, and dropped on rebuild (Wave 4b section 14.1) |
| read | `nils session list` |
| change | no command |
| export | no command |
| delete | with the registry, or by the next rebuild |

## questions and their answers

| | |
|---|---|
| what | a saved question (a selection) with its immutable versions, the handle a question left behind with the keys it named and the pages it was read by, and an uploaded identifier list by reference (Wave 4b section 8) |
| where | rows of selection, selection_version, handle, handle_member, handle_page, values_source, values_member and ask_document in the registry |
| holds | quasi-identifying: the subject keys a handle named, the dates and ages its pages carry<br>technical: the question itself, its hash, its provenance (who, when, node, pack, epoch, scheme) |
| owner | the research group that asks; the data controller sets the retention |
| kept | handle, its question and its hash: for ever, withdrawable with a reason; handle_member and handle_page: 90 days after the last read, longer while a cohort or a selection names the handle; values_member: the lifetime of the handle that cites it, the upload itself gone on resolution; selection and selection_version: for ever, they hold the question and never subject data (Wave 4b section 14.1) |
| read | `nils custody` |
| change | no command |
| export | no command |
| delete | the rows by nils ask handles prune (slice 10 of Wave 4b); the record with the registry |

## catalog curation

| | |
|---|---|
| what | what a person wrote about a catalog path: a description, a caveat, guidance for an assistant, a visibility or a class, keyed by the path so a re-sync never overwrites it (Wave 4b section 9) |
| where | rows of catalog_curation in the registry |
| holds | technical: prose about fields and kinds; never a subject's data |
| owner | the research group |
| kept | for ever (Wave 4b section 14.1) |
| read | `nils custody` |
| change | no command |
| export | no command |
| delete | with the registry |

## identifier read audit

| | |
|---|---|
| what | who read which handle, when, which columns and how many rows, and for what purpose: every page read and every export at every detail (Wave 4c section 6.1), and every identifier projection (Wave 4b section 9), with who acted for the principal |
| where | rows of handle_read_audit in the registry |
| holds | quasi-identifying: the principal<br>technical: the handle, the columns, the row count, the epoch, the time; never an identifier |
| owner | the registry's operator; read by whoever answers for the archive |
| kept | for ever, like the rest of the audit (Wave 4b section 14.1) |
| read | `nils custody` |
| change | no command |
| export | no command |
| delete | with the registry |

## derivatives

| | |
|---|---|
| what | files made from the archive that are not the archive, a mask, an embedding, a pipeline's output, each named by its sha256 and kept in a working place, with a row saying what it is, what it belongs to, where it lives, its bytes and digest, and who registered it (record 42) |
| where | files under derivatives in a working place, and none is bound now, so none can be added; rows of derivative in the registry |
| holds | quasi-identifying: drawn from the pixels of a subject's stacks, and the stack, series or subject each belongs to<br>technical: the kind, the digest, the size, the media type, the place and the path, who registered it |
| owner | the research group that owns the archive |
| kept | for ever; a newer file supersedes an older one by a link and both stay |
| read | `nils derivative list`<br>`nils derivative show <id>`<br>GET /api/derivatives/{id}/content<br>GET /api/derivatives/{id}/content?transport=share, where the place declares a share path |
| change | `nils derivative add <file>` |
| export | GET /api/derivatives/{id}/content |
| delete | with the registry and the working place; nils has no command for one |

## pipelines

| | |
|---|---|
| what | the pipeline catalog (record 43): each descriptor (nils.job.yml) kept whole with its digest, its image pinned by a registry manifest digest; and each run over a frozen selection: every parameter, the runtime and its version, the host and the device, the models and the label set it read, the handle it pinned, its status, its summary and the digest of its results; each unit of a run as the pipeline lane scheduled it (record 49); the lane's budget, card and places, and the path of each secret input the site set; in the lane's scratch place, the input it was given (a release in the BIDS layout, or stacks.json, and each unit's own where units run apart), its manifest, its containers' logs and apptainer's copies of images, by digest |
| where | rows of pipeline and pipeline_run in the registry; a run's folders go under runs/<run> in a working place, and none is bound now, so no pipeline can run |
| holds | quasi-identifying: a run's input folder is a release of its selection (pixels and dates), and its log is what the pipeline printed<br>technical: names, versions, digests, parameters, the runtime, host and device, the principals and the times<br>secret: none; a secret input's path is kept, never its bytes, and what a container left is swept of it |
| owner | the operator who added each pipeline; each run is its principal's |
| kept | the rows for good, since a derivative names the run that made it; a run's folder under runs until an operator removes it |
| read | `nils pipeline list`<br>`nils pipeline show <pipeline>`<br>`nils pipeline runs [<run>]` |
| change | `nils pipeline add <nils.job.yml>`<br>`nils pipeline runtime --set <choice>`<br>`nils pipeline lane --cores <n> --memory-gb <n> --gpu-card <n\|none> --output-place <place> --scratch-place <place>`<br>`nils pipeline secret set <id> --file <path>`<br>`nils run <pipeline> --select selection:<name>@<v>`<br>`nils run --resume <run>` |
| export | `nils pipeline show <pipeline> --json`<br>`nils pipeline runs <run> --json` |
| delete | with the registry and the working place; nils has no command for one |

## logs

| | |
|---|---|
| what | none: progress is printed to stderr and not stored; the counts of a run are its batch record |
| where | nowhere |
| holds | nothing |
| owner | the registry's operator |
| kept | not kept |
| read | no command |
| change | no command |
| export | no command |
| delete | nothing to delete |

