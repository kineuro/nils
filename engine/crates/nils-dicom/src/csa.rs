// SPDX-License-Identifier: AGPL-3.0-only

//! The Siemens CSA image header (0029,1010), SV10 variant only, read for one
//! value: `PhaseEncodingDirectionPositive`, v0's `dwi_siemens_pe_dir_positive`
//! (`docs/specs/wave1-parse-and-digest.md`, §6.2). The layout is v0's parser,
//! element for element: the magic, a tag count, then per tag a 64-byte name
//! ended by the first NUL, three unused words, an item count and one unused
//! word, and per item its length, three unused words and the value padded to
//! four bytes.

/// The first value of the named CSA tag, as text, when the header is SV10 and
/// carries the tag with a value.
pub fn first_value(data: &[u8], name: &str) -> Option<String> {
    if data.len() < 16 || &data[..4] != b"SV10" {
        return None;
    }
    let n_tags = u32_at(data, 8)?;
    let mut pos = 16usize;
    for _ in 0..n_tags {
        if pos + 84 > data.len() {
            return None;
        }
        let name_bytes = &data[pos..pos + 64];
        let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(64);
        let tag_name = &name_bytes[..end];
        pos += 64 + 12;
        let n_items = u32_at(data, pos)?;
        pos += 8;
        let mut first: Option<String> = None;
        for _ in 0..n_items {
            if pos + 16 > data.len() {
                return None;
            }
            let length = u32_at(data, pos)? as usize;
            pos += 16;
            if length > 0 {
                let aligned = (length + 3) & !3;
                let end = pos.checked_add(length)?.min(data.len());
                if first.is_none() {
                    let raw = &data[pos..end];
                    let trimmed_nul = raw.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
                    let text: String = raw[..trimmed_nul].iter().map(|&b| b as char).collect();
                    first = Some(text.trim().to_string());
                }
                pos = pos.checked_add(aligned)?;
            }
        }
        if tag_name == name.as_bytes() {
            return first.filter(|s| !s.is_empty());
        }
    }
    None
}

fn u32_at(data: &[u8], pos: usize) -> Option<u32> {
    let bytes: [u8; 4] = data.get(pos..pos + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

/// Build an SV10 header for tests: each entry is a name and its values.
pub fn build_sv10(entries: &[(&str, &[&str])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"SV10");
    out.extend_from_slice(&[4, 3, 2, 1]);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&77u32.to_le_bytes());
    for (name, values) in entries {
        let mut name_bytes = [0u8; 64];
        let n = name.len().min(63);
        name_bytes[..n].copy_from_slice(&name.as_bytes()[..n]);
        out.extend_from_slice(&name_bytes);
        out.extend_from_slice(&[0u8; 12]);
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
        for v in *values {
            let len = v.len() as u32;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&[0u8; 12]);
            out.extend_from_slice(v.as_bytes());
            let pad = ((v.len() + 3) & !3) - v.len();
            out.extend(std::iter::repeat_n(0u8, pad));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_first_value_of_a_named_tag() {
        let data = build_sv10(&[
            ("EchoLinePosition", &["64"]),
            ("PhaseEncodingDirectionPositive", &["1", "0"]),
            ("Empty", &[]),
        ]);
        assert_eq!(
            first_value(&data, "PhaseEncodingDirectionPositive").as_deref(),
            Some("1")
        );
        assert_eq!(
            first_value(&data, "EchoLinePosition").as_deref(),
            Some("64")
        );
        assert_eq!(first_value(&data, "Empty"), None);
        assert_eq!(first_value(&data, "Missing"), None);
    }

    #[test]
    fn refuses_other_formats_and_short_data() {
        assert_eq!(first_value(b"SV10", "x"), None);
        assert_eq!(first_value(b"", "x"), None);
        let mut data = build_sv10(&[("PhaseEncodingDirectionPositive", &["1"])]);
        data[..4].copy_from_slice(b"SV20");
        assert_eq!(first_value(&data, "PhaseEncodingDirectionPositive"), None);
        // cut inside the item header: the value and its 16-byte header are gone
        let data = build_sv10(&[("PhaseEncodingDirectionPositive", &["1"])]);
        assert_eq!(
            first_value(&data[..data.len() - 20], "PhaseEncodingDirectionPositive"),
            None
        );
    }
}
