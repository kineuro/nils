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
assistant, `assistant/` and `kvasir/`. A directory of DICOM the engine may
read can be named here; it becomes an ingest root and a `source` place, and
it is mounted read only in a container. `--dir PATH`, `--source PATH`.

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
may open it: only this machine, or this network. Choosing the network while
nobody signs in says plainly that anyone on that network who finds the port
gets the whole registry, and offers to keep the people in the desk instead.
The desk's `bind`, `origin` and `also_origins` follow the answer. Where a
default port is taken, the wizard says which and moves that part to the next
free one; where they are free it asks nothing.

Where the desk keeps the people, the wizard offers to add the first person,
who may do everything. It holds the name and the password to the desk's own
rules as they are typed (a username of letters, digits, dots, dashes and
underscores; a password of at least eight characters, its spaces kept), so an
answer the desk would refuse is asked for again. For an identity provider it
prints the `nils-desk register --authentik ...` line rather than pretending
to have run it.

An install that would not work is not finished. Before anything is placed,
the plan names what this machine lacks for it (Node 22, git and npm for the
assistant; podman or docker for their runtime), and nothing is changed until
it is there. A model is taken only once it answers one short question. While
installing, a failure that leaves a chosen part unusable stops the install
with the reason: the desk not installed or nobody added to it, a place not
declared, the rule packs, the assistant, Kvasir, its model or its key, or a
service that does not start. `nils uninstall` removes what was placed,
and `nils setup` starts again. An update or a repair says the same and goes
on, so what still works keeps running.

**6. What this machine can do.** The wizard reads the graphics card
(`nvidia-smi`, then `rocm-smi`, then an Apple machine's unified memory) and
says what that means before the assistant is offered:

| The card | What fits |
|---|---|
| 24 GB or more | A 27B model at 4 bit, with room for the context. That is what the stations were written against. |
| 12 to 24 GB | A 7B to 14B model. The stations still work, with more retries on the harder questions. |
| Under 12 GB, or none | No local model worth serving. A model on another machine you can reach, a commercial provider through Kvasir, the model gateway (the prompt then leaves the machine, and Kvasir marks that backend remote), or no assistant at all, which costs nothing else. |

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
there is no service manager the commands are printed instead.
`--service` / `--no-service`.

**8. The plan, and then the work.** Everything decided, on one screen, and
under `--print` the exact commands and unit files as well. Then the parts
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
backend and the places declared:

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
unit, a Node part is its release tag fetched, checked out and built again.
One line each, and the engine last, since it replaces the binary doing the
replacing. `nils setup --update` is the same work from the wizard's side.

## Removing it

```
nils uninstall
```

It asks what should go. Keeping your data removes the services, the
programs, the packs and Kvasir's directory, with the models Kvasir holds,
their keys, its subscriptions and the assistant's key, and keeps the
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
| `--backend sqlite\|postgres`, `--dsn`, `--schema` | Where the registry is kept |
| `--source PATH` | A directory of DICOM the engine may read |
| `--key-file FILE` | The registry key's passphrase, instead of a prompt |
| `--service`, `--no-service` | Write and start services, or do not |
| `--yes`, `-y` | Take every default without asking |
| `--print` | Say what it would do, with every command and unit, and change nothing |
| `--update` | Straight to the installs, for the parts already there |
| `--channel URL` | Where releases come from |

Windows is not supported yet; the wizard says so and points at the
documentation.
