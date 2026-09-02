# Custody

Every store the registry at `<home>` keeps (backend sqlite), rendered by `nils custody --markdown`: where it lives, which classes of data it holds (§4.3 of the Wave 1 specification), how long it is kept, and the command that reads, changes, exports or deletes it. Every command named here exists, and nothing is retained that this page does not show. `nils custody` prints the same table with the files and counts of the moment; `--json` is the machine-readable form.

## configuration

| | |
|---|---|
| what | nils.toml: the backend, the Postgres dsn if written there, the schema, the key store path |
| where | `<home>/nils.toml` |
| holds | technical<br>secret: a password in the dsn, when one is written there instead of NILS_DSN |
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
| kept | a file's row until the file changes or a run reads it again with --retry-quarantine; the review items until decided (review apply is Wave 4's) |
| read | `nils quarantine list [--batch <id>] [--class <c>]`<br>`nils review list [--kind ingest.quarantine]`<br>`nils review show <id>` |
| change | `nils digest <root> --retry-quarantine` |
| export | `nils quarantine list --json` |
| delete | with the registry |

## job records

| | |
|---|---|
| what | every run and purge: its arguments, host and pid, progress, counts and outcome |
| where | rows of job and ingest_batch in the registry |
| holds | quasi-identifying: the root path in a run's arguments<br>technical: the counts, the host name, the pid, the times, the outcome |
| kept | until deleted with the registry |
| read | `nils status [--batch <id>]` |
| change | no command |
| export | `nils status --json`<br>`nils status --batch <id> --json` |
| delete | with the registry |

## logs

| | |
|---|---|
| what | none: progress is printed to stderr and not stored; the counts of a run are its batch record |
| where | nowhere |
| holds | nothing |
| kept | not kept |
| read | no command |
| change | no command |
| export | no command |
| delete | nothing to delete |

