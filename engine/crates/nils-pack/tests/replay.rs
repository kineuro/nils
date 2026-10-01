// SPDX-License-Identifier: AGPL-3.0-only

//! Record 53 S3: the MRI pack replayed over header packets. A packet that
//! carries the private elements the pack shows exercises the rules that read
//! them, as production does, and one without them does not; an element the
//! pack does not show is not read, and a value shaped like an identifier is
//! withheld; the session list of a packet exercises the session pass. Every
//! packet is invented.

use std::path::Path;

use serde_json::{Value, json};

fn mri() -> nils_pack::Pack {
    nils_pack::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packs/mri"),
        None,
    )
    .expect("the MRI pack loads")
}

/// A GE EPIMix block whose words do not say EPIMix, as 5 archive stacks were.
fn epimix(private: Value) -> Value {
    json!({
        "stack": 11,
        "header": {
            "texts": {"series_description": "Ax T2"},
            "sequence": {"image_type": "ORIGINAL\\PRIMARY\\OTHER", "scanning_sequence": "EP",
                         "mr_acquisition_type": "2D"},
            "physics": {"manufacturer": "GE MEDICAL SYSTEMS", "repetition_time": 3000,
                        "echo_time": 100, "flip_angle": 90},
            "geometry": {"orientation": "Axial", "n_slices": 30, "n_instances": 30},
            "private": private,
        },
    })
}

#[test]
fn a_packet_with_the_shown_private_elements_exercises_the_rules_that_read_them() {
    let pack = mri();
    let with = nils_pack::replay::replay(
        &pack,
        &epimix(json!({"ge_pulse_sequence_name": "ksepimix_2"})),
    )
    .unwrap();
    assert_eq!(with["values"]["provenance"], "EPIMix", "{with}");
    assert_eq!(with["private"], json!(["ge_pulse_sequence_name"]), "{with}");
    let without = nils_pack::replay::replay(&pack, &epimix(json!({}))).unwrap();
    assert_ne!(without["values"]["provenance"], "EPIMix", "{without}");

    // An ingested element the pack does not show is not read, whatever the
    // packet holds; a shown one whose value looks like an identifier is
    // withheld, and the rule that reads it does not fire.
    let hidden = nils_pack::replay::replay(
        &pack,
        &epimix(json!({"ge_peak_sar": "1.2", "ge_pulse_sequence_name": "121212-1212"})),
    )
    .unwrap();
    assert_eq!(hidden["private"], json!([]), "{hidden}");
    assert_eq!(
        hidden["withheld"],
        json!(["ge_pulse_sequence_name"]),
        "{hidden}"
    );
    assert_ne!(hidden["values"]["provenance"], "EPIMix", "{hidden}");
    assert!(!hidden.to_string().contains("121212"), "{hidden}");

    // Philips' isotropic image by its private direction.
    let philips = json!({
        "stack": 12,
        "header": {
            "texts": {"series_description": "DTI_high"},
            "sequence": {"image_type": "ORIGINAL\\PRIMARY\\DIFFUSION\\NONE", "scanning_sequence": "SE",
                         "mr_acquisition_type": "2D"},
            "physics": {"manufacturer": "Philips Medical Systems", "repetition_time": 5000,
                        "echo_time": 80, "diffusion_b_value": "1000"},
            "private": {"philips_diffusion_direction": "I", "philips_scanning_technique": "DwiSE"},
        },
    });
    let p = nils_pack::replay::replay(&pack, &philips).unwrap();
    assert!(
        p["values"]["construct"].as_str().unwrap().contains("Trace"),
        "{p}"
    );
}

/// A GE SWAN image and its session, as a packet builder writes it: each
/// other stack with the fields the session pass names and its relations.
fn swan(computed: Value) -> Value {
    let geometry = json!({"orientation": "Axial", "rows": 256, "columns": 256, "n_slices": 60});
    json!({
        "stack": 21,
        "header": {
            "texts": {"series_description": "Ax SWAN"},
            "sequence": {"image_type": "ORIGINAL\\PRIMARY\\OTHER", "scanning_sequence": "GR",
                         "mr_acquisition_type": "3D"},
            "physics": {"manufacturer": "GE MEDICAL SYSTEMS", "repetition_time": 40,
                        "echo_time": 25, "flip_angle": 15},
            "geometry": geometry.as_object().unwrap().clone().into_iter()
                .chain([("series_number".to_string(), json!(5))]).collect::<serde_json::Map<_, _>>(),
        },
        "session": [
            {"this_stack": true, "series_number": 5, "description": "Ax SWAN"},
            computed,
        ],
    })
}

#[test]
fn a_packet_s_session_list_exercises_the_session_pass() {
    let pack = mri();
    let sibling = |frame: Value| {
        json!({
            "stack": 22, "series_number": 500, "description": "SWI: Ax SWAN",
            "same_series": false, "same_frame_of_reference": frame,
            "fields": {"manufacturer": "GE MEDICAL SYSTEMS", "image_type": "ORIGINAL\\PRIMARY\\OTHER",
                       "orientation": "Axial", "rows": 256, "columns": 256, "n_slices": 60},
        })
    };
    let r = nils_pack::replay::replay(&pack, &swan(sibling(json!(true)))).unwrap();
    assert_eq!(r["values"]["construct"], "Magnitude", "{r}");
    assert_eq!(r["values"]["provenance"], "RawRecon", "{r}");
    assert_eq!(r["values"]["base"], "T2*w", "{r}");
    assert_eq!(r["tiers"]["construct"], "session", "{r}");
    assert_eq!(
        r["session"][0]["rule"], "ge_swan_beside_its_computed_output",
        "{r}"
    );
    assert_eq!(r["session"][0]["cited"], json!([22]), "{r}");
    // The disposition phase ran on what the session decided.
    assert!(r["values"]["directory_type"].is_string(), "{r}");

    // A packet that cannot say the frame of reference cannot replay it,
    // and the route's fallback stands.
    let r = nils_pack::replay::replay(&pack, &swan(sibling(Value::Null))).unwrap();
    assert_eq!(r["values"]["construct"], "SWI", "{r}");
    assert_eq!(r["tiers"]["construct"], "stated", "{r}");
    assert_eq!(r["session"], json!([]), "{r}");
}

#[test]
fn a_replay_names_the_axes_a_rule_decided_as_nothing() {
    // A phase image has no base, which a rule decides; a gradient echo
    // whose base no rule reached is only empty. A grade scores the first
    // as none and the second as a gap, so the line says which is which.
    let pack = mri();
    let packet = |image_type: &str, stack: u64| {
        json!({
            "stack": stack,
            "header": {
                "texts": {"series_description": "Ax 3D"},
                "sequence": {"image_type": image_type, "scanning_sequence": "GR",
                             "sequence_variant": "SP", "mr_acquisition_type": "3D"},
                "physics": {"manufacturer": "SIEMENS", "repetition_time": 40,
                            "echo_time": 20, "flip_angle": 15},
            },
        })
    };
    let phase = nils_pack::replay::replay(&pack, &packet("ORIGINAL\\PRIMARY\\P\\ND", 21)).unwrap();
    assert_eq!(phase["values"]["base"], "", "{phase}");
    assert!(
        phase["none"].as_array().unwrap().contains(&json!("base")),
        "{phase}"
    );
    let magnitude =
        nils_pack::replay::replay(&pack, &packet("ORIGINAL\\PRIMARY\\M\\ND", 22)).unwrap();
    assert_ne!(magnitude["values"]["base"], "", "{magnitude}");
    assert!(
        !magnitude["none"]
            .as_array()
            .unwrap()
            .contains(&json!("base")),
        "{magnitude}"
    );
}
