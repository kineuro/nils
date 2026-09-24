// SPDX-License-Identifier: AGPL-3.0-only

//! What a run left under its output folder: a unit's files found by the
//! descriptor's path templates, a file a results entry names checked to lie
//! under the folder, and each file's size and sha256, read by the engine and
//! never taken from the pipeline.

use std::io::{Read, Write};
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

/// Read a file under the output folder, the engine's or a pipeline's,
/// only where [`inside`] takes it: never through a link out of the folder.
pub fn read_inside(root: &Path, rel: &str) -> Result<Vec<u8>, String> {
    let file = inside(root, rel)?;
    std::fs::read(&file).map_err(|e| format!("{rel}: {e}"))
}

/// Write a file the engine makes, refusing one that is there already and
/// never following a link: the file is created new (`O_EXCL`) and opened
/// with `O_NOFOLLOW`, so a link a container planted is refused, not
/// written through.
pub fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NOFOLLOW);
    }
    let mut f = open.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Which of `outputs` a unit's file is: the first whose template, with the
/// unit's own words filled in, matches it. None when the file is not the
/// unit's, however another unit's template would read it.
pub fn unit_output<'a>(
    outputs: &'a [crate::descriptor::Output],
    rel: &str,
    vars: &[(&str, &str)],
) -> Option<&'a crate::descriptor::Output> {
    outputs.iter().find(|o| {
        // a * beside a unit's word lets one unit take another's file
        if crate::descriptor::wildcard_touches_a_word(&o.template) {
            return false;
        }
        let mut t = o.template.clone();
        for (k, v) in vars {
            t = t.replace(&format!("{{{k}}}"), &glob::Pattern::escape(v));
        }
        !t.contains('{')
            && glob::Pattern::new(&t).is_ok_and(|p| {
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

    /// The review of record 43: an engine write never follows a link a
    /// container planted, and a unit claims only its own files.
    #[test]
    fn an_engine_write_refuses_a_link_and_a_unit_claims_only_its_own_files() {
        let dir = temp("write-new");
        let host = dir.join("host.txt");
        std::fs::write(&host, "the host's own").unwrap();
        #[cfg(unix)]
        {
            let link = dir.join("planted.json");
            std::os::unix::fs::symlink(&host, &link).unwrap();
            assert!(write_new(&link, b"engine").is_err());
            assert_eq!(std::fs::read_to_string(&host).unwrap(), "the host's own");
            // a link out of the folder is not read either
            let out = dir.join("out");
            std::fs::create_dir_all(&out).unwrap();
            std::os::unix::fs::symlink(&host, out.join("results.json")).unwrap();
            assert!(read_inside(&out, "results.json").is_err());
        }
        assert!(write_new(&host, b"engine").is_err(), "there already");
        let fresh = dir.join("fresh.json");
        write_new(&fresh, b"engine").unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"engine");

        let outputs = vec![crate::descriptor::Output {
            id: "o".into(),
            kind: "output".into(),
            template: "stack-{stack}/out.txt".into(),
            media_type: None,
            run_level: false,
            card: None,
        }];
        assert!(unit_output(&outputs, "stack-4/out.txt", &[("stack", "4")]).is_some());
        assert!(unit_output(&outputs, "stack-5/out.txt", &[("stack", "4")]).is_none());
        assert!(unit_output(&outputs, "other/out.txt", &[("stack", "4")]).is_none());
        // stack 4 never takes stack 45's file through a * beside its word
        let loose = vec![crate::descriptor::Output {
            template: "{stack}*.nii".into(),
            ..outputs[0].clone()
        }];
        assert!(unit_output(&loose, "45.nii", &[("stack", "4")]).is_none());
    }
}
