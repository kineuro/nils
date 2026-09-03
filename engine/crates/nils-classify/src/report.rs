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

/// What a classification run says when it is done.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Classified {
    pub job_id: i64,
    pub epoch: i64,
    /// `name@version` of the pack that judged, which every row records too.
    pub pack: String,
    pub read: i64,
    pub written: i64,
    /// Stacks whose modality this pack does not judge. An explicit outcome,
    /// never a review item and never `misc` (§9).
    pub no_pack: i64,
    pub evidence: i64,
    /// Axes a person's decision decided rather than a rule (§8.3).
    pub decided: i64,
    /// Stacks the pack says nobody is to be asked about.
    pub silent: i64,
    /// How many axis verdicts each tier decided, as `axis:tier`. A rule's
    /// reach is a number here rather than a review item per stack.
    pub by_tier: std::collections::BTreeMap<String, i64>,
    /// The number that matters: a pack that flags everything has failed even
    /// if it agrees with v0 (§8.2).
    pub review_items: i64,
    pub seconds: f64,
    pub peak_rss: Option<u64>,
    pub cancelled: bool,
}

impl Classified {
    pub fn new(job_id: i64, epoch: i64, pack: String) -> Classified {
        Classified {
            job_id,
            epoch,
            pack,
            read: 0,
            written: 0,
            no_pack: 0,
            evidence: 0,
            decided: 0,
            silent: 0,
            by_tier: std::collections::BTreeMap::new(),
            review_items: 0,
            seconds: 0.0,
            peak_rss: None,
            cancelled: false,
        }
    }

    pub fn rate(&self) -> f64 {
        if self.seconds <= 0.0 {
            return 0.0;
        }
        self.read as f64 / self.seconds
    }

    /// What share of the classified stacks raised something for a person.
    pub fn review_share(&self) -> f64 {
        if self.written == 0 {
            return 0.0;
        }
        self.review_items as f64 / self.written as f64
    }
}

impl fmt::Display for Classified {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "classify with {} (job {})", self.pack, self.job_id)?;
        writeln!(f, "  stacks read      {:>12}", self.read)?;
        writeln!(f, "  classified       {:>12}", self.written)?;
        writeln!(f, "  no pack          {:>12}", self.no_pack)?;
        writeln!(f, "  evidence rows    {:>12}", self.evidence)?;
        if self.decided > 0 {
            writeln!(f, "  decided by hand  {:>12}", self.decided)?;
        }
        if self.silent > 0 {
            writeln!(
                f,
                "  asked nothing of {:>12}   stacks the pack rules out",
                self.silent
            )?;
        }
        let mut weakest: Vec<(&String, &i64)> = self
            .by_tier
            .iter()
            .filter(|(k, _)| k.ends_with(":physics") || k.ends_with(":default"))
            .collect();
        weakest.sort_by_key(|(_, n)| -**n);
        for (what, n) in weakest.iter().take(3) {
            writeln!(f, "  {what:<28} {n:>8}   decided with no keyword")?;
        }
        writeln!(
            f,
            "  review items     {:>12}   {:.1}% of the stacks",
            self.review_items,
            100.0 * self.review_share()
        )?;
        let mut weakest: Vec<(&String, &i64)> = self
            .by_tier
            .iter()
            .filter(|(k, _)| k.ends_with(":physics") || k.ends_with(":default"))
            .collect();
        weakest.sort_by_key(|(_, n)| -**n);
        for (what, n) in weakest.iter().take(3) {
            writeln!(f, "  {what:<28} {n:>8}   decided with no keyword")?;
        }
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
