// SPDX-License-Identifier: AGPL-3.0-only

//! What a fingerprint run says when it is done.

use std::fmt;

/// The counts of one run, printable as a page or as JSON.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    pub job_id: i64,
    pub epoch: i64,
    /// Stacks the run looked at.
    pub read: i64,
    /// Stacks already derived and still agreeing with their instance count.
    pub skipped: i64,
    /// Fingerprints written.
    pub written: i64,
    /// Stacks whose first instance carries no pixel spacing, so the field of
    /// view is unknown. A count worth watching: it is usually a vendor whose
    /// geometry lives somewhere the reader does not look yet.
    pub without_geometry: i64,
    pub seconds: f64,
    pub peak_rss: Option<u64>,
    pub cancelled: bool,
}

impl Report {
    pub fn new(job_id: i64, epoch: i64) -> Report {
        Report {
            job_id,
            epoch,
            read: 0,
            skipped: 0,
            written: 0,
            without_geometry: 0,
            seconds: 0.0,
            peak_rss: None,
            cancelled: false,
        }
    }

    /// Stacks a second over the whole run.
    pub fn rate(&self) -> f64 {
        if self.seconds <= 0.0 {
            return 0.0;
        }
        self.read as f64 / self.seconds
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "fingerprint (job {}, epoch {})", self.job_id, self.epoch)?;
        writeln!(f, "  stacks read      {:>12}", self.read)?;
        writeln!(f, "  written          {:>12}", self.written)?;
        writeln!(f, "  already derived  {:>12}", self.skipped)?;
        writeln!(f, "  without geometry {:>12}", self.without_geometry)?;
        writeln!(f, "  {:.1} s, {:.0} stacks/s", self.seconds, self.rate())?;
        if let Some(rss) = self.peak_rss {
            writeln!(
                f,
                "  peak RSS         {:>9.2} GiB",
                rss as f64 / (1 << 30) as f64
            )?;
        }
        if self.cancelled {
            writeln!(f, "  cancelled; what was written is committed")?;
        }
        Ok(())
    }
}
