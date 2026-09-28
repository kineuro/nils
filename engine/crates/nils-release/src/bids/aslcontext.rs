// SPDX-License-Identifier: AGPL-3.0-only

//! The `aslcontext.tsv` of an ASL image (BIDS 1.11, `perf`).
//!
//! BIDS requires beside every `_asl` image a table with one row per volume
//! saying what the volume is: control, label, m0scan, deltam, cbf or noRF.
//! The pack's mapping says it per construct (a scanner's own perfusion
//! weighted image is `deltam` throughout), and the number of rows is the
//! number of volumes the converter wrote, which is read from the NIfTI header
//! and never guessed from the DICOM.

use std::io::Read;
use std::path::Path;

/// How many volumes a NIfTI image holds, from its header: `dim[4]` where
/// `dim[0]` says there is a fourth dimension, else one. NIfTI-1 and NIfTI-2,
/// either byte order, gzipped when the name ends in `.gz`.
pub fn volumes(path: &Path) -> Result<i64, String> {
    let file = std::fs::File::open(path).map_err(|_| "the image is unreadable".to_string())?;
    let mut head = Vec::with_capacity(540);
    let read = match path.extension().is_some_and(|e| e == "gz") {
        true => flate2::read::GzDecoder::new(file)
            .take(540)
            .read_to_end(&mut head),
        false => file.take(540).read_to_end(&mut head),
    };
    read.map_err(|_| "the image's header is unreadable".to_string())?;
    volumes_of(&head)
}

/// [`volumes`] on the header's bytes.
pub fn volumes_of(head: &[u8]) -> Result<i64, String> {
    let short = || "the image's header is too short to be NIfTI".to_string();
    let int32 = |at: usize, big: bool| -> Option<i64> {
        let b: [u8; 4] = head.get(at..at + 4)?.try_into().ok()?;
        Some(match big {
            true => i32::from_be_bytes(b),
            false => i32::from_le_bytes(b),
        } as i64)
    };
    let int16 = |at: usize, big: bool| -> Option<i64> {
        let b: [u8; 2] = head.get(at..at + 2)?.try_into().ok()?;
        Some(match big {
            true => i16::from_be_bytes(b),
            false => i16::from_le_bytes(b),
        } as i64)
    };
    let int64 = |at: usize, big: bool| -> Option<i64> {
        let b: [u8; 8] = head.get(at..at + 8)?.try_into().ok()?;
        Some(match big {
            true => i64::from_be_bytes(b),
            false => i64::from_le_bytes(b),
        })
    };
    // `sizeof_hdr` says the version and, read in the wrong order, the byte
    // order: 348 for NIfTI-1 and 540 for NIfTI-2.
    let (two, big) = match (int32(0, false), int32(0, true)) {
        (Some(348), _) => (false, false),
        (_, Some(348)) => (false, true),
        (Some(540), _) => (true, false),
        (_, Some(540)) => (true, true),
        (None, _) => return Err(short()),
        _ => return Err("the image's header is not NIfTI".to_string()),
    };
    // dim[0..8]: int16 at 40 in NIfTI-1, int64 at 16 in NIfTI-2
    let dim = |i: usize| match two {
        false => int16(40 + 2 * i, big),
        true => int64(16 + 8 * i, big),
    };
    let rank = dim(0).ok_or_else(short)?;
    if !(1..=7).contains(&rank) {
        return Err(format!("the image's header says {rank} dimensions"));
    }
    if rank < 4 {
        return Ok(1);
    }
    let n = dim(4).ok_or_else(short)?;
    match n >= 1 {
        true => Ok(n),
        false => Err(format!("the image's header says {n} volumes")),
    }
}

/// The table: its header and one row per volume, each the one type.
pub fn table(volume_type: &str, volumes: i64) -> String {
    let mut out = String::from("volume_type\n");
    for _ in 0..volumes.max(0) {
        out.push_str(volume_type);
        out.push('\n');
    }
    out
}

/// The table's file name beside an image named `stem`: the suffix `asl`
/// becomes `aslcontext`, every entity stays.
pub fn file_name(stem: &str) -> Option<String> {
    stem.strip_suffix("_asl")
        .map(|s| format!("{s}_aslcontext.tsv"))
}

/// Write the table beside the image `stem` in `dir`, reading the image the
/// converter wrote there (`.nii.gz` or `.nii`). The file's name, relative to
/// `dir`, is the answer.
pub fn write(dir: &Path, stem: &str, volume_type: &str) -> Result<String, String> {
    let name =
        file_name(stem).ok_or_else(|| "an aslcontext is only beside an asl image".to_string())?;
    let image = [".nii.gz", ".nii"]
        .iter()
        .map(|e| dir.join(format!("{stem}{e}")))
        .find(|p| p.is_file())
        .ok_or_else(|| "the ASL image the converter wrote is not there".to_string())?;
    let n = volumes(&image)?;
    std::fs::write(dir.join(&name), table(volume_type, n))
        .map_err(|_| "the aslcontext could not be written".to_string())?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A NIfTI-1 header of 348 bytes with the dims given.
    fn nifti1(dims: &[i16], big: bool) -> Vec<u8> {
        let mut h = vec![0u8; 348];
        let put32 = |v: i32| match big {
            true => v.to_be_bytes(),
            false => v.to_le_bytes(),
        };
        h[0..4].copy_from_slice(&put32(348));
        for (i, d) in dims.iter().enumerate() {
            let b = match big {
                true => d.to_be_bytes(),
                false => d.to_le_bytes(),
            };
            h[40 + 2 * i..42 + 2 * i].copy_from_slice(&b);
        }
        h[344..348].copy_from_slice(b"n+1\0");
        h
    }

    fn nifti2(dims: &[i64]) -> Vec<u8> {
        let mut h = vec![0u8; 540];
        h[0..4].copy_from_slice(&540i32.to_le_bytes());
        for (i, d) in dims.iter().enumerate() {
            h[16 + 8 * i..24 + 8 * i].copy_from_slice(&d.to_le_bytes());
        }
        h
    }

    #[test]
    fn the_volume_count_is_dim_4_of_a_4d_image_and_one_of_a_3d_one() {
        assert_eq!(volumes_of(&nifti1(&[4, 64, 64, 20, 30], false)), Ok(30));
        assert_eq!(volumes_of(&nifti1(&[4, 64, 64, 20, 30], true)), Ok(30));
        assert_eq!(volumes_of(&nifti1(&[3, 64, 64, 20, 1], false)), Ok(1));
        assert_eq!(volumes_of(&nifti1(&[3, 64, 64, 20, 0], false)), Ok(1));
        assert_eq!(volumes_of(&nifti2(&[4, 64, 64, 20, 7])), Ok(7));
        assert!(volumes_of(&[0u8; 10]).is_err());
        assert!(volumes_of(&[1u8; 348]).is_err(), "not NIfTI");
        assert!(volumes_of(&nifti1(&[4, 64, 64, 20, 0], false)).is_err());
    }

    #[test]
    fn a_gzipped_image_is_read_through_its_compression() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("nils-aslcontext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stem = "sub-x_ses-1_run-2_asl";
        let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut body = nifti1(&[4, 2, 2, 2, 3], false);
        body.extend_from_slice(&[0u8; 4 + 2 * 2 * 2 * 3 * 2]);
        z.write_all(&body).unwrap();
        std::fs::write(dir.join(format!("{stem}.nii.gz")), z.finish().unwrap()).unwrap();
        let written = write(&dir, stem, "deltam");
        let text = std::fs::read_to_string(dir.join("sub-x_ses-1_run-2_aslcontext.tsv"));
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(written.as_deref(), Ok("sub-x_ses-1_run-2_aslcontext.tsv"));
        assert_eq!(text.unwrap(), "volume_type\ndeltam\ndeltam\ndeltam\n");
    }

    #[test]
    fn the_table_is_one_row_per_volume_under_its_header() {
        assert_eq!(table("deltam", 2), "volume_type\ndeltam\ndeltam\n");
        assert_eq!(table("m0scan", 1), "volume_type\nm0scan\n");
        assert_eq!(
            file_name("sub-x_acq-PCASL_asl").as_deref(),
            Some("sub-x_acq-PCASL_aslcontext.tsv")
        );
        assert_eq!(file_name("sub-x_m0scan"), None);
        assert!(write(Path::new("/nonexistent"), "sub-x_T1w", "deltam").is_err());
        // and the standard has the file, as a table
        let g = crate::bids::schema::group_of("perf", "aslcontext").unwrap();
        assert_eq!(g.extensions, &[".tsv"]);
        for t in nils_pack::bids::ASL_VOLUME_TYPES {
            assert!(!t.is_empty());
        }
    }
}
