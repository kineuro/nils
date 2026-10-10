// SPDX-License-Identifier: AGPL-3.0-only

//! Record 55 H3 (2026-10-09): the axes a release reads by name are the ones
//! `nils_pack::matters::RELEASE_READS` says, read off the release's own
//! source. A missing answer on an axis that matters is a question, so a read
//! added here without the list would leave a gap nobody is asked about, and
//! a list entry the code no longer reads would ask about nothing.

use std::collections::BTreeSet;
use std::path::Path;

/// Every `get("<name>")` outside the tests of `run.rs`, where a release
/// turns a stack's stored axes into its names, its folder and what it does
/// with the stack.
fn read_by_name() -> BTreeSet<String> {
    let src = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/run.rs"))
        .expect("run.rs");
    let body = match src.find("\n#[cfg(test)]\nmod tests") {
        Some(at) => &src[..at],
        None => &src[..],
    };
    let mut out = BTreeSet::new();
    let mut rest = body;
    while let Some(at) = rest.find("get(\"") {
        let after = &rest[at + 5..];
        if let Some(end) = after.find('"') {
            let name = &after[..end];
            if after[end..].starts_with("\")")
                && !name.is_empty()
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                && !rest[..at].ends_with('.')
            {
                out.insert(name.to_string());
            }
        }
        rest = &rest[at + 5..];
    }
    out
}

#[test]
fn the_axes_a_release_reads_by_name_are_the_ones_said_to_matter() {
    let read = read_by_name();
    let listed: BTreeSet<String> = nils_pack::matters::RELEASE_READS
        .iter()
        .map(|(axis, _)| axis.to_string())
        .collect();
    assert!(!read.is_empty(), "no read found: the scan is broken");
    assert_eq!(
        read, listed,
        "run.rs reads these axes by name; nils_pack::matters::RELEASE_READS must say the same"
    );
}

#[test]
fn the_mri_pack_s_axes_that_matter_and_where_a_missing_one_is_asked() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    let pack = nils_pack::load(&dir, None).expect("the MRI pack");
    let matters = nils_pack::matters::of(&pack);
    let names: Vec<&str> = matters.axes.keys().map(String::as_str).collect();
    // the name, the main scans and a release read these; quality only
    // through an acq- group of the BIDS name, role only to find a pick's
    // candidates
    for axis in [
        "base",
        "body_part",
        "construct",
        "directory_type",
        "disposition",
        "modifier",
        "post_contrast",
        "provenance",
        "role",
        "technique",
    ] {
        assert!(names.contains(&axis), "{axis} matters: {names:?}");
    }
    // described, never read by a name, a pick or a release
    for axis in ["contrast_mix", "base_basis", "body_region", "convertible"] {
        assert!(!names.contains(&axis), "{axis} does not matter: {names:?}");
    }
    // a missing answer is asked only on an axis that holds one value, has
    // no default and is not its own operation's (the body part and the
    // post-contrast, record 56 section 2)
    assert_eq!(
        pack.review.by_model,
        vec![
            "body_part".to_string(),
            "body_region".to_string(),
            "post_contrast".to_string()
        ]
    );
    assert_eq!(
        nils_pack::matters::missing_asked(&pack),
        vec!["base".to_string()]
    );
}
