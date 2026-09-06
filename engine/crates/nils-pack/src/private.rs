// SPDX-License-Identifier: AGPL-3.0-only

//! Which private elements the digest reads and which a release keeps
//! (`docs/specs/wave3-anonymize-and-bids.md`, §8.4;
//! `docs/specs/wave4a-engine-completes.md`, §5).
//!
//! Two lists at one grain. Reading an element into the registry and letting
//! it leave in a release are different risks, so `ingest` is broad and
//! `release` is narrow, and both address a creator and an offset rather than
//! a block's position. A private dictionary, shipped as pack data, gives an
//! element its vendor's name and its VR.
//!
//! Private elements are **dropped by default**, because a private element is
//! by definition one whose meaning the standard does not fix: some carry a
//! diffusion direction and some carry the operator's name, and nothing in the
//! file says which. The allowlist is the exception, and it is pack-shaped data
//! rather than a table in the engine, because which vendor element carries a
//! gradient is knowledge about scanners that changes without the engine
//! changing.
//!
//! v0 removes 119 named standard tags and **touches no private element at
//! all**, so every vendor block leaves the building. Siemens CSA headers alone
//! have carried the patient name, the operator and the institution in
//! shipping firmware.

/// One private element a release keeps, by the block its creator reserves.
///
/// Addressed by creator rather than by position, because the block a creator
/// reserves moves from file to file: `(0019,0010)` in one and `(0019,0011)` in
/// the next, with the elements at `10xx` and `11xx`. v0's reader takes the
/// fixed slot and reads whatever is there, which is how a value can be read
/// from the wrong vendor's block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allowed {
    pub creator: String,
    pub group: u16,
    /// The offset within the block: the low byte of the element.
    pub element: u8,
    /// What it carries, so a reader of the pack can judge the exception.
    pub why: String,
}

impl Allowed {
    /// How the row and the report name it.
    pub fn text(&self) -> String {
        format!(
            "({:04X},xx{:02X}) {}",
            self.group, self.element, self.creator
        )
    }
}

/// One private element the digest reads into the registry (Wave 4a §5.2),
/// published as a field of the pack that any rule may read by `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingest {
    pub creator: String,
    pub group: u16,
    /// The offset within the block: the low byte of the element.
    pub element: u8,
    /// The field name a rule reads it by. The pack's own vocabulary, so a
    /// rule reads `siemens_b_value` and not an address.
    pub name: String,
    /// The value representation, from the entry or the dictionary, so the
    /// bytes of an implicit VR file read as the number they are.
    pub vr: Option<String>,
    /// What the dictionary calls it, for the report.
    pub dictionary_name: Option<String>,
    /// What kind of thing it is: parameter, geometry, diffusion, timing,
    /// reconstruction. Declared, not inferred.
    pub kind: Option<String>,
    /// Why it is read: the evidence that put it on the list.
    pub why: String,
}

impl Ingest {
    /// The key the registry stores the value under, shared with the digest:
    /// `0019xx0C SIEMENS MR HEADER`.
    pub fn address(&self) -> String {
        format!(
            "{:04X}xx{:02X} {}",
            self.group,
            self.element,
            self.creator.trim()
        )
    }

    /// How the report names it.
    pub fn text(&self) -> String {
        format!(
            "({:04X},xx{:02X}) {}",
            self.group, self.element, self.creator
        )
    }
}

/// What the dictionary knows about one private element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub vr: String,
    pub vm: String,
    pub name: String,
}

/// The private dictionary (§5.3): what a vendor calls an element and what
/// VR it has, as pack data rather than as code, so `(0051,xx0C)` reads as
/// its name and values stop arriving as `UN`. Generated from a public
/// source; the pack's `PROVENANCE.md` says which and under what licence.
///
/// Keyed by the creator folded to lower case, because one corpus spells a
/// Philips creator two ways.
#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    entries: std::collections::HashMap<(String, u16, u8), Entry>,
    creators: std::collections::HashSet<String>,
}

impl Dictionary {
    /// Parse the tab-separated form: `creator`, `group` as four hex digits,
    /// `offset` as two, `vr`, `vm`, `name`; `#` starts a comment.
    pub fn parse(text: &str) -> Result<Dictionary, String> {
        let mut d = Dictionary::default();
        for (n, line) in text.lines().enumerate() {
            // Only the line ending: a trailing tab is an empty last field.
            let line = line.trim_end_matches(['\r', '\n']);
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() != 6 {
                return Err(format!(
                    "line {}: {} fields, not the six of creator, group, offset, vr, vm, name",
                    n + 1,
                    parts.len()
                ));
            }
            let group = u16::from_str_radix(parts[1], 16)
                .map_err(|_| format!("line {}: {} is not a hex group", n + 1, parts[1]))?;
            let element = u8::from_str_radix(parts[2], 16)
                .map_err(|_| format!("line {}: {} is not a hex offset", n + 1, parts[2]))?;
            d.insert(
                parts[0],
                group,
                element,
                Entry {
                    vr: parts[3].to_string(),
                    vm: parts[4].to_string(),
                    name: parts[5].to_string(),
                },
            );
        }
        Ok(d)
    }

    pub fn insert(&mut self, creator: &str, group: u16, element: u8, entry: Entry) {
        let key = creator.trim().to_lowercase();
        self.creators.insert(key.clone());
        self.entries.insert((key, group, element), entry);
    }

    /// Fold another dictionary in; a later entry for the same address wins.
    pub fn extend(&mut self, other: Dictionary) {
        for ((creator, group, element), entry) in other.entries {
            self.creators.insert(creator.clone());
            self.entries.insert((creator, group, element), entry);
        }
    }

    pub fn lookup(&self, creator: &str, group: u16, element: u8) -> Option<&Entry> {
        self.entries
            .get(&(creator.trim().to_lowercase(), group, element))
    }

    /// Whether the dictionary knows the creator at all.
    pub fn knows(&self, creator: &str) -> bool {
        self.creators.contains(&creator.trim().to_lowercase())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn creators(&self) -> usize {
        self.creators.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dictionary_names_an_element_whatever_case_the_creator_came_in() {
        // One corpus spells a Philips creator two ways; the name must not
        // depend on which way a file spelled it.
        let d = Dictionary::parse(
            "# a comment\nPhilips Imaging DD 001\t2001\t03\tFL\t1\tDiffusion B-Factor\n",
        )
        .unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(
            d.lookup("PHILIPS IMAGING DD 001", 0x2001, 0x03)
                .map(|e| e.name.as_str()),
            Some("Diffusion B-Factor")
        );
        assert!(d.knows("philips imaging dd 001 "));
        assert!(d.lookup("Philips Imaging DD 001", 0x2001, 0x04).is_none());
    }

    #[test]
    fn a_line_that_is_not_six_fields_is_refused_with_its_number() {
        let e = Dictionary::parse("A\t0019\t0C\tIS\n").unwrap_err();
        assert!(e.starts_with("line 1:"), "{e}");
        let e = Dictionary::parse("A\tzz\t0C\tIS\t1\tx\n").unwrap_err();
        assert!(e.contains("hex group"), "{e}");
    }

    #[test]
    fn an_ingested_element_is_addressed_the_way_the_digest_stores_it() {
        let i = Ingest {
            creator: "SIEMENS MR HEADER ".into(),
            group: 0x0019,
            element: 0x0C,
            name: "siemens_b_value".into(),
            vr: Some("IS".into()),
            dictionary_name: None,
            kind: None,
            why: "test".into(),
        };
        assert_eq!(i.address(), "0019xx0C SIEMENS MR HEADER");
        assert_eq!(i.text(), "(0019,xx0C) SIEMENS MR HEADER ");
    }
}
