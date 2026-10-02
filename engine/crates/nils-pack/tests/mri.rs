// SPDX-License-Identifier: AGPL-3.0-only

//! The MRI pack in the repository loads, and its own corpus is what says so.

use std::path::PathBuf;

fn packs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packs")
}

#[test]
fn the_mri_pack_loads_and_its_corpus_holds() {
    let pack = match nils_pack::load(&packs().join("mri"), None) {
        Ok(p) => p,
        Err(e) => panic!("the MRI pack does not load:\n{e}"),
    };
    assert_eq!(pack.name, "mri");
    assert_eq!(pack.id(), "mri@0.24.0");
    assert_eq!(pack.modality, "MR");
    assert_eq!(
        pack.parsers.len(),
        6,
        "v0 has five parsers, and MRI pack 0.21.0 adds the Philips per-frame \
         ImageType"
    );
    assert_eq!(
        pack.parsers.iter().map(|p| p.preds.len()).sum::<usize>(),
        240,
        "v0's 220 predicates, all of them, the two record 37 added, the \
         five of pack 0.10.0: the time reversed steady state and the anchored \
         Siemens stems, the two of pack 0.11.0: GE's MT_GEMS and Siemens' \
         dynamic FLASH, the one of pack 0.12.0: Philips' projection image \
         without GE's collapse, the one of pack 0.13.0: a workstation's \
         thick-slab average, the one of pack 0.15.0: a Siemens BLADE \
         diffusion, the one of pack 0.16.0: a Siemens BLADE turbo spin \
         echo by its stem, the two of pack 0.18.0: GE's SCOUT and \
         Siemens' standard deviation projections, and the four of pack \
         0.21.0: the Dixon parts W, F, IP and OP of the Philips per-frame \
         ImageType, and the one of pack 0.24.0: a Siemens BLADE \
         inversion-recovery TSE by its stem"
    );
    assert_eq!(
        pack.flags.len(),
        477,
        "v0's 138 flags and the seven helpers it keeps as context methods: \
         record 37 removed four that said the Dixon part twice and added \
         four that say what is wrong with an image, pack 0.9.0 added the \
         dual-echo TSE, pack 0.10.0 the 36 of fMRI, perfusion and the \
         gradient-echo family, pack 0.11.0 the 17 of its new constructs \
         and the re-measures' fixes, pack 0.12.0 the five of dev-1's: \
         a projection plane, a projection named, a phase word that is no \
         phase image, RESTORE and an MT weighting named, and pack 0.13.0 \
         the 22 of dev-1's second round: an echo train, PROPELLER, FLAIR and \
         STIR by inversion time, an inversion stated, black blood, MultiVane, WATS, t1_mpr, a \
         workstation reformat, a diffusion projection, trace, b0, MD, FA \
         and eADC by name, three for filtered images, PROMO, SyMRI's \
         tissue maps and a GE calibration scan, and pack 0.14.0 the 19 \
         of dev-2's: a synthetic image or map, a FLAIR on a spin-echo \
         readout and the IR-TSE readout it makes, PROPELLER and radial by \
         name, a GE spin echo in research mode, a multi-echo GRE by name \
         and the ME-GRE it makes, Philips' mFFE, GE's FSPGR, TOF by words, \
         MobiView, radial MIP views, MT switched off, SmartBrain, \
         spectroscopy, a GE SWI magnitude, and GE's Evec and tensor outputs, \
         and pack 0.15.0 the 9 of Nima's rulings of 2026-09-30 and dev-3's: \
         a TSE diffusion by its words, by Philips' bandwidth and as the \
         DWI-TSE it makes, Philips' projection trace and the trace image, a \
         GE EPI SWI by its train, GE's GRASS, GE's MAGiC and Siemens' tun, \
         and pack 0.16.0 the 3 of dev-4's: a reformat named for another \
         plane than its protocol, a dual-echo spin echo named FLAIR and a \
         Siemens acquired diffusion image, and pack 0.18.0 the 49 of the \
         sealed checkpoint's rule bugs and the vendor conventions of the \
         deep research: GE's rewritten numeric headers, an enhanced object's \
         spin echo by its train, a DSC that is no SWI, the DSC maps and a \
         derived MoCo DSC, a single-volume DSC, Philips' s3DI and QMap, \
         EPIMix by its pulse sequence, the names read as what they do not \
         mean, an EPI named T2*, a b value that is no diffusion, GE's SPGR, \
         TRICKS, SPGR-EPI and fast steady-state echo, a Philips diffusion \
         by its technique, MobiView, an acquired SWI echo, Silent MRA, TTEST \
         on an ASL, the projections and reformats, a subtraction, the QMap \
         maps by name, Philips' isotropic image, a diffusion SBRef, MRCP, \
         VASCTOF as TOF, FIESTA-C and the CSF null of an inversion, and \
         the 8 of the values the vocabulary lacked: MRS, ZTE, CE-MRA and \
         its words, NeuroMix's pulse sequence, PBP, K2 and a velocity image, and pack 0.19.0 the 23 of record 53: an enhanced object's mechanism by the MR Pulse Sequence module's table (a spin, gradient or both echo, EPI, a train, single shot, SE-EPI, GRE-EPI, GRASE, SS-TSE, 3D-TSE, TSE, bSSFP, SS-GRE, SSFP, spoiled, TOF, PC, ASL, radial, spiral and inversion recovery) and an MR spectroscopy object by its SOP class, and pack \
         0.20.0 the 30 of a development grade's nine disputes and rule \
         bugs, less the derived MoCo DSC it withdraws: a Philips FFE by its \
         spoiling, private technique and steady state, a Philips SWIp \
         output of combined echoes, a Siemens MEAN phase or magnitude over echoes, a \
         Philips FFE written one echo per stack or with one echo, a Siemens \
         real inversion recovery, a tensor's source by name, GE's AvDC as a \
         tensor output, a planning series of few slices and a Swedish \
         survey, a reversed-phase b=0 run and its own sign, a Siemens \
         in-phase and opposed-phase echo by name and echo time, a FLAIR \
         name the physics rule out, GE's research-mode spin echo by its \
         train and gradient echo by its name, GE's DTI output on a numeric \
         header, a Siemens gradient-echo stem against letters inside a \
         word, a minIP in mixed case and GE's Inhance velocity, and pack \
         0.21.0 the 41 of record 48's conventions round 3 and a development \
         grade's rule bugs: the vendors' localizer flags and the slice \
         count, a stitched composite, a MAXIMUM projection, a CSA MPR image, \
         a temporal standard deviation, GE's and Philips' phase-contrast \
         outputs, a calibration scan, GE's gradient-echo pulse sequences, \
         Philips' mDixon TSE and Dixon part, Siemens' Dixon part, Philips' \
         echo count, a 3D TFE with no inversion, Silent MRA's output, GE's \
         private phase, a spin-echo component without PSIR, a registered \
         copy, MUSE and IRIS, GE's fast recovery, an acquired SWI echo, a \
         single-echo MEMP, a short-TE spin echo named T2, an MTw gradient \
         echo, a direction count named, and an echo STAGE acquired, and pack \
         0.22.0 the 3 of a development grade's rule bugs: an image typed \
         LOCALIZER on a gradient echo, and a STAGE echo named PD or T1, and \
         pack 0.23.0 the 4 of the next: a tensor source and a plain DWI as a \
         session sees them, a diffusion named for fewer than six directions, \
         and a Siemens range reformat, and pack 0.24.0 the 4 of the broken \
         T2* exclusion: a position display on a gradient echo, a \
         gradient-echo scout, a GE fast gradient echo by its pulse sequence, \
         and a readout that keeps a spin-echo word off, and the 9 of the \
         final certificate's Siemens rule gaps: a reformat of an inversion \
         recovery whose component only its source states and the one the \
         rules leave IR and MPR, its real and magnitude sources, a reformat \
         with no black-blood word and a black-blood source, MOLLI's inline \
         T1 map, a MEAN magnitude over echoes, and an MPRAGE by its shot \
         interval, the 11 of its GE rule gaps (an SPGR name on a TOF \
         option, a MAGiC real image, a SWI output and a multi-echo QSM by \
         the pulse sequence, GE's own map names, a B0 map and a SWI read out \
         by EPI), the 4 of its settled disputes (a 3D ksepi gradient echo, \
         a Philips directional set, a Siemens filter copy and a QSM-named \
         echo), the 10 of its Philips and keyword rule gaps, and a TWIST-VIBE \
         dynamic run kept out of CE-MRA, and the 24 of round 4, the name \
         against the physics: the field bins, the inversion and its STIR, \
         T1-IR and FLAIR windows, the weighting words, the readouts, R1 to R8 \
         and their qualifiers"
    );
    assert!(pack.cases >= 15, "{} cases", pack.cases);
    assert!(pack.overlay.is_none());
}

/// Record 37, slice S5. The archive states an identity in `ImageType` on
/// 17,065 stacks that no axis of the pack read, in thirteen flags that were
/// parsed and never looked at again. Every spelling the 2026-09-19 survey
/// counted is below, with the axis it now reaches: the table is the breadth
/// of the archive's vocabulary, where the pack's own corpus carries the
/// headline claims one at a time.
#[test]
fn every_image_type_identity_record_37_named_reaches_an_axis() {
    let pack = nils_pack::load(&packs().join("mri"), None).expect("the MRI pack loads");
    // image type, axis, what the row should store.
    let want: &[(&str, &str, &str)] = &[
        // Composed: the largest single discriminator in the archive, 2,792
        // stacks over six spellings, parsed into a dead `is_composite`.
        ("DERIVED\\PRIMARY\\M\\COMPOSED", "construct", "Composed"),
        ("DERIVED\\PRIMARY\\M\\COMP_SP", "construct", "Composed"),
        ("DERIVED\\PRIMARY\\M\\COMP_AD", "construct", "Composed"),
        ("DERIVED\\PRIMARY\\M\\COMP_AN", "construct", "Composed"),
        ("DERIVED\\PRIMARY\\M\\COMP_AD_N", "construct", "Composed"),
        ("DERIVED\\PRIMARY\\M\\COMP_MIP", "construct", "Composed"),
        // The echo-combined image, 1,154 stacks, in a dead `is_mean`. It
        // reaches the axis through the susceptibility route as well, which
        // is where 1,084 of those stacks are.
        ("ORIGINAL\\PRIMARY\\M\\MEAN", "construct", "EchoCombined"),
        (
            "DERIVED\\PRIMARY\\SWI\\MEAN",
            "construct",
            "EchoCombined,SWI",
        ),
        (
            "DERIVED\\PRIMARY\\MINIP\\MEAN",
            "construct",
            "EchoCombined,MinIP",
        ),
        // The Dixon parts, 2,773 stacks. These have been on the construct
        // axis since it was written; what was dead was a second spelling of
        // them, and the case here is that the part survived its removal.
        ("ORIGINAL\\PRIMARY\\W\\WATER", "construct", "Water"),
        ("ORIGINAL\\PRIMARY\\M\\FAT", "construct", "Fat"),
        ("ORIGINAL\\PRIMARY\\M\\IN_PHASE", "construct", "InPhase"),
        ("ORIGINAL\\PRIMARY\\M\\IP", "construct", "InPhase"),
        ("ORIGINAL\\PRIMARY\\M\\OUT_PHASE", "construct", "OutPhase"),
        ("ORIGINAL\\PRIMARY\\M\\OPP_PHASE", "construct", "OutPhase"),
        // The quantitative maps, 3,633 stacks. `QMAP` said which quantity it
        // was of in a predicate the axis never read, so every one of them
        // came out as a T1 map and a T2 map at once.
        ("DERIVED\\PRIMARY\\QMAP\\T1", "construct", "T1map"),
        ("DERIVED\\PRIMARY\\QMAP\\T2", "construct", "T2map"),
        ("DERIVED\\PRIMARY\\QMAP\\PD", "construct", "PDmap"),
        ("DERIVED\\PRIMARY\\QMAP", "construct", "Qmap"),
        ("DERIVED\\PRIMARY\\T1 MAP", "construct", "T1map"),
        ("DERIVED\\PRIMARY\\R1", "construct", "R1map"),
        ("DERIVED\\PRIMARY\\R2", "construct", "R2map"),
        ("DERIVED\\PRIMARY\\FLIP ANGLE MAP", "construct", "B1map"),
        // The projection spellings with no predicate at all, 549 stacks, and
        // the truncated value on 326 more.
        ("DERIVED\\PRIMARY\\MINIMUM", "construct", "MinIP"),
        // MRI pack 0.21.0: Philips' MAXIMUM is a MIP only where the geometry
        // shows a projection (record 48, conventions round 3, case 2); the token
        // alone, with no slices stated, is none.
        ("DERIVED\\PRIMARY\\MAXIMUM", "construct", ""),
        ("DERIVED\\PRIMARY\\HD MIP", "construct", "MIP"),
        ("DERIVED\\PRIMARY\\MAX_IP", "construct", "MIP"),
        ("DERIVED\\PRIMARY\\MIPT", "construct", "MIP"),
        ("DERIVED\\PRIMARY\\CPR", "construct", "MPR"),
        (
            "DERIVED\\PRIMARY\\PROJECTION IMAG",
            "provenance",
            "ProjectionDerived",
        ),
        (
            "DERIVED\\PRIMARY\\COLLAPSE",
            "provenance",
            "ProjectionDerived",
        ),
        ("DERIVED\\PRIMARY\\PJN", "provenance", "ProjectionDerived"),
        // A statistic over a series, 357 stacks, telling 209 colliding names
        // apart.
        ("DERIVED\\PRIMARY\\TTEST", "construct", "TTestMap"),
        // The metal artefact technique, 16 stacks in two spellings, in a
        // dead `is_mavric`. The composite it also writes is the sum over the
        // spectral bins of one acquisition, and since MRI pack 0.18.0 no
        // construct (the 2026-09-30 deep research, case 3).
        ("ORIGINAL\\PRIMARY\\M\\MAVRIC", "technique", "MAVRIC"),
        ("DERIVED\\PRIMARY\\MAVRIC_COMPOSITE", "technique", "MAVRIC"),
        ("DERIVED\\PRIMARY\\MAVRIC_COMPOSITE", "construct", ""),
        // And what the file says is wrong with its own image.
        (
            "DERIVED\\PRIMARY\\SWI\\NAVAIL",
            "quality",
            "InputUnavailable",
        ),
        ("ORIGINAL\\PRIMARY\\M\\DISTORTED", "quality", "Distorted"),
        ("DERIVED\\PRIMARY\\ENCRYPTED", "quality", "Encrypted"),
        // An ordinary stack says none of it.
        ("ORIGINAL\\PRIMARY\\M\\ND\\NORM", "quality", ""),
    ];
    let mut wrong = Vec::new();
    for (image_type, axis, expected) in want {
        let mut stack = nils_pack::Stack::new();
        stack
            .set(
                "image_type",
                nils_pack::stack::Value::Text(Some(image_type)),
            )
            .expect("image_type is a field");
        let got = nils_pack::Evaluated::new(&pack, &stack)
            .classify()
            .stored(axis);
        if got != *expected {
            wrong.push(format!(
                "  {image_type}: {axis} is {got:?}, not {expected:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} identities do not reach their axis:\n{}",
        wrong.len(),
        want.len(),
        wrong.join("\n")
    );
}

/// A BIDS suffix that waits for a technique names the technique by its
/// identity. One spelled as a label (`TIRM` for `IR-TSE`) or misspelled
/// would load and never match, and the stack would fall through to its base
/// contrast without a word, so the pack is refused and says where.
#[test]
fn a_bids_suffix_waits_for_a_technique_the_pack_has() {
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                copy(&p, &to.join(e.file_name()));
            } else {
                std::fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }
    let to = std::env::temp_dir().join(format!("nils-bids-technique-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&to);
    copy(&packs().join("mri"), &to);
    let bids = std::fs::read_to_string(to.join("bids.yml")).unwrap();
    assert!(bids.contains("when_technique: MP2RAGE"));
    std::fs::write(
        to.join("bids.yml"),
        bids.replacen("when_technique: MP2RAGE", "when_technique: MP2RAG", 1),
    )
    .unwrap();
    let e = match nils_pack::load(&to, None) {
        Ok(_) => panic!("a technique the pack does not have loaded"),
        Err(e) => e.to_string(),
    };
    let _ = std::fs::remove_dir_all(&to);
    assert!(e.contains("when_technique"), "{e}");
    assert!(
        e.contains("MP2RAG is not a value of the technique axis"),
        "{e}"
    );
}

/// Pack 0.11.0: an `asl` suffix may say what every volume of the image is,
/// which the release writes as the aslcontext.tsv beside it. One of the
/// standard's volume types, and only on suffix `asl`.
#[test]
fn an_asl_suffix_says_its_volume_type_and_nothing_else_may() {
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                copy(&p, &to.join(e.file_name()));
            } else {
                std::fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }
    let to = std::env::temp_dir().join(format!("nils-bids-aslcontext-{}", std::process::id()));
    let adc = "    ADC: {datatype: dwi, suffix: ADC}";
    let load = |line: &str| {
        let _ = std::fs::remove_dir_all(&to);
        copy(&packs().join("mri"), &to);
        let bids = std::fs::read_to_string(to.join("bids.yml")).unwrap();
        assert!(bids.contains(adc), "the test edits the ADC line");
        std::fs::write(to.join("bids.yml"), bids.replacen(adc, line, 1)).unwrap();
        let loaded = nils_pack::load(&to, None).map_err(|e| e.to_string());
        let _ = std::fs::remove_dir_all(&to);
        loaded
    };
    let pack = load("    ADC: {datatype: perf, suffix: asl, aslcontext: deltam}").unwrap();
    assert_eq!(
        pack.bids.from_construct["ADC"].aslcontext.as_deref(),
        Some("deltam")
    );
    let e = match load("    ADC: {datatype: perf, suffix: asl, aslcontext: deltaM}") {
        Ok(_) => panic!("loaded"),
        Err(e) => e,
    };
    assert!(e.contains("deltaM is not a BIDS volume type"), "{e}");
    assert!(e.contains("aslcontext"), "{e}");
    let e = match load("    ADC: {datatype: perf, suffix: m0scan, aslcontext: m0scan}") {
        Ok(_) => panic!("loaded"),
        Err(e) => e,
    };
    assert!(e.contains("aslcontext is for suffix asl"), "{e}");
    // and the shipped pack's suffixes load with or without one
    let pack = nils_pack::load(&packs().join("mri"), None).unwrap();
    for (value, named) in &pack.bids.from_construct {
        if let Some(t) = &named.aslcontext {
            assert_eq!(named.suffix, "asl", "{value}");
            assert!(nils_pack::bids::ASL_VOLUME_TYPES.contains(&t.as_str()));
        }
    }
}
