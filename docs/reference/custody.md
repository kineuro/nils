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
| kept | until deleted; nothing expires on its own, and a run marks files that vanished as gone instead of deleting their rows |
| read | `nils status [--batch <id>]`<br>`nils quarantine list`<br>`nils review list` |
| change | `nils digest <root>` |
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
| change | `nils digest <root>`<br>`nils linkage import <csv>`<br>`nils linkage link \| unlink`<br>`nils linkage id-type add` |
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
| what | what a pack decided about each stack, one row per axis, with the evidence that made it and any decision a person recorded |
| where | rows of stack_fingerprint, classification, classification_axis, classification_evidence and decision in the registry |
| holds | technical: the fields a pack reads, the axes, the tiers and confidences, the rule that fired<br>a person's words: the why on a decision |
| owner | the pack's author for the rules, the reviewers for the decisions |
| kept | until the next run of that job replaces it; a decision until withdrawn, and a withdrawn one for good |
| read | `nils explain <stack>`<br>`nils review list`<br>`nils pack show <name>` |
| change | `nils fingerprint`<br>`nils classify`<br>`nils review decide <id> --value <v>` |
| export | `nils explain <stack> --json` |
| delete | with the registry |

## clinical layer

| | |
|---|---|
| what | what v0 kept in a second database, in the one registry (Wave 4a section 7.1): cohorts and their members, the vocabulary of diseases and observation kinds, each subject's diseases, the events, and the subject's demographics |
| where | `<home>/registry.db`, mode 600 (SQLite keeps registry.db-wal and registry.db-shm beside it while a connection is open) |
| holds | quasi-identifying: birth dates, sex, dates of death, the dates of diagnoses, onsets, treatments and every observation<br>clinical: diagnoses and their types, the scales and their values, the treatments |
| owner | the research group that owns the cohort |
| kept | for ever; a correction supersedes the old row and the old row stays (section 13.2) |
| read | `nils clinical vocabulary list`<br>`nils custody` |
| change | `nils clinical vocabulary load` |
| export | `nils release` |
| delete | remove `<home>/registry.db` (nils has no command for it) |

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
| what | who projected identifiers through which handle, when, which columns and how many rows, at every role (Wave 4b section 9) |
| where | rows of handle_read_audit in the registry |
| holds | quasi-identifying: the principal<br>technical: the handle, the columns, the row count, the epoch, the time; never an identifier |
| owner | the registry's operator; read by whoever answers for the archive |
| kept | for ever, like the rest of the audit (Wave 4b section 14.1) |
| read | `nils custody` |
| change | no command |
| export | no command |
| delete | with the registry |

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

