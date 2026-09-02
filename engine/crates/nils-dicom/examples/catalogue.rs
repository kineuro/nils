// SPDX-License-Identifier: AGPL-3.0-only

//! Render the field catalogue as `docs/reference/catalogue.md`:
//! `cargo run -p nils-dicom --example catalogue > ../docs/reference/catalogue.md`
//! from `engine/`, or `--write` to put it there directly.

use std::path::Path;

fn main() {
    let md = nils_dicom::catalogue::render_markdown();
    if std::env::args().any(|a| a == "--write") {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/reference/catalogue.md");
        std::fs::write(&path, md).expect("write catalogue.md");
        eprintln!("wrote {}", path.display());
    } else {
        print!("{md}");
    }
}
