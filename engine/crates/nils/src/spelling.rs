// SPDX-License-Identifier: AGPL-3.0-only

//! One spelling of a value in a row, on the engine's side.
//!
//! The pack's resolver ([`nils_pack::rules::Axis::value_named`]) reads any
//! name of a value as the value, and says what a row stores for it. The
//! registry holds no pack, so it is handed the resolver's answers as data
//! ([`nils_registry::spelling::Spellings`]). This is where the two meet:
//! the pack this engine serves, its spellings, and a registry given them
//! when it is opened, so that every decision written through it is written
//! in the stored form and every row read through it is read as the value
//! it names.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use nils_registry::spelling::Spellings;
use nils_registry::{Home, Registry};

/// The pack a serving engine was started with: its directory and its name.
/// Set once by `nils serve`; a command at the keyboard finds the pack
/// where every command does.
static SERVED: OnceLock<(Option<PathBuf>, String)> = OnceLock::new();

/// The pack whose rules sort the registry where no command names another.
const PACK: &str = "mri";

/// Say which pack this process serves.
pub(crate) fn serve_from(dir: Option<PathBuf>, name: &str) {
    let _ = SERVED.set((dir, name.to_string()));
}

/// The pack in force for a registry: the served one, else the one the
/// commands find. None where no pack is installed.
pub(crate) fn pack(home: &Home) -> Option<Arc<nils_pack::Pack>> {
    match SERVED.get() {
        Some((dir, name)) => crate::reader::served_pack(dir.as_deref(), name),
        None => {
            let dir = crate::pack_dir(home, None).ok()?;
            crate::reader::served_pack(Some(&dir), PACK)
        }
    }
}

/// A pack's spellings as the registry takes them.
pub(crate) fn of(pack: &nils_pack::Pack) -> Spellings {
    Spellings::new(pack.names())
}

/// A pack's coarse forms as the registry counts the decisions that do not
/// agree across them ([`nils_registry::coarse`]): each coarse axis with the
/// axis it is the coarse form of and, by each of its values, the values of
/// that axis it does not cover, all as a row stores them.
pub(crate) fn coarse_forms(pack: &nils_pack::Pack) -> Vec<nils_registry::coarse::Form> {
    nils_pack::coarse::forms(pack)
        .iter()
        .map(|f| nils_registry::coarse::Form {
            coarse: f.coarse.name.clone(),
            fine: f.fine.name.clone(),
            apart: f.apart(),
        })
        .collect()
}

/// The spellings of the pack in force for a registry; none where no pack
/// is installed, and a text is then read and written as it is.
pub(crate) fn of_home(home: &Home) -> Spellings {
    pack(home).map(|p| of(&p)).unwrap_or_default()
}

/// Give a registry the spellings of the pack in force, asked for when
/// they are first needed: loading a pack is not what most commands that
/// open a registry are for.
pub(crate) fn attach(registry: &mut Registry) {
    let home = registry.home().clone();
    registry.spell_from(move || Some(of_home(&home)));
}

/// [`attach`] on a registry just opened.
pub(crate) fn attached(mut registry: Registry) -> Registry {
    attach(&mut registry);
    registry
}
