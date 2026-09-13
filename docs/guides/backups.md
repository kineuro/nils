# Backups

An archive is one directory: the registry and the linkage store, the registry's configuration, and a manifest naming every file with its size and digest. The key store is never in an archive. Copy it on its own, as the custody page says: without it, a restored registry cannot give a subject the same code again.

## Back up now

```sh
nils backup [--dir <dir>] [--keep N] [--rehearse]
```

With no `--dir` the archive goes to the backup place the registry's place names, else `<home>/backups`. `--keep N` keeps the newest N archives of this registry in that directory and removes the others; a directory that holds another registry's archive, or no manifest, is never removed. `--rehearse` rehearses a restore of the archive once it is written, and the backup fails if the rehearsal does.

Through the engine's doors a backup is a job, written to the directory `nils serve --backup-dir` names:

```sh
curl -X POST http://127.0.0.1:8437/api/jobs \
  -H 'content-type: application/json' \
  -d '{"command": ["backup", "--keep", "14", "--rehearse"]}'
```

## Check an archive

```sh
nils verify <archive> [--rehearse]
```

`nils verify` checks every file against the manifest. `--rehearse` also opens every store as a restore would, without applying it: a SQLite store read only, with its integrity checked and its registry named, and a Postgres dump read back by `pg_restore --list`. The outcome is kept beside the manifest in `checked.json`, and the list of archives shows it.

## On a schedule

An admin sets the schedule from the desk's settings, or at the door:

```sh
curl -X PUT http://127.0.0.1:8437/api/backups/schedule \
  -H 'content-type: application/json' \
  -d '{"every": "day", "at": "02:00", "keep": 14}'
```

`every` is `off`, `day` or `week`; a week names its `day`, such as `sunday`. The time is read in the registry's timezone, so every day at 02:00 is two in the morning where the registry is, whatever the host's clock says. The schedule is kept in the registry, so it moves with a restore.

A `nils serve` that runs the queue (`--worker`) and has a `--backup-dir` queues a backup each time the schedule comes round, rehearsed and kept to `keep`. A host that was off at the hour queues one backup when it is back, not one for every hour it missed, and a scheduled backup waits while another backup is queued or running.

`GET /api/backups` lists the archives, newest first, each with its size, how long it took, whether it is this registry's and its last check, beside the schedule and when it next comes round.

## The registry's calendar

Every answer with a date is read in the registry's timezone, and a week starts on the registry's day, never the browser's. Both change with `nils settings set`, or at `PUT /api/settings`:

```sh
nils settings set timezone Europe/Stockholm
nils settings set week_start sunday
```

The epoch moves with a change, since every dated answer may change with it, and the change is audited.

## Restore

```sh
nils restore <archive> --yes
```

Stop `nils serve` first. The archive is verified, and the registry as it stands is archived to `<home>/backups-before-restore` before the stores are replaced.
