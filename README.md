# NILS

**Neuroimaging Intelligent Linked System.** NILS digests DICOM into a registry, classifies every series with versioned modality packs, answers questions over the registry in one query language, and exports BIDS with provenance. It is one binary that runs on a laptop or a server, with optional apps for query, review and agents on top, and it can join a federation of nodes where the compute travels and the data stays. Every judgement it makes is a knob you can inspect, and every store it keeps is listed on one page.

> **Pre-alpha.** This repository is the v1 rewrite of NILS, developed in the open from its first commit on 2026-09-02; nothing here runs yet. NILS v0, the 0.x line in daily use in our group, lives in the private repository `kineuro/nils_private`; its public mirror is archived at [kineuro/nils-legacy](https://github.com/kineuro/nils-legacy).

## Where things are

| | |
|---|---|
| [`docs/decisions/`](docs/) | The design record: what NILS v1 is, and why, decision by decision. Start there. |
| [`docs/specs/`](docs/specs/) | One specification per wave of the build, written before the wave's code. Wave 1, parse and digest, is the first. |
| [`contracts/`](contracts/) | The interfaces others build against: the query AST schema, the pack specification and vocabulary, the OpenAPI description, the MCP schemas, the federation protocol, `nils.job.yml`. Apache-2.0. |
| [`packs/mri/`](packs/mri/) | The first-party MRI modality pack. |
| [`spikes/`](spikes/) | Throwaway code behind decisions. The language spike, which chose Rust, reports in [`spikes/lang/`](spikes/lang/). |
| [`evals/`](evals/) | The gold tasks and the scoring for the query language and the agent. |

The engine's source directory, `engine/`, appears with the first slice of Wave 1 ([`docs/specs/wave1-parse-and-digest.md`](docs/specs/wave1-parse-and-digest.md), §14).

## License

The engine, the apps and the first-party packs are [AGPL-3.0-only](LICENSE). The contracts and the client libraries are [Apache-2.0](contracts/LICENSE), so that anything can implement or consume them. The documentation is [CC BY 4.0](docs/LICENSE). Contributions to the engine need a signed [contributor license agreement](CLA.md); contributions to the contracts carry a [Developer Certificate of Origin](CONTRIBUTING.md#the-contracts-dco) sign-off. The name is a trademark: see [TRADEMARKS.md](TRADEMARKS.md).

## Contributing and security

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request and [SECURITY.md](SECURITY.md) before reporting a vulnerability. Issues are open now; the code follows.

Built by [kineuro](https://github.com/kineuro), Experimental Neuroradiology Research at Karolinska Institutet, Stockholm.
