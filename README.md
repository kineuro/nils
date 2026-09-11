# NILS

**Neuroimaging Intelligent Linked System.** NILS reads DICOM into a registry, classifies every scan with a rule pack, answers questions over the registry, and writes releases out as BIDS with their provenance.

This repository is the engine: the `nils` command, the registry and the rule packs, and the setup wizard that installs every part of NILS.

> **Pre-alpha.** NILS installs and runs, and its interfaces still change between releases. NILS v0, the 0.x line, is archived at [kineuro/nils-legacy](https://github.com/kineuro/nils-legacy).

## Install

On Linux or macOS:

```sh
curl -fsSL https://nils.kineuro.se/get | sh
```

This puts `nils` on the machine and starts `nils setup`, which asks what to install and where, then installs it and keeps it running. Later:

```sh
nils update --all    # the newest release of every part
nils uninstall       # remove NILS, keeping the data or not
```

## Documentation

**[kineuro.se/nils/docs](https://kineuro.se/nils/docs/)**

- [What NILS is](https://kineuro.se/nils/docs/intro/what-nils-is/)
- [Install and get started](https://kineuro.se/nils/docs/intro/install/)
- [The engine on its own](https://kineuro.se/nils/docs/engine/install/), without the wizard

## The parts of NILS

| Repository | Part |
|---|---|
| **kineuro/nils** | The engine: the registry, the rule packs, the `nils` command and the setup wizard. Everything else talks to it. |
| [kineuro/nils-desk](https://github.com/kineuro/nils-desk) | The desk: the web application over the engine, and where people sign in. |
| [kineuro/nils-assistant](https://github.com/kineuro/nils-assistant) | The assistant: turns a question in words into one the engine answers. |
| [kineuro/kvasir](https://github.com/kineuro/kvasir) | The model gateway: every call the assistant makes to a model goes through it. |

The engine works on its own. The desk and the assistant are optional, and the gateway comes with the assistant.

## In this repository

| | |
|---|---|
| [`engine/`](engine/) | The Rust workspace that builds `nils`, and how to build and test it. |
| [`packs/`](packs/) | The first-party rule packs, MRI and clinical. |
| [`contracts/`](contracts/) | The interfaces the other parts build against: the pack format, the OpenAPI description, the MCP schemas and the suite every part is tested with. |
| [`docs/`](docs/) | The design record, the specification of each stage of the build, and reference pages checked against the code. |

## License

The engine and the first-party packs are [AGPL-3.0-only](LICENSE), the contracts [Apache-2.0](contracts/LICENSE), and the documentation [CC BY 4.0](docs/LICENSE). Contributing needs a signed [contributor license agreement](CLA.md): see [CONTRIBUTING.md](CONTRIBUTING.md), and [SECURITY.md](SECURITY.md) for reporting a vulnerability. The name is a trademark: see [TRADEMARKS.md](TRADEMARKS.md).

Built by [kineuro](https://github.com/kineuro), Experimental Neuroradiology Research at Karolinska Institutet, Stockholm.
