// SPDX-License-Identifier: AGPL-3.0-only

//! What a run left under its output folder: a unit's files found by the
//! descriptor's path templates, a file a results entry names checked to lie
//! under the folder, and each file's size and sha256, read by the engine and
//! never taken from the pipeline.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// The size and the sha256 (hex) of a file, read through once.
pub fn sha256_file(path: &Path) -> std::io::Result<(u64, String)> {
    let mut f = std::fs::File::open(path)?;
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        context.update(&buffer[..n]);
    }
    Ok((total, hex::encode(context.finish().as_ref())))
}

/// A file a pipeline named, relative to its output folder: refused when it
/// is absolute, steps out, is not a regular file, or resolves (through a
/// link) to anywhere outside the folder. Answers the file on disk.
pub fn inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    if rel.is_empty()
        || p.is_absolute()
        || p.components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!("{rel} is not a path under the output folder"));
    }
    let real_root = std::fs::canonicalize(root).map_err(|e| format!("the output folder: {e}"))?;
    let joined = root.join(p);
    let real = std::fs::canonicalize(&joined).map_err(|_| format!("{rel} is not there"))?;
    if !real.starts_with(&real_root) {
        return Err(format!("{rel} leads outside the output folder"));
    }
    if !real.is_file() {
        return Err(format!("{rel} is not a file"));
    }
    Ok(real)
}

/// The files under `root` a path template finds for one unit, relative to
/// `root` and sorted: the template's `{name}` words replaced by the unit's
/// values, `*` matching inside one segment.
pub fn found(root: &Path, template: &str, vars: &[(&str, &str)]) -> Vec<String> {
    let mut expanded = template.to_string();
    for (k, v) in vars {
        expanded = expanded.replace(&format!("{{{k}}}"), &glob::Pattern::escape(v));
    }
    if expanded.contains('{') {
        return Vec::new();
    }
    let base = glob::Pattern::escape(&root.display().to_string());
    let pattern = format!("{base}/{expanded}");
    let Ok(paths) = glob::glob_with(
        &pattern,
        glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        },
    ) else {
        return Vec::new();
    };
    let mut out: Vec<String> = paths
        .filter_map(Result::ok)
        .filter(|p| p.is_file())
        .filter_map(|p| {
            p.strip_prefix(root)
                .ok()
                .map(|r| r.to_string_lossy().into_owned())
        })
        .collect();
    out.sort();
    out
}

/// Whether a relative path is one a template could find for some unit, and
/// which output that is: the first whose template, read with its words as
/// `*`, matches.
pub fn which_output<'a>(
    outputs: &'a [crate::descriptor::Output],
    rel: &str,
) -> Option<&'a crate::descriptor::Output> {
    outputs.iter().find(|o| {
        let mut t = o.template.clone();
        for w in ["{subject}", "{session}", "{stack}"] {
            t = t.replace(w, "*");
        }
        glob::Pattern::new(&t).is_ok_and(|p| {
            p.matches_with(
                rel,
                glob::MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: true,
                    require_literal_leading_dot: false,
                },
            )
        })
    })
}

/// The media type of a file: the one declared, else by its extension.
pub fn media_type(rel: &str, declared: Option<&str>) -> String {
    if let Some(d) = declared {
        return d.to_string();
    }
    let lower = rel.to_ascii_lowercase();
    let by_ext = [
        (".nii.gz", "application/x-nifti+gzip"),
        (".nii", "application/x-nifti"),
        (".json", "application/json"),
        (".tsv", "text/tab-separated-values"),
        (".csv", "text/csv"),
        (".npy", "application/x-npy"),
        (".npz", "application/x-npz"),
        (".png", "image/png"),
        (".txt", "text/plain"),
        (".gz", "application/gzip"),
    ];
    by_ext
        .iter()
        .find(|(e, _)| lower.ends_with(e))
        .map(|(_, t)| (*t).to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("nils-pipeline-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_unit_s_files_are_found_by_its_template_and_nothing_outside_is_taken() {
        let root = temp("found");
        let out = root.join("out");
        std::fs::create_dir_all(out.join("sub-a/ses-1/anat")).unwrap();
        std::fs::create_dir_all(out.join("sub-a/ses-2/anat")).unwrap();
        std::fs::write(
            out.join("sub-a/ses-1/anat/sub-a_ses-1_desc-n4_T1w.nii.gz"),
            b"one",
        )
        .unwrap();
        std::fs::write(
            out.join("sub-a/ses-2/anat/sub-a_ses-2_acq-x_desc-n4_T1w.nii.gz"),
            b"two",
        )
        .unwrap();
        std::fs::write(root.join("secret"), b"not the pipeline's").unwrap();
        let t = "sub-{subject}/ses-{session}/anat/*_desc-n4_T1w.nii.gz";
        assert_eq!(
            found(&out, t, &[("subject", "a"), ("session", "1")]),
            ["sub-a/ses-1/anat/sub-a_ses-1_desc-n4_T1w.nii.gz"]
        );
        assert_eq!(
            found(&out, t, &[("subject", "a"), ("session", "3")]),
            Vec::<String>::new()
        );
        // a word that looks like a pattern is a word
        assert!(found(&out, t, &[("subject", "*"), ("session", "*")]).is_empty());

        assert!(inside(&out, "sub-a/ses-1/anat/sub-a_ses-1_desc-n4_T1w.nii.gz").is_ok());
        assert!(
            inside(&out, "../secret")
                .unwrap_err()
                .contains("not a path")
        );
        assert!(inside(&out, "/etc/passwd").is_err());
        assert!(inside(&out, "sub-a").unwrap_err().contains("not a file"));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("secret"), out.join("link")).unwrap();
            assert!(inside(&out, "link").unwrap_err().contains("outside"));
        }
        let (bytes, hex) =
            sha256_file(&out.join("sub-a/ses-1/anat/sub-a_ses-1_desc-n4_T1w.nii.gz")).unwrap();
        assert_eq!(bytes, 3);
        assert_eq!(format!("sha256:{hex}"), crate::sha256(b"one"));
        assert_eq!(media_type("x.nii.gz", None), "application/x-nifti+gzip");
        assert_eq!(media_type("x.bin", Some("a/b")), "a/b");
        let _ = std::fs::remove_dir_all(&root);
    }
}
