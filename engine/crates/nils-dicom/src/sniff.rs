// SPDX-License-Identifier: AGPL-3.0-only

//! The first look at a file: is it a Part 10 file, a bare data set, or neither
//! (`docs/specs/wave1-parse-and-digest.md`, §6.1).
//!
//! A Part 10 file carries `DICM` at offset 128, after the preamble, or at offset
//! 0 when the preamble was dropped. A bare data set has no marker at all and
//! starts with its first element, which in every image is in group 0008, so its
//! first two bytes are `08 00` in little endian. Anything else is not tried
//! further: the spike's harness and pydicom agreed on every such file of the
//! nmosd corpus.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// What the first bytes of a file say.
#[derive(Debug)]
pub enum Sniff {
    /// `DICM` at offset 128 or at offset 0.
    Part10,
    /// No marker; the first bytes look like an element of group 0008.
    BareDataset,
    /// Neither: text, an image, a Finder file, an empty file.
    Other,
    /// The file could not be opened or read.
    Unreadable(io::Error),
}

/// The bytes the sniff needs: the preamble plus the marker.
pub const SNIFF_LEN: usize = 132;

/// Classify the first bytes of a file already in memory (at most [`SNIFF_LEN`]
/// are looked at).
pub fn sniff_bytes(head: &[u8]) -> Sniff {
    if head.len() >= SNIFF_LEN && &head[128..132] == b"DICM" {
        return Sniff::Part10;
    }
    if head.len() >= 4 && &head[..4] == b"DICM" {
        return Sniff::Part10;
    }
    if head.len() >= 8 && head[0] == 0x08 && head[1] == 0x00 {
        return Sniff::BareDataset;
    }
    Sniff::Other
}

/// Open the file and classify its first bytes.
pub fn sniff(path: &Path) -> Sniff {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) => return Sniff::Unreadable(e),
    };
    let mut head = [0u8; SNIFF_LEN];
    let mut filled = 0;
    while filled < SNIFF_LEN {
        match file.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Sniff::Unreadable(e),
        }
    }
    sniff_bytes(&head[..filled])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_after_preamble_is_part10() {
        let mut head = vec![0u8; 128];
        head.extend_from_slice(b"DICM");
        assert!(matches!(sniff_bytes(&head), Sniff::Part10));
    }

    #[test]
    fn marker_at_start_is_part10() {
        assert!(matches!(
            sniff_bytes(b"DICM\x02\x00\x00\x00UL"),
            Sniff::Part10
        ));
    }

    #[test]
    fn group_0008_first_is_bare() {
        assert!(matches!(
            sniff_bytes(b"\x08\x00\x05\x00\x0a\x00\x00\x00ISO_IR 100"),
            Sniff::BareDataset
        ));
    }

    #[test]
    fn text_and_short_files_are_other() {
        assert!(matches!(sniff_bytes(b"hello\n"), Sniff::Other));
        assert!(matches!(sniff_bytes(b""), Sniff::Other));
        assert!(matches!(sniff_bytes(b"\x08\x00"), Sniff::Other));
        let mut head = vec![0u8; 128];
        head.extend_from_slice(b"DICX");
        assert!(matches!(sniff_bytes(&head), Sniff::Other));
    }
}
