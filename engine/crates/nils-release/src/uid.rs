// SPDX-License-Identifier: AGPL-3.0-only

//! Remapping UIDs, keyed and deterministically
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.2).
//!
//! The same UID gives the same new UID for ever, so two releases of
//! overlapping selections agree and a study exported twice is one study. It
//! costs no table: the mapping is a function of the key, so nothing has to be
//! stored and nothing can go stale.
//!
//! Nothing downstream needs the original, because the join is the registry's
//! id and not a UID.
//!
//! v0 does not do this at all. Its scrubber skips every element whose VR is UI
//! or whose name contains "uid", so every UID leaves the building unchanged.
//! That is the whole reason §4.3 exists: a UID commonly embeds the acquisition
//! date, so a release that shifts dates and keeps UIDs has shifted nothing.

use blake2::Blake2bMac;
use blake2::digest::consts::U32;
use blake2::digest::{FixedOutput, KeyInit, Update};

/// The domain of the UID subkey, so that a remapped UID cannot be used to
/// reason about a pseudonym and the reverse.
pub const UID_DOMAIN: &[u8] = b"nils/release/uid";

/// A DICOM UID is at most 64 characters.
const MAX: usize = 64;

/// Where new UIDs are hung.
///
/// `2.25` is the arc DICOM PS3.5 B.2 sets aside for a UUID expressed as a
/// decimal integer, and it is the honest default: it is legal, it needs no
/// registration, and it cannot collide with anybody's. A deployment with a
/// registered arc of its own says so and gets shorter UIDs that name it.
///
/// Which arc a released dataset should carry is not a question this code can
/// answer (§15, open question 1).
pub const UUID_ARC: &str = "2.25";

/// What a root has to be before anything is hung from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootError(pub String);

impl std::fmt::Display for RootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RootError {}

/// The prefix every UID this release writes hangs from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root(String);

impl Root {
    /// A root, checked: digits and dots, each component without a leading
    /// zero, and short enough to leave room for what is hung from it.
    ///
    /// The room matters. A UID is 64 characters and a suffix of nothing is not
    /// a UID, so a root that leaves fewer than eight is refused here rather
    /// than producing truncated UIDs that collide with each other.
    pub fn new(text: &str) -> Result<Root, RootError> {
        let text = text.trim().trim_end_matches('.');
        if text.is_empty() {
            return Err(RootError("a UID root is not empty".into()));
        }
        for part in text.split('.') {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(RootError(format!(
                    "{text} is not a UID root: every component is digits"
                )));
            }
            if part.len() > 1 && part.starts_with('0') {
                return Err(RootError(format!(
                    "{text} is not a UID root: {part} has a leading zero"
                )));
            }
        }
        if text.len() + 9 > MAX {
            return Err(RootError(format!(
                "{text} leaves {} characters of a UID's 64, and a suffix needs at least 8",
                MAX - text.len() - 1
            )));
        }
        Ok(Root(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Root {
    fn default() -> Root {
        Root(UUID_ARC.to_string())
    }
}

/// Remaps a UID to a new one under a key.
#[derive(Debug, Clone)]
pub struct Remap {
    root: Root,
    subkey: [u8; 32],
}

impl Remap {
    pub fn new(root: Root, key: &[u8]) -> Remap {
        Remap {
            root,
            subkey: nils_registry::pseudonym::subkey(key, UID_DOMAIN),
        }
    }

    pub fn root(&self) -> &str {
        self.root.as_str()
    }

    /// The new UID of an old one.
    ///
    /// The digest is rendered as a decimal integer, because a UID component is
    /// digits and nothing else. It is truncated to what the root leaves rather
    /// than wrapped, since a shorter component is still unique enough: 128 bits
    /// of a keyed digest is more than an archive will ever need, and the
    /// remaining room is usually far more than that.
    pub fn of(&self, uid: &str) -> String {
        let uid = uid.trim().trim_end_matches('\0');
        let mut mac = <Blake2bMac<U32> as KeyInit>::new_from_slice(&self.subkey)
            .expect("a 32 byte key is a valid blake2b key");
        Update::update(&mut mac, uid.as_bytes());
        let digest = mac.finalize_fixed();

        // The digest as one big decimal number, most significant byte first.
        let mut decimal: Vec<u8> = vec![0];
        for byte in digest.iter() {
            let mut carry = u32::from(*byte);
            for d in decimal.iter_mut().rev() {
                let v = u32::from(*d) * 256 + carry;
                *d = (v % 10) as u8;
                carry = v / 10;
            }
            while carry > 0 {
                decimal.insert(0, (carry % 10) as u8);
                carry /= 10;
            }
        }
        let room = MAX - self.root.as_str().len() - 1;
        let text: String = decimal
            .iter()
            .map(|d| char::from(b'0' + d))
            .take(room)
            .collect();
        // A component may not have a leading zero, and the digest sometimes
        // starts with one.
        let text = match text.strip_prefix('0') {
            Some(rest) if !rest.is_empty() => format!("1{rest}"),
            _ => text,
        };
        format!("{}.{}", self.root.as_str(), text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remap() -> Remap {
        Remap::new(Root::default(), b"a key of some length")
    }

    #[test]
    fn the_same_uid_gives_the_same_new_one_for_ever() {
        // Which is what makes two releases of overlapping selections agree,
        // and what makes it need no table.
        let a = remap();
        let b = Remap::new(Root::default(), b"a key of some length");
        let uid = "1.2.840.113619.2.55.3.604688119.868.1234567890.123";
        assert_eq!(a.of(uid), b.of(uid));
        assert_eq!(a.of(uid), a.of(uid));
    }

    #[test]
    fn a_different_uid_gives_a_different_one() {
        let r = remap();
        assert_ne!(r.of("1.2.3"), r.of("1.2.4"));
    }

    #[test]
    fn a_different_key_gives_a_different_one() {
        let a = remap();
        let b = Remap::new(Root::default(), b"another key entirely...");
        assert_ne!(a.of("1.2.3"), b.of("1.2.3"));
    }

    #[test]
    fn what_comes_out_is_a_uid() {
        let r = remap();
        for uid in [
            "1.2.3",
            "",
            "1.2.840.10008.5.1.4.1.1.4",
            "1.3.12.2.1107.5.2",
        ] {
            let out = r.of(uid);
            assert!(out.len() <= MAX, "{out} is {} long", out.len());
            assert!(out.starts_with("2.25."), "{out}");
            for part in out.split('.') {
                assert!(!part.is_empty(), "{out}");
                assert!(part.bytes().all(|b| b.is_ascii_digit()), "{out}");
                assert!(part.len() == 1 || !part.starts_with('0'), "{out}");
            }
        }
    }

    #[test]
    fn trailing_padding_is_not_part_of_the_uid() {
        // DICOM pads an odd-length UI value with a null, and a reader that
        // keeps it would map the padded and unpadded forms of one UID to two.
        let r = remap();
        assert_eq!(r.of("1.2.3"), r.of("1.2.3\0"));
        assert_eq!(r.of("1.2.3"), r.of(" 1.2.3 "));
    }

    #[test]
    fn a_root_is_checked_before_anything_hangs_from_it() {
        assert!(Root::new("1.2.826.0.1.3680043.10.1234").is_ok());
        assert!(Root::new("2.25").is_ok());
        assert!(Root::new("2.25.").is_ok(), "a trailing dot is trimmed");
        assert!(Root::new("").is_err());
        assert!(Root::new("1.2.abc").is_err());
        assert!(Root::new("1.02.3").is_err(), "a leading zero");
        // Long enough to leave no room for a suffix worth having.
        let long = "1.".repeat(30);
        assert!(Root::new(&long).is_err());
    }

    #[test]
    fn a_registered_arc_leaves_more_room_and_says_whose_it_is() {
        let r = Remap::new(
            Root::new("1.2.826.0.1.3680043.10.1234").unwrap(),
            b"a key of some length",
        );
        let out = r.of("1.2.3");
        assert!(out.starts_with("1.2.826.0.1.3680043.10.1234."));
        assert!(out.len() <= MAX);
    }
}
