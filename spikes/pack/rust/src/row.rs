// SPDX-License-Identifier: AGPL-3.0-only
//! The stack a pack is evaluated against, and the two CSVs the spike reads.

/// Every field a pack may name. The engine's fingerprint will have more; these
/// are the ones v0's classifier reads, which is what the spike needs.
pub const FIELDS: &[&str] = &[
    "te",
    "tr",
    "ti",
    "flip_angle",
    "echo_train_length",
    "n_instances",
    "fov_x",
    "fov_y",
    "aspect_ratio",
    "b_value",
    "acquisition_type",
    "modality",
    "manufacturer",
    "model",
    "orientation",
    "split_reason",
    "echo_number",
    "sequence_name",
    "image_type",
    "scanning_sequence",
    "sequence_variant",
    "scan_options",
    "text_blob",
    "contrast_blob",
];

pub const F_TE: usize = 0;
pub const F_TR: usize = 1;
pub const F_TI: usize = 2;
pub const F_FLIP: usize = 3;
pub const F_N_INSTANCES: usize = 5;
pub const F_MODALITY: usize = 11;

pub fn field_index(name: &str) -> Option<usize> {
    FIELDS.iter().position(|f| *f == name)
}

/// One stack, as v0's `stack_fingerprint` holds it.
#[derive(Default, Clone)]
pub struct Fingerprint {
    pub id: i64,
    pub num: [Option<f64>; 10],
    pub text: Vec<String>,
}

impl Fingerprint {
    pub fn s(&self, i: usize) -> &str {
        self.text.get(i.wrapping_sub(10)).map_or("", |s| s.as_str())
    }
}

fn num(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok()
}

/// The header of the export, in the order `export.sh` writes it.
const FP_COLS: &[&str] = &[
    "series_stack_id",
    "modality",
    "manufacturer",
    "manufacturer_model",
    "stack_sequence_name",
    "text_search_blob",
    "contrast_search_blob",
    "stack_orientation",
    "fov_x",
    "fov_y",
    "aspect_ratio",
    "image_type",
    "scanning_sequence",
    "sequence_variant",
    "scan_options",
    "mr_te",
    "mr_tr",
    "mr_ti",
    "mr_flip_angle",
    "mr_echo_train_length",
    "mr_echo_number",
    "mr_acquisition_type",
    "mr_diffusion_b_value",
    "stack_n_instances",
];

pub fn read_fingerprints(path: &str) -> Result<Vec<Fingerprint>, String> {
    let mut rdr = csv::Reader::from_path(path).map_err(|e| format!("{path}: {e}"))?;
    let head = rdr.headers().map_err(|e| e.to_string())?.clone();
    let at = |name: &str| -> Result<usize, String> {
        head.iter()
            .position(|h| h == name)
            .ok_or_else(|| format!("{path}: no column {name}"))
    };
    let idx: Vec<usize> = FP_COLS
        .iter()
        .map(|c| at(c))
        .collect::<Result<_, _>>()?;
    let mut out = Vec::with_capacity(600_000);
    for rec in rdr.records() {
        let r = rec.map_err(|e| e.to_string())?;
        let g = |i: usize| -> &str { r.get(idx[i]).unwrap_or("") };
        let mut fp = Fingerprint {
            id: g(0).parse().unwrap_or(0),
            ..Default::default()
        };
        fp.num[F_TE] = num(g(15));
        fp.num[F_TR] = num(g(16));
        fp.num[F_TI] = num(g(17));
        fp.num[F_FLIP] = num(g(18));
        fp.num[4] = num(g(19)); // echo train length
        fp.num[F_N_INSTANCES] = num(g(23));
        fp.num[6] = num(g(8));
        fp.num[7] = num(g(9));
        fp.num[8] = num(g(10));
        fp.num[9] = num(g(22)); // b value, stored as text in v0
        // text fields, in the order FIELDS lists them from index 10
        fp.text = vec![
            g(21).to_string(), // acquisition_type
            g(1).to_string(),  // modality
            g(2).to_string(),  // manufacturer
            g(3).to_string(),  // model
            g(7).to_string(),  // orientation
            String::new(),     // split_reason: v0's fingerprint has none
            g(20).to_string(), // echo_number
            g(4).to_string(),  // sequence_name
            g(11).to_string(), // image_type
            g(12).to_string(), // scanning_sequence
            g(13).to_string(), // sequence_variant
            g(14).to_string(), // scan_options
            g(5).to_string(),  // text_blob
            g(6).to_string(),  // contrast_blob
        ];
        out.push(fp);
    }
    Ok(out)
}

/// v0's verdict for one stack: what the spike compares against, and what the
/// physics vote's reference pool is built from.
#[derive(Default, Clone)]
pub struct Verdict {
    pub id: i64,
    pub directory_type: String,
    pub base: String,
    pub technique: String,
    pub construct_csv: String,
    pub provenance: String,
}

pub fn read_verdicts(path: &str) -> Result<Vec<Verdict>, String> {
    let mut rdr = csv::Reader::from_path(path).map_err(|e| format!("{path}: {e}"))?;
    let head = rdr.headers().map_err(|e| e.to_string())?.clone();
    let at = |name: &str| -> Result<usize, String> {
        head.iter()
            .position(|h| h == name)
            .ok_or_else(|| format!("{path}: no column {name}"))
    };
    let (i_id, i_dt, i_base, i_tech, i_con, i_prov) = (
        at("series_stack_id")?,
        at("directory_type")?,
        at("base")?,
        at("technique")?,
        at("construct_csv")?,
        at("provenance")?,
    );
    let mut out = Vec::with_capacity(600_000);
    for rec in rdr.records() {
        let r = rec.map_err(|e| e.to_string())?;
        out.push(Verdict {
            id: r.get(i_id).unwrap_or("").parse().unwrap_or(0),
            directory_type: r.get(i_dt).unwrap_or("").to_string(),
            base: r.get(i_base).unwrap_or("").to_string(),
            technique: r.get(i_tech).unwrap_or("").to_string(),
            construct_csv: r.get(i_con).unwrap_or("").to_string(),
            provenance: r.get(i_prov).unwrap_or("").to_string(),
        });
    }
    Ok(out)
}
