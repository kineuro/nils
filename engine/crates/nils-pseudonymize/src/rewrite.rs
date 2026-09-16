// SPDX-License-Identifier: AGPL-3.0-only

//! One file rewritten (record 26 §3): the header read and framed, the
//! identifier read as the digest reads it, the plan applied in memory, the
//! header re-encoded and the pixel bytes copied from the source verbatim,
//! hashed while writing, into `.part` and renamed into place. The pixel
//! data never crosses a parser, which is what keeps memory flat and the
//! step at the copy's speed (the spike of record 26).

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use blake2::{Blake2s256, Digest};
use dicom_core::Tag;
use dicom_dictionary_std::tags;
use dicom_object::{DefaultDicomObject, FileMetaTableBuilder, InMemDicomObject};
use nils_dicom::{Framed, QuarantineClass, Refusal};
use nils_digest::rule::{Ident, Rule};
use nils_pack::private::Allowed;
use nils_release::dates::Offset;
use nils_release::policy::{Policy, Uids};
use nils_release::scrub::{self, Applied, Plan};
use nils_release::tags::Category;

use crate::layout::Facts;

/// The output's buffer: a file of the common size is one or two writes.
pub const WRITE_BUF: usize = 256 << 10;

/// The copy buffer the pixel tail goes through, one per worker.
pub const COPY_BUF: usize = 256 << 10;

/// The four groups the pseudonymiser always removes: v0's list, tag for
/// tag (record 26 §3). Never the times, and never the dates, which are the
/// science.
pub const CATEGORIES: [Category; 4] = [
    Category::Patient,
    Category::Trial,
    Category::Provider,
    Category::Institution,
];

/// A file read and understood, ready to be resolved and written.
pub struct Prepared {
    pub framed: Framed,
    pub ident: Ident,
    pub facts: Facts,
    /// The file's SOPInstanceUID, which the pseudonymised copy keeps: a
    /// tree that holds it already holds this file (lab 26, defect 7).
    pub sop_uid: String,
}

fn text(dataset: &InMemDicomObject, tag: Tag) -> Option<String> {
    let e = dataset.get(tag)?;
    let s = e.value().to_str().ok()?;
    let s = s.trim_matches(['\0', ' ']);
    (!s.is_empty()).then(|| s.to_string())
}

fn int(dataset: &InMemDicomObject, tag: Tag) -> Option<i64> {
    text(dataset, tag)?.trim().parse().ok()
}

/// Read the header of the file at `path`, frame its pixels, and resolve
/// its identifier under the rule; refused as the digest refuses a file it
/// cannot read, with the same classes.
pub fn prepare(path: &Path, rel: &str, rule: &Rule) -> Result<Prepared, Refusal> {
    let framed = nils_dicom::read_framed(path).map_err(nils_dicom::extract::refusal_of)?;
    let dataset = &framed.header.dataset;
    for (tag, keyword) in [
        (tags::STUDY_INSTANCE_UID, "StudyInstanceUID"),
        (tags::SERIES_INSTANCE_UID, "SeriesInstanceUID"),
        (tags::SOP_INSTANCE_UID, "SOPInstanceUID"),
        (tags::SOP_CLASS_UID, "SOPClassUID"),
    ] {
        if text(dataset, tag).is_none() {
            return Err(Refusal::new(
                QuarantineClass::MissingUid,
                Some(keyword.to_string()),
            ));
        }
    }
    let study_uid = text(dataset, tags::STUDY_INSTANCE_UID).unwrap_or_default();
    let sop_uid = text(dataset, tags::SOP_INSTANCE_UID).unwrap_or_default();
    let charset = nils_dicom::charset_of(dataset);
    let identity = nils_dicom::identity_values(dataset, rule.fields(), &charset);
    let ident = rule.trace(&identity.values, &study_uid, rel).ident;
    let facts = Facts {
        study_date: text(dataset, tags::STUDY_DATE).unwrap_or_default(),
        study_uid,
        series_number: int(dataset, tags::SERIES_NUMBER),
        instance_number: int(dataset, tags::INSTANCE_NUMBER),
    };
    Ok(Prepared {
        framed,
        ident,
        facts,
        sop_uid,
    })
}

/// What every file of a run is rewritten under: the policy that keeps
/// dates and UIDs, the four categories, the pack's allowlist, the
/// dataset's lists.
pub struct Scrub<'a> {
    pub policy: Policy,
    pub private: &'a [Allowed],
    pub keep: &'a [Tag],
    pub remove: &'a [Tag],
}

impl<'a> Scrub<'a> {
    pub fn new(private: &'a [Allowed], keep: &'a [Tag], remove: &'a [Tag]) -> Scrub<'a> {
        Scrub {
            policy: Policy {
                dates: nils_release::dates::Policy::Keep,
                uids: Uids::Preserve,
                ..Policy::default()
            },
            private,
            keep,
            remove,
        }
    }
}

/// What writing one file did.
pub struct Outcome {
    pub out_size: u64,
    pub digest: String,
    pub applied: Applied,
}

/// A writer that hashes what passes through it, so the output's digest
/// costs no read back.
struct Hashing<W: Write> {
    inner: W,
    hasher: Blake2s256,
    bytes: u64,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Apply the plan to a prepared file under `code` and write it to
/// `target` (its parent made by the caller), or only apply it and count
/// when `dry_run`. The source is opened again for its tail, from the pixel
/// data on, copied through `buf`.
pub fn write(
    prepared: Prepared,
    code: &str,
    scrub: &Scrub<'_>,
    source: &Path,
    target: &Path,
    buf: &mut [u8],
    dry_run: bool,
) -> Result<Outcome, String> {
    let Framed {
        header,
        pixel_at,
        pixel_end,
        ..
    } = prepared.framed;
    let ts = header.transfer_syntax().to_string();
    let mut object: DefaultDicomObject = match header.meta {
        Some(meta) => header.dataset.with_exact_meta(meta),
        // a bare data set leaves as a Part 10 file in the syntax it was
        // read with, so the copied tail still reads
        None => header
            .dataset
            .with_meta(FileMetaTableBuilder::new().transfer_syntax(&ts))
            .map_err(|e| {
                format!(
                    "no file meta group could be made: {}",
                    first_line(&e.to_string())
                )
            })?,
    };
    let plan = Plan {
        policy: &scrub.policy,
        categories: &CATEGORIES,
        private: scrub.private,
        code,
        offset: Offset(0),
        remap: None,
        keep: scrub.keep,
        remove: scrub.remove,
    };
    let applied = scrub::apply(&mut object, &plan);
    if dry_run {
        return Ok(Outcome {
            out_size: 0,
            digest: String::new(),
            applied,
        });
    }
    let part = target.with_extension("dcm.part");
    let written = (|| -> Result<(u64, String), String> {
        let file = File::create(&part).map_err(|e| format!("unwritable: {e}"))?;
        let mut w = Hashing {
            inner: BufWriter::with_capacity(WRITE_BUF, file),
            hasher: Blake2s256::new(),
            bytes: 0,
        };
        w.write_all(&[0u8; 128])
            .and_then(|()| w.write_all(b"DICM"))
            .map_err(|e| format!("unwritable: {e}"))?;
        object
            .write_meta(&mut w)
            .map_err(|e| format!("unwritable: {}", first_line(&e.to_string())))?;
        object
            .write_dataset(&mut w)
            .map_err(|e| format!("unwritable: {}", first_line(&e.to_string())))?;
        if pixel_end > pixel_at {
            let mut src = File::open(source).map_err(|e| format!("unreadable: {e}"))?;
            src.seek(SeekFrom::Start(pixel_at))
                .map_err(|e| format!("unreadable: {e}"))?;
            let mut left = pixel_end - pixel_at;
            while left > 0 {
                let want = (buf.len() as u64).min(left) as usize;
                let n = src
                    .read(&mut buf[..want])
                    .map_err(|e| format!("unreadable: {e}"))?;
                if n == 0 {
                    break;
                }
                w.write_all(&buf[..n])
                    .map_err(|e| format!("unwritable: {e}"))?;
                left -= n as u64;
            }
        }
        w.flush().map_err(|e| format!("unwritable: {e}"))?;
        Ok((w.bytes, hex::encode(w.hasher.finalize())))
    })();
    let (out_size, digest) = match written {
        Ok(w) => w,
        Err(why) => {
            let _ = std::fs::remove_file(&part);
            return Err(why);
        }
    };
    std::fs::rename(&part, target).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("could not be moved into place: {e}")
    })?;
    Ok(Outcome {
        out_size,
        digest,
        applied,
    })
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::VR;
    use nils_dicom::synth::{self, MetaFields, TempDir};

    fn file(dir: &TempDir, name: &str, pixels: &[u8]) -> (std::path::PathBuf, Vec<u8>) {
        let mut e = synth::minimal_mr("1.2.3.1", "1.2.3.2", "1.2.3.3");
        e.push(synth::text(tags::PATIENT_ID, VR::LO, "199001011234"));
        e.push(synth::text(tags::PATIENT_NAME, VR::PN, "Doe^Jane"));
        e.push(synth::text(tags::PATIENT_BIRTH_DATE, VR::DA, "19900101"));
        e.push(synth::text(tags::PATIENT_SEX, VR::CS, "F"));
        e.push(synth::text(tags::STUDY_DATE, VR::DA, "20240131"));
        e.push(synth::text(tags::STUDY_TIME, VR::TM, "101500"));
        e.push(synth::text(tags::SERIES_NUMBER, VR::IS, "7"));
        e.push(synth::text(tags::INSTANCE_NUMBER, VR::IS, "42"));
        e.push(synth::text(tags::INSTITUTION_NAME, VR::LO, "Somewhere"));
        e.push(synth::text(Tag(0x0019, 0x0010), VR::LO, "A VENDOR"));
        e.push(synth::text(Tag(0x0019, 0x100C), VR::IS, "1000"));
        e.push(synth::text(Tag(0x0019, 0x1099), VR::LO, "the operator"));
        e.push(synth::bytes(tags::PIXEL_DATA, VR::OW, pixels.to_vec()));
        let bytes = synth::part10(&MetaFields::mr("1.2.3.3"), &e, true);
        (dir.file(name, &bytes), bytes)
    }

    #[test]
    fn a_file_is_prepared_then_written_with_its_pixels_copied_verbatim() {
        let dir = TempDir::new("rewrite");
        let pixels: Vec<u8> = (0..8192u32).map(|i| (i * 7 % 256) as u8).collect();
        let (path, _) = file(&dir, "in/a.dcm", &pixels);
        let rule = Rule::default();
        let prepared = prepare(&path, "in/a.dcm", &rule).unwrap();
        assert_eq!(prepared.ident.value, "199001011234");
        assert!(!prepared.ident.fell_back);
        assert_eq!(prepared.facts.study_date, "20240131");
        assert_eq!(prepared.facts.series_number, Some(7));
        assert_eq!(prepared.facts.instance_number, Some(42));

        let allowed = [Allowed {
            creator: "A VENDOR".into(),
            group: 0x0019,
            element: 0x0C,
            why: "a test".into(),
        }];
        let keep = [tags::PATIENT_SEX];
        let scrub = Scrub::new(&allowed, &keep, &[]);
        let target = dir.path().join("out/x.dcm");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let mut buf = vec![0u8; 1024];
        let outcome = write(
            prepared,
            "abc123def456",
            &scrub,
            &path,
            &target,
            &mut buf,
            false,
        )
        .unwrap();
        assert!(!dir.path().join("out/x.dcm.part").exists());
        let out = std::fs::read(&target).unwrap();
        assert_eq!(out.len() as u64, outcome.out_size);
        assert_eq!(hex::encode(Blake2s256::digest(&out)), outcome.digest);
        assert!(out.ends_with(&pixels), "the tail is the source's pixels");
        let read = nils_dicom::read(&target).unwrap();
        let ds = &read.dataset;
        assert_eq!(text(ds, tags::PATIENT_ID).as_deref(), Some("abc123def456"));
        assert_eq!(text(ds, tags::PATIENT_NAME), None);
        assert_eq!(text(ds, tags::PATIENT_BIRTH_DATE), None);
        assert_eq!(text(ds, tags::PATIENT_SEX).as_deref(), Some("F"));
        assert_eq!(text(ds, tags::PATIENT_AGE).as_deref(), Some("034Y"));
        assert_eq!(text(ds, tags::STUDY_DATE).as_deref(), Some("20240131"));
        assert_eq!(text(ds, tags::STUDY_TIME).as_deref(), Some("101500"));
        assert_eq!(
            text(ds, tags::STUDY_INSTANCE_UID).as_deref(),
            Some("1.2.3.1")
        );
        assert_eq!(text(ds, tags::SOP_INSTANCE_UID).as_deref(), Some("1.2.3.3"));
        assert_eq!(text(ds, tags::INSTITUTION_NAME), None);
        assert_eq!(text(ds, Tag(0x0019, 0x100C)).as_deref(), Some("1000"));
        assert_eq!(text(ds, Tag(0x0019, 0x1099)), None);
        assert_eq!(
            read.meta
                .as_ref()
                .unwrap()
                .media_storage_sop_instance_uid
                .trim_matches(['\0', ' ']),
            "1.2.3.3"
        );
        assert!(outcome.applied.total("removed") >= 4);
        // the pixels read back as an element of the right length
        let whole = dicom_object::OpenFileOptions::new()
            .open_file(&target)
            .unwrap();
        let pixel = whole.element(tags::PIXEL_DATA).unwrap();
        assert_eq!(pixel.value().to_bytes().unwrap().len(), pixels.len());

        // a dry run applies and counts but writes nothing
        let prepared = prepare(&path, "in/a.dcm", &rule).unwrap();
        let dry = dir.path().join("out/dry.dcm");
        let outcome = write(
            prepared,
            "abc123def456",
            &scrub,
            &path,
            &dry,
            &mut buf,
            true,
        )
        .unwrap();
        assert!(!dry.exists());
        assert_eq!(outcome.out_size, 0);
        assert!(outcome.applied.total("removed") >= 4);

        // what the reader refuses, prepare refuses with the same class
        let junk = dir.file("in/junk", b"not a file the reader knows");
        let refused = prepare(&junk, "in/junk", &rule).err().expect("refused");
        assert_eq!(refused.class, QuarantineClass::NotDicom);
        let mut e = synth::minimal_mr("1.2.3.1", "1.2.3.2", "1.2.3.3");
        e.retain(|el| el.tag != tags::STUDY_INSTANCE_UID);
        let no_study = dir.file(
            "in/nostudy.dcm",
            &synth::part10(&MetaFields::mr("1.2.3.3"), &e, true),
        );
        let refused = prepare(&no_study, "in/nostudy.dcm", &rule)
            .err()
            .expect("refused");
        assert_eq!(refused.class, QuarantineClass::MissingUid);
        assert_eq!(refused.detail.as_deref(), Some("StudyInstanceUID"));
    }
}
