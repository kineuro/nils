<!-- SPDX-License-Identifier: Apache-2.0 -->

# The MCP door contract

What a third party MCP client gets from the engine's `/mcp` door (D11; C43
keeps our own stations on the HTTP doors). The vocabulary is sixteen
operations; a pack opts each in as a tool by name with its own description
and rules, and a tool's input schema is its operation's, verbatim, so a
client that knows this document knows every tool any pack can expose.

`VERSION` is the contract version. **It changes only by a pull request that
says so**, in its title, and that bumps `VERSION` and adds a new `vN/`
directory beside the old one, which stays.

| version | document | since |
|---|---|---|
| 1 | [`v1/mcp.schema.json`](v1/mcp.schema.json) | Wave 4c slice A7, 2026-09-09: the sixteen operations (the twelve of Wave 4b and the four of Wave 4c §6.4: guide, draft, job, job_status), the input schema per operation, the door each calls, the tool object, the result envelope, the paging contract and the policy fields |

The engine's test (`engine/crates/nils/tests/contracts.rs`) holds the
operation list to the engine's own and every input schema to what a live
server lists, and `tests/mcp.rs` holds the envelope and the paging to what a
call returns.
