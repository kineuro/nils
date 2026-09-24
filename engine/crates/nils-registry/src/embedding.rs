// SPDX-License-Identifier: AGPL-3.0-only

//! The embedding cache (record 43 S4, study section 3.5): derivatives of
//! kind `embedding`, one per stack, encoder and preprocessing version.
//!
//! An encoder is a model row of kind `encoder` and task `encoder`,
//! registered by the digest of its weights ([`register_encoder`]); the
//! weights themselves stay in the pipeline image. An embedding is one file
//! per stack holding the slice indices it was made from and a float32
//! matrix, one row per slice, in the format [`encode`] writes and
//! `contracts/job/v1/embedding.md` fixes for the images. Its key is the
//! stack, the encoder's model id and the preprocessing version, held by a
//! unique index over the live rows (migration 60), which fixes v0's key
//! that had neither the encoder's revision nor the preprocessing.
//!
//! A runner asks [`existing`] which stacks have an embedding already under
//! the key, mounts those and hands the pipeline only the [`missing`] ones.
//! A new preprocessing version is a new key, so everything is computed
//! again and the old rows stay; a new encoder is a new model, the same.
//!
//! Record 11's reading holds: the cache is derivative state, not registry
//! state. The registry holds the row and the digest; the bytes are the
//! working place's.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::Registry;
use crate::audit::{self, Action, Entry};
use crate::derivative::{self, Derivative};
use crate::model::{self, Model};
use crate::schema::Type;
use crate::store::{Error as StoreError, Param, Store};

/// The derivative kind.
pub const KIND: &str = "embedding";

/// The task an encoder is registered under.
pub const TASK: &str = "encoder";

/// The media type of an embedding file.
pub const MEDIA_TYPE: &str = "application/vnd.nils.embedding";

/// The extension an embedding file is written with.
pub const EXTENSION: &str = ".emb";

/// The first eight bytes of an embedding file: the name and the format
/// version.
pub const MAGIC: &[u8; 8] = b"NILSEMB1";

/// Where the matrix starts is a multiple of this, so a reader maps it as
/// float32 without a copy.
pub const ALIGN: usize = 64;

#[derive(Debug)]
pub enum Error {
    Store(StoreError),
    /// Well formed and not allowed: an unknown or retired encoder, a stack
    /// that is not there, a version that is no version.
    Refused(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Store(e) => write!(f, "{e}"),
            Error::Refused(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<StoreError> for Error {
    fn from(e: StoreError) -> Error {
        Error::Store(e)
    }
}

fn refused(m: impl Into<String>) -> Error {
    Error::Refused(m.into())
}

// ------------------------------------------------------------- the file

/// What an embedding file says about itself, the JSON header.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Header {
    pub stack_id: i64,
    /// The encoder's weight digest, `sha256:` and 64 hex digits.
    pub encoder: String,
    pub preprocess_version: String,
    /// One per row: the index of the slice (the frame within the stack,
    /// from 0) the row was made from.
    pub slices: Vec<u32>,
    /// The width of a row.
    pub dim: u32,
}

/// One embedding file, read whole.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    pub header: Header,
    /// `slices.len()` rows of `dim` values, row after row.
    pub matrix: Vec<f32>,
}

impl Embedding {
    /// The row of one slice.
    pub fn row(&self, i: usize) -> &[f32] {
        let dim = self.header.dim as usize;
        &self.matrix[i * dim..(i + 1) * dim]
    }
}

/// The file's bytes: the magic, the header's length as a little-endian
/// u32, the header as compact JSON, zero bytes to the next multiple of
/// [`ALIGN`], then the matrix as little-endian float32, row after row, and
/// nothing after it.
pub fn encode(e: &Embedding) -> Result<Vec<u8>, String> {
    check(&e.header, e.matrix.len())?;
    let header = serde_json::to_vec(&json!({
        "format": "nils-embedding",
        "stack_id": e.header.stack_id,
        "encoder": e.header.encoder,
        "preprocess_version": e.header.preprocess_version,
        "rows": e.header.slices.len(),
        "dim": e.header.dim,
        "dtype": "<f4",
        "slices": e.header.slices,
    }))
    .map_err(|err| err.to_string())?;
    let start = data_offset(header.len());
    let mut out = Vec::with_capacity(start + e.matrix.len() * 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    out.resize(start, 0);
    for v in &e.matrix {
        out.extend_from_slice(&v.to_le_bytes());
    }
    Ok(out)
}

fn data_offset(header_len: usize) -> usize {
    (12 + header_len).div_ceil(ALIGN) * ALIGN
}

fn check(h: &Header, values: usize) -> Result<(), String> {
    if !model::is_digest(&h.encoder) {
        return Err(format!(
            "the encoder {} is not a digest: sha256: and 64 lowercase hex digits",
            h.encoder
        ));
    }
    if !version_ok(&h.preprocess_version) {
        return Err(format!(
            "{:?} is not a preprocessing version: a word without spaces",
            h.preprocess_version
        ));
    }
    if h.slices.is_empty() || h.dim == 0 {
        return Err("an embedding holds at least one row of at least one value".into());
    }
    if values != h.slices.len() * h.dim as usize {
        return Err(format!(
            "{} slices of {} values are {} values, and the matrix holds {values}",
            h.slices.len(),
            h.dim,
            h.slices.len() * h.dim as usize
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    if let Some(s) = h.slices.iter().find(|s| !seen.insert(**s)) {
        return Err(format!("slice {s} has two rows"));
    }
    Ok(())
}

/// Read a file's bytes, refusing anything that is not exactly one: the
/// magic, a header that says what [`Header`] says, rows and dim that agree
/// with the slices and the length, and only finite values.
pub fn decode(bytes: &[u8]) -> Result<Embedding, String> {
    let (header, start) = decode_header(bytes)?;
    let values = header.slices.len() * header.dim as usize;
    let data = &bytes[start..];
    if data.len() != values * 4 {
        return Err(format!(
            "the matrix is {} bytes and {} rows of {} float32 are {}",
            data.len(),
            header.slices.len(),
            header.dim,
            values * 4
        ));
    }
    let matrix: Vec<f32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    if let Some(i) = matrix.iter().position(|v| !v.is_finite()) {
        return Err(format!(
            "row {} holds a value that is not a number",
            i / header.dim as usize
        ));
    }
    Ok(Embedding { header, matrix })
}

/// Read only the header, and where the matrix starts: enough to check a
/// file against the row it is registered as without reading the matrix.
pub fn decode_header(bytes: &[u8]) -> Result<(Header, usize), String> {
    if bytes.len() < 12 || &bytes[..8] != MAGIC {
        return Err("not an embedding file: it does not start with NILSEMB1".into());
    }
    let len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let text = bytes
        .get(12..12 + len)
        .ok_or("the header is longer than the file")?;
    let v: Value =
        serde_json::from_slice(text).map_err(|e| format!("the header is not JSON: {e}"))?;
    if v["format"] != "nils-embedding" {
        return Err("the header does not say format nils-embedding".into());
    }
    if v["dtype"] != "<f4" {
        return Err(format!(
            "the matrix is {}, and an embedding is little-endian float32 (<f4)",
            v["dtype"]
        ));
    }
    let header: Header = serde_json::from_value(v.clone())
        .map_err(|e| format!("the header does not say what an embedding is: {e}"))?;
    if v["rows"].as_u64() != Some(header.slices.len() as u64) {
        return Err(format!(
            "the header says {} rows and names {} slices",
            v["rows"],
            header.slices.len()
        ));
    }
    let start = data_offset(len);
    if bytes.len() < start {
        return Err("the file ends before its matrix starts".into());
    }
    if bytes[12 + len..start].iter().any(|b| *b != 0) {
        return Err("the padding after the header is not zero bytes".into());
    }
    check(&header, header.slices.len() * header.dim as usize)?;
    Ok((header, start))
}

fn version_ok(v: &str) -> bool {
    !v.is_empty() && v.len() <= 128 && !v.chars().any(|c| c.is_whitespace() || c.is_control())
}

// ------------------------------------------------------------- encoders

/// An encoder as a pipeline image declares it.
#[derive(Debug, Clone)]
pub struct Encoder<'a> {
    pub name: &'a str,
    pub version: &'a str,
    /// sha256 of the weights: of the weights file, or for weights of many
    /// files of their sorted `sha256sum` listing.
    pub weights_digest: &'a str,
    /// The image the weights are baked into, by manifest digest.
    pub image_digest: Option<&'a str>,
}

/// Register an encoder by its weight digest, or answer the one registered
/// under it already, so that an image registering its encoders on every
/// run registers each once. A digest registered as another kind of model
/// is refused.
pub fn register_encoder(
    registry: &mut Registry,
    e: &Encoder<'_>,
    who: &str,
) -> Result<Model, model::Error> {
    if let Some(held) = model::by_digest(registry.store(), e.weights_digest)? {
        if held.kind != "encoder" {
            return Err(model::Error::Refused(format!(
                "{} is registered already as model {} ({}), a {} and not an encoder",
                e.weights_digest,
                held.id,
                held.label(),
                held.kind
            )));
        }
        return Ok(held);
    }
    let mut card = json!({
        "name": e.name,
        "version": e.version,
        "kind": "encoder",
        "digest": e.weights_digest,
        "task": TASK,
        "artifact": {"format": "other"},
        "intended_use": "turns the slices of a stack into features; its weights stay in the pipeline image",
    });
    if let Some(image) = e.image_digest {
        card["image_digest"] = Value::from(image);
    }
    model::register(registry, &card, who)
}

/// The encoder a model id names, refused when it is not one.
fn encoder(store: &mut Store, id: i64) -> Result<Model, Error> {
    let m =
        model::get(store, id)?.ok_or_else(|| refused(format!("no model {id} is registered")))?;
    if m.kind != "encoder" {
        return Err(refused(format!(
            "model {} ({}) is a {}, not an encoder",
            m.id,
            m.label(),
            m.kind
        )));
    }
    Ok(m)
}

// ------------------------------------------------------------- the cache

/// The live embedding of each of `stacks` under one encoder and
/// preprocessing version, by stack. A stack with none is absent from the
/// map: that is the part a pipeline computes.
pub fn existing(
    store: &mut Store,
    stacks: &[i64],
    encoder_id: i64,
    preprocess_version: &str,
) -> Result<BTreeMap<i64, Derivative>, Error> {
    encoder(store, encoder_id)?;
    let d = store.dialect();
    let mut out = BTreeMap::new();
    let mut wanted: Vec<i64> = stacks.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    for chunk in wanted.chunks(500) {
        let list = chunk
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "{} WHERE kind = {} AND withdrawn_at IS NULL AND model_id = {} \
             AND preprocess_version = {} AND stack_id IN ({list}) ORDER BY id",
            derivative::select(store),
            d.param(1, Type::Text),
            d.param(2, Type::Int),
            d.param(3, Type::Text),
        );
        for r in store.query(
            &sql,
            &[
                Param::from(KIND),
                Param::Int(encoder_id),
                Param::from(preprocess_version),
            ],
        )? {
            let row = derivative::of(&r)?;
            if let Some(stack) = row.stack_id {
                out.insert(stack, row);
            }
        }
    }
    Ok(out)
}

/// The stacks among `stacks` with no live embedding under the key, in
/// order: what a pipeline is given to embed.
pub fn missing(
    store: &mut Store,
    stacks: &[i64],
    encoder_id: i64,
    preprocess_version: &str,
) -> Result<Vec<i64>, Error> {
    let have = existing(store, stacks, encoder_id, preprocess_version)?;
    let mut out: Vec<i64> = stacks
        .iter()
        .copied()
        .filter(|s| !have.contains_key(s))
        .collect();
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// One embedding to register, its file already in its working place.
#[derive(Debug, Clone)]
pub struct New<'a> {
    pub stack_id: i64,
    /// The encoder's model id.
    pub encoder_id: i64,
    pub preprocess_version: &'a str,
    pub place_id: i64,
    pub path: &'a str,
    pub bytes: i64,
    /// Lowercase hex, 64 digits.
    pub sha256: &'a str,
    pub registered_by: &'a str,
    pub actor: Option<&'a Value>,
    pub run_id: Option<i64>,
    pub created_at: &'a str,
}

/// What a registration did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registered {
    /// A new row.
    New(i64),
    /// The key held a live row already, which is kept: the new file is not
    /// registered. `same` says whether its digest was the file's. Two runs
    /// on different devices do not give the same bytes (record 43 R5), so
    /// a difference is not an error; the old row stands until someone
    /// withdraws it.
    Kept { id: i64, same: bool },
}

impl Registered {
    pub fn id(&self) -> i64 {
        match self {
            Registered::New(id) | Registered::Kept { id, .. } => *id,
        }
    }
}

/// Register an embedding under its key, or keep the one the key holds.
/// Refused for a model that is not an encoder, a retired encoder, a stack
/// the registry does not hold, and a version or digest that is not one.
/// Audited as `derivative.register`, which moves no epoch.
///
/// Called outside a transaction, a second writer that wins the race to the
/// key is answered by reading its row again; inside one the unique index's
/// refusal is the caller's.
pub fn register(registry: &mut Registry, n: &New<'_>) -> Result<Registered, Error> {
    let store = registry.store();
    let enc = encoder(store, n.encoder_id)?;
    if enc.state == "retired" {
        return Err(refused(format!(
            "encoder {} ({}) is retired and makes no new embeddings",
            enc.id,
            enc.label()
        )));
    }
    if !version_ok(n.preprocess_version) {
        return Err(refused(format!(
            "{:?} is not a preprocessing version: a word without spaces",
            n.preprocess_version
        )));
    }
    if n.sha256.len() != 64
        || !n
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(refused("sha256 is 64 lowercase hexadecimal digits"));
    }
    let belongs =
        derivative::belongs(store, Some(n.stack_id), None, None, None).map_err(|e| match e {
            Ok(m) => Error::Refused(m),
            Err(e) => Error::Store(e),
        })?;
    let held = |store: &mut Store| -> Result<Option<Registered>, Error> {
        Ok(
            existing(store, &[n.stack_id], n.encoder_id, n.preprocess_version)?
                .remove(&n.stack_id)
                .map(|d| Registered::Kept {
                    id: d.id,
                    same: d.sha256 == n.sha256,
                }),
        )
    };
    if let Some(kept) = held(store)? {
        return Ok(kept);
    }
    let written = derivative::insert(
        store,
        &derivative::New {
            kind: KIND,
            belongs: &belongs,
            place_id: n.place_id,
            path: n.path,
            bytes: n.bytes,
            sha256: n.sha256,
            media_type: MEDIA_TYPE,
            registered_by: n.registered_by,
            actor: n.actor,
            model_id: Some(n.encoder_id),
            run_id: n.run_id,
            preprocess_version: Some(n.preprocess_version),
            supersedes_id: None,
            created_at: n.created_at,
        },
    );
    let id = match written {
        Ok(id) => id,
        Err(e) => {
            return match held(store) {
                Ok(Some(kept)) => Ok(kept),
                _ => Err(Error::Store(e)),
            };
        }
    };
    audit::record(
        registry,
        &Entry {
            principal: n.registered_by,
            action: Action::DerivativeRegister,
            scope: json!({
                "derivative": id, "kind": KIND, "scope": "stack", "stack": n.stack_id,
                "subject": belongs.subject_id, "model": n.encoder_id, "run": n.run_id,
            }),
            policy: None,
            job_id: None,
            details: Some(json!({
                "bytes": n.bytes, "sha256": n.sha256,
                "preprocess_version": n.preprocess_version,
            })),
        },
    )?;
    Ok(Registered::New(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest() -> String {
        format!("sha256:{}", "e".repeat(64))
    }

    fn sample() -> Embedding {
        Embedding {
            header: Header {
                stack_id: 7,
                encoder: digest(),
                preprocess_version: "center3-224".into(),
                slices: vec![4, 5, 6],
                dim: 2,
            },
            matrix: vec![0.5, -1.0, 0.25, 2.0, 1e-3, 3.5],
        }
    }

    #[test]
    fn a_file_reads_back_as_it_was_written_with_its_matrix_aligned() {
        let e = sample();
        let bytes = encode(&e).unwrap();
        assert_eq!(&bytes[..8], MAGIC);
        let (header, start) = decode_header(&bytes).unwrap();
        assert_eq!(start % ALIGN, 0);
        assert_eq!(bytes.len(), start + 6 * 4);
        assert_eq!(header, e.header);
        let back = decode(&bytes).unwrap();
        assert_eq!(back, e);
        assert_eq!(back.row(1), &[0.25, 2.0]);
        // the matrix is little-endian float32 where the header says
        assert_eq!(&bytes[start..start + 4], &0.5f32.to_le_bytes());
    }

    /// The file the Python of `contracts/job/v1/embedding.md` wrote, run
    /// as it stands there with numpy, is the file the engine writes.
    #[test]
    fn the_python_of_the_contract_writes_the_same_bytes() {
        let python = include_bytes!("../tests/fixtures/python-written.emb");
        assert_eq!(decode(python).unwrap(), sample());
        assert_eq!(encode(&sample()).unwrap(), python);
    }

    #[test]
    fn a_file_that_is_not_exactly_one_is_refused() {
        let good = encode(&sample()).unwrap();
        let mut short = good.clone();
        short.pop();
        assert!(decode(&short).unwrap_err().contains("bytes"));
        let mut long = good.clone();
        long.extend_from_slice(&[0, 0, 0, 0]);
        assert!(decode(&long).unwrap_err().contains("bytes"));
        let mut nan = good.clone();
        let at = nan.len() - 4;
        nan[at..].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&nan).unwrap_err().contains("row 2"));
        assert!(
            decode(b"NILSEMB0\0\0\0\0")
                .unwrap_err()
                .contains("NILSEMB1")
        );
        let mut e = sample();
        e.header.slices = vec![4, 4, 6];
        assert!(encode(&e).unwrap_err().contains("two rows"));
        let mut e = sample();
        e.matrix.pop();
        assert!(encode(&e).unwrap_err().contains("holds 5"));
        let mut e = sample();
        e.header.preprocess_version = "a b".into();
        assert!(encode(&e).is_err());
    }
}
