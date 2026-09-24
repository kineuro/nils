// SPDX-License-Identifier: AGPL-3.0-only

//! The pipeline lane's arithmetic (record 49 A1 and A2): the budget a lane
//! runs its units within, what this machine really has, and the GPU lease.
//!
//! The budget is a setting, 48 cores and 512 GB by default (record 49 R6),
//! and never more than the machine or the container the engine runs in
//! offers: the cores this process may use, and the smaller of the memory
//! the kernel reports and the memory cap of the process's cgroup. A unit
//! starts only when its declared cores and memory fit in what the running
//! units leave, so the lane fills the budget and never oversubscribes it.
//!
//! The GPU lease is taken on one named card: before a GPU unit starts, the
//! card's free memory is read from `nvidia-smi`, and the unit waits while
//! that, less what the lane's own running units on the card declared, is
//! below its need. Staff share the card (record 49 R2), so the card's own
//! word is what counts, not the lane's bookkeeping alone.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The default budget of the lane (record 49 R6).
pub const DEFAULT_CORES: u32 = 48;
pub const DEFAULT_MEMORY_GB: u64 = 512;

/// Memory in MiB, the unit the budget counts in.
pub fn mib_of_gb(gb: f64) -> u64 {
    (gb * 1024.0).ceil().max(0.0) as u64
}

/// What one scheduling unit asks of the lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask {
    pub cores: u32,
    pub memory_mib: u64,
}

/// The lane's budget and what its running units hold of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ledger {
    pub cores: u32,
    pub memory_mib: u64,
    pub held_cores: u32,
    pub held_memory_mib: u64,
}

impl Ledger {
    pub fn new(cores: u32, memory_mib: u64) -> Ledger {
        Ledger {
            cores,
            memory_mib,
            held_cores: 0,
            held_memory_mib: 0,
        }
    }

    /// Whether an ask could ever run within this budget.
    pub fn could(&self, ask: Ask) -> bool {
        ask.cores <= self.cores && ask.memory_mib <= self.memory_mib
    }

    /// Whether an ask fits in what the running units leave now.
    pub fn fits(&self, ask: Ask) -> bool {
        self.held_cores + ask.cores <= self.cores
            && self.held_memory_mib + ask.memory_mib <= self.memory_mib
    }

    /// Take an ask that fits; false, and nothing taken, when it does not.
    pub fn take(&mut self, ask: Ask) -> bool {
        if !self.fits(ask) {
            return false;
        }
        self.held_cores += ask.cores;
        self.held_memory_mib += ask.memory_mib;
        true
    }

    /// Give back what a unit that ended held.
    pub fn give(&mut self, ask: Ask) {
        self.held_cores = self.held_cores.saturating_sub(ask.cores);
        self.held_memory_mib = self.held_memory_mib.saturating_sub(ask.memory_mib);
    }
}

/// What this machine, or the container the engine runs in, offers: the
/// cores this process may use and the memory it may hold, in MiB.
pub fn host() -> (u32, u64) {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    let meminfo = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| mem_total_mib(&t));
    let cap = cgroup_memory_cap_mib();
    let memory = match (meminfo, cap) {
        (Some(m), Some(c)) => m.min(c),
        (Some(m), None) => m,
        (None, Some(c)) => c,
        (None, None) => u64::MAX / 4,
    };
    (cores, memory)
}

/// `MemTotal` of `/proc/meminfo`, in MiB.
pub fn mem_total_mib(meminfo: &str) -> Option<u64> {
    meminfo.lines().find_map(|l| {
        let rest = l.strip_prefix("MemTotal:")?;
        let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
        Some(kib / 1024)
    })
}

/// The memory cap of this process's cgroup (v2 `memory.max`, v1
/// `memory.limit_in_bytes`), in MiB, where there is one.
fn cgroup_memory_cap_mib() -> Option<u64> {
    let own = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    for line in own.lines() {
        let mut parts = line.splitn(3, ':');
        let (_, controllers, path) = (parts.next()?, parts.next()?, parts.next()?);
        let file = if controllers.is_empty() {
            Path::new("/sys/fs/cgroup")
                .join(path.trim_start_matches('/'))
                .join("memory.max")
        } else if controllers.split(',').any(|c| c == "memory") {
            Path::new("/sys/fs/cgroup/memory")
                .join(path.trim_start_matches('/'))
                .join("memory.limit_in_bytes")
        } else {
            continue;
        };
        if let Some(cap) = std::fs::read_to_string(file).ok().and_then(|t| cap_mib(&t)) {
            return Some(cap);
        }
    }
    None
}

/// A cgroup memory cap's text as MiB; `max`, or a number so large it is
/// no cap, is none.
pub fn cap_mib(text: &str) -> Option<u64> {
    let t = text.trim();
    if t == "max" {
        return None;
    }
    let bytes: u64 = t.parse().ok()?;
    // v1 writes "no cap" as a number near i64::MAX
    if bytes >= (1u64 << 60) {
        return None;
    }
    Some(bytes / (1024 * 1024))
}

/// The lane's budget: the setting, never above what the machine offers.
/// Answers the ledger and, where the machine cut the setting, why.
pub fn budget(cores: u32, memory_gb: u64, host: (u32, u64)) -> (Ledger, Option<String>) {
    let want_mib = memory_gb.saturating_mul(1024);
    let c = cores.min(host.0).max(1);
    let m = want_mib.min(host.1);
    let mut why = Vec::new();
    if c < cores {
        why.push(format!(
            "{c} cores, since this machine offers {} of the {cores} set",
            host.0
        ));
    }
    if m < want_mib {
        why.push(format!(
            "{} GB of memory, since this machine or its container caps it there, below the {memory_gb} GB set",
            m / 1024
        ));
    }
    (Ledger::new(c, m), (!why.is_empty()).then(|| why.join("; ")))
}

/// The free memory of one card, in MiB, as `nvidia-smi --query-gpu=
/// memory.free --format=csv,noheader -i <card>` says it.
pub fn gpu_free_mib(smi: &Path, card: u32) -> Result<u64, String> {
    let mut child = Command::new(smi)
        .args([
            "--query-gpu=memory.free",
            "--format=csv,noheader",
            "-i",
            &card.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("nvidia-smi: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > Duration::from_secs(20) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("nvidia-smi did not answer within 20 s".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("nvidia-smi: {e}")),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("nvidia-smi: {e}"))?;
    if !out.status.success() {
        return Err(format!("nvidia-smi does not answer for card {card}"));
    }
    parse_free(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| format!("nvidia-smi's answer for card {card} is not a memory size"))
}

/// `12345 MiB` (or a bare number, or GiB) as MiB.
pub fn parse_free(text: &str) -> Option<u64> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut words = line.split_whitespace();
    let n: f64 = words.next()?.parse().ok()?;
    let unit = words.next().unwrap_or("MiB");
    let mib = match unit {
        "MiB" | "MB" => n,
        "GiB" | "GB" => n * 1024.0,
        "KiB" | "KB" => n / 1024.0,
        _ => return None,
    };
    (mib >= 0.0 && mib.is_finite()).then_some(mib as u64)
}

/// Whether a unit that needs `need` MiB of the card may take a lease now:
/// the card's free memory, less what the lane's own running units there
/// declared (they may not have allocated it yet), covers the need.
pub fn lease_fits(free_mib: u64, held_mib: u64, need_mib: u64) -> bool {
    free_mib.saturating_sub(held_mib) >= need_mib
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The proof of record 49 A1 in arithmetic: however units of mixed
    /// needs arrive and end, the held cores and memory never pass the
    /// budget, and a unit that fits is never refused.
    #[test]
    fn the_ledger_never_passes_the_budget() {
        let mut l = Ledger::new(48, 512 * 1024);
        let asks = [
            Ask {
                cores: 8,
                memory_mib: 16 * 1024,
            },
            Ask {
                cores: 4,
                memory_mib: 200 * 1024,
            },
            Ask {
                cores: 32,
                memory_mib: 8 * 1024,
            },
            Ask {
                cores: 1,
                memory_mib: 1024,
            },
        ];
        let mut running: Vec<Ask> = Vec::new();
        let mut seed: u64 = 7;
        for _ in 0..10_000 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let a = asks[(seed >> 33) as usize % asks.len()];
            if (seed >> 20).is_multiple_of(3)
                && let Some(done) = running.pop()
            {
                l.give(done);
            }
            let fits =
                l.held_cores + a.cores <= 48 && l.held_memory_mib + a.memory_mib <= 512 * 1024;
            assert_eq!(l.take(a), fits);
            if fits {
                running.push(a);
            }
            assert!(l.held_cores <= l.cores && l.held_memory_mib <= l.memory_mib);
            let sum: u32 = running.iter().map(|a| a.cores).sum();
            assert_eq!(sum, l.held_cores);
        }
        assert!(!l.could(Ask {
            cores: 49,
            memory_mib: 1
        }));
    }

    #[test]
    fn the_budget_is_the_setting_within_the_machine() {
        let (l, why) = budget(48, 512, (64, 1024 * 1024));
        assert_eq!((l.cores, l.memory_mib), (48, 512 * 1024));
        assert!(why.is_none());
        let (l, why) = budget(48, 512, (16, 128 * 1024));
        assert_eq!((l.cores, l.memory_mib), (16, 128 * 1024));
        let why = why.unwrap();
        assert!(why.contains("16 cores") && why.contains("128 GB"), "{why}");
        assert_eq!(
            mem_total_mib("MemTotal:       32768000 kB\nMemFree: 1 kB\n"),
            Some(32000)
        );
        assert_eq!(cap_mib("max\n"), None);
        assert_eq!(cap_mib("9223372036854771712"), None);
        assert_eq!(cap_mib("8589934592"), Some(8192));
        let (cores, memory) = host();
        assert!(cores >= 1 && memory > 0);
    }

    #[test]
    fn the_card_s_word_is_read_and_a_lease_counts_the_lane_s_own() {
        assert_eq!(parse_free("23456 MiB\n"), Some(23456));
        assert_eq!(parse_free("  512\n"), Some(512));
        assert_eq!(parse_free("2 GiB"), Some(2048));
        assert_eq!(parse_free("[N/A]"), None);
        assert_eq!(parse_free(""), None);
        assert!(lease_fits(10_000, 0, 8_192));
        assert!(!lease_fits(10_000, 4_096, 8_192));
        assert!(!lease_fits(2_000, 0, 8_192));
        assert!(!lease_fits(100, 4_096, 1));
    }

    /// No real card is asked: a stand-in `nvidia-smi` answers.
    #[test]
    fn a_stand_in_nvidia_smi_is_read_by_card() {
        let dir = std::env::temp_dir().join(format!("nils-lane-smi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let smi = dir.join("nvidia-smi");
        std::fs::write(
            &smi,
            "#!/bin/sh\n[ \"$1\" = --query-gpu=memory.free ] || exit 9\n[ \"$4\" = 1 ] && { echo '4096 MiB'; exit 0; }\nexit 6\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&smi, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(gpu_free_mib(&smi, 1), Ok(4096));
            assert!(gpu_free_mib(&smi, 0).is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
