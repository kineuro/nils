<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Setting NILS up

`nils setup` installs every part of NILS, makes what they need, and writes
down what it did. It is a wizard: one step at a time, numbered choices with
the default in brackets, and nothing written before a summary has said what
will be written. It is the other end of the one line installer, which
fetches the binary; the binary does the rest.

```
nils setup
```

Piped, with no terminal to ask on, it takes every default and says so, so
`curl ... | sh` never waits at a prompt. `NILS_NO_TTY=1` says the same thing
to a script that does have a terminal.

## The eight steps

**1. What to install.** The engine alone, the engine and the desk (the
default), or everything, which adds the assistant and Kvasir, the model
gateway.
`--parts engine,desk,assistant` answers it.

**2. Where it runs.** On this machine, as two small binaries, or in
containers. Podman is preferred where both are installed; where neither is,
the machine is the only offer and the wizard says which to install for the
other. `--runtime machine|podman|docker`.

**3. Where it lives.** One directory, `~/nils` by default, holding
`registry/`, `desk/`, `backups/`, `working/`, `export/` and, with the
assistant, `assistant/`, `kvasir/` and `llama.cpp/`. A directory of DICOM the engine may
read can be named here; it becomes an ingest root and a `source` place, and
it is mounted read only in a container. `--dir PATH`, `--source PATH`.

A site that has places of its own names them instead, once each, with the
role and the guarantees they really have: `--place
archives=/data/archives,role=backup,snapshots,protected`. After the name and
the directory come the words that describe it. `role=` is one of source,
registry, working, export, share, exchange or backup, and it is what binds
what may be written there. `backup=` names the backup place that backs this
one up. `snapshots`, `protected` and `fast` are what the site declares about
the storage behind it. `read` makes it one of the directories the engine
reads DICOM from, which the desk offers by name and a queued job may name; a
source place is a dataset and is read whether or not it is marked, and a
place of any other role is read only where the site marked it, so declaring
a place never widens what can be browsed. Where any place is named, those
are the places the install declares, in place of the five above, and
`--source` is not taken beside them. The place with the `registry` role is
where this install keeps its registry, and the archives go to a backup place
that is named: anything else about them is said before a file is written,
with the fix in the sentence.

`--backup-dir DIR` is where the engine writes its archives, which is
otherwise the backup place the registry names, or `<dir>/backups` for an
install that named no places. `--pack-dir DIR` is where the rule packs are
put and where the engine is told to read them from, and `--workers N` how
many requests it answers at once.

**4. The registry.** Where a registry already exists, it is left alone.
Otherwise the wizard asks for a passphrase for the registry's key, twice and
never echoed, explains in one line what the key is for, and asks whether the
registry itself is a SQLite file or a Postgres database. A Postgres
connection is tried before anything is written, so a wrong connection string
costs a sentence rather than half a registry. `--backend sqlite|postgres`,
`--dsn`, `--schema`, `--key-file FILE`.

Where there is no terminal to ask on and no `--key-file`, a passphrase is
made from the machine's own randomness and written to `<dir>/key.passphrase`
readable by nobody else. Move it into your password manager and delete the
file.

**5. Who may sign in.** Three answers, and each one sets both parts at once:

| | The engine | The desk |
|---|---|---|
| Nobody | `--auth off` | `mode = "off"` |
| The desk keeps the people | `--auth oidc`, trusting the desk | `mode = "local"` |
| An identity provider | `--auth oidc` | `mode = "oidc"` |

`--mode off|local|oidc`. With the desk installed, the wizard then asks who
may open it: only this machine, this network, or a proxy of yours, which is
how a site puts the desk behind a name of its own with a certificate.
Choosing the network or a proxy while nobody signs in says plainly that
anyone who reaches it gets the whole registry, and offers to keep the people
in the desk instead. The desk's `bind`, `origin` and `also_origins` follow
the answer. Where a default port is taken, the wizard says which and moves
that part to the next free one; where they are free it asks nothing.

Behind a proxy the address a browser opens and the address the desk binds
are two answers and not one. `--origin https://nils.example.org` gives the
first: it is what the desk compares a write against, what it signs the
tokens the other parts trust with, and what a person is sent back to after
signing in at a provider, so the engine's trust in the desk follows it too.
The desk still binds this machine's loopback, for a proxy running here, and
with `--reach network` beside it every address, for a proxy on another
machine; either way the loopback stays among the addresses it also answers
at, so a browser on the machine keeps working. An address that is not a
scheme and a host is refused before anything is written, in words naming
what is wrong with it.

Where the desk keeps the people, the wizard offers to add the first person,
who may do everything. It holds the name and the password to the desk's own
rules as they are typed (a username of letters, digits, dots, dashes and
underscores; a password of at least eight characters, its spaces kept), so an
answer the desk would refuse is asked for again. For an identity provider it
prints the `nils-desk register --authentik ...` line rather than pretending
to have run it.

An install that would not work is not finished. Before anything is placed, the
plan names what this machine lacks for it (Node 22, git and npm for the
assistant, and on Linux the libraries llama.cpp links, such as `libgomp.so.1`
from `libgomp1` on Debian and Ubuntu; podman or docker for their runtime), and
nothing is changed until it is there. A model is taken only once it answers
one short question. While installing, a failure that leaves a chosen part
unusable stops the install with the reason: the desk not installed or nobody
added to it, a place not declared, the rule packs, the assistant, Kvasir, its
model or its key, or a service that does not start. `nils uninstall` removes
what was placed, and `nils setup` starts again. An update or a repair says the
same and goes on, so what still works keeps running.

**6. What this machine can do.** The wizard reads the graphics card
(`nvidia-smi`, then `rocm-smi`, then an Apple machine's unified memory) and
says what that means before the assistant is offered:

| The card | What fits |
|---|---|
| 24 GB or more | A 27B model at 4 bit, with room for the context. That is what the stations were written against. |
| 12 to 24 GB | A 7B to 14B model. The stations still work, with more retries on the harder questions. |
| Under 12 GB, or none | No local model worth serving. A model on another machine you can reach, a commercial provider through Kvasir, the model gateway (the prompt then leaves the machine, and Kvasir marks that backend remote), or no assistant at all, which costs nothing else. |

With the assistant, setup also takes llama.cpp's server for this machine,
which runs a model Kvasir downloads once an admin starts it from Kvasir: on
Linux the Vulkan build where there is a graphics card or a render node and the
CPU build otherwise, and on macOS the build for its processor. Where a Linux
machine has no Vulkan loader, the plan says that a model then runs on the
processor, and names the package that brings one (`libvulkan1` on Debian and
Ubuntu, `vulkan-loader` on Fedora). On Linux, `ldd` reads what the build
links once it is unpacked, and before anything is placed where an earlier run
unpacked it: a library the machine lacks is named with its package where that
is known, and an install stops before any service starts. A build that does not
run at all is said in the loader's words. An update, a repair or the restart
after `nils update --all` says the same and holds llama.cpp alone back: its
unit is stopped and taken out of the boot, rather than left failing and
restarting on a build that has been replaced, and every other service starts
without it. Otherwise setup names the devices llama.cpp runs a model on.

The wizard never installs a model. It asks what the assistant talks to: a
model server on this machine or on another machine of yours, a commercial
provider, or a model named later. A model is taken only once it answers one
short question from this machine.

Where nobody signs in, it also offers your ChatGPT subscription. Once Kvasir
runs, setup shows a link and a code and waits while you open the link, sign
in with ChatGPT and enter the code. The assistant's purposes that read no
rows of the archive then go to the subscription, and the prompt leaves your
systems; those that read rows stay closed to it until an admin opens them in
the desk's settings. A sign-in that fails or outlives its code stops the
install, and `nils setup` run again signs in when you keep the subscription.
With nobody at the terminal, no sign-in is started. Where people sign in, a
subscription is each person's own, and each signs in to theirs from the desk.

**7. Keeping it running.** Systemd user units on Linux, podman quadlets for a
podman run, a compose file for a docker one, launchd agents on macOS. Where
there is no service manager the commands are printed instead. The units of a
machine or a podman install are the account's own, so systemd must have a
session of that account to take them: on a machine nobody is logged in to
there is none, and the wizard says so at this step and prints the commands
instead of writing units nothing would start. `--service` there is refused
before anything is written, with the account named, so that the fix, signing
in as it or `loginctl enable-linger <account>`, is made before the install
rather than after it. The account is lingered before the units are handed
over, which is what gives such an account a user manager at all. llama.cpp runs
on this machine whichever runtime the parts use: a systemd user unit of its
own, `nils-llama.service`, started before Kvasir, or a launchd agent on macOS.
`--service` / `--no-service`.

Those are the services of the account that runs setup, which is what one
machine with one person on it wants. `--system` writes the services of the
machine instead, in `/etc/systemd/system`, and each part runs as an account
of its own: `--account engine=nils --account desk=nils-desk --account
assistant=nils-assistant`, and `nils` for a part not named. It is asked for
outright and never arrived at, since it is written as root into a directory
of the machine's; it needs Linux, systemd, root and `--runtime machine`. Each
account named has to be on the machine already: a service account is the
site's own to make, and setup makes none. One that is not there is named in a
refusal before anything is written, at the question rather than when systemd
would fail to start the unit.

`--capabilities CAP_DAC_OVERRIDE,CAP_DAC_READ_SEARCH` gives the engine's
service the capabilities it needs to read and write across the filesystems a
site mounts, which is the reason for these services at all: a service of an
account's own cannot carry a capability, whatever it is asked for. A part
that runs as an account other than the engine's is kept out of the home
directories, and out of the registry, the archives and the folders of DICOM,
which it never reads: it asks the engine for what it shows. systemd looks at
each of those paths as root before the part starts, and one root cannot look
at, as on a share that squashes root, stops the part, so setup looks at them
first: a path the part's account cannot reach either is left out of its unit,
with a comment saying why, and one the account can reach is refused before
anything is written, with the folder to give root search permission on. Kept
out of the homes, llama.cpp is given `LLAMA_CACHE`, `HF_HOME` and
`HF_HUB_CACHE` under `<dir>/kvasir/runtime/cache` for the models it
downloads; where its unit sees a home, its cache stays where it was. What
each part reads and writes is given to the account that part runs as, so the
desk's folder is the desk's and Kvasir's is the assistant's, while the base
directory and the registry key's passphrase stay with the account that ran
setup. The services and the accounts are written down, so an update and a
repair write the same services rather than falling back to an account's own,
and one run by somebody who cannot write them says so before it changes
anything.

Root runs setup for such an install, and root is not the account the registry
has to answer: the engine's service reaches it as the engine's account. So
setup takes every step on the registry as that account, through the engine's
own binary under `runuser`, started in `/` with the environment the engine's
service runs with and nothing of root's. That account adds the registry's key,
with the passphrase on its input; makes the registry, with setup's own
pseudonym settings and the connection string on its input, never on a
command line, which every account on the machine can read; tries a Postgres,
the same way; declares the places; and reads the source places back. That is why a
Postgres that authenticates peers, which refuses root, and a registry on a
share that squashes root, which does not open to root, both work: each is
asked by the account that will use it. A registry that does not answer that
account is said, and never quietly replaced by the places in the setup record,
which hold none added at the desk since: an install, an update, a repair and a
reapply stop on it before they change anything, `--print` says it, and the
restart after `nils update --all` says it and how to mend it. A machine
without `runuser` is named among what the plan lacks, before anything is
placed.

Kvasir's and the assistant's source is taken the same way, as the account
those two run as. Git, `npm ci` and the build run under `runuser` in the part's
folder, which setup makes that account's before the clone, or hands to it
whole where a checkout is there already, so every file in it is born that
account's and a rerun, `nils setup --update` and `nils update --all` find a
checkout that account can fetch into and build. They run with the account's own home
where it owns one and may write there, and otherwise with
`<dir>/build-cache/<account>`, where npm keeps its cache; no part keeps
anything there, so it can be removed whenever nothing is building. Where root
cannot act as the account, because the machine no longer has it or has no
`runuser`, root takes the steps itself, and each git command in the checkout
names that one folder with `-c safe.directory` on its own command line.
Nothing is written into root's git settings or the machine's, which would
trust the folder for everything run as root from then on.

Something has to restart those services and replace the parts' files when a
release lands, and both are root's. The supervisor does not run as root to do
it, and the account that runs the engine is not given sudo: it reads every
scan in the registry, and sudo for it would make a compromise of the engine a
compromise of the machine. Setup writes a small root owned program instead,
`/usr/local/sbin/nils-manage`, and a rule in `/etc/sudoers.d/nils-manage`
naming the exact command lines one account may run it with: `restart` with
the name of a part of this install or `all`, `reapply` with or without `all`,
and `update`. The program answers to those words and nothing else, and takes
no path and no command of its caller's, so there is nothing to pass it that
makes it do more. The rule is read by `visudo` before it is put in place,
since a sudoers file sudo cannot parse shuts an operator out of root; where
visudo refuses it nothing is written and the install stops with what visudo
said. A machine without `sudo` or `visudo` is named among what the plan lacks,
before anything is placed.

The supervisor runs as the account the rule names, and `--system` requires it:
`--account supervisor=nils-deploy`. It is deliberately not the account any part
runs as, so there is no fallback to `nils`, and an install that leaves it out,
or that names an account a part already runs as, is refused at the question
before anything is written. Like every account this installer names it has to
be on the machine already, and setup makes none. That account can restart this
install's
services and put new files in place for its parts, which is what following a
release is. It cannot run anything else as root, reach another install, or
change the rule, the accounts the parts run as, or the capabilities the
engine keeps. `--print` shows the program and the rule in full, which is also
what to install by hand on a machine where setup may not write them.

The supervisor reports this install from the record, which lives in the home
of whoever installed it and which that account cannot read, so a copy of it
is written to `<dir>/supervise/config` every time the record is written. It
holds no secret: where the parts are, how they run, and what answers where.

An install whose services are the account's own needs none of this and grows
none of it: it restarts its own units and replaces files it already owns.

**8. The plan, and then the work.** Everything decided, on one screen, and
under `--print` the exact commands and unit files as well: for an install on
this machine every unit it would write, where it would write them, and every
call it would make to hand them over and start them; for the services of a
machine, each step taken as another account, after the `runuser` that runs
it: the registry's key and the step that makes the registry where there is no
registry yet, with the passphrase and the connection string on their input,
the source steps of Kvasir and the assistant, with the home they build with
where that is the build cache, and the one step that declares the places,
with each place it is given on its input; for a container run the commands
and the quadlets or the compose file.
Then the parts
that are missing are downloaded from their releases and checked against the
release's `SHA256SUMS`, the registry is made, the places are declared, the
desk's configuration is written, and the services are started. It ends with
what was installed, where, the addresses, and the next commands.

## Where the parts come from

The engine is the binary doing the asking, and the desk comes from
[`kineuro/nils-desk`](https://github.com/kineuro/nils-desk) releases, both
checked against the release's checksums and installed beside the running
`nils` or in `<dir>/bin` where that directory is not writable. `--channel`
(or `NILS_RELEASES`) points the whole thing at a deployment's own release
directory. In a container run the images are
`ghcr.io/kineuro/nils` and `ghcr.io/kineuro/nils-desk`, tagged as the
release is tagged, so version 1.0.0-alpha.2 is the image `v1.0.0-alpha.2`.
Both are public and pull without an account. Where an image cannot be
pulled, a Containerfile is written beside the base directory and the image
is built there from the release binary already downloaded.

The assistant and Kvasir, the model gateway, ship no binary: they are cloned
from [`kineuro/kvasir`](https://github.com/kineuro/kvasir) and
[`kineuro/nils-assistant`](https://github.com/kineuro/nils-assistant) at the
release tags this version of `nils` names, and built with Node 22. Where
Node 22, git or npm is missing, the plan says so and nothing is placed.

Kvasir's `kvasir.json` is written from its own example with the port this
setup chose, an admin token made here, how Kvasir knows its callers, and
every purpose the assistant's stations use. It names no model, since Kvasir
holds its models in its own database. Once Kvasir runs, the model you chose
is added through Kvasir, which tries it with one short request from where
Kvasir runs and holds it only once it answers. A model that answered the
wizard on this machine and not Kvasir, as a server on this machine's
loopback does not answer a container, stops the install with Kvasir's own
words. Kvasir warms and admits a local model it is given, and setup waits
for the admission. A backend that is not on this machine is marked remote,
and the wizard says that the prompt then leaves the machine. Until Kvasir
holds the model, it is kept with its key in
`<dir>/kvasir/backends-to-add.json`; that file and `kvasir.json` are
readable by nobody else.

llama.cpp comes from [its releases](https://github.com/ggml-org/llama.cpp/releases)
at the build this version of `nils` pins, b10964, and each archive is checked
against the sha256 pinned beside it. `NILS_SETUP_LLAMA_RELEASES` points at a
mirror laid out the same way (`<build>/llama-<build>-bin-<variant>.tar.gz`),
held to the same digests. An archive that cannot be downloaded, or that does
not match, stops an install. The build is unpacked into
`<dir>/llama.cpp/<build>-<variant>/` and runs in router mode: no model is
loaded until Kvasir loads one, and at most one at a time. It listens on
127.0.0.1:7110, or on docker's bridge for a docker install, and reads
`<dir>/kvasir/runtime/`: the presets Kvasir writes, and a key only Kvasir and
llama.cpp read. Its log is written there too. `kvasir.json` names it under
`local.runtime`, and where Kvasir runs in a container it also names
`hostAlias`, the name Kvasir reaches this machine's own loopback by.

`<dir>/assistant/assistant.env` holds everything the assistant reads: the
engine's address, Kvasir's, the key file, the model (`chatgpt` for the
subscription), the stores and the port. The assistant's key is minted at
Kvasir once Kvasir answers; where it is not up yet, `nils setup` and repair
adds the model and mints the key once it is.

## Containers

A podman run is one pod named `nils` publishing only the desk's port, with
the engine and the desk inside it and `:U` on every bind mount so a rootless
container owns what it reads. A docker run is a network named `nils`, the
desk publishing the port, and no `:U`. Either way the registry is made by
the engine itself, running the key and init steps inside a container against
the mounted volume, and the volumes are `<dir>/registry`, `<dir>/backups`,
`<dir>/desk` and any source directory named, that one read only.

Podman is kept running by quadlets in `~/.config/containers/systemd/`
(`nils.pod`, `nils-engine.container`, `nils-desk.container`); docker by a
`compose.yaml` in the base directory.

## What it wrote down

`~/.config/nils/setup.toml` (or `$XDG_CONFIG_HOME/nils/setup.toml`) records
the base directory, the parts with their versions and how each runs, the
mode, the runtime, the service manager, the ports, the reachability, the
address a browser opens the desk at where a proxy answers for it (`origin`,
written only for such an install), the backend, the places declared, the
layout a site described (`[site]`, written only where one was), and for an
install whose services are the machine's own the account each part runs as
and the capabilities the engine keeps (`[system]`, written only for such an
install):

```toml
dir = "/home/you/nils"
mode = "local"
runtime = "podman"
service = "podman quadlets"
reach = "loopback"
backend = "sqlite"
at = "2026-09-10T12:00:00Z"

[ports]
engine = 8437
desk = 7200
kvasir = 7100
assistant = 7300

[[places]]
name = "registry"
role = "registry"
path = "/home/you/nils/registry"

[parts.engine]
version = "1.0.0-alpha.2"
path = "ghcr.io/kineuro/nils:v1.0.0-alpha.2"
kind = "podman"
```

An install whose services are the machine's own also records them:

```toml
service = "systemd system units"

[system]
capabilities = ["CAP_DAC_OVERRIDE", "CAP_DAC_READ_SEARCH"]

[system.accounts]
desk = "nils-desk"
engine = "nils"
supervisor = "nils-deploy"

[helper]
account = "nils-deploy"
path = "/usr/local/sbin/nils-manage"
rule = "/etc/sudoers.d/nils-manage"
```

A site that named its own places records them, with the settings beside
them, so that an update and a repair declare the same places and write the
same settings:

```toml
[site]
pack_dir = "/srv/nils/engine/packs"
workers = 64

[[site.places]]
name = "archives"
role = "backup"
path = "/data/archives/registry"
snapshots = true
protected = true

[[site.places]]
name = "work"
role = "working"
path = "/work"
protected = true
fast = true
read = true
```

Run `nils setup` again and it opens with what is installed, where, in what
mode and how it runs, then offers to update everything, change something,
add a part, or repair, which writes the configuration and the services again
from what the state records. A `kvasir.json` from before Kvasir held its
models has its backends taken out, and each is added back through Kvasir
once it runs, under the same id and with the key its key file holds.

## Updating

```
nils update --all
```

Every part the state names, each in the way it runs: a binary is replaced
from its release, a container is a pull of the new tag and a restart of its
unit, a Node part is its release tag fetched, checked out and built again,
and llama.cpp's build is taken again when a version pins another, or taken
for an install from before that has the assistant, and restarted with the
rest. One line each, and the engine last, since it replaces the binary doing
the replacing. `nils setup --update` is the same work from the wizard's side.

## Removing it

```
nils uninstall
```

It asks what should go. Keeping your data removes the services, the
programs, the packs, llama.cpp's build and Kvasir's directory, with the
models Kvasir holds, their keys, its subscriptions and the assistant's key,
and keeps the
registry and its key, the backups, the desk's people and the assistant's
history. Everything removes the base directory as well, and the registry's
key cannot be recovered. `--keep-data` and `--purge` answer it.

## The flags

| | |
|---|---|
| `--parts engine,desk,assistant` | What to install |
| `--dir PATH` | The one directory everything lives under |
| `--runtime machine\|podman\|docker` | Where the parts run |
| `--mode off\|local\|oidc` | Who may sign in |
| `--reach loopback\|network` | Who may open the desk |
| `--origin URL` | The address a browser opens the desk at, where a proxy of yours answers for it |
| `--backend sqlite\|postgres`, `--dsn`, `--schema` | Where the registry is kept |
| `--source PATH` | A directory of DICOM the engine may read |
| `--place NAME=DIR,role=ROLE,...` | A place this site has, with its role and its guarantees |
| `--backup-dir DIR` | Where the engine writes its archives |
| `--pack-dir DIR` | Where the engine reads its rule packs |
| `--workers N` | How many requests the engine answers at once |
| `--key-file FILE` | The registry key's passphrase, instead of a prompt |
| `--service`, `--no-service` | Write and start services, or do not |
| `--system` | Write the services of this machine, in `/etc/systemd/system`; root's to do |
| `--account PART=ACCOUNT` | With `--system`: the account a part runs as; `supervisor=` is required, and must be nobody else's |
| `--capabilities LIST` | With `--system`: the capabilities the engine's service keeps |
| `--yes`, `-y` | Take every default without asking |
| `--print` | Say what it would do, with every command and unit, and change nothing |
| `--update` | Straight to the installs, for the parts already there |
| `--channel URL` | Where releases come from |

Windows is not supported yet; the wizard says so and points at the
documentation.
