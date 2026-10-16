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
///
/// The old scheme: every handover under a key opened with the same password.
/// Since registry schema 105 (record 55, ruling 35 (c)) each handover has its
/// own ([`handover_password`]); this one is still what opens the archives of
/// a handover made before, whose row names no `password`.
pub fn password(key: &[u8]) -> String {
    hex::encode(nils_registry::pseudonym::subkey(key, PASSWORD_DOMAIN))
}

/// The domain a handover's own password is derived under, the handover's id
/// after it.
const HANDOVER_DOMAIN: &[u8] = b"nils/handover/password/one/";

/// The password NILS made for one handover (record 55, ruling 35 (c)):
/// derived from the key and the handover's id, so no two handovers share
/// one, and never stored, since it can be derived again. Printable, as
/// [`password`] is.
pub fn handover_password(key: &[u8], handover: i64) -> String {
    let mut domain = HANDOVER_DOMAIN.to_vec();
    domain.extend_from_slice(handover.to_string().as_bytes());
    hex::encode(nils_registry::pseudonym::subkey(key, &domain))
}

/// How a handover's archives are kept closed, as its row says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// From before registry schema 105: the password of its key, shared.
    Key,
    /// A password of its own, derived from its key and its id.
    Made,
    /// A password the person handing over typed, sealed under the
    /// registry's key.
    Typed,
    /// Not encrypted, for the reason the row keeps.
    None,
}

impl Kind {
    /// The word the row keeps; empty for [`Kind::Key`].
    pub fn word(self) -> &'static str {
        match self {
            Kind::Key => "",
            Kind::Made => "made",
            Kind::Typed => "typed",
            Kind::None => "none",
        }
    }

    /// What the row keeps, read; an empty or absent word is the old scheme.
    pub fn read(word: Option<&str>) -> Kind {
        match word.unwrap_or("") {
            "made" => Kind::Made,
            "typed" => Kind::Typed,
            "none" => Kind::None,
            _ => Kind::Key,
        }
    }

    /// In a person's words, for a list.
    pub fn words(self) -> &'static str {
        match self {
            Kind::Key => {
                "encrypted, the key's password (the old scheme, shared by every handover under the key)"
            }
            Kind::Made => "encrypted, a password of its own made by NILS",
            Kind::Typed => "encrypted, a password typed when it was handed over",
            Kind::None => "not encrypted",
        }
    }
}

/// The shortest and longest a typed password may be. Printable characters
/// only: it is given to 7z on its input, a line at a time.
pub const TYPED_SHORTEST: usize = 8;
pub const TYPED_LONGEST: usize = 256;

/// Why a typed password is refused, or none. Never quotes it.
pub fn typed_refusal(password: &str) -> Option<String> {
    let n = password.chars().count();
    if n < TYPED_SHORTEST {
        return Some(format!(
            "a handover's password is at least {TYPED_SHORTEST} characters"
        ));
    }
    if n > TYPED_LONGEST {
        return Some(format!(
            "a handover's password is at most {TYPED_LONGEST} characters"
        ));
    }
    if password.chars().any(char::is_control) {
        return Some("a handover's password holds no line break or other control character".into());
    }
    None
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
    fn each_handover_has_its_own_password() {
        let key = b"a key of some length";
        let one = handover_password(key, 1);
        assert_eq!(one.len(), 64);
        assert_eq!(one, handover_password(key, 1));
        assert_ne!(one, handover_password(key, 2));
        assert_ne!(one, handover_password(key, 11));
        assert_ne!(one, password(key), "never the key's shared one");
    }

    #[test]
    fn a_typed_password_is_refused_without_being_quoted() {
        assert!(typed_refusal("short").is_some());
        assert!(typed_refusal("long enough").is_none());
        let bad = typed_refusal("two\nlines here").unwrap();
        assert!(!bad.contains("lines here"));
        assert_eq!(Kind::read(None), Kind::Key);
        assert_eq!(Kind::read(Some(Kind::Made.word())), Kind::Made);
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
