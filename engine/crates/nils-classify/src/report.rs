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
    /// Studies told whether they hold a primary (§6), which is half of the
    /// session rescue. Counted because it is not the number of stacks read:
    /// a study whose stacks are only partly derived is left to say nothing.
    pub studies_settled: i64,
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
            studies_settled: 0,
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
        writeln!(f, "  studies settled  {:>12}", self.studies_settled)?;
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
    /// Stacks given a disposition after the passes (Wave 3 §7).
    #[serde(default)]
    pub disposed: i64,
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
    /// What each pass did (§7): its reference, its scale and its methods.
    pub passes: Vec<crate::passes::Ran>,
    /// How many axis verdicts each tier decided, as `axis:tier`. A rule's
    /// reach is a number here rather than a review item per stack.
    pub by_tier: std::collections::BTreeMap<String, i64>,
    /// The number that matters: a pack that flags everything has failed even
    /// if it agrees with v0 (§8.2). A count of items, not of stacks: one
    /// stack can raise a conflict and a weak answer and is two items here.
    pub review_items: i64,
    /// The distinct stacks those items stand on (record 35). This is the
    /// only one of the three that may be held against the stacks the run
    /// classified, and it is what the share in the report is worked out
    /// from.
    #[serde(default)]
    pub review_stacks: i64,
    /// Wave 4a §10.2: the items those questions collapsed into, one per
    /// (kind, value, tier). This is the length of the queue a person reads.
    pub review_groups: i64,
    /// Per axis, the answers written at exactly that axis's own review
    /// threshold (§8.2). A threshold is read as strictly below, so these are
    /// answers and not questions, and a threshold set at the confidence a
    /// rule always writes takes a whole axis out of the queue by its
    /// boundary. That is a fact about the pack, so it is a number in the
    /// report rather than a review item per stack.
    #[serde(default)]
    pub at_threshold: std::collections::BTreeMap<String, i64>,
    /// Wave 4c §6.6: what the evaluator noticed and did not act on, by
    /// kind, over every batch: `axis_conflict`, `axis_unresolved`,
    /// `keyword_shadowed` (keywords that can never match) and
    /// `overlay_unused` (site terms that matched nothing). The rows are in
    /// the batch's `diagnostic` table with samples.
    #[serde(default)]
    pub diagnostics: std::collections::BTreeMap<String, i64>,
    /// Record 48: the pack's own constraints the rules' answers broke, as
    /// `excluded:<id>` or `implied:<set/rule>`, a stack counted once per
    /// constraint. Each raised a `classify.excluded` or `classify.implied`
    /// item, and the axes it involves were written below every threshold.
    #[serde(default)]
    pub broken: std::collections::BTreeMap<String, i64>,
    /// The stacks whose answer broke at least one of them.
    #[serde(default)]
    pub broken_stacks: i64,
    pub seconds: f64,
    pub peak_rss: Option<u64>,
    pub cancelled: bool,
}

impl Classified {
    pub fn new(job_id: i64, epoch: i64, pack: String) -> Classified {
        Classified {
            job_id,
            disposed: 0,
            epoch,
            pack,
            read: 0,
            written: 0,
            no_pack: 0,
            evidence: 0,
            decided: 0,
            silent: 0,
            passes: Vec::new(),
            by_tier: std::collections::BTreeMap::new(),
            review_items: 0,
            review_stacks: 0,
            review_groups: 0,
            at_threshold: std::collections::BTreeMap::new(),
            diagnostics: std::collections::BTreeMap::new(),
            broken: std::collections::BTreeMap::new(),
            broken_stacks: 0,
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

    /// Every answer that sits exactly on its own review threshold, over
    /// every axis and every pass: the population one hundredth from being a
    /// question and never asked about.
    pub fn on_the_threshold(&self) -> i64 {
        self.at_threshold.values().sum::<i64>()
            + self.passes.iter().map(|p| p.at_threshold).sum::<i64>()
    }

    /// What share of the classified stacks raised something for a person.
    ///
    /// Record 35: stacks over stacks. The share used to be the items over
    /// the stacks, which counted a stack twice for asking two questions and
    /// then read the answer as a share of stacks, so a run that asked about
    /// two thirds of the archive reported nearly all of it.
    pub fn review_share(&self) -> f64 {
        if self.written == 0 {
            return 0.0;
        }
        self.review_stacks as f64 / self.written as f64
    }
}

impl fmt::Display for Classified {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "classify with {} (job {})", self.pack, self.job_id)?;
        writeln!(f, "  stacks read      {:>12}", self.read)?;
        writeln!(f, "  classified       {:>12}", self.written)?;
        writeln!(f, "  no pack          {:>12}", self.no_pack)?;
        writeln!(f, "  evidence rows    {:>12}", self.evidence)?;
        if self.disposed > 0 {
            writeln!(
                f,
                "  disposed         {:>12}   after the passes",
                self.disposed
            )?;
        }
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
        // Record 35: three numbers, each against what it is a count of. The
        // stacks are a share of the stacks classified; the items are what
        // those stacks raised, which is more than one on a stack that both
        // disagrees with itself and answers weakly; the questions are the
        // length of the queue a person opens.
        writeln!(
            f,
            "  stacks to review {:>12}   {:.1}% of the {} classified",
            self.review_stacks,
            100.0 * self.review_share(),
            self.written
        )?;
        writeln!(
            f,
            "  review items     {:>12}   on those stacks, as {} question(s)",
            self.review_items, self.review_groups
        )?;
        if self.on_the_threshold() > 0 {
            let mut on: Vec<(&String, &i64)> = self.at_threshold.iter().collect();
            on.sort_by_key(|(axis, n)| (-**n, (*axis).clone()));
            let line: Vec<String> = on.iter().map(|(axis, n)| format!("{axis} {n}")).collect();
            writeln!(
                f,
                "  on the threshold  {:>11}   answers at exactly the confidence their threshold names{}",
                self.on_the_threshold(),
                if line.is_empty() {
                    String::new()
                } else {
                    format!("\n    {}", line.join(", "))
                }
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
        for p in &self.passes {
            writeln!(
                f,
                "  {} ({}) against {} of {} stack(s)",
                p.pass, p.kind, p.reference, p.pool
            )?;
            writeln!(
                f,
                "    {} of {} target(s) answered, {} review item(s)",
                p.decided, p.targets, p.review_items
            )?;
            let mut how: Vec<(&String, &i64)> = p.by_method.iter().collect();
            how.sort_by_key(|(_, n)| -**n);
            for (method, n) in how.iter().take(5) {
                writeln!(f, "    {method:<24} {n:>8}")?;
            }
        }
        if self.broken_stacks > 0 {
            let line: Vec<String> = self
                .broken
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect();
            writeln!(
                f,
                "  against the pack {:>12}   stacks whose answer breaks its own constraints: {}",
                self.broken_stacks,
                line.join(", ")
            )?;
        }
        if !self.diagnostics.is_empty() {
            let line: Vec<String> = self
                .diagnostics
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect();
            writeln!(f, "  diagnostics      {}", line.join(", "))?;
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
