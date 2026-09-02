# Contracts

The interfaces that other software builds against, each versioned on its own and licensed [Apache-2.0](LICENSE) so that anything can implement or consume them without touching the engine's license:

| Contract | What it fixes | Arrives |
|---|---|---|
| `query-ast/` | The JSON Schema of the query AST that the engine executes, the one door of every question (D5, D20) | Wave 1 |
| `pack/` | The modality pack specification and the shared vocabulary (D12, D26) | Wave 2, after the pack-format prototype (C11) |
| `openapi/` | The HTTP API of the engine and the apps (D5, C38) | Wave 3 |
| `mcp/` | The MCP tool schemas the agent uses (D11) | Wave 4 |
| `federation/` | The request, disclosure and result protocol between nodes (D27, D28, D29) | Wave 7 |
| `job/` | `nils.job.yml`, the pipeline job description (D9) | Wave 5 |

Contributions here are covered by the Developer Certificate of Origin, not the CLA: sign off your commits (`git commit -s`). Every file starts with `SPDX-License-Identifier: Apache-2.0`.
