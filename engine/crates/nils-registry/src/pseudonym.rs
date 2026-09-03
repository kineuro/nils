// SPDX-License-Identifier: AGPL-3.0-only

//! The pseudonym schemes (`docs/specs/wave1-parse-and-digest.md`, §7.1) and
//! the subkeys of the linkage store (§7.2). Every function here is pinned to
//! the fixtures of the spec, so a change to the hashing is a failing test.

use std::fmt;
use std::str::FromStr;

use blake2::digest::array::ArraySize;
use blake2::digest::consts::{True, U8, U32, U64};
use blake2::digest::typenum::IsLessOrEqual;
use blake2::digest::{FixedOutput, KeyInit, Update};
use blake2::{Blake2b, Blake2bMac, Digest};

/// The two schemes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// v0's: the keyed 8-byte BLAKE2b of the identifier, as 16 hex characters.
    Blake2b8,
    /// The default: a keyed 32-byte BLAKE2b kept as the digest, with a
    /// Crockford base32 display code of `display_length` characters.
    Blake2b32,
}

impl Scheme {
    pub const DEFAULT: Scheme = Scheme::Blake2b32;

    pub fn name(self) -> &'static str {
        match self {
            Scheme::Blake2b8 => "blake2b-8",
            Scheme::Blake2b32 => "blake2b-32",
        }
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A scheme name that is neither of the two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownScheme(pub String);

impl fmt::Display for UnknownScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown pseudonym scheme {:?}; expected blake2b-32 or blake2b-8",
            self.0
        )
    }
}

impl std::error::Error for UnknownScheme {}

impl FromStr for Scheme {
    type Err = UnknownScheme;

    fn from_str(s: &str) -> Result<Scheme, UnknownScheme> {
        match s {
            "blake2b-8" => Ok(Scheme::Blake2b8),
            "blake2b-32" => Ok(Scheme::Blake2b32),
            other => Err(UnknownScheme(other.to_string())),
        }
    }
}

/// The default display length of `blake2b-32`: 12 characters, 60 bits.
pub const DEFAULT_DISPLAY_LENGTH: usize = 12;

/// The longest display code: the whole 256-bit digest.
pub const MAX_DISPLAY_LENGTH: usize = 52;

/// A subject's code under a scheme: what is shown and what is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    /// `subject.code`.
    pub code: String,
    /// `subject.code_digest`.
    pub digest: Vec<u8>,
}

/// The code of a value that is already one (§7.3): the code as read, with
/// the digest the scheme would have derived, so that the display-code check
/// and the linkage store work as they do for a derived code.
pub fn verbatim(scheme: Scheme, key: &[u8], code: &str) -> Code {
    let digest = match scheme {
        Scheme::Blake2b8 => keyed::<U8>(key, code.as_bytes()),
        Scheme::Blake2b32 => keyed::<U32>(key, code.as_bytes()),
    };
    Code {
        code: code.to_string(),
        digest,
    }
}

/// The code of `identifier` under `scheme` with `key`.
pub fn code(scheme: Scheme, key: &[u8], identifier: &str, display_length: usize) -> Code {
    match scheme {
        Scheme::Blake2b8 => {
            let digest = keyed::<U8>(key, identifier.as_bytes());
            Code {
                code: hex::encode(&digest),
                digest,
            }
        }
        Scheme::Blake2b32 => {
            let digest = keyed::<U32>(key, identifier.as_bytes());
            Code {
                code: crockford(&digest, display_length),
                digest,
            }
        }
    }
}

fn keyed<N>(key: &[u8], data: &[u8]) -> Vec<u8>
where
    N: ArraySize + IsLessOrEqual<U64, Output = True>,
{
    let mut mac = Blake2bMac::<N>::new_from_slice(key).expect("a key of at most 64 bytes");
    Update::update(&mut mac, data);
    mac.finalize_fixed().to_vec()
}

const CROCKFORD: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// The first `length` characters of the digest in Crockford base32, lower
/// case, five bits per character from the most significant bit on.
pub fn crockford(digest: &[u8], length: usize) -> String {
    let mut out = String::with_capacity(length);
    let mut bit = 0usize;
    while out.len() < length {
        let mut v = 0u32;
        for i in 0..5 {
            let b = bit + i;
            let byte = digest.get(b / 8).copied().unwrap_or(0);
            v = (v << 1) | u32::from((byte >> (7 - b % 8)) & 1);
        }
        out.push(CROCKFORD[v as usize] as char);
        bit += 5;
    }
    out
}

/// The first 8 hex characters of the unkeyed BLAKE2b of the key: what `nils
/// key list` shows instead of the bytes (§7.2).
pub fn fingerprint(key: &[u8]) -> String {
    let digest = Blake2b::<U32>::digest(key);
    hex::encode(&digest[..4])
}

/// The domain of the lookup subkey.
pub const LOOKUP_DOMAIN: &[u8] = b"nils/linkage/lookup";
/// The domain of the encryption subkey.
pub const ENCRYPT_DOMAIN: &[u8] = b"nils/linkage/encrypt";

/// `BLAKE2b-256(key, domain)`: the subkeys of §7.2.
pub fn subkey(key: &[u8], domain: &[u8]) -> [u8; 32] {
    let v = keyed::<U32>(key, domain);
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    out
}

/// The lookup of an identifier of a type under the lookup subkey:
/// `BLAKE2b-256(k_lookup, id_type || 0x00 || value)`.
pub fn lookup(k_lookup: &[u8; 32], id_type: &str, value: &str) -> [u8; 32] {
    let mut data = Vec::with_capacity(id_type.len() + 1 + value.len());
    data.extend_from_slice(id_type.as_bytes());
    data.push(0);
    data.extend_from_slice(value.as_bytes());
    let v = keyed::<U32>(k_lookup, &data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"nils-fixture-key";

    #[test]
    fn blake2b_8_matches_v0() {
        let c = code(Scheme::Blake2b8, KEY, "PID-0001", DEFAULT_DISPLAY_LENGTH);
        assert_eq!(c.code, "771c4326c89c082c");
        assert_eq!(hex::encode(&c.digest), "771c4326c89c082c");
    }

    #[test]
    fn blake2b_32_matches_the_fixture() {
        let c = code(Scheme::Blake2b32, KEY, "PID-0001", DEFAULT_DISPLAY_LENGTH);
        assert_eq!(
            hex::encode(&c.digest),
            "ec0b67a602077942a174a5c8d1683043e58e1b18c44e83769a20be0f4dd43927"
        );
        assert_eq!(c.code, "xg5pf9g20xwm");
        assert_eq!(crockford(&c.digest, 52).len(), 52);
        assert_eq!(&crockford(&c.digest, 52)[..12], "xg5pf9g20xwm");
    }

    #[test]
    fn subkeys_and_lookup_match_the_fixtures() {
        let k_lookup = subkey(KEY, LOOKUP_DOMAIN);
        let k_encrypt = subkey(KEY, ENCRYPT_DOMAIN);
        assert_eq!(
            hex::encode(k_lookup),
            "d7d3eeb7a8fb4fc9c1cdd83c215c93fabef487366ee678717f8edd0935336fa0"
        );
        assert_eq!(
            hex::encode(k_encrypt),
            "1313a85029438352d9ebb2b8f4b03f32390dfd160355b1ace070bb40f87aabc2"
        );
        assert_eq!(
            hex::encode(lookup(&k_lookup, "patient-id", "PID-0001")),
            "a548a6fa8cf22772d1de1ee342ff8bd7460c15b1c01e0e189f297cf8a168bd0c"
        );
    }

    #[test]
    fn fingerprints_are_short_and_stable() {
        let f = fingerprint(KEY);
        assert_eq!(f.len(), 8);
        assert_eq!(f, fingerprint(KEY));
        assert_ne!(f, fingerprint(b"another"));
    }

    #[test]
    fn scheme_names_round_trip() {
        assert_eq!("blake2b-8".parse::<Scheme>(), Ok(Scheme::Blake2b8));
        assert_eq!("blake2b-32".parse::<Scheme>(), Ok(Scheme::Blake2b32));
        assert_eq!(
            "sha".parse::<Scheme>().unwrap_err().to_string(),
            "unknown pseudonym scheme \"sha\"; expected blake2b-32 or blake2b-8"
        );
    }
}
