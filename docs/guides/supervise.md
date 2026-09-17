<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# The supervisor

`nils supervise` is a subcommand of the one binary, run as a service on a
host with the privilege to replace the parts on that host and nothing else
(Wave 5, section 10.4). It reports what is installed with versions and
contract versions, watches a release channel that names signed artifacts,
fetches an artifact, verifies its signature against a key the deployment
holds, applies it, restarts the part, and answers the desk over the same
contract shape everything else does. The product ships the verifier and the
format; the channel and the signing key are the deployment's.

Everything the supervisor does can be done by hand with the same verbs, and
the last section says how. Where no supervisor is installed, the desk shows
the command alone.

## The artifact

An artifact is three files beside each other, named after the part, its
version and the target it was built for, `<part>-<version>-<target>`:

| file | what it is |
|---|---|
| `<stem>.tar.gz` | the part's files, relative paths, deterministic order |
| `<stem>.json` | the manifest: `part`, `version`, `target`, `contracts` (the contract versions the part speaks, `openapi`, `pack`, `review_item`, `suite`, `mcp`), `sha256` of the tarball, `built_at`, `files` |
| `<stem>.sig` | an ed25519 signature over the manifest's bytes, hex |

The target is `<os>-<arch>` as the binary sees it, `linux-x86_64` on the
group's hosts. The signature covers the manifest, and the manifest names the
tarball's digest, so a tarball that was changed fails the digest and a
manifest that was changed fails the signature.

To make one from a directory of built files:

```sh
nils supervise pack --part engine --version 1.1.0 --dir out/engine --out channel/engine \
  --contract openapi=3 --contract pack=4 --contract review_item=4 --contract suite=1 --contract mcp=1
nils supervise sign --key keys/supervise.key channel/engine/engine-1.1.0-linux-x86_64.json
```

## The keys

```sh
nils supervise keygen --out keys
```

writes `keys/supervise.key`, the private key (PKCS#8, mode 600), and
`keys/supervise.pub`, the public key as one hex line. The private key lives
where releases are built and nowhere else. The public line goes into the
trust file on every host, one key per line; a deployment may trust more than
one key, and it rotates a key by adding the new line before the old one is
removed.

## The channel

The channel is one directory per part, reachable by URL, holding the
artifacts and a `latest.json`:

```json
{
  "version": "1.1.0",
  "manifest_url": "engine-1.1.0-linux-x86_64.json",
  "artifact_url": "engine-1.1.0-linux-x86_64.tar.gz",
  "signature_url": "engine-1.1.0-linux-x86_64.sig"
}
```

Relative names are read beside `latest.json`; absolute URLs are taken as
they are. `https://` and `file://` both work, so a channel can be a static
web directory or a share the hosts mount. Publishing a release is copying
the three files in and rewriting `latest.json`.

## The service

`supervise.toml`:

```toml
bind = "127.0.0.1:8470"
trust = "trust.pub"
log = "supervise.log"
poll_seconds = 900
settle_seconds = 30

[tokens]
"a-long-random-token" = "admin@host"

[[parts]]
name = "engine"
install = "/opt/nils/engine"
restart = "systemctl --user restart nils-engine"
channel = "https://releases.example.org/nils"
capabilities = "http://127.0.0.1:8437/api/capabilities"
```

Relative paths are read beside the file. An install whose services are the
machine's own restarts them without `--user`: `systemctl restart
nils-engine`. Every part names where it is
installed, the command that restarts it (run by `sh -c`), the channel, and
how the supervisor tells it came back: its capabilities door, whose
`engine.version` (or `desk.version`, or `version`) must answer with the new
version within `settle_seconds`, or a `version_file` under the install
directory whose content is the version, for a part without a door.

```sh
nils supervise run --config supervise.toml
```

Three doors, each under a bearer token from `[tokens]` (the admin
entitlement; the desk reaches them through its own proxy):

| door | what it does |
|---|---|
| `GET /api/supervise/capabilities` | the supervisor's version and target; per part: what is installed (from `installed.json` in the install directory), its contracts and digest, its health as its own door answers, and `newer` when the channel's latest is ahead |
| `POST /api/supervise/update` `{part, version?, allow_contract_change?}` | fetch the channel's latest (a `version` that is not the latest is refused by name), verify, apply, restart, settle; 200 with the log row, 409 with the verifier's or the settle's named reason |
| `GET /api/supervise/log` | every apply and every refusal, one row each, with the manifest digest |

Refusals are named: `no signature`, `bad signature`, `digest mismatch`,
`target mismatch`, `contract mismatch`. A contract mismatch is a contract
whose major moved against what is installed; the desk's closure panel names
what changes ("engine 1.1 changes the pack contract to v5; two apps speak
v4") and passes `allow_contract_change` when a person accepts it.

An apply unpacks into `<install>/.staging/<version>`, checks every file the
manifest names arrived, moves the files in place one by one keeping the
previous ones under `<install>/.previous`, writes `installed.json`, runs the
restart command and waits for the part to answer with the new version. A
part that does not come back is rolled back from `.previous` and restarted
again, and the log row says so.

## The install

A supervisor on a host where `nils setup` made the install also reports that
install and acts on it, from the setup record of the account it runs as
(`~/.config/nils/setup.toml`):

- `GET /api/supervise/install` says where everything lives and how it runs,
  each part's version, where each part answers and who can reach it there,
  whether each service runs, whether a newer release is out, and the
  machine's card.
- `POST /api/supervise/restart` restarts one part, or every part in the order
  they start.
- `POST /api/supervise/reapply` writes the engine's unit or container again
  from the record, with every source place the registry holds, and starts
  only the engine again. A container sees only what was mounted when it
  started, so a folder added as a source needs this.
- `POST /api/supervise/update-all` runs `nils update --all`.
- `POST /api/supervise/look` says what a folder holds before it is added:
  each folder inside with its files, how many of a sample are DICOM, and
  their modalities and scanners.
- `POST /api/supervise/folders` lists the folders inside a folder, for a path
  browser: each with whether this account may open it, whether a disk is
  mounted there, and whether `/etc/fstab` names a disk there that nothing is
  mounted for, beside the disk the folder is on with its free and total room.
  With no path it says where to start: the root, the home folder, every disk
  mounted that holds data, and every disk `/etc/fstab` names that is not
  mounted. The supervisor serves one call at a time, so a folder that takes
  more than five seconds to read, as a network mount that does not answer
  may, is answered without its listing, and the answer says so.

A restart, a reapply and an update run apart from the call that started them,
one at a time, and `GET /api/supervise/runs/{id}` says how each went, with the
last lines of its output. By hand they are `nils supervise restart --part
engine`, `nils supervise reapply --part engine` and `nils update --all`.

## The privilege, where the services are the machine's own

Restarting those services and replacing the parts' files is root's, and the
supervisor does not run as root to do it. `nils setup --system` writes a small
root owned program, `/usr/local/sbin/nils-manage`, and a rule in
`/etc/sudoers.d/nils-manage` naming the whole command lines the account the
supervisor runs as may call it with, and no others:

```
nils-deploy ALL=(root) NOPASSWD: /usr/local/sbin/nils-manage restart engine,
/usr/local/sbin/nils-manage restart desk, /usr/local/sbin/nils-manage restart gateway,
/usr/local/sbin/nils-manage restart assistant, /usr/local/sbin/nils-manage restart all,
/usr/local/sbin/nils-manage reapply, /usr/local/sbin/nils-manage update
```

sudo matches a command and its words exactly, so a call carrying any other
word matches no rule and is turned away before the program is reached, and
the program itself answers only to those words: `restart` with the name of a
part of this install or `all`, `reapply`, which writes the engine's unit again
from the record and starts it, and `update`, which runs `nils update --all`.
It takes no path, no unit name and no command of its caller's.

The account is named with `--account supervisor=nils-deploy`, which `--system`
requires, and it is deliberately not the account any part runs as: the engine's
reads every scan in the registry, and an engine that could also replace the
parts and restart them would carry the machine with it. An install that leaves
it out, or names an account a part already runs as, is refused before anything
is written.

So on such an install the doors go through it: `POST /api/supervise/restart`
runs `sudo -n /usr/local/sbin/nils-manage restart <part>`, `POST
/api/supervise/reapply` runs `... reapply`, and `POST
/api/supervise/update-all` runs `... update`. Run as root, the same commands
do the work themselves and ask sudo for nothing. An install whose services are
an account's own has no helper and needs none: it restarts its own units.

What the account can do with it is restart this install's services and put new
files in place for its parts, which is what following a release is. What it
cannot do is run anything else as root, reach another install, or change the
rule, the accounts the parts run as, or the capabilities the engine keeps.

## The same steps by hand

Without a supervisor, or to see what one does, the four steps are these,
with the same verbs.

1. Fetch the three files from the channel into a directory.

   ```sh
   curl -sO https://releases.example.org/nils/engine/engine-1.1.0-linux-x86_64.json
   curl -sO https://releases.example.org/nils/engine/engine-1.1.0-linux-x86_64.sig
   curl -sO https://releases.example.org/nils/engine/engine-1.1.0-linux-x86_64.tar.gz
   ```

2. Verify the signature, the digest and the target against the trust file,
   and the contracts against what is installed.

   ```sh
   nils supervise verify --trust /etc/nils/trust.pub \
     --installed /opt/nils/engine/installed.json \
     engine-1.1.0-linux-x86_64.json
   ```

   The verb exits 1 with the named reason on any failure and prints the
   manifest on success. Do not go on after a refusal.

3. Unpack beside the install and swap the files.

   ```sh
   mkdir -p /opt/nils/engine/.staging/1.1.0 /opt/nils/engine/.previous
   tar -xzf engine-1.1.0-linux-x86_64.tar.gz -C /opt/nils/engine/.staging/1.1.0
   mv /opt/nils/engine/nils /opt/nils/engine/.previous/nils
   mv /opt/nils/engine/.staging/1.1.0/nils /opt/nils/engine/nils
   cp engine-1.1.0-linux-x86_64.json /opt/nils/engine/installed.json
   ```

4. Restart the part and check it answers with the new version.

   ```sh
   systemctl --user restart nils-engine
   curl -s http://127.0.0.1:8437/api/capabilities | grep '"version"'
   ```

   Where the services are the machine's own, that is `systemctl restart
   nils-engine`, without `--user`. If the part does not come back, put
   `.previous/nils` back and restart again.

`nils supervise update --config supervise.toml engine` does the four steps
in one go from the command line, with the same log row the door writes.
