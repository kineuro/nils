// SPDX-License-Identifier: AGPL-3.0-only

//! The peak resident set size of this process, for the report's last line.

/// Peak RSS in bytes, when the platform tells it.
#[cfg(target_os = "linux")]
pub fn peak_rss() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Peak RSS in bytes, when the platform tells it. `ru_maxrss` is bytes on
/// macOS and kilobytes on the BSDs.
#[cfg(all(unix, not(target_os = "linux")))]
#[allow(
    unsafe_code,
    reason = "getrusage fills a plain struct through the pointer it is given"
)]
pub fn peak_rss() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: RUSAGE_SELF is a valid selector and the pointer is to a struct of
    // the right type that lives for the whole call.
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: getrusage returned 0, so the struct is initialised.
    let usage = unsafe { usage.assume_init() };
    let max = u64::try_from(usage.ru_maxrss).ok()?;
    Some(if cfg!(target_os = "macos") {
        max
    } else {
        max * 1024
    })
}

/// Peak RSS in bytes, when the platform tells it.
#[cfg(not(unix))]
pub fn peak_rss() -> Option<u64> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn the_peak_is_known_and_plausible() {
        let rss = super::peak_rss().expect("peak RSS on unix");
        assert!(rss > 1 << 20, "{rss} bytes");
        assert!(rss < 1 << 40, "{rss} bytes");
    }
}
