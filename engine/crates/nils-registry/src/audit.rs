// SPDX-License-Identifier: AGPL-3.0-only

//! The audit log that is a table (Wave 4a §9.2): who did what, to which
//! scope, when, under which policy. Every decision, import, release,
//! handover, linkage change and acknowledgement writes one row here, in
//! the same transaction as the thing itself where there is one, and the
//! epoch advances with the ones that change a judgement (§13.5), so a
//! handle re-read after a human decision reports a different epoch.
//!
//! What a row holds is the shape of the act and never the identifiers: a
//! purge records how many, a reveal records which subject and why, a
//! release records its name, version and policy.

use crate::Registry;
use crate::schema::{Type, table};
use crate::store::{Error as StoreError, Insert, Param, Store};
use crate::time::now_iso;

/// What was done. The names are the verbs', dotted where a verb has
/// several acts, so that a filter on `linkage.` reads every linkage act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Decision,
    ReviewAccept,
    ClinicalImport,
    VocabularyLoad,
    LinkageImport,
    LinkageLink,
    LinkageUnlink,
    LinkagePurge,
    LinkageReveal,
    /// Wave 4c §6.5: an archive was written.
    Backup,
    /// Wave 4c §6.6: an overlay proposed as a registry object, and one
    /// adopted, which queues the reclassify that changes judgements.
    OverlayPropose,
    OverlayAdopt,
    Release,
    Handover,
    ReleaseWithdraw,
    JobsPrune,
    // Wave 4b §8.1: the cohort acts and the saved ask, widened with the
    // writer and not after it, so a promotion is never a membership writer
    // with no act and no epoch bump.
    CohortCreate,
    CohortMemberAdd,
    CohortMemberRemove,
    CohortPromote,
    /// Record 26 §8 and §9: a digest of a dataset fed its cohort; a cohort
    /// was renamed, its owner or description set, retired, or brought back.
    CohortJoin,
    CohortRename,
    CohortSet,
    CohortRetire,
    CohortRestore,
    SelectionSave,
    /// Wave 5 §12.8: a handle stopped reproducing under a change the
    /// dependency door named.
    HandleInvalidate,
    /// Wave 5 §12.7: a person opened a stack's pixels through the instance
    /// door; one row per stack opened, not per tile.
    InstanceOpen,
    /// Wave 5 §12.5: a place added, changed or retired.
    PlaceAdd,
    PlaceSet,
    PlaceRetire,
    /// Wave 5 §10.3: the registry's timezone or week start changed, which
    /// moves the epoch, since every dated answer is read under them.
    SettingsSet,
    /// Wave 5 §10.3: the backup schedule set.
    BackupSchedule,
    /// Record 26 §6: an alias subject merged into a canonical one, which
    /// moves every row of the alias and so the epoch.
    SubjectMerge,
    /// Record 26 §1: a dataset's originals moved into another place, and a
    /// dataset's originals deleted. What the row holds is the shape of the
    /// act: the dataset, where they went, how many files and bytes, and
    /// the reason the person gave.
    OriginalsVault,
    OriginalsPurge,
    /// Record 42 S2: a model registered, a check recorded on it, a model
    /// promoted in its slot (retiring the one before), a model retired.
    /// None changes a judgement: a model's answer is staged until a person
    /// commits it, and the commit is the act that moves the epoch.
    ModelRegister,
    ModelAdmit,
    ModelPromote,
    ModelRetire,
    /// Record 42 S3: a person picked the stack that stands for a session's
    /// role, or withdrew a person's pick. The ask reads picks, so both
    /// move the epoch.
    PickSet,
    PickWithdraw,
    /// Record 42 S4: a derivative registered in a working place, and one
    /// read whole through its door. Neither changes a judgement.
    DerivativeRegister,
    DerivativeRead,
    /// Record 42 S5 and S6: a campaign made, an item claimed under a lease,
    /// answered, given back, measured by an external metric, and the
    /// campaign closed. None of them is a decision; the decisions a close
    /// writes are audited by `apply` as every decision is.
    CampaignCreate,
    CampaignClaim,
    CampaignAnswer,
    CampaignRelease,
    CampaignMetric,
    CampaignClose,
    /// Record 48, after the first real read: an open axes campaign's
    /// question moved to fewer asked axes and more derived ones.
    CampaignRequestion,
    /// An open axes campaign's frozen constraints moved to the served
    /// pack's version, its answers kept as given.
    CampaignRepack,
    /// Record 50 R3: answers suggested for a campaign's items from outside
    /// the engine, with their author, imported. A suggestion is no answer.
    CampaignSuggest,
    /// Record 42 S7: a label set written out with its digest, and labels
    /// from v0 imported as person decisions.
    LabelsExport,
    LabelsImport,
    /// Record 40 R3: a sample sealed for certification, never training data.
    LabelsSeal,
    /// Record 48 R2: a certificate recorded for a sealed sample, and the
    /// sample unsealed by it, so its labels may train the next model.
    LabelsCertificate,
    LabelsUnseal,
    /// Record 43: a descriptor added to the pipeline catalog, a pipeline
    /// run over a frozen selection (its derivatives and review items with
    /// it), and the runtime an operator chose. None changes a judgement: a
    /// run's outputs are derivatives and its proposals evidence.
    PipelineAdd,
    PipelineRun,
    PipelineRuntime,
    /// A repair of what an older engine left in the registry: stacks and
    /// series that hold no instance removed (`nils repair empty-stacks`).
    /// The rows gone are what an ask reads, so it moves the epoch.
    RegistryRepair,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Decision => "decision",
            Action::ReviewAccept => "review.accept",
            Action::ClinicalImport => "clinical.import",
            Action::VocabularyLoad => "vocabulary.load",
            Action::LinkageImport => "linkage.import",
            Action::LinkageLink => "linkage.link",
            Action::LinkageUnlink => "linkage.unlink",
            Action::LinkagePurge => "linkage.purge",
            Action::LinkageReveal => "linkage.reveal",
            Action::Backup => "backup",
            Action::OverlayPropose => "overlay.propose",
            Action::OverlayAdopt => "overlay.adopt",
            Action::Release => "release",
            Action::Handover => "handover",
            Action::ReleaseWithdraw => "release.withdraw",
            Action::JobsPrune => "jobs.prune",
            Action::CohortCreate => "cohort.create",
            Action::CohortMemberAdd => "cohort.member.add",
            Action::CohortMemberRemove => "cohort.member.remove",
            Action::CohortPromote => "cohort.promote",
            Action::CohortJoin => "cohort.join",
            Action::CohortRename => "cohort.rename",
            Action::CohortSet => "cohort.set",
            Action::CohortRetire => "cohort.retire",
            Action::CohortRestore => "cohort.restore",
            Action::SelectionSave => "selection.save",
            Action::HandleInvalidate => "handle.invalidate",
            Action::InstanceOpen => "instance.open",
            Action::PlaceAdd => "place.add",
            Action::PlaceSet => "place.set",
            Action::PlaceRetire => "place.retire",
            Action::SettingsSet => "settings.set",
            Action::BackupSchedule => "backup.schedule",
            Action::SubjectMerge => "subject.merge",
            Action::OriginalsVault => "originals.vault",
            Action::OriginalsPurge => "originals.purge",
            Action::ModelRegister => "model.register",
            Action::ModelAdmit => "model.admit",
            Action::ModelPromote => "model.promote",
            Action::ModelRetire => "model.retire",
            Action::PickSet => "pick.set",
            Action::PickWithdraw => "pick.withdraw",
            Action::DerivativeRegister => "derivative.register",
            Action::DerivativeRead => "derivative.read",
            Action::CampaignCreate => "campaign.create",
            Action::CampaignClaim => "campaign.claim",
            Action::CampaignAnswer => "campaign.answer",
            Action::CampaignRelease => "campaign.release",
            Action::CampaignMetric => "campaign.metric",
            Action::CampaignClose => "campaign.close",
            Action::CampaignRequestion => "campaign.requestion",
            Action::CampaignRepack => "campaign.repack",
            Action::CampaignSuggest => "campaign.suggest",
            Action::LabelsExport => "labels.export",
            Action::LabelsImport => "labels.import",
            Action::LabelsSeal => "labels.seal",
            Action::LabelsCertificate => "labels.certificate",
            Action::LabelsUnseal => "labels.unseal",
            Action::PipelineAdd => "pipeline.add",
            Action::PipelineRun => "pipeline.run",
            Action::PipelineRuntime => "pipeline.runtime",
            Action::RegistryRepair => "registry.repair",
        }
    }

    /// Whether the act changes a judgement, which is when the epoch
    /// advances (§13.5). An acknowledgement and a reveal change none.
    pub fn changes_judgement(self) -> bool {
        !matches!(
            self,
            Action::ReviewAccept
                | Action::LinkageReveal
                | Action::JobsPrune
                | Action::Backup
                | Action::OverlayPropose
                | Action::InstanceOpen
                | Action::PlaceAdd
                | Action::PlaceSet
                | Action::PlaceRetire
                | Action::BackupSchedule
                | Action::ModelRegister
                | Action::ModelAdmit
                | Action::ModelPromote
                | Action::ModelRetire
                | Action::DerivativeRegister
                | Action::DerivativeRead
                | Action::CampaignCreate
                | Action::CampaignClaim
                | Action::CampaignAnswer
                | Action::CampaignRelease
                | Action::CampaignMetric
                | Action::CampaignClose
                | Action::CampaignRequestion
                | Action::CampaignRepack
                | Action::CampaignSuggest
                | Action::LabelsExport
                | Action::LabelsSeal
                | Action::LabelsCertificate
                | Action::LabelsUnseal
                | Action::PipelineAdd
                | Action::PipelineRun
                | Action::PipelineRuntime
        )
    }
}

/// One act to record.
#[derive(Debug, Clone)]
pub struct Entry<'a> {
    pub principal: &'a str,
    pub action: Action,
    /// What it touched: ids, names, counts. Never an identifier.
    pub scope: serde_json::Value,
    /// The policy it ran under, where there is one (a release's).
    pub policy: Option<serde_json::Value>,
    /// The job it ran as, where there is one.
    pub job_id: Option<i64>,
    /// Anything else worth a line: the why, the counts.
    pub details: Option<serde_json::Value>,
}

/// Write the row, and advance the epoch when the act changes a judgement.
/// Returns the row's id. Runs inside the caller's transaction when there
/// is one.
pub fn record(registry: &mut Registry, entry: &Entry<'_>) -> Result<i64, StoreError> {
    record_judging(registry, entry, entry.action.changes_judgement())
}

/// [`record`], saying whether this act changes a judgement where its action
/// alone cannot: a decision written staged is not in force, so it moves no
/// epoch, and the commit that puts it in force does (record 42 R6). Were a
/// stage to move the epoch, every staged decision would drift from itself
/// and the next stage, and no commit would go through without `--anyway`.
pub fn record_judging(
    registry: &mut Registry,
    entry: &Entry<'_>,
    changes_judgement: bool,
) -> Result<i64, StoreError> {
    let epoch = if changes_judgement {
        Some(
            registry
                .next_epoch()
                .map_err(|e| StoreError::Message(e.to_string()))?,
        )
    } else {
        None
    };
    write(registry.store(), entry, epoch)
}

/// [`record`] on a registry store held without its [`Registry`]: a merge
/// run inside an import. The epoch is read from the store and bumped
/// there, so a `Registry` open beside it reads the new value on its next
/// refresh.
pub fn record_in(store: &mut Store, entry: &Entry<'_>) -> Result<i64, StoreError> {
    let epoch = if entry.action.changes_judgement() {
        Some(crate::home::next_epoch_in(store)?)
    } else {
        None
    };
    write(store, entry, epoch)
}

fn write(store: &mut Store, entry: &Entry<'_>, epoch: Option<i64>) -> Result<i64, StoreError> {
    let now = now_iso();
    let rows = store.insert(
        &Insert::new(
            table("audit"),
            &[
                "at",
                "principal",
                "action",
                "scope",
                "policy",
                "job_id",
                "epoch",
                "details",
                "actor",
            ],
        )
        .returning(&["id"]),
        &[vec![
            Param::from(now.as_str()),
            Param::from(entry.principal),
            Param::from(entry.action.name()),
            Param::from(entry.scope.to_string()),
            entry
                .policy
                .as_ref()
                .map_or(Param::Null, |p| Param::from(p.to_string())),
            entry.job_id.map_or(Param::Null, Param::Int),
            epoch.map_or(Param::Null, Param::Int),
            entry
                .details
                .as_ref()
                .map_or(Param::Null, |d| Param::from(d.to_string())),
            Param::from(crate::actor::current().to_string()),
        ]],
    )?;
    rows.first()
        .ok_or_else(|| StoreError::Message("the audit row was not written back".into()))?
        .int(0)
}

/// One row as read.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: i64,
    pub at: String,
    pub principal: String,
    pub action: String,
    pub scope: serde_json::Value,
    pub policy: Option<serde_json::Value>,
    pub job_id: Option<i64>,
    pub epoch: Option<i64>,
    pub details: Option<serde_json::Value>,
    /// Wave 4c §5.5: who acted for the principal; absent is its own value.
    pub actor: Option<serde_json::Value>,
}

impl Row {
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "at": self.at,
            "principal": self.principal,
            "action": self.action,
            "scope": self.scope,
            "policy": self.policy,
            "job_id": self.job_id,
            "epoch": self.epoch,
            "details": self.details,
            "actor": self.actor,
        })
    }
}

/// What to list.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// This principal exactly.
    pub principal: Option<String>,
    /// This action, or every action under a dotted prefix (`linkage.`).
    pub action: Option<String>,
    /// From this time on (an ISO stamp; compared as text, which is right
    /// for the stamps the registry writes).
    pub since: Option<String>,
    pub limit: usize,
}

/// The rows, newest first.
pub fn list(store: &mut Store, filter: &Filter) -> Result<Vec<Row>, StoreError> {
    let d = store.dialect();
    let t = table("audit");
    let text = |c: &str| d.text_of(t.column(c).expect("audit column"));
    let mut wheres: Vec<String> = Vec::new();
    let mut params: Vec<Param> = Vec::new();
    if let Some(p) = &filter.principal {
        params.push(Param::from(p.as_str()));
        wheres.push(format!("principal = {}", d.param(params.len(), Type::Text)));
    }
    if let Some(a) = &filter.action {
        if let Some(prefix) = a.strip_suffix('.') {
            params.push(Param::from(format!("{prefix}.%")));
            wheres.push(format!("action LIKE {}", d.param(params.len(), Type::Text)));
        } else {
            params.push(Param::from(a.as_str()));
            wheres.push(format!("action = {}", d.param(params.len(), Type::Text)));
        }
    }
    if let Some(s) = &filter.since {
        params.push(Param::from(s.as_str()));
        wheres.push(format!(
            "{} >= {}",
            text("at"),
            d.param(params.len(), Type::Text)
        ));
    }
    let filter_sql = if wheres.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", wheres.join(" AND "))
    };
    let sql = format!(
        "SELECT id, {}, principal, action, {}, {}, job_id, epoch, {}, {} FROM {}{filter_sql} \
         ORDER BY id DESC LIMIT {}",
        text("at"),
        text("scope"),
        text("policy"),
        text("details"),
        text("actor"),
        store.qualified("audit"),
        filter.limit.max(1)
    );
    let json = |s: Option<&str>| s.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    store
        .query(&sql, &params)?
        .iter()
        .map(|r| {
            Ok(Row {
                id: r.int(0)?,
                at: r.opt_text(1)?.unwrap_or_default().to_string(),
                principal: r.text(2)?.to_string(),
                action: r.text(3)?.to_string(),
                scope: json(r.opt_text(4)?).unwrap_or(serde_json::Value::Null),
                policy: json(r.opt_text(5)?),
                job_id: r.opt_int(6)?,
                epoch: r.opt_int(7)?,
                details: json(r.opt_text(8)?),
                actor: json(r.opt_text(9)?),
            })
        })
        .collect()
}
