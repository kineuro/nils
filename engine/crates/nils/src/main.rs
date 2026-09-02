// SPDX-License-Identifier: AGPL-3.0-only

//! `nils`, the binary: the command line, configuration, `custody` and the output
//! formatting (`docs/specs/wave1-parse-and-digest.md`, §3 and §13).
//!
//! Slice 1 of the build is the skeleton: `nils --version` and `nils --help`.
//! `init`, `key`, `digest`, `status` and `custody` arrive with the slices that
//! give them something to do (§14).

use clap::Parser;

/// NILS digests DICOM into a registry: one binary, on a laptop or a server.
#[derive(Debug, Parser)]
#[command(name = "nils", version, about, arg_required_else_help = true)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
