// SPDX-License-Identifier: AGPL-3.0-only

//! The registry home: the directory `--registry` names (`NILS_REGISTRY`, else
//! the working directory). It holds `nils.toml`, the SQLite files `registry.db`
//! and `linkage.db` when the backend is SQLite, and the key store `keys/`. On
//! Postgres the two stores are two schemas of the database the DSN names.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Backend;
use crate::keys::{KeyError, KeyStore};
use crate::migrate::{self, Kind, SCHEMA_VERSION, Standing};
use crate::pseudonym::{MAX_DISPLAY_LENGTH, Scheme};
use crate::store::{self, Param, Store};
use crate::time::now_iso;

pub const CONFIG_FILE: &str = "nils.toml";
pub const REGISTRY_DB: &str = "registry.db";
pub const LINKAGE_DB: &str = "linkage.db";
pub const DEFAULT_KEYS_DIR: &str = "keys";
pub const DEFAULT_SCHEMA: &str = "nils";
/// The environment variable `--registry` falls back to.
pub const REGISTRY_ENV: &str = "NILS_REGISTRY";
/// A DSN in the environment overrides the one in `nils.toml`.
pub const DSN_ENV: &str = "NILS_DSN";

/// The session scheme every registry starts with (§7.3, Wave 3 decides the
/// rest): one session per study date.
pub const DEFAULT_SESSION_SCHEME: &str =
    r#"{"window_days":0,"label":"date","anchor":"first","overrides":[]}"#;

/// `nils.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub backend: Backend,
    /// The Postgres connection string; none on SQLite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsn: Option<String>,
    /// The Postgres schema of the registry; the linkage store lives in
    /// `<schema>_linkage`.
    #[serde(default = "default_schema")]
    pub schema: String,
    /// The key store, relative to the home unless absolute.
    #[serde(default = "default_keys_dir")]
    pub keys_dir: String,
}

fn default_schema() -> String {
    DEFAULT_SCHEMA.to_string()
}

fn default_keys_dir() -> String {
    DEFAULT_KEYS_DIR.to_string()
}

impl Config {
    pub fn linkage_schema(&self) -> String {
        format!("{}_linkage", self.schema)
    }
}

/// `registry_meta`, typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub registry_id: String,
    pub schema_version: i64,
    pub epoch: i64,
    pub created_at: String,
    pub pseudonym_scheme: Scheme,
    /// The name of the key in the key store; never its bytes.
    pub pseudonym_key: String,
    pub display_length: usize,
    /// JSON, as written.
    pub session_scheme: String,
}

/// What `nils init` takes.
#[derive(Debug, Clone)]
pub struct InitOptions {
    pub backend: Backend,
    pub dsn: Option<String>,
    pub schema: Option<String>,
    pub scheme: Scheme,
    pub key: String,
    pub display_length: usize,
    pub session_scheme: Option<String>,
}

/// What the home can fail with.
#[derive(Debug)]
pub enum HomeError {
    Store(store::Error),
    Key(KeyError),
    Io {
        path: PathBuf,
        error: io::Error,
    },
    /// A `nils.toml` that does not parse, or a value that does not fit.
    Config(String),
    /// Something the caller asked that cannot be.
    Message(String),
}

impl fmt::Display for HomeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HomeError::Store(e) => write!(f, "{e}"),
            HomeError::Key(e) => write!(f, "{e}"),
            HomeError::Io { path, error } => write!(f, "{}: {error}", path.display()),
            HomeError::Config(m) => write!(f, "{CONFIG_FILE}: {m}"),
            HomeError::Message(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for HomeError {}

impl From<store::Error> for HomeError {
    fn from(e: store::Error) -> HomeError {
        HomeError::Store(e)
    }
}

impl From<KeyError> for HomeError {
    fn from(e: KeyError) -> HomeError {
        HomeError::Key(e)
    }
}

fn io_err(path: &Path, error: io::Error) -> HomeError {
    HomeError::Io {
        path: path.to_path_buf(),
        error,
    }
}

/// The registry home, before or after `init`.
#[derive(Debug, Clone)]
pub struct Home {
    dir: PathBuf,
}

impl Home {
    pub fn new(dir: impl Into<PathBuf>) -> Home {
        Home { dir: dir.into() }
    }

    /// `--registry`, else `NILS_REGISTRY`, else the working directory.
    pub fn resolve(flag: Option<&Path>) -> Home {
        match flag {
            Some(p) => Home::new(p),
            None => match std::env::var_os(REGISTRY_ENV) {
                Some(v) if !v.is_empty() => Home::new(PathBuf::from(v)),
                _ => Home::new("."),
            },
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join(CONFIG_FILE)
    }

    /// Whether `nils.toml` is there.
    pub fn exists(&self) -> bool {
        self.config_path().is_file()
    }

    pub fn read_config(&self) -> Result<Config, HomeError> {
        let path = self.config_path();
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(HomeError::Message(format!(
                    "no registry at {}: {CONFIG_FILE} is missing; run nils init",
                    self.dir.display()
                )));
            }
            Err(e) => return Err(io_err(&path, e)),
        };
        toml::from_str(&text).map_err(|e| HomeError::Config(e.to_string()))
    }

    fn write_config(&self, config: &Config) -> Result<(), HomeError> {
        let path = self.config_path();
        let text = toml::to_string(config).map_err(|e| HomeError::Config(e.to_string()))?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|e| io_err(&path, e))?;
        io::Write::write_all(&mut file, text.as_bytes()).map_err(|e| io_err(&path, e))
    }

    /// The key store the configuration names, or the default before `init`.
    pub fn keys(&self, config: Option<&Config>) -> KeyStore {
        let dir = config.map_or(DEFAULT_KEYS_DIR, |c| c.keys_dir.as_str());
        KeyStore::new(self.dir.join(dir))
    }

    fn dsn_of(config: &Config) -> Result<String, HomeError> {
        if let Ok(v) = std::env::var(DSN_ENV)
            && !v.is_empty()
        {
            return Ok(v);
        }
        config.dsn.clone().ok_or_else(|| {
            HomeError::Config(format!(
                "backend is postgres but no dsn is set; add dsn = \"...\" or set {DSN_ENV}"
            ))
        })
    }

    fn open_store(&self, config: &Config, kind: Kind) -> Result<Store, HomeError> {
        match config.backend {
            Backend::Sqlite => {
                let file = match kind {
                    Kind::Registry => REGISTRY_DB,
                    Kind::Linkage => LINKAGE_DB,
                };
                let path = self.dir.join(file);
                let fresh = !path.exists();
                let store = Store::open_sqlite(&path)?;
                if fresh {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                            .map_err(|e| io_err(&path, e))?;
                    }
                }
                Ok(store)
            }
            Backend::Postgres => {
                let dsn = Self::dsn_of(config)?;
                let schema = match kind {
                    Kind::Registry => config.schema.clone(),
                    Kind::Linkage => config.linkage_schema(),
                };
                let mut store = Store::connect_postgres(&dsn, &schema)?;
                store.batch(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"))?;
                Ok(store)
            }
        }
    }

    /// Create a registry here (§13 `nils init`). The key must already be in
    /// the key store; the home must not already be one.
    pub fn init(&self, opts: &InitOptions) -> Result<Registry, HomeError> {
        if self.exists() {
            return Err(HomeError::Message(format!(
                "{} is already a registry ({CONFIG_FILE} exists)",
                self.dir.display()
            )));
        }
        if opts.display_length == 0 || opts.display_length > MAX_DISPLAY_LENGTH {
            return Err(HomeError::Message(format!(
                "display length {} is out of range (1 to {MAX_DISPLAY_LENGTH})",
                opts.display_length
            )));
        }
        if opts.backend == Backend::Sqlite && opts.dsn.is_some() {
            return Err(HomeError::Message(
                "a dsn goes with --backend postgres".to_string(),
            ));
        }
        let session_scheme = match &opts.session_scheme {
            Some(s) => {
                let v: serde_json::Value = serde_json::from_str(s)
                    .map_err(|e| HomeError::Message(format!("session scheme: {e}")))?;
                if !v.is_object() {
                    return Err(HomeError::Message(
                        "session scheme: expected a JSON object".to_string(),
                    ));
                }
                v.to_string()
            }
            None => DEFAULT_SESSION_SCHEME.to_string(),
        };
        fs::create_dir_all(&self.dir).map_err(|e| io_err(&self.dir, e))?;
        let config = Config {
            backend: opts.backend,
            dsn: opts.dsn.clone(),
            schema: opts.schema.clone().unwrap_or_else(default_schema),
            keys_dir: default_keys_dir(),
        };
        let keys = self.keys(Some(&config));
        // the key is read once, to prove it is there and fits; its bytes go
        // nowhere
        let key = keys.read(&opts.key)?;
        if key.is_empty() || key.len() > crate::keys::MAX_KEY_BYTES {
            return Err(HomeError::Key(KeyError::BadLength(key.len())));
        }
        drop(key);

        let mut store = self.open_store(&config, Kind::Registry)?;
        if migrate::standing(&mut store, Kind::Registry)? != Standing::Empty {
            return Err(HomeError::Message(format!(
                "{} already holds a registry",
                match config.backend {
                    Backend::Sqlite => self.dir.join(REGISTRY_DB).display().to_string(),
                    Backend::Postgres => format!("schema {}", config.schema),
                }
            )));
        }
        migrate::migrate(&mut store, Kind::Registry)?;
        let meta = Meta {
            registry_id: uuid::Uuid::new_v4().to_string(),
            schema_version: SCHEMA_VERSION,
            epoch: 0,
            created_at: now_iso(),
            pseudonym_scheme: opts.scheme,
            pseudonym_key: opts.key.clone(),
            display_length: opts.display_length,
            session_scheme,
        };
        store.begin()?;
        for (k, v) in meta.rows() {
            set_meta(&mut store, k, &v)?;
        }
        store.commit()?;

        let mut linkage = self.open_store(&config, Kind::Linkage)?;
        migrate::migrate(&mut linkage, Kind::Linkage)?;
        // the linkage store carries the id of the registry it belongs to, so
        // that one copied next to another registry is refused (§4.2)
        set_meta_in(
            &mut linkage,
            "linkage_meta",
            "registry_id",
            &meta.registry_id,
        )?;
        drop(linkage);

        self.write_config(&config)?;
        Ok(Registry {
            home: self.clone(),
            config,
            store,
            meta,
            migrated: Vec::new(),
        })
    }

    /// Open the registry here, running any migration it is behind on and
    /// refusing one that is ahead.
    pub fn open(&self) -> Result<Registry, HomeError> {
        let config = self.read_config()?;
        let mut store = self.open_store(&config, Kind::Registry)?;
        let migrated = match migrate::standing(&mut store, Kind::Registry)? {
            Standing::Empty => {
                return Err(HomeError::Message(format!(
                    "{} names a registry that has no tables; run nils init in a fresh directory",
                    self.config_path().display()
                )));
            }
            Standing::Current => Vec::new(),
            Standing::Behind(_) => migrate::migrate(&mut store, Kind::Registry)?,
            Standing::Ahead(v) => {
                return Err(HomeError::Message(format!(
                    "the registry has schema version {v}, ahead of this binary's {SCHEMA_VERSION}; use a newer nils"
                )));
            }
        };
        let meta = read_meta(&mut store)?;
        Ok(Registry {
            home: self.clone(),
            config,
            store,
            meta,
            migrated,
        })
    }
}

impl Meta {
    /// The rows of `registry_meta`.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        vec![
            ("registry_id", self.registry_id.clone()),
            ("schema_version", self.schema_version.to_string()),
            ("epoch", self.epoch.to_string()),
            ("created_at", self.created_at.clone()),
            ("pseudonym_scheme", self.pseudonym_scheme.name().to_string()),
            ("pseudonym_key", self.pseudonym_key.clone()),
            ("display_length", self.display_length.to_string()),
            ("session_scheme", self.session_scheme.clone()),
        ]
    }
}

fn set_meta(store: &mut Store, key: &str, value: &str) -> Result<(), store::Error> {
    set_meta_in(store, "registry_meta", key, value)
}

fn set_meta_in(store: &mut Store, table: &str, key: &str, value: &str) -> Result<(), store::Error> {
    let table = store.qualified(table);
    let d = store.dialect();
    let sql = format!(
        "INSERT INTO {table} (key, value) VALUES ({}, {}) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        d.param(1, crate::schema::Type::Text),
        d.param(2, crate::schema::Type::Text)
    );
    store.execute(&sql, &[Param::from(key), Param::from(value)])?;
    Ok(())
}

fn get_meta_in(store: &mut Store, table: &str, key: &str) -> Result<Option<String>, store::Error> {
    let table = store.qualified(table);
    let sql = format!(
        "SELECT value FROM {table} WHERE key = {}",
        store.dialect().param(1, crate::schema::Type::Text)
    );
    match store.query_opt(&sql, &[Param::from(key)])? {
        Some(row) => Ok(Some(row.text(0)?.to_string())),
        None => Ok(None),
    }
}

fn read_meta(store: &mut Store) -> Result<Meta, HomeError> {
    let table = store.qualified("registry_meta");
    let rows = store.query(&format!("SELECT key, value FROM {table}"), &[])?;
    let mut map = std::collections::HashMap::new();
    for r in &rows {
        map.insert(r.text(0)?.to_string(), r.text(1)?.to_string());
    }
    let take = |k: &str| -> Result<String, HomeError> {
        map.get(k)
            .cloned()
            .ok_or_else(|| HomeError::Message(format!("registry_meta has no {k}")))
    };
    let number = |k: &str| -> Result<i64, HomeError> {
        take(k)?
            .parse()
            .map_err(|_| HomeError::Message(format!("registry_meta.{k} is not a number")))
    };
    let scheme = take("pseudonym_scheme")?
        .parse::<Scheme>()
        .map_err(|e| HomeError::Message(e.to_string()))?;
    let display_length = number("display_length")?;
    if display_length < 1 || display_length > MAX_DISPLAY_LENGTH as i64 {
        return Err(HomeError::Message(format!(
            "registry_meta.display_length {display_length} is out of range"
        )));
    }
    Ok(Meta {
        registry_id: take("registry_id")?,
        schema_version: number("schema_version")?,
        epoch: number("epoch")?,
        created_at: take("created_at")?,
        pseudonym_scheme: scheme,
        pseudonym_key: take("pseudonym_key")?,
        display_length: display_length as usize,
        session_scheme: map
            .get("session_scheme")
            .cloned()
            .unwrap_or_else(|| DEFAULT_SESSION_SCHEME.to_string()),
    })
}

/// An open registry: its home, configuration, store and metadata.
pub struct Registry {
    home: Home,
    config: Config,
    store: Store,
    meta: Meta,
    migrated: Vec<i64>,
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Registry({}, {}, {})",
            self.home.dir.display(),
            self.config.backend,
            self.meta.registry_id
        )
    }
}

impl Registry {
    pub fn home(&self) -> &Home {
        &self.home
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn store(&mut self) -> &mut Store {
        &mut self.store
    }

    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// The migrations `open` applied, oldest first; empty when none were.
    pub fn migrated(&self) -> &[i64] {
        &self.migrated
    }

    pub fn keys(&self) -> KeyStore {
        self.home.keys(Some(&self.config))
    }

    /// The pseudonym key's bytes, read from the key store for the writer.
    pub fn pseudonym_key(&self) -> Result<Vec<u8>, HomeError> {
        Ok(self.keys().read(&self.meta.pseudonym_key)?)
    }

    /// A second connection to the registry, for a reader that runs beside the
    /// writer.
    pub fn open_reader(&self) -> Result<Store, HomeError> {
        self.home.open_store(&self.config, Kind::Registry)
    }

    /// The linkage store, migrated if behind and refused if ahead, and
    /// refused when it belongs to another registry (§4.2). A store from
    /// before slice 4 carries no registry id yet and is claimed on first open.
    pub fn open_linkage(&self) -> Result<Store, HomeError> {
        let mut store = self.home.open_store(&self.config, Kind::Linkage)?;
        migrate::migrate(&mut store, Kind::Linkage)?;
        match get_meta_in(&mut store, "linkage_meta", "registry_id")? {
            Some(id) if id == self.meta.registry_id => {}
            Some(id) => {
                return Err(HomeError::Message(format!(
                    "the linkage store belongs to registry {id}, not to this one ({}); it was copied from another registry",
                    self.meta.registry_id
                )));
            }
            None => set_meta_in(
                &mut store,
                "linkage_meta",
                "registry_id",
                &self.meta.registry_id,
            )?,
        }
        Ok(store)
    }

    /// Set one `registry_meta` row; the caller owns the transaction.
    pub fn set_meta(&mut self, key: &str, value: &str) -> Result<(), HomeError> {
        set_meta(&mut self.store, key, value)?;
        Ok(())
    }

    /// Bump the epoch (§4.2) inside the caller's transaction and return the
    /// new value.
    pub fn next_epoch(&mut self) -> Result<i64, HomeError> {
        let next = self.meta.epoch + 1;
        set_meta(&mut self.store, "epoch", &next.to_string())?;
        self.meta.epoch = next;
        Ok(next)
    }

    /// Re-read the metadata, after another process may have written it.
    pub fn refresh_meta(&mut self) -> Result<(), HomeError> {
        self.meta = read_meta(&mut self.store)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pseudonym::DEFAULT_DISPLAY_LENGTH;
    use nils_dicom::synth::TempDir;

    fn opts(key: &str) -> InitOptions {
        InitOptions {
            backend: Backend::Sqlite,
            dsn: None,
            schema: None,
            scheme: Scheme::DEFAULT,
            key: key.to_string(),
            display_length: DEFAULT_DISPLAY_LENGTH,
            session_scheme: None,
        }
    }

    #[test]
    fn init_needs_the_key_and_refuses_a_second_time() {
        let dir = TempDir::new("home");
        let home = Home::new(dir.path().join("reg"));
        let err = home.init(&opts("k")).unwrap_err().to_string();
        assert!(err.contains("no key named k"), "{err}");
        home.keys(None).add("k", b"nils-fixture-key").unwrap();
        let mut reg = home.init(&opts("k")).unwrap();
        assert_eq!(reg.meta().schema_version, SCHEMA_VERSION);
        assert_eq!(reg.meta().epoch, 0);
        assert_eq!(reg.meta().pseudonym_key, "k");
        assert_eq!(reg.meta().session_scheme, DEFAULT_SESSION_SCHEME);
        assert_eq!(reg.pseudonym_key().unwrap(), b"nils-fixture-key");
        assert!(home.exists());
        assert!(dir.path().join("reg").join(LINKAGE_DB).is_file());
        let err = home.init(&opts("k")).unwrap_err().to_string();
        assert!(err.contains("already a registry"), "{err}");

        reg.store().begin().unwrap();
        assert_eq!(reg.next_epoch().unwrap(), 1);
        reg.store().commit().unwrap();
        drop(reg);

        let mut again = home.open().unwrap();
        assert_eq!(again.meta().epoch, 1);
        assert!(again.migrated().is_empty());
        let mut linkage = again.open_linkage().unwrap();
        let n = linkage.query("SELECT COUNT(*) FROM id_type", &[]).unwrap()[0]
            .int(0)
            .unwrap();
        assert_eq!(n, 2);
        let owner = linkage
            .query(
                "SELECT value FROM linkage_meta WHERE key = 'registry_id'",
                &[],
            )
            .unwrap()[0]
            .text(0)
            .unwrap()
            .to_string();
        assert_eq!(owner, again.meta().registry_id);
        assert_eq!(
            migrate::standing(again.store(), Kind::Registry).unwrap(),
            Standing::Current
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in [CONFIG_FILE, REGISTRY_DB, LINKAGE_DB] {
                let mode = fs::metadata(dir.path().join("reg").join(f))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600, "{f}");
            }
        }
    }

    #[test]
    fn a_registry_from_the_future_is_refused() {
        let dir = TempDir::new("home-future");
        let home = Home::new(dir.path());
        home.keys(None).add("k", b"x").unwrap();
        let mut reg = home.init(&opts("k")).unwrap();
        reg.set_meta("schema_version", "42").unwrap();
        drop(reg);
        let err = home.open().unwrap_err().to_string();
        assert!(err.contains("ahead of this binary"), "{err}");
    }

    #[test]
    fn a_linkage_store_of_another_registry_is_refused_and_an_unclaimed_one_is_claimed() {
        let dir = TempDir::new("home-linkage");
        let a = Home::new(dir.path().join("a"));
        a.keys(None).add("k", b"x").unwrap();
        let reg_a = a.init(&opts("k")).unwrap();
        let b = Home::new(dir.path().join("b"));
        b.keys(None).add("k", b"x").unwrap();
        let reg_b = b.init(&opts("k")).unwrap();
        // b's linkage store copied next to a
        fs::copy(
            dir.path().join("b").join(LINKAGE_DB),
            dir.path().join("a").join(LINKAGE_DB),
        )
        .unwrap();
        let err = reg_a.open_linkage().unwrap_err().to_string();
        assert!(err.contains("belongs to registry"), "{err}");
        assert!(err.contains(&reg_b.meta().registry_id), "{err}");
        // a store from before slice 4 has no owner and is claimed
        let mut store = reg_b.open_linkage().unwrap();
        store
            .execute("DELETE FROM linkage_meta WHERE key = 'registry_id'", &[])
            .unwrap();
        drop(store);
        let mut store = reg_b.open_linkage().unwrap();
        let owner = store
            .query(
                "SELECT value FROM linkage_meta WHERE key = 'registry_id'",
                &[],
            )
            .unwrap()[0]
            .text(0)
            .unwrap()
            .to_string();
        assert_eq!(owner, reg_b.meta().registry_id);
    }

    #[test]
    fn config_round_trips_with_defaults() {
        let c: Config = toml::from_str("backend = \"sqlite\"\n").unwrap();
        assert_eq!(c.backend, Backend::Sqlite);
        assert_eq!(c.keys_dir, "keys");
        assert_eq!(c.schema, "nils");
        assert_eq!(c.linkage_schema(), "nils_linkage");
        let text = toml::to_string(&c).unwrap();
        assert!(!text.contains("dsn"));
        let p: Config =
            toml::from_str("backend = \"postgres\"\ndsn = \"postgres://x\"\nschema = \"s\"\n")
                .unwrap();
        assert_eq!(p.dsn.as_deref(), Some("postgres://x"));
        assert_eq!(p.linkage_schema(), "s_linkage");
        assert!(toml::from_str::<Config>("backend = \"oracle\"\n").is_err());
    }

    #[test]
    fn bad_options_are_refused_before_anything_is_written() {
        let dir = TempDir::new("home-bad");
        let home = Home::new(dir.path().join("r"));
        let mut o = opts("k");
        o.display_length = 0;
        assert!(
            home.init(&o)
                .unwrap_err()
                .to_string()
                .contains("out of range")
        );
        let mut o = opts("k");
        o.session_scheme = Some("[]".to_string());
        assert!(
            home.init(&o)
                .unwrap_err()
                .to_string()
                .contains("JSON object")
        );
        let mut o = opts("k");
        o.dsn = Some("postgres://x".to_string());
        assert!(home.init(&o).unwrap_err().to_string().contains("postgres"));
        assert!(!home.exists());
    }
}
