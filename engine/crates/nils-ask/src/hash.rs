// SPDX-License-Identifier: AGPL-3.0-only

//! The content hash of an ask (§4.4, rule 13): BLAKE2b over the desugared
//! core, canonical JSON, with parameters unbound and options sorted. The
//! declarations of scalar parameters are part of the core (their names and
//! types are what the question means); their values are not.

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use serde_json::Value;

use crate::ast::{Ask, canonical_json};

/// The registry's reading of dates (Wave 5 section 12.6): the timezone the
/// engine read them under and the day a week starts on. Both are the
/// registry's, never the browser's, and both are part of the core, so one
/// document under two timezones is two questions. The defaults are left
/// out of the core, so a registry at UTC and Monday hashes as it always
/// has.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Locale {
    pub timezone: String,
    pub week_start: String,
}

impl Default for Locale {
    fn default() -> Locale {
        Locale {
            timezone: "UTC".to_string(),
            week_start: "monday".to_string(),
        }
    }
}

impl Locale {
    pub const WEEK_STARTS: [&'static str; 3] = ["monday", "sunday", "saturday"];

    /// A timezone is `UTC` or an IANA name, `Area/City`; a week starts on
    /// one of three days.
    pub fn check(&self) -> Result<(), String> {
        let tz = &self.timezone;
        let iana = tz.split('/').count() >= 2
            && tz
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '+' | '-'));
        if tz != "UTC" && !iana {
            return Err(format!(
                "{tz} is not a timezone; UTC or an IANA name like Europe/Stockholm"
            ));
        }
        if !Self::WEEK_STARTS.contains(&self.week_start.as_str()) {
            return Err(format!(
                "{} is not a week start; one of {}",
                self.week_start,
                Self::WEEK_STARTS.join(", ")
            ));
        }
        Ok(())
    }
}

/// The core of a desugared ask, as a JSON value with parameter values
/// removed and every object's keys sorted.
pub fn core(ask: &Ask) -> Value {
    core_under(ask, &Locale::default())
}

/// The core under a registry's locale: the timezone and the week start
/// join it when they are not the defaults.
pub fn core_under(ask: &Ask, locale: &Locale) -> Value {
    let mut v = serde_json::to_value(ask).expect("an ask serializes");
    if let Some(Value::Object(params)) = v.get_mut("params") {
        for (_, decl) in params.iter_mut() {
            if let Value::Object(d) = decl {
                d.remove("value");
                d.remove("description");
            }
        }
    }
    if let Value::Object(top) = &mut v {
        top.remove("name");
        let default = Locale::default();
        if locale.timezone != default.timezone {
            top.insert("timezone".into(), Value::String(locale.timezone.clone()));
        }
        if locale.week_start != default.week_start {
            top.insert(
                "week_start".into(),
                Value::String(locale.week_start.clone()),
            );
        }
    }
    v
}

/// The hash, hex, 64 characters, under the default locale.
pub fn content_hash(ask: &Ask) -> String {
    content_hash_under(ask, &Locale::default())
}

/// The hash under a registry's locale.
pub fn content_hash_under(ask: &Ask, locale: &Locale) -> String {
    let text = canonical_json(&core_under(ask, locale));
    let mut hasher = Blake2b::<U32>::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}
