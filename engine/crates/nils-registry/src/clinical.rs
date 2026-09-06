// SPDX-License-Identifier: AGPL-3.0-only

//! The clinical vocabulary (`docs/specs/wave4a-engine-completes.md`, §7.1):
//! the diseases a registry may record, their types, and the kinds of
//! observation an event may be.
//!
//! Pack data rather than a table the engine seeds, because which scales a
//! clinic records is knowledge about the clinic and changes without the
//! engine changing. A load upserts by name: a name that exists is updated in
//! place, a new one is added, and nothing is ever removed by a load, because
//! an event already recorded against a kind must keep its kind.

use serde::Deserialize;

use crate::schema::{Type, table};
use crate::store::{Error, Insert, Param, Store};

/// The vocabulary, as `packs/<name>/vocabulary.yml` declares it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Vocabulary {
    #[serde(default)]
    pub diseases: Vec<Disease>,
    #[serde(default)]
    pub observation_types: Vec<ObservationType>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Disease {
    pub name: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub types: Vec<DiseaseType>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct DiseaseType {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ObservationType {
    pub name: String,
    pub category: String,
    /// `numeric`, `text`, `boolean`, `json`, or none for an observation
    /// that is a date and nothing else.
    #[serde(default)]
    pub value_type: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// The scale a release names by default (§7.4).
    #[serde(default)]
    pub primary: bool,
    /// Never released, by name or by default (§7.4).
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct File {
    vocabulary: Vocabulary,
}

/// The value types an observation kind may declare.
pub const VALUE_TYPES: &[&str] = &["numeric", "text", "boolean", "json"];

impl Vocabulary {
    /// Parse the YAML of a vocabulary file, and refuse what a load could not
    /// hold: a name twice, a value type nobody knows, a bound the wrong way
    /// round.
    pub fn parse(yaml: &str) -> Result<Vocabulary, String> {
        let file: File = serde_saphyr::from_str(yaml).map_err(|e| e.to_string())?;
        let v = file.vocabulary;
        let mut seen = std::collections::HashSet::new();
        for d in &v.diseases {
            if d.name.trim().is_empty() {
                return Err("a disease with no name".into());
            }
            if !seen.insert(d.name.to_lowercase()) {
                return Err(format!("disease {} is declared twice", d.name));
            }
            let mut types = std::collections::HashSet::new();
            for t in &d.types {
                if !types.insert(t.name.to_lowercase()) {
                    return Err(format!("{} type {} is declared twice", d.name, t.name));
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        for o in &v.observation_types {
            if o.name.trim().is_empty() {
                return Err("an observation type with no name".into());
            }
            if !seen.insert(o.name.to_lowercase()) {
                return Err(format!("observation type {} is declared twice", o.name));
            }
            if let Some(t) = &o.value_type
                && !VALUE_TYPES.contains(&t.as_str())
            {
                return Err(format!(
                    "observation type {}: {t} is not a value type; those are {}",
                    o.name,
                    VALUE_TYPES.join(", ")
                ));
            }
            if let (Some(lo), Some(hi)) = (o.min, o.max)
                && lo > hi
            {
                return Err(format!(
                    "observation type {}: min {lo} is above max {hi}",
                    o.name
                ));
            }
        }
        Ok(v)
    }
}

/// What a load did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Loaded {
    pub diseases_added: usize,
    pub diseases_updated: usize,
    pub disease_types_added: usize,
    pub disease_types_updated: usize,
    pub observation_types_added: usize,
    pub observation_types_updated: usize,
}

impl Loaded {
    pub fn changed(&self) -> usize {
        self.diseases_added
            + self.diseases_updated
            + self.disease_types_added
            + self.disease_types_updated
            + self.observation_types_added
            + self.observation_types_updated
    }
}

fn opt(v: &Option<String>) -> Param {
    match v {
        Some(s) => Param::from(s.as_str()),
        None => Param::Null,
    }
}

fn num(v: Option<f64>) -> Param {
    match v {
        Some(x) => Param::Double(x),
        None => Param::Null,
    }
}

/// Load a vocabulary: upsert by name, add what is new, update what changed,
/// remove nothing. Idempotent: a second load of the same file changes
/// nothing and says so.
pub fn load(store: &mut Store, v: &Vocabulary) -> Result<Loaded, Error> {
    let mut out = Loaded::default();
    let d = store.dialect();
    // --- diseases
    let disease_t = table("disease");
    let by_name = format!(
        "SELECT id, code, description FROM {} WHERE name = {}",
        store.qualified("disease"),
        d.param(1, Type::Text)
    );
    for disease in &v.diseases {
        let disease_id = match store.query_opt(&by_name, &[Param::from(disease.name.as_str())])? {
            Some(r) => {
                let id = r.int(0)?;
                let same = r.opt_text(1)?.map(str::to_string) == disease.code
                    && r.opt_text(2)?.map(str::to_string) == disease.description;
                if !same {
                    store.update_by_id(
                        disease_t,
                        &[
                            ("code", opt(&disease.code)),
                            ("description", opt(&disease.description)),
                        ],
                        "id",
                        id,
                    )?;
                    out.diseases_updated += 1;
                }
                id
            }
            None => {
                let rows = store.insert(
                    &Insert::new(disease_t, &["name", "code", "description"]).returning(&["id"]),
                    &[vec![
                        Param::from(disease.name.as_str()),
                        opt(&disease.code),
                        opt(&disease.description),
                    ]],
                )?;
                out.diseases_added += 1;
                rows.first().map(|r| r.int(0)).transpose()?.unwrap_or(0)
            }
        };
        let type_t = table("disease_type");
        let type_by_name = format!(
            "SELECT id, description, sort_order FROM {} WHERE disease_id = {} AND name = {}",
            store.qualified("disease_type"),
            d.param(1, Type::Int),
            d.param(2, Type::Text)
        );
        for (i, t) in disease.types.iter().enumerate() {
            let order = i as i64 + 1;
            match store.query_opt(
                &type_by_name,
                &[Param::Int(disease_id), Param::from(t.name.as_str())],
            )? {
                Some(r) => {
                    let same = r.opt_text(1)?.map(str::to_string) == t.description
                        && r.opt_int(2)? == Some(order);
                    if !same {
                        store.update_by_id(
                            type_t,
                            &[
                                ("description", opt(&t.description)),
                                ("sort_order", Param::Int(order)),
                            ],
                            "id",
                            r.int(0)?,
                        )?;
                        out.disease_types_updated += 1;
                    }
                }
                None => {
                    store.insert(
                        &Insert::new(type_t, &["disease_id", "name", "description", "sort_order"]),
                        &[vec![
                            Param::Int(disease_id),
                            Param::from(t.name.as_str()),
                            opt(&t.description),
                            Param::Int(order),
                        ]],
                    )?;
                    out.disease_types_added += 1;
                }
            }
        }
    }
    // --- observation types
    let obs_t = table("observation_type");
    let obs_by_name = format!(
        "SELECT id, category, value_type, unit, min_value, max_value, is_primary, description, \
         is_sensitive FROM {} WHERE name = {}",
        store.qualified("observation_type"),
        d.param(1, Type::Text)
    );
    let columns = [
        "category",
        "value_type",
        "unit",
        "min_value",
        "max_value",
        "is_primary",
        "description",
        "is_sensitive",
    ];
    for o in &v.observation_types {
        let values = |o: &ObservationType| -> Vec<Param> {
            vec![
                Param::from(o.category.as_str()),
                opt(&o.value_type),
                opt(&o.unit),
                num(o.min),
                num(o.max),
                Param::Int(i64::from(o.primary)),
                opt(&o.description),
                Param::Int(i64::from(o.sensitive)),
            ]
        };
        match store.query_opt(&obs_by_name, &[Param::from(o.name.as_str())])? {
            Some(r) => {
                let same = r.text(1)? == o.category
                    && r.opt_text(2)?.map(str::to_string) == o.value_type
                    && r.opt_text(3)?.map(str::to_string) == o.unit
                    && r.opt_double(4)? == o.min
                    && r.opt_double(5)? == o.max
                    && r.int(6)? == i64::from(o.primary)
                    && r.opt_text(7)?.map(str::to_string) == o.description
                    && r.opt_int(8)?.unwrap_or(0) == i64::from(o.sensitive);
                if !same {
                    let vals = values(o);
                    let set: Vec<(&str, Param)> = columns.iter().copied().zip(vals).collect();
                    store.update_by_id(obs_t, &set, "id", r.int(0)?)?;
                    out.observation_types_updated += 1;
                }
            }
            None => {
                let mut cols = vec!["name"];
                cols.extend(columns);
                let mut row = vec![Param::from(o.name.as_str())];
                row.extend(values(o));
                store.insert(&Insert::new(obs_t, &cols), &[row])?;
                out.observation_types_added += 1;
            }
        }
    }
    Ok(out)
}

/// One observation kind as the registry holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Kind {
    pub id: i64,
    pub name: String,
    pub category: String,
    pub value_type: Option<String>,
    pub unit: Option<String>,
    pub primary: bool,
    /// Never released (§7.4).
    pub sensitive: bool,
}

/// The observation kinds the registry holds, by name.
pub fn observation_types(store: &mut Store) -> Result<Vec<Kind>, Error> {
    let sql = format!(
        "SELECT id, name, category, value_type, unit, is_primary, is_sensitive FROM {} ORDER BY name",
        store.qualified("observation_type")
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[])? {
        out.push(Kind {
            id: r.int(0)?,
            name: r.text(1)?.to_string(),
            category: r.text(2)?.to_string(),
            value_type: r.opt_text(3)?.map(str::to_string),
            unit: r.opt_text(4)?.map(str::to_string),
            primary: r.int(5)? != 0,
            sensitive: r.opt_int(6)?.unwrap_or(0) != 0,
        });
    }
    Ok(out)
}

/// The diseases the registry holds, each with its types in order.
pub fn diseases(store: &mut Store) -> Result<Vec<(i64, Disease)>, Error> {
    let sql = format!(
        "SELECT id, name, code, description FROM {} ORDER BY name",
        store.qualified("disease")
    );
    let types_sql = format!(
        "SELECT name, description FROM {} WHERE disease_id = {} ORDER BY sort_order, name",
        store.qualified("disease_type"),
        store.dialect().param(1, Type::Int)
    );
    let mut out = Vec::new();
    for r in store.query(&sql, &[])? {
        let id = r.int(0)?;
        let mut d = Disease {
            name: r.text(1)?.to_string(),
            code: r.opt_text(2)?.map(str::to_string),
            description: r.opt_text(3)?.map(str::to_string),
            types: Vec::new(),
        };
        for t in store.query(&types_sql, &[Param::Int(id)])? {
            d.types.push(DiseaseType {
                name: t.text(0)?.to_string(),
                description: t.opt_text(1)?.map(str::to_string),
            });
        }
        out.push((id, d));
    }
    Ok(out)
}

/// The kind of observation named, by name, folded on case.
pub fn kind_named(store: &mut Store, name: &str) -> Result<Option<Kind>, Error> {
    Ok(observation_types(store)?
        .into_iter()
        .find(|k| k.name.eq_ignore_ascii_case(name.trim())))
}

/// Month zero per subject, by code, for a scheme anchored on a kind of
/// event (Wave 4a §7.3): the earliest event of that kind the subject has,
/// among those not superseded. A subject with none is absent, and the
/// resolver treats it as it treats an `explicit` subject with no row.
pub fn anchor_events(
    store: &mut Store,
    kind: i64,
) -> Result<std::collections::HashMap<String, crate::day::Day>, Error> {
    let d = store.dialect();
    let event_t = table("event");
    let date = d.text_of_qualified(Some("e"), event_t.column("event_date").expect("event_date"));
    let sql = format!(
        "SELECT su.code, MIN({date}) FROM {} e JOIN {} su ON su.id = e.subject_id \
         WHERE e.observation_type_id = {} AND e.superseded_by IS NULL GROUP BY su.code",
        store.qualified("event"),
        store.qualified("subject"),
        d.param(1, Type::Int)
    );
    let mut out = std::collections::HashMap::new();
    for r in store.query(&sql, &[Param::Int(kind)])? {
        if let Some(day) = r
            .opt_text(1)?
            .and_then(|t| crate::day::Day::parse(&t.replace('-', "")))
        {
            out.insert(r.text(0)?.to_string(), day);
        }
    }
    Ok(out)
}

/// The event of a kind nearest to a day (Wave 4a §7.3): the one temporal
/// function the engine needs beyond the anchor, which the release and the
/// gate's bar both use. The general windows are the question wave's.
#[derive(Debug, Clone, PartialEq)]
pub struct Nearest {
    pub event_id: i64,
    pub date: crate::day::Day,
    /// Days from the day asked about to the event: negative when the event
    /// came before it.
    pub offset_days: i64,
    pub value: Option<String>,
    pub number: Option<f64>,
}

/// The nearest event of `kind` to `day` for one subject, or none when the
/// subject has no event of that kind. **The tie rule**: of two events the
/// same distance away, the earlier one, because an observation made before
/// a scan describes the state the scan saw and one made after may describe
/// what the scan changed.
pub fn nearest(
    store: &mut Store,
    subject: i64,
    kind: i64,
    day: crate::day::Day,
) -> Result<Option<Nearest>, Error> {
    let d = store.dialect();
    let event_t = table("event");
    let date = d.text_of_qualified(Some("e"), event_t.column("event_date").expect("event_date"));
    let sql = format!(
        "SELECT e.id, {date}, e.value, e.number FROM {} e \
         WHERE e.subject_id = {} AND e.observation_type_id = {} AND e.superseded_by IS NULL",
        store.qualified("event"),
        d.param(1, Type::Int),
        d.param(2, Type::Int)
    );
    let mut best: Option<Nearest> = None;
    for r in store.query(&sql, &[Param::Int(subject), Param::Int(kind)])? {
        let Some(date) = crate::day::Day::parse(&r.text(1)?.replace('-', "")) else {
            continue;
        };
        let offset = day.days_to(date);
        let candidate = Nearest {
            event_id: r.int(0)?,
            date,
            offset_days: offset,
            value: r.opt_text(2)?.map(str::to_string),
            number: r.opt_double(3)?,
        };
        let closer = match &best {
            None => true,
            Some(b) => {
                offset.abs() < b.offset_days.abs()
                    || (offset.abs() == b.offset_days.abs() && date < b.date)
            }
        };
        if closer {
            best = Some(candidate);
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vocabulary_parses_and_refuses_what_a_load_could_not_hold() {
        let v = Vocabulary::parse(
            "vocabulary:\n  diseases:\n    - name: MS\n      code: G35\n      types:\n        - {name: RRMS}\n  observation_types:\n    - {name: EDSS, category: scale, value_type: numeric, unit: points, min: 0, max: 10, primary: true}\n    - {name: Diagnosis, category: assessment}\n",
        )
        .unwrap();
        assert_eq!(v.diseases.len(), 1);
        assert_eq!(v.diseases[0].types[0].name, "RRMS");
        assert_eq!(v.observation_types.len(), 2);
        assert!(v.observation_types[0].primary);
        assert_eq!(v.observation_types[1].value_type, None);

        let twice = "vocabulary:\n  diseases: [{name: MS}, {name: ms}]\n";
        assert!(
            Vocabulary::parse(twice)
                .unwrap_err()
                .contains("declared twice")
        );
        let bad = "vocabulary:\n  observation_types: [{name: X, category: c, value_type: date}]\n";
        assert!(
            Vocabulary::parse(bad)
                .unwrap_err()
                .contains("not a value type")
        );
        let bounds = "vocabulary:\n  observation_types: [{name: X, category: c, value_type: numeric, min: 5, max: 1}]\n";
        assert!(Vocabulary::parse(bounds).unwrap_err().contains("above max"));
    }
}
