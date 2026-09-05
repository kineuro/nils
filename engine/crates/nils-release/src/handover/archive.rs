// SPDX-License-Identifier: AGPL-3.0-only

//! The archiver (`docs/specs/wave3-anonymize-and-bids.md`, §11).
//!
//! `7z` with encrypted headers, because the filenames of a de-identified tree
//! are themselves a description of a cohort: `sub-<code>/ses-M06/anat/...`
//! names how many people, how many visits and what was acquired, and a
//! recipient's mail server should not be able to read it.
//!
//! Three departures from v0's `compress/`, each measured on `7-Zip 26.00`.
//!
//! **The password never appears in `argv`.** v0 builds `-p{password}` into the
//! command line, where `ps` shows it to every user on the host for as long as
//! the archive takes to write, which for a 100 GB chunk is a long time. `7z`
//! reads the password from standard input when `-p` is given with no value, so
//! it is written to the child's stdin and nowhere else.
//!
//! **The archives are read back, and that is not a nicety.** `7z t` returns 0
//! for a good archive, 2 for a wrong password and 255 for none at all, which is
//! the rare case of a tool whose status says what happened. It caught two real
//! things on the first runs of this, neither of them documented anywhere:
//! `7z a -p` asks for the password **twice**, once to set it and once to
//! confirm it; and **`-p` means the opposite thing to `t`**, where it is not
//! "ask me" but "the password is the empty string", so a verification that
//! passes it reports a wrong password for every archive of a good set. v0 hits
//! neither, because it puts the password in `argv` and never reads anything
//! back unless asked.
//!
//! **Solid blocks stay off** (`-ms=off`), as in v0. A solid archive compresses
//! better and loses everything after the first damaged byte; a handover is the
//! one place where surviving damage is worth more than a few percent.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The archiver a deployment has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archiver {
    pub path: PathBuf,
    /// The whole first line it prints, which carries the version and the build.
    pub version: String,
}

impl Archiver {
    /// Find it and ask its version, or say what is missing.
    ///
    /// Once, before anything is packed. A handover that discovers a missing
    /// archiver after writing 400 GB has written 400 GB for nothing.
    pub fn find(path: &Path) -> Result<Archiver, String> {
        let out = Command::new(path).output().map_err(|e| {
            format!(
                "{} is not runnable ({e}). 7-Zip is a prerequisite of a handover: install \
                 p7zip or 7zip, and pass --7z if it is not on the path.",
                path.display()
            )
        })?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text
            .lines()
            .find(|l| l.to_lowercase().contains("7-zip"))
            .unwrap_or("")
            .trim();
        if line.is_empty() {
            return Err(format!(
                "{} ran and said nothing about itself",
                path.display()
            ));
        }
        Ok(Archiver {
            path: path.to_path_buf(),
            version: line.to_string(),
        })
    }

    pub fn describe(&self) -> String {
        self.version.clone()
    }

    /// Pack `members`, relative to `root`, into `archive`.
    ///
    /// The member list goes on stdin as well, through `-i@`, because a chunk of
    /// a real dataset names tens of thousands of directories and an argument
    /// list has a length nobody documents.
    pub fn pack(
        &self,
        root: &Path,
        members: &[String],
        archive: &Path,
        password: &str,
        level: u8,
    ) -> Result<(), String> {
        if members.is_empty() {
            return Err("nothing to pack".to_string());
        }
        // The list is a file rather than an argument, and it lives beside the
        // archive so that an interrupted run leaves it to be found.
        let list = archive.with_extension("7z.list");
        let mut text = String::new();
        for m in members {
            text.push_str(m);
            text.push('\n');
        }
        std::fs::write(&list, text).map_err(|e| format!("could not write the file list ({e})"))?;

        let mut child = Command::new(&self.path)
            .current_dir(root)
            .arg("a")
            .arg("-t7z")
            .arg(format!("-mx={level}"))
            .arg("-mmt=on")
            // The filenames are a description of the cohort, so they are
            // encrypted too.
            .arg("-mhe=on")
            // Not solid: a handover that loses everything after one damaged
            // byte is not a handover.
            .arg("-ms=off")
            // The password comes on stdin, not in argv.
            .arg("-p")
            .arg("-bso0")
            .arg("-bsp0")
            .arg(archive)
            .arg(format!("-i@{}", list.display()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("7z could not be run ({e})"))?;
        // **Twice.** `7z a -p` asks for the password and then asks again to
        // confirm it, and a run that answers once writes an archive with a
        // password nobody has: it exits 0, and the failure is only found by
        // reading the archive back. Which is why a handover reads them back.
        if let Some(stdin) = child.stdin.as_mut() {
            writeln!(stdin, "{password}\n{password}")
                .map_err(|e| format!("7z would not take the password ({e})"))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| format!("7z did not finish ({e})"))?;
        std::fs::remove_file(&list).ok();
        if !out.status.success() {
            std::fs::remove_file(archive).ok();
            return Err(format!("7z failed ({})", said(&out.stderr, &out.stdout)));
        }
        Ok(())
    }

    /// Read the archive back and say whether it is intact.
    ///
    /// The password is part of the test: an archive nobody can open is not an
    /// archive that was handed over.
    pub fn verify(&self, archive: &Path, password: &str) -> Result<(), String> {
        // **No `-p` here**, which is the asymmetry: on `a` it means "ask me for
        // a password", and on `t` it means "the password is the empty string".
        // With it, every archive of a good set reports a wrong password. 7z
        // asks for the password of an encrypted archive anyway, on stdin.
        let mut child = Command::new(&self.path)
            .arg("t")
            .arg("-bso0")
            .arg("-bsp0")
            .arg(archive)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("7z could not be run ({e})"))?;
        if let Some(stdin) = child.stdin.as_mut() {
            writeln!(stdin, "{password}").ok();
        }
        let out = child
            .wait_with_output()
            .map_err(|e| format!("7z did not finish ({e})"))?;
        match out.status.success() {
            true => Ok(()),
            // 2 is a wrong password and 255 is no password, which are worth
            // telling apart from a damaged archive.
            false => Err(match out.status.code() {
                Some(2) => "the password does not open it".to_string(),
                Some(255) => "7z was given no password".to_string(),
                _ => format!("7z t failed ({})", said(&out.stderr, &out.stdout)),
            }),
        }
    }
}

/// Recovery records, when a deployment has `par2` and asks for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Par2 {
    pub path: PathBuf,
    /// The redundancy, as a percentage.
    pub percent: u8,
}

impl Par2 {
    pub fn find(percent: u8) -> Result<Par2, String> {
        let path = which("par2create").ok_or_else(|| {
            "par2 recovery records were asked for and par2create is not on the path".to_string()
        })?;
        Ok(Par2 { path, percent })
    }

    pub fn cover(&self, archive: &Path) -> Result<(), String> {
        let out = Command::new(&self.path)
            .arg("-q")
            .arg(format!("-r{}", self.percent))
            .arg(archive)
            .output()
            .map_err(|e| format!("par2create could not be run ({e})"))?;
        match out.status.success() {
            true => Ok(()),
            false => Err(format!(
                "par2create failed ({})",
                said(&out.stderr, &out.stdout)
            )),
        }
    }
}

/// The last line a tool said, with the paths taken out: a report that repeats
/// a path names the tree.
fn said(stderr: &[u8], stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = match text.trim().is_empty() {
        true => String::from_utf8_lossy(stdout).trim().to_string(),
        false => text.trim().to_string(),
    };
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("no output")
        .split_whitespace()
        .filter(|w| !w.contains('/') && !w.contains('\\'))
        .collect::<Vec<_>>()
        .join(" ")
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_archiver_is_one_refusal_before_anything_is_packed() {
        let e = Archiver::find(Path::new("/nonexistent/7z")).unwrap_err();
        assert!(e.contains("--7z"), "{e}");
        assert!(e.contains("prerequisite"), "{e}");
    }

    #[test]
    fn nothing_to_pack_is_said_rather_than_run() {
        let a = Archiver {
            path: PathBuf::from("/nonexistent/7z"),
            version: "test".into(),
        };
        assert_eq!(
            a.pack(Path::new("/tmp"), &[], Path::new("/tmp/x.7z"), "p", 1),
            Err("nothing to pack".to_string())
        );
    }

    #[test]
    fn what_a_tool_said_never_carries_a_path() {
        assert_eq!(
            said(b"ERROR: /srv/secret/sub-abc : Cannot open", b""),
            "ERROR: : Cannot open"
        );
        assert_eq!(said(b"", b""), "no output");
    }

    #[test]
    fn par2_is_asked_for_and_refused_when_it_is_not_there() {
        // Asked for and absent is a refusal, not a silent skip: recovery
        // records nobody wrote are the ones nobody misses until they are
        // needed.
        if which("par2create").is_none() {
            assert!(Par2::find(5).is_err());
        }
    }
}
