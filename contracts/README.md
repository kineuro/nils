# Contracts

The interfaces that other software builds against, each versioned on its own and licensed [Apache-2.0](LICENSE) so that anything can implement or consume them without touching the engine's license:

| Contract | What it fixes | State |
|---|---|---|
| `pack/` | The modality pack manifest (D12, D26); the pack format is data since C11 | version 1, written down in Wave 4a |
| `review-item/` | The review item every emitter writes and every consumer reads (D7) | version 4 (Wave 4c A6); version 1 written down in Wave 4a |
| `openapi/` | The HTTP API of the engine (D5, C38) | version 3 (Wave 4c A4); version 0 was the empty skeleton of Wave 4a §11 |
| `query-ast/` | The JSON Schema of the query AST that the engine executes, the one door of every question (D5, D20) | Wave 4b |
| `mcp/` | The MCP door: the operation vocabulary, the input schema per operation, the result envelope, the paging and the policy fields (D11, C43) | version 1, written down in Wave 4c slice A7 |
| `suite/` | The vocabulary every part of the suite shares: entitlements, headers and the actor, purposes and content classes, the deployment capabilities document, app registry entries, station manifests, the trust list vectors (Wave 4c §4.5) | version 1, written down in Wave 4c slice A7 |
| `job/` | `nils.job.yml`, the pipeline job description (D9) | Wave 5 |
| `federation/` | The request, disclosure and result protocol between nodes (D27, D28, D29) | Wave 7 |

Each contract directory carries a `VERSION` file and one `vN/` directory per
version, and the rule they all share: **a version changes only by a pull
request that says so** in its title, bumping `VERSION` and adding the next
`vN/` beside the old one, which stays. The engine keeps a test on every
contract, even one that tests little, because a test that exists is amended
and one that does not is never written.

Contributions here are covered by the Developer Certificate of Origin, not the CLA: sign off your commits (`git commit -s`). Every file starts with `SPDX-License-Identifier: Apache-2.0`.
