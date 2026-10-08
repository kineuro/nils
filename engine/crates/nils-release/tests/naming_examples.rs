// SPDX-License-Identifier: AGPL-3.0-only

//! Naming examples for record 55 C4: what the release code names a set of
//! typical stacks in the three styles (BIDS full, BIDS minimal, descriptive)
//! against the MRI pack itself, and where each goes. Prints a table; run with
//! `cargo test -p nils-release --test naming_examples -- --ignored --nocapture`.

use std::collections::BTreeMap;

use nils_release::bids::name::{Facts, Name, build};
use nils_release::bids::place::{Options, Route, route};
use nils_release::bids::repeat::Acquisition;
use nils_release::bids::separate::{Member, separator};
use nils_release::name::{Fields, Named, Naming, describe, disambiguate_by};

struct S {
    id: &'static str,
    what: &'static str,
    dir: &'static str,
    disposition: &'static str,
    body: &'static str,
    orient: &'static str,
    acq: &'static str,
    base: Option<&'static str>,
    technique: Option<&'static str>,
    modifiers: &'static [&'static str],
    constructs: &'static [&'static str],
    provenance: Option<&'static str>,
    contrast: bool,
    echo: Option<i64>,
    b: Option<f64>,
    dirs: Option<i64>,
    thickness: Option<f64>,
}

const D: S = S {
    id: "",
    what: "",
    dir: "anat",
    disposition: "acquisition",
    body: "brain",
    orient: "Axial",
    acq: "2D",
    base: None,
    technique: None,
    modifiers: &[],
    constructs: &["ND"],
    provenance: None,
    contrast: false,
    echo: None,
    b: None,
    dirs: None,
    thickness: None,
};

#[rustfmt::skip]
fn stacks() -> Vec<S> {
    vec![
        S { id: "1", what: "Ax 2D FLAIR IR-TSE brain", base: Some("T2w"), technique: Some("IR-TSE"), modifiers: &["FLAIR"], ..D },
        S { id: "2", what: "Sag 3D T1w MPRAGE brain, post-contrast", orient: "Sagittal", acq: "3D", base: Some("T1w"), technique: Some("MPRAGE"), contrast: true, ..D },
        S { id: "3", what: "Sag 2D T2w TSE spine", body: "spine", orient: "Sagittal", base: Some("T2w"), technique: Some("TSE"), ..D },
        S { id: "4a", what: "SyMRI synthetic T1w", disposition: "scanner_derived", base: Some("T1w"), technique: Some("MDME"), constructs: &["SyntheticT1w"], provenance: Some("SyMRI"), ..D },
        S { id: "4b", what: "SyMRI synthetic FLAIR", disposition: "scanner_derived", base: Some("T2w"), technique: Some("MDME"), modifiers: &["FLAIR"], constructs: &["SyntheticFLAIR"], provenance: Some("SyMRI"), ..D },
        S { id: "4c", what: "SyMRI T1map", disposition: "scanner_derived", technique: Some("MDME"), constructs: &["T1map"], provenance: Some("SyMRI"), ..D },
        S { id: "4d", what: "SyMRI PDmap", disposition: "scanner_derived", technique: Some("MDME"), constructs: &["PDmap"], provenance: Some("SyMRI"), ..D },
        S { id: "5a", what: "EPIMix T1w", base: Some("T1w"), technique: Some("MS-EPI"), provenance: Some("EPIMix"), ..D },
        S { id: "5b", what: "EPIMix T2w", base: Some("T2w"), technique: Some("MS-EPI"), provenance: Some("EPIMix"), ..D },
        S { id: "5c", what: "EPIMix FLAIR", base: Some("T2w"), technique: Some("MS-EPI"), modifiers: &["FLAIR"], provenance: Some("EPIMix"), ..D },
        S { id: "5d", what: "EPIMix DWI", dir: "dwi", base: Some("DWI"), technique: Some("MS-EPI"), provenance: Some("EPIMix"), b: Some(1000.0), ..D },
        S { id: "6a", what: "SWI magnitude", acq: "3D", base: Some("T2starw"), technique: Some("GRE"), constructs: &["Magnitude"], provenance: Some("SWIRecon"), ..D },
        S { id: "6b", what: "SWI phase", acq: "3D", base: Some("T2starw"), technique: Some("GRE"), constructs: &["Phase"], provenance: Some("SWIRecon"), ..D },
        S { id: "6c", what: "SWI processed", disposition: "scanner_derived", acq: "3D", base: Some("SWI"), technique: Some("GRE"), constructs: &["SWI"], provenance: Some("SWIRecon"), ..D },
        S { id: "6d", what: "SWI MinIP", disposition: "reformat", acq: "3D", base: Some("SWI"), technique: Some("GRE"), constructs: &["MinIP"], provenance: Some("SWIRecon"), ..D },
        S { id: "7a", what: "DTI DWI b1000 30 dir", dir: "dwi", base: Some("DWI"), technique: Some("DWI-EPI"), b: Some(1000.0), dirs: Some(30), ..D },
        S { id: "7b", what: "DTI ADC", dir: "dwi", disposition: "scanner_derived", base: Some("DWI"), technique: Some("DWI-EPI"), constructs: &["ADC"], provenance: Some("DTIRecon"), b: Some(1000.0), ..D },
        S { id: "7c", what: "DTI FA", dir: "dwi", disposition: "scanner_derived", base: Some("DWI"), technique: Some("DWI-EPI"), constructs: &["FA"], provenance: Some("DTIRecon"), b: Some(1000.0), ..D },
        S { id: "7d", what: "DTI trace", dir: "dwi", disposition: "scanner_derived", base: Some("DWI"), technique: Some("DWI-EPI"), constructs: &["Trace"], provenance: Some("DTIRecon"), b: Some(1000.0), ..D },
        S { id: "8a", what: "MP2RAGE INV1", orient: "Sagittal", acq: "3D", technique: Some("MP2RAGE"), constructs: &["INV1"], ..D },
        S { id: "8b", what: "MP2RAGE INV2", orient: "Sagittal", acq: "3D", technique: Some("MP2RAGE"), constructs: &["INV2"], ..D },
        S { id: "8c", what: "MP2RAGE UNIT1", orient: "Sagittal", acq: "3D", disposition: "scanner_derived", base: Some("T1w"), technique: Some("MP2RAGE"), constructs: &["Uniform"], ..D },
        S { id: "9a", what: "TOF-MRA", acq: "3D", base: Some("T1w"), technique: Some("TOF-MRA"), ..D },
        S { id: "9b", what: "TOF-MRA MIP", disposition: "reformat", acq: "3D", base: Some("T1w"), technique: Some("TOF-MRA"), constructs: &["MIP"], ..D },
        S { id: "10a", what: "ME-GRE echo 1", base: Some("T2starw"), technique: Some("ME-GRE"), echo: Some(1), ..D },
        S { id: "10b", what: "ME-GRE echo 2", base: Some("T2starw"), technique: Some("ME-GRE"), echo: Some(2), ..D },
        S { id: "11a", what: "Dixon water", acq: "3D", base: Some("T1w"), technique: Some("GRE"), modifiers: &["Dixon"], constructs: &["Water"], ..D },
        S { id: "11b", what: "Dixon fat", acq: "3D", base: Some("T1w"), technique: Some("GRE"), modifiers: &["Dixon"], constructs: &["Fat"], ..D },
        S { id: "12", what: "ASL CBF", dir: "perf", disposition: "scanner_derived", base: Some("PWI"), technique: Some("ASL"), constructs: &["CBF"], provenance: Some("ASLRecon"), ..D },
        S { id: "13", what: "DSC CBV", dir: "perf", disposition: "scanner_derived", base: Some("PWI"), technique: Some("Perfusion-EPI"), constructs: &["CBV"], provenance: Some("PerfusionRecon"), ..D },
        S { id: "14a", what: "FLAIR 3 mm", base: Some("T2w"), technique: Some("IR-TSE"), modifiers: &["FLAIR"], thickness: Some(3.0), ..D },
        S { id: "14b", what: "FLAIR 5 mm", base: Some("T2w"), technique: Some("IR-TSE"), modifiers: &["FLAIR"], thickness: Some(5.0), ..D },
        S { id: "15", what: "localizer", dir: "localizer", disposition: "scout", base: Some("T1w"), technique: Some("GRE"), ..D },
    ]
}

fn axes(s: &S) -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut a: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    a.insert("body_part", vec![s.body]);
    a.insert("orientation", vec![s.orient]);
    a.insert("acquisition_type", vec![s.acq]);
    if let Some(b) = s.base {
        a.insert("base", vec![b]);
    }
    if let Some(t) = s.technique {
        a.insert("technique", vec![t]);
    }
    if !s.modifiers.is_empty() {
        a.insert("modifier", s.modifiers.to_vec());
    }
    if !s.constructs.is_empty() {
        a.insert("construct", s.constructs.to_vec());
    }
    if let Some(p) = s.provenance {
        a.insert("provenance", vec![p]);
    }
    if s.contrast {
        a.insert("post_contrast", vec!["given"]);
    }
    a
}

#[test]
#[ignore]
fn naming_examples() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri");
    let pack = nils_pack::load(&dir, None).expect("the MRI pack loads");
    let all = stacks();
    let facts = |s: &S| Facts {
        intent: Some(s.dir),
        constructs: s.constructs.to_vec(),
        technique: s.technique,
        modifiers: s.modifiers.to_vec(),
        base: s.base,
        provenance: s.provenance,
        post_contrast: s.contrast,
        echo: s.echo,
        axes: axes(s),
        ..Facts::default()
    };
    let acqs: Vec<Acquisition> = all
        .iter()
        .map(|s| Acquisition {
            slice_thickness: s.thickness,
            ..Acquisition::default()
        })
        .collect();
    let i14: Vec<usize> = (0..all.len())
        .filter(|i| all[*i].id.starts_with("14"))
        .collect();
    let styled = |naming: Naming| -> Vec<Result<Name, String>> {
        let mut out: Vec<Result<Name, String>> = all
            .iter()
            .map(|s| build(&facts(s), &pack.bids, naming).map_err(|e| e.to_string()))
            .collect();
        // The two FLAIRs that differ only in thickness: the release's own rule.
        let members: Vec<Member> = i14
            .iter()
            .map(|i| Member {
                acquisition: Some(&acqs[*i]),
                said: None,
            })
            .collect();
        let marks = separator(&members, &pack.bids).expect("thickness separates");
        for (i, m) in i14.iter().zip(&marks) {
            out[*i] = Ok(out[*i].as_ref().unwrap().marked(m.as_ref().unwrap()));
        }
        out
    };
    let full = styled(Naming::Full);
    let minimal = styled(Naming::Minimal);

    let labels: Vec<[String; 3]> = all
        .iter()
        .map(|s| {
            [
                s.base
                    .map(|b| {
                        if b == "T2starw" {
                            "T2*w".into()
                        } else {
                            b.to_string()
                        }
                    })
                    .unwrap_or_default(),
                s.modifiers.join(","),
                s.constructs.join(","),
            ]
        })
        .collect();
    let named: Vec<Named> = all
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let f = Fields {
                body_part: Some(s.body),
                spinal_cord: s.body == "spine",
                orientation: Some(s.orient),
                base: Some(labels[i][0].as_str()).filter(|v| !v.is_empty()),
                acquisition_type: Some(s.acq),
                modifier: Some(labels[i][1].as_str()).filter(|v| !v.is_empty()),
                technique: s.technique,
                acceleration: None,
                construct: Some(labels[i][2].as_str()).filter(|v| !v.is_empty()),
                post_contrast: s.contrast,
                datatype: Some(s.dir),
                dwi_b_value: s.b,
                dwi_pe_direction: None,
                dwi_directions: s.dirs,
            };
            Named {
                stack: i as i64,
                name: describe(&f, true, true),
                folder: s.dir.to_string(),
                echo: s.echo,
                inversion_time: None,
                series: i as i64,
                siblings: if s.echo.is_some() { 2 } else { 1 },
                split: s.echo.map(|_| "multi_echo".to_string()),
                index: s.echo.unwrap_or(0),
            }
        })
        .collect();
    // Two sessions: the thickness pair in its own, as it would be.
    let (mut one, mut two): (Vec<Named>, Vec<Named>) = named
        .into_iter()
        .partition(|n| !all[n.stack as usize].id.starts_with("14"));
    for bucket in [&mut one, &mut two] {
        disambiguate_by(bucket, |stacks| {
            let m: Vec<Member> = stacks
                .iter()
                .map(|s| Member {
                    acquisition: Some(&acqs[*s as usize]),
                    said: None,
                })
                .collect();
            separator(&m, &pack.bids).map(|v| v.into_iter().map(|m| m.map(|m| m.token)).collect())
        });
    }
    let mut named: Vec<Named> = one.into_iter().chain(two).collect();
    named.sort_by_key(|n| n.stack);

    let place = |r: &Route, n: &Result<Name, String>, s: &S, desc: &str, d: &str| -> String {
        match (r, n) {
            (Route::Raw, Ok(n)) => format!("{}/{}", n.datatype, n.stem("01", "01")),
            (Route::Derivatives, Ok(n)) => {
                format!(
                    "derivatives/nils/{}/{}",
                    n.datatype,
                    n.stem_with_desc("01", "01", desc)
                )
            }
            (Route::Derivatives, Err(_)) => format!(
                "derivatives/nils/{}/sub-01_ses-01_desc-{}_{}",
                s.dir,
                d.chars()
                    .filter(char::is_ascii_alphanumeric)
                    .collect::<String>(),
                s.base.unwrap_or("")
            ),
            (Route::SourceData, _) => "sourcedata/ (DICOM, descriptive name)".to_string(),
            (r, n) => format!("{} {:?}", r.name(), n.as_ref().err()),
        }
    };
    println!("id\twhat\troute\tfull\tminimal\tdescriptive");
    for (i, s) in all.iter().enumerate() {
        let synthetic = pack.bids.is_synthetic(s.provenance, s.constructs);
        let derived_by = pack.bids.derived_by(s.constructs);
        let desc = derived_by
            .or_else(|| {
                s.constructs
                    .iter()
                    .find(|c| pack.bids.synthetic_construct.iter().any(|x| x == *c))
                    .copied()
            })
            .or_else(|| s.constructs.iter().find(|c| **c != "ND").copied())
            .unwrap_or("");
        let named_ok = full[i]
            .clone()
            .map_err(|e| nils_release::bids::name::Why::NotInSchema(e, String::new()));
        let r = route(
            Some(s.disposition),
            synthetic,
            derived_by.is_some(),
            &named_ok,
            Options::default(),
        );
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            s.id,
            s.what,
            r.name(),
            place(&r, &full[i], s, desc, &named[i].name),
            place(&r, &minimal[i], s, desc, &named[i].name),
            named[i].name
        );
    }
}
