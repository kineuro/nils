// SPDX-License-Identifier: AGPL-3.0-only

//! The handover (`docs/specs/wave3-anonymize-and-bids.md`, §11).
//!
//! How a dataset physically leaves. It is the last step of a release, because a
//! release that cannot be handed over is not finished, and v0 has it
//! (`compress/`) while no v1 wave owned it until the capability audit.
//!
//! What v1 adds is that **the archive set is part of the release record**: each
//! archive is a row with its checksum, the people in it and the release it
//! belongs to, so "what did we send them, and is it still intact" is a query
//! rather than a folder somebody remembers.

pub mod archive;
pub mod plan;
pub mod run;

/// The domain the archive password is derived under.
///
/// A password is a key and never a column (§11), so it is not stored: it is
/// derived from a named key in the store, the same way the UID remapping of
/// §8.2 is, under a domain of its own so that neither can be used to reason
/// about the other.
const PASSWORD_DOMAIN: &[u8] = b"nils/handover/password";

/// The password of a handover under one key.
///
/// Deterministic, so a recipient who lost it can be given it again, and a
/// second handover under the same key opens with the same password. Printable,
/// because it goes into somebody's password manager.
pub fn password(key: &[u8]) -> String {
    hex::encode(nils_registry::pseudonym::subkey(key, PASSWORD_DOMAIN))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_is_derived_and_never_stored() {
        let a = password(b"a key of some length");
        assert_eq!(a.len(), 64, "printable, and it goes in a password manager");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            a,
            password(b"a key of some length"),
            "and it is the same twice"
        );
        assert_ne!(a, password(b"another key entirely"));
    }

    #[test]
    fn it_is_not_the_key_and_not_the_uid_subkey() {
        // Domain separation, so that one leaving does not compromise the other.
        let key = b"a key of some length";
        assert_ne!(password(key), hex::encode(key));
        assert_ne!(
            password(key),
            hex::encode(nils_registry::pseudonym::subkey(
                key,
                b"nils/release/uid/v1"
            ))
        );
    }
}
