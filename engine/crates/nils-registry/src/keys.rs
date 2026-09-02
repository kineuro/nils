// SPDX-License-Identifier: AGPL-3.0-only

//! The key store (`docs/specs/wave1-parse-and-digest.md`, §7.2): a directory
//! of files, one key each, mode 700 and 600 on Unix. A key appears nowhere but
//! here: `list` shows names, lengths and fingerprints, never bytes, and no
//! error here carries a key's content.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::pseudonym::fingerprint;

/// The longest key: BLAKE2b's key size.
pub const MAX_KEY_BYTES: usize = 64;

/// What the key store can fail with.
#[derive(Debug)]
pub enum KeyError {
    /// A name with a character outside `[A-Za-z0-9._-]`, or empty.
    BadName(String),
    /// A key that already exists.
    Exists(String),
    /// A key that does not.
    Missing(String),
    /// Empty, or longer than 64 bytes.
    BadLength(usize),
    /// A key the registry names, which cannot be removed.
    InUse(String),
    Io {
        path: PathBuf,
        error: io::Error,
    },
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::BadName(n) => write!(
                f,
                "key name {n:?}: use letters, digits, '.', '_' and '-' only"
            ),
            KeyError::Exists(n) => write!(f, "key {n} already exists; remove it first"),
            KeyError::Missing(n) => write!(f, "no key named {n}"),
            KeyError::BadLength(0) => f.write_str("the key is empty"),
            KeyError::BadLength(n) => {
                write!(
                    f,
                    "the key is {n} bytes; at most {MAX_KEY_BYTES} are accepted"
                )
            }
            KeyError::InUse(n) => write!(
                f,
                "key {n} is the registry's pseudonym key and cannot be removed"
            ),
            KeyError::Io { path, error } => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for KeyError {}

/// One key as `list` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyInfo {
    pub name: String,
    pub bytes: usize,
    pub fingerprint: String,
}

/// The directory of keys.
#[derive(Debug, Clone)]
pub struct KeyStore {
    dir: PathBuf,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
        && !name.starts_with('.')
}

impl KeyStore {
    pub fn new(dir: impl Into<PathBuf>) -> KeyStore {
        KeyStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_of(&self, name: &str) -> Result<PathBuf, KeyError> {
        if !valid_name(name) {
            return Err(KeyError::BadName(name.to_string()));
        }
        Ok(self.dir.join(name))
    }

    fn io(path: &Path, error: io::Error) -> KeyError {
        KeyError::Io {
            path: path.to_path_buf(),
            error,
        }
    }

    /// Create the directory, mode 700, if it is not there.
    pub fn ensure(&self) -> Result<(), KeyError> {
        if !self.dir.is_dir() {
            fs::create_dir_all(&self.dir).map_err(|e| Self::io(&self.dir, e))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))
                .map_err(|e| Self::io(&self.dir, e))?;
        }
        Ok(())
    }

    /// Store `bytes` under `name`. The caller has already stripped a trailing
    /// newline if it wanted to; the store keeps what it is given.
    pub fn add(&self, name: &str, bytes: &[u8]) -> Result<KeyInfo, KeyError> {
        let path = self.path_of(name)?;
        if bytes.is_empty() || bytes.len() > MAX_KEY_BYTES {
            return Err(KeyError::BadLength(bytes.len()));
        }
        self.ensure()?;
        if path.exists() {
            return Err(KeyError::Exists(name.to_string()));
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|e| Self::io(&path, e))?;
        io::Write::write_all(&mut file, bytes).map_err(|e| Self::io(&path, e))?;
        Ok(KeyInfo {
            name: name.to_string(),
            bytes: bytes.len(),
            fingerprint: fingerprint(bytes),
        })
    }

    /// The bytes of a key.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, KeyError> {
        let path = self.path_of(name)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(KeyError::Missing(name.to_string()))
            }
            Err(e) => Err(Self::io(&path, e)),
        }
    }

    /// Every key, by name.
    pub fn list(&self) -> Result<Vec<KeyInfo>, KeyError> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(Self::io(&self.dir, e)),
        };
        for entry in entries {
            let entry = entry.map_err(|e| Self::io(&self.dir, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !valid_name(&name) || !entry.path().is_file() {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|e| Self::io(&entry.path(), e))?;
            out.push(KeyInfo {
                name,
                bytes: bytes.len(),
                fingerprint: fingerprint(&bytes),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Remove a key; `in_use` is the name the registry's metadata holds, which
    /// cannot go.
    pub fn remove(&self, name: &str, in_use: Option<&str>) -> Result<(), KeyError> {
        let path = self.path_of(name)?;
        if in_use == Some(name) {
            return Err(KeyError::InUse(name.to_string()));
        }
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(KeyError::Missing(name.to_string()))
            }
            Err(e) => Err(Self::io(&path, e)),
        }
    }
}

/// What `nils key add` reads: the bytes with one trailing newline removed, and
/// whether one was.
pub fn strip_newline(bytes: &[u8]) -> (&[u8], bool) {
    if let Some(rest) = bytes.strip_suffix(b"\r\n") {
        (rest, true)
    } else if let Some(rest) = bytes.strip_suffix(b"\n") {
        (rest, true)
    } else {
        (bytes, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nils_dicom::synth::TempDir;

    #[test]
    fn keys_are_added_listed_read_and_removed() {
        let dir = TempDir::new("keys");
        let store = KeyStore::new(dir.path().join("keys"));
        assert!(store.list().unwrap().is_empty());
        let info = store.add("fixture", b"nils-fixture-key").unwrap();
        assert_eq!(info.bytes, 16);
        assert_eq!(info.fingerprint.len(), 8);
        assert!(matches!(
            store.add("fixture", b"x"),
            Err(KeyError::Exists(_))
        ));
        assert!(matches!(store.add("", b"x"), Err(KeyError::BadName(_))));
        assert!(matches!(store.add("a/b", b"x"), Err(KeyError::BadName(_))));
        assert!(matches!(
            store.add("empty", b""),
            Err(KeyError::BadLength(0))
        ));
        assert!(matches!(
            store.add("long", &[7u8; 65]),
            Err(KeyError::BadLength(65))
        ));
        assert_eq!(store.read("fixture").unwrap(), b"nils-fixture-key");
        assert!(matches!(store.read("nope"), Err(KeyError::Missing(_))));
        assert_eq!(store.list().unwrap(), vec![info]);
        assert!(matches!(
            store.remove("fixture", Some("fixture")),
            Err(KeyError::InUse(_))
        ));
        store.remove("fixture", None).unwrap();
        assert!(store.list().unwrap().is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.dir()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn one_trailing_newline_is_stripped() {
        assert_eq!(strip_newline(b"abc\n"), (&b"abc"[..], true));
        assert_eq!(strip_newline(b"abc\r\n"), (&b"abc"[..], true));
        assert_eq!(strip_newline(b"abc\n\n"), (&b"abc\n"[..], true));
        assert_eq!(strip_newline(b"abc"), (&b"abc"[..], false));
    }
}
