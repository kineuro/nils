// SPDX-License-Identifier: AGPL-3.0-only

//! The acquisition family the pack's routes recognised in a stack
//! (2026-10-14): SyMRI's, an SWI's, a mix sequence's or STAGE's. A route is
//! a rule set with an entry condition (spec section 6.5), and every axis its
//! rules decide cites it on the stack's evidence, so the family is read from
//! what the registry holds and nothing is classified again. The scans doors
//! carry it as `acquisition_family`, and a release spells it in the names of
//! an acquisition's own images and writes it into every sidecar (record 55,
//! ruling 23: "on SWI we need the SWI in the name too").

use std::collections::HashMap;

use nils_registry::store::{Error as StoreError, Store};

/// The pack's routes whose acquisitions are grouped as a family, by the
/// name of the rule set, and the family each makes: SyMRI's, SWI's, the mix
/// sequences' (EPIMix and NeuroMix, one route) and STAGE's.
pub const ROUTE_FAMILIES: [(&str, &str); 4] = [
    ("symri", "symri"),
    ("swi", "swi"),
    ("epimix", "mix"),
    ("stage", "stage"),
];

/// The provenance each route enters on, which its products keep and which
/// a name spells as `rec-`.
const ENTERED_ON: [(&str, &str); 5] = [
    ("SyMRI", "symri"),
    ("EPIMix", "mix"),
    ("NeuroMix", "mix"),
    ("STAGE", "stage"),
    ("SWIRecon", "swi"),
];

/// The acquisition family of a stack: the family of the route whose rules
/// decided one of its axes (`routes`, the rule sets its evidence cites), so
/// an acquisition's own images are in it (SyMRI's acquisition, an SWI's
/// magnitude and phase, which the route calls RawRecon, and its minIP,
/// which it calls ProjectionDerived); else of the route its provenance
/// enters (a route that entered and had nothing left to decide, as on a
/// display composite, is cited by no evidence); else none.
pub fn acquisition_family(routes: &[String], provenance: Option<&str>) -> Option<&'static str> {
    if let Some((_, family)) = ROUTE_FAMILIES
        .iter()
        .find(|(set, _)| routes.iter().any(|r| r == set))
    {
        return Some(family);
    }
    ENTERED_ON
        .iter()
        .find(|(p, _)| provenance == Some(*p))
        .map(|(_, family)| *family)
}

/// The word a descriptive name spells a family with.
pub fn word(family: &str) -> &'static str {
    match family {
        "symri" => "SyMRI",
        "swi" => "SWI",
        "mix" => "Mix",
        "stage" => "STAGE",
        _ => "",
    }
}

/// The family a stack's names spell (record 55, ruling 23): its acquisition
/// family, unless the name says it already: a product of the family's route,
/// whose provenance `rec-` spells (`rec-SyMRI`, `rec-SWIRecon`, `rec-STAGE`,
/// `rec-EPIMix`); a stack in a folder of its own (`anat/SyMRI/`); or one a
/// word of whose name is the family's (an SWI's minIP, base SWI; its
/// processed image, construct SWI).
pub fn spelled<'a>(
    family: Option<&'a str>,
    provenance: Option<&str>,
    own_folder: Option<&str>,
    words: &[&str],
) -> Option<&'a str> {
    let family = family?;
    if own_folder.is_some()
        || ENTERED_ON
            .iter()
            .any(|(p, f)| *f == family && provenance == Some(*p))
    {
        return None;
    }
    let word = word(family);
    if words
        .iter()
        .flat_map(|w| w.split(','))
        .any(|w| w.trim().eq_ignore_ascii_case(word))
    {
        return None;
    }
    Some(family)
}

/// The routes of [`ROUTE_FAMILIES`] that decided an axis of each of
/// `stacks` (every stack of the registry where `stacks` is none), as their
/// evidence cites them.
pub fn routes_of(
    store: &mut Store,
    stacks: Option<&[i64]>,
) -> Result<HashMap<i64, Vec<String>>, StoreError> {
    let named = ROUTE_FAMILIES
        .iter()
        .map(|(set, _)| format!("'{set}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let table = store.qualified("classification_evidence");
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    let mut read = |store: &mut Store, within: String| -> Result<(), StoreError> {
        for r in store.query(
            &format!(
                "SELECT DISTINCT stack_id, rule_set FROM {table} WHERE rule_set IN ({named}){within}"
            ),
            &[],
        )? {
            out.entry(r.int(0)?)
                .or_default()
                .push(r.text(1)?.to_string());
        }
        Ok(())
    };
    match stacks {
        None => read(store, String::new())?,
        Some(stacks) => {
            for chunk in stacks.chunks(500) {
                let list = chunk
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                if !list.is_empty() {
                    read(store, format!(" AND stack_id IN ({list})"))?;
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routes(sets: &[&str]) -> Vec<String> {
        sets.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_family_is_the_route_that_decided_else_the_one_its_provenance_enters() {
        assert_eq!(
            acquisition_family(&routes(&["swi", "body_part"]), Some("RawRecon")),
            Some("swi")
        );
        assert_eq!(
            acquisition_family(&routes(&["epimix"]), Some("EPIMix")),
            Some("mix")
        );
        assert_eq!(
            acquisition_family(&routes(&["display"]), Some("SyMRI")),
            Some("symri")
        );
        assert_eq!(
            acquisition_family(&routes(&[]), Some("NeuroMix")),
            Some("mix")
        );
        assert_eq!(
            acquisition_family(&routes(&["base"]), Some("RawRecon")),
            None
        );
    }

    #[test]
    fn a_name_spells_the_family_where_nothing_in_it_says_it_already() {
        // an SWI's magnitude and phase: RawRecon, nothing says SWI
        assert_eq!(
            spelled(
                Some("swi"),
                Some("RawRecon"),
                None,
                &["T2*w", "GRE", "Magnitude"]
            ),
            Some("swi")
        );
        assert_eq!(
            spelled(Some("swi"), Some("RawRecon"), None, &["Phase"]),
            Some("swi")
        );
        // its processed image, `rec-SWIRecon`, and its minIP, base SWI
        assert_eq!(spelled(Some("swi"), Some("SWIRecon"), None, &["SWI"]), None);
        assert_eq!(
            spelled(
                Some("swi"),
                Some("ProjectionDerived"),
                None,
                &["SWI", "MinIP"]
            ),
            None
        );
        // SyMRI's acquisition is in SyMRI's folder, its products `rec-SyMRI`
        assert_eq!(
            spelled(Some("symri"), Some("RawRecon"), Some("SyMRI"), &["MDME"]),
            None
        );
        assert_eq!(
            spelled(Some("symri"), Some("SyMRI"), Some("SyMRI"), &["T1w"]),
            None
        );
        // what no route recognised spells nothing
        assert_eq!(spelled(None, Some("RawRecon"), None, &["T1w"]), None);
        assert_eq!(word("swi"), "SWI");
        assert_eq!(word("mix"), "Mix");
    }
}
