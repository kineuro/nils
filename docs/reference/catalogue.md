# The field catalogue

Generated from `engine/crates/nils-dicom/src/catalogue.rs` by `cargo run -p nils-dicom --example catalogue`; do not edit by hand. One row per column the digest writes (`docs/specs/wave1-parse-and-digest.md`, §6.2): the source, the converter (§6.3), the sensitivity class (§4.3) and a note. A chain is tried in order and the first present, non-empty element wins; an empty element does not stop a chain. `fg X.Y` is the Enhanced MR fallback: SharedFunctionalGroupsSequence, then the first item of PerFrameFunctionalGroupsSequence, in each the first item of X and its Y.

## subject (2)

| column | source | converter | class | note |
|---|---|---|---|---|
| `birth_date` | PatientBirthDate (0010,0030) | date | quasi-identifying | addition: v0 filled it through its importer |
| `sex` | PatientSex (0010,0040) | text | quasi-identifying | addition: v0 filled it through its importer |

## study (12)

| column | source | converter | class | note |
|---|---|---|---|---|
| `study_date` | StudyDate (0008,0020) | date | quasi-identifying |  |
| `study_time` | StudyTime (0008,0030) | time | quasi-identifying |  |
| `pps_start_date` | PerformedProcedureStepStartDate (0040,0244) | date | quasi-identifying | addition: a date the vote reads when StudyDate is gone (Wave 3 §4.2) |
| `pps_end_date` | PerformedProcedureStepEndDate (0040,0250) | date | quasi-identifying | addition: the same |
| `issue_date` | IssueDateOfImagingServiceRequest (0040,2004) | date | quasi-identifying | addition: the same, and weaker |
| `study_description` | StudyDescription (0008,1030) | text | quasi-identifying |  |
| `study_comments` | StudyComments (0032,4000) | text | quasi-identifying |  |
| `modalities_in_study` | ModalitiesInStudy (0008,0061) | text | technical | v0's study.modality |
| `manufacturer` | Manufacturer (0008,0070) | text | technical |  |
| `manufacturer_model_name` | ManufacturerModelName (0008,1090) | text | technical |  |
| `station_name` | StationName (0008,1010) | text | quasi-identifying |  |
| `institution_name` | InstitutionName (0008,0080) | text | quasi-identifying |  |

## series (31)

| column | source | converter | class | note |
|---|---|---|---|---|
| `modality` | Modality, else ModalitiesInStudy | text | technical | Modality, else a single-valued ModalitiesInStudy; PET becomes PT |
| `frame_of_reference_uid` | FrameOfReferenceUID (0020,0052) | text | technical |  |
| `implementation_class_uid` | ImplementationClassUID (0002,0012), else meta ImplementationClassUID | text | technical | the file meta when the element is absent (v0) |
| `media_storage_sop_instance_uid` | MediaStorageSOPInstanceUID (0002,0003), else meta MediaStorageSOPInstanceUID | text | technical | the file meta when the element is absent (v0) |
| `sop_class_uid` | SOPClassUID (0008,0016), else meta MediaStorageSOPClassUID | text | technical | the file meta when the element is absent (v0) |
| `implementation_version_name` | ImplementationVersionName (0002,0013), else meta ImplementationVersionName | text | technical | the file meta when the element is absent (v0) |
| `sequence_name` | SequenceName (0018,0024) | text | quasi-identifying |  |
| `protocol_name` | ProtocolName (0018,1030) | text | quasi-identifying |  |
| `series_date` | SeriesDate (0008,0021) | date | quasi-identifying |  |
| `series_time` | SeriesTime (0008,0031) | time | quasi-identifying |  |
| `series_description` | SeriesDescription (0008,103E) | text | quasi-identifying |  |
| `body_part_examined` | BodyPartExamined (0018,0015) | text | technical |  |
| `burned_in_annotation` | BurnedInAnnotation (0028,0301) | text | technical | addition: what the file says about text in its own pixels (Wave 3 §8.4); v0 never reads it |
| `scanning_sequence` | ScanningSequence, then private per-frame .ScanningSequence | text | technical | the private per-frame sequences are the Philips (2005,140F) and the Siemens (0021,1201) one, read without a creator check (v0) |
| `sequence_variant` | SequenceVariant, then private per-frame .SequenceVariant | text | technical | the private per-frame sequences are the Philips (2005,140F) and the Siemens (0021,1201) one, read without a creator check (v0) |
| `scan_options` | ScanOptions (0018,0022) | text | technical |  |
| `series_comments` | none | text | quasi-identifying | v0 named a keyword that is no DICOM element; always null, kept for parity |
| `image_type` | ImageType (0008,0008) | text | technical |  |
| `slice_thickness` | SliceThickness, then fg PixelMeasuresSequence.SliceThickness | double | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `spacing_between_slices` | SpacingBetweenSlices, then fg PixelMeasuresSequence.SpacingBetweenSlices | double | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `images_in_acquisition` | ImagesInAcquisition (0020,1002) | text | technical | text, as v0 stored it |
| `image_orientation_patient` | ImageOrientationPatient, then fg PlaneOrientationSequence.ImageOrientationPatient | text | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `image_position_patient` | ImagePositionPatient (0020,0032) | text | technical |  |
| `patient_position` | PatientPosition (0018,5100) | text | technical |  |
| `contrast_bolus_agent` | ContrastBolusAgent (0018,0010) | text | technical |  |
| `contrast_bolus_route` | ContrastBolusRoute (0018,1040) | text | technical |  |
| `contrast_bolus_total_dose` | ContrastBolusTotalDose (0018,1044) | double | technical |  |
| `contrast_bolus_start_time` | ContrastBolusStartTime (0018,1042) | time | quasi-identifying |  |
| `contrast_bolus_volume` | ContrastBolusVolume (0018,1041) | double | technical |  |
| `contrast_flow_rate` | ContrastFlowRate (0018,1046) | double | technical |  |
| `contrast_flow_duration` | ContrastFlowDuration (0018,1047) | double | technical |  |

## series_mr (32, MR only)

| column | source | converter | class | note |
|---|---|---|---|---|
| `mr_acquisition_type` | MRAcquisitionType (0018,0023) | text | technical |  |
| `angio_flag` | AngioFlag (0018,0025) | text | technical |  |
| `repetition_time` | RepetitionTime, then fg MRTimingAndRelatedParametersSequence.RepetitionTime, then private per-frame .RepetitionTime | double | technical | as on the stack |
| `echo_time` | EchoTime, then fg MREchoSequence.EffectiveEchoTime, then private per-frame .EchoTime | double | technical | as on the stack |
| `inversion_time` | InversionTime (0018,0082) | double | technical |  |
| `inversion_times` | InversionTimes (0018,9079) | text | technical |  |
| `flip_angle` | FlipAngle, then fg MRTimingAndRelatedParametersSequence.FlipAngle, then private per-frame .FlipAngle | double | technical | as on the stack |
| `phase_contrast` | PhaseContrast (0018,9014) | text | technical |  |
| `number_of_averages` | NumberOfAverages, then fg MRAveragesSequence.NumberOfAverages | double | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `imaging_frequency` | ImagingFrequency (0018,0084) | double | technical |  |
| `imaged_nucleus` | ImagedNucleus (0018,0085) | text | technical |  |
| `echo_numbers` | EchoNumbers (0018,0086) | text | technical |  |
| `magnetic_field_strength` | MagneticFieldStrength (0018,0087) | double | technical |  |
| `number_of_phase_encoding_steps` | NumberOfPhaseEncodingSteps (0018,0089) | text | technical | text, as v0 stored it |
| `echo_train_length` | EchoTrainLength, then fg MRTimingAndRelatedParametersSequence.EchoTrainLength, then private per-frame .EchoTrainLength | int | technical | as on the stack |
| `percent_sampling` | PercentSampling, then fg MRFOVGeometrySequence.PercentSampling | double | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `percent_phase_field_of_view` | PercentPhaseFieldOfView, then fg MRFOVGeometrySequence.PercentPhaseFieldOfView | double | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `pixel_bandwidth` | PixelBandwidth, then fg MRImagingModifierSequence.PixelBandwidth | text | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `receive_coil_name` | ReceiveCoilName, then fg MRReceiveCoilSequence.ReceiveCoilName | text | technical | as on the stack |
| `transmit_coil_name` | TransmitCoilName, then fg MRTransmitCoilSequence.TransmitCoilName | text | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `acquisition_matrix` | AcquisitionMatrix (0018,1310) | text | technical |  |
| `phase_encoding_direction` | InPlanePhaseEncodingDirection (0018,1312) | text | technical | addition: InPlanePhaseEncodingDirection; v0's keyword PhaseEncodingDirection is no element and the column was always null |
| `sar` | SAR (0018,1316) | double | technical |  |
| `dbdt` | dBdt (0018,1318) | text | technical | dBdt, text as v0 stored it |
| `b1rms` | B1rms (0018,1320) | double | technical |  |
| `temporal_position_identifier` | TemporalPositionIdentifier (0020,0100) | int | technical |  |
| `number_of_temporal_positions` | NumberOfTemporalPositions (0020,0105) | int | technical |  |
| `temporal_resolution` | TemporalResolution (0020,0110) | text | technical | text, as v0 stored it |
| `parallel_acquisition_technique` | ParallelAcquisitionTechnique, then fg MRModifierSequence.ParallelAcquisitionTechnique | text | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `parallel_reduction_factor_in_plane` | ParallelReductionFactorInPlane, then fg MRModifierSequence.ParallelReductionFactorInPlane | text | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `dwi_siemens_pe_dir_positive` | (0029,xx10) SIEMENS CSA HEADER, SV10 PhaseEncodingDirectionPositive | int | technical | CSA image header, SV10 only (v0) |
| `dwi_ge_n_directions` | (0043,xx30) GEMS_PARM_01 | int | technical | private, by creator block; bytes read as SS |

## series_ct (24, CT only)

| column | source | converter | class | note |
|---|---|---|---|---|
| `kvp` | KVP (0018,0060) | double | technical |  |
| `data_collection_diameter` | DataCollectionDiameter (0018,0090) | double | technical |  |
| `reconstruction_diameter` | ReconstructionDiameter (0018,1100) | double | technical |  |
| `gantry_detector_tilt` | GantryDetectorTilt (0018,1120) | double | technical |  |
| `table_height` | TableHeight (0018,1130) | double | technical |  |
| `rotation_direction` | RotationDirection (0018,1140) | text | technical |  |
| `exposure_time` | ExposureTime (0018,1150) | double | technical |  |
| `x_ray_tube_current` | XRayTubeCurrent (0018,1151) | double | technical |  |
| `exposure` | Exposure (0018,1152) | double | technical |  |
| `filter_type` | FilterType (0018,1160) | text | technical |  |
| `generator_power` | GeneratorPower (0018,1170) | double | technical |  |
| `focal_spots` | FocalSpots (0018,1190) | text | technical |  |
| `convolution_kernel` | ConvolutionKernel (0018,1210) | text | technical |  |
| `revolution_time` | RevolutionTime (0018,9305) | double | technical |  |
| `single_collimation_width` | SingleCollimationWidth (0018,9306) | double | technical |  |
| `total_collimation_width` | TotalCollimationWidth (0018,9307) | double | technical |  |
| `table_speed` | TableSpeed (0018,9309) | double | technical |  |
| `table_feed_per_rotation` | TableFeedPerRotation (0018,9310) | double | technical |  |
| `spiral_pitch_factor` | SpiralPitchFactor (0018,9311) | double | technical |  |
| `exposure_modulation_type` | ExposureModulationType (0018,9323) | text | technical |  |
| `ctdi_vol` | CTDIvol (0018,9345) | double | technical | CTDIvol |
| `ctdi_phantom_type_code_sequence` | CTDIPhantomTypeCodeSequence (0018,9346) | json | technical | DICOM JSON of the items |
| `calcium_scoring_mass_factor_device` | CalciumScoringMassFactorDevice (0018,9352) | double | technical |  |
| `calcium_scoring_mass_factor_patient` | CalciumScoringMassFactorPatient (0018,9351) | double | technical |  |

## series_pet (29, PT only)

| column | source | converter | class | note |
|---|---|---|---|---|
| `radiopharmaceutical` | Radiopharmaceutical, then RadiopharmaceuticalInformationSequence[0].Radiopharmaceutical | text | technical | addition: the first item of RadiopharmaceuticalInformationSequence, where the PET IOD keeps it; v0 read the top level only |
| `radionuclide_total_dose` | RadionuclideTotalDose, then RadiopharmaceuticalInformationSequence[0].RadionuclideTotalDose | double | technical | addition: the first item of RadiopharmaceuticalInformationSequence, where the PET IOD keeps it; v0 read the top level only |
| `radionuclide_half_life` | RadionuclideHalfLife, then RadiopharmaceuticalInformationSequence[0].RadionuclideHalfLife | double | technical | addition: the first item of RadiopharmaceuticalInformationSequence, where the PET IOD keeps it; v0 read the top level only |
| `radionuclide_positron_fraction` | RadionuclidePositronFraction, then RadiopharmaceuticalInformationSequence[0].RadionuclidePositronFraction | double | technical | addition: the first item of RadiopharmaceuticalInformationSequence, where the PET IOD keeps it; v0 read the top level only |
| `radiopharmaceutical_start_time` | RadiopharmaceuticalStartTime, then RadiopharmaceuticalInformationSequence[0].RadiopharmaceuticalStartTime | time | quasi-identifying | addition: the first item of RadiopharmaceuticalInformationSequence, where the PET IOD keeps it; v0 read the top level only |
| `radiopharmaceutical_stop_time` | RadiopharmaceuticalStopTime, then RadiopharmaceuticalInformationSequence[0].RadiopharmaceuticalStopTime | time | quasi-identifying | addition: the first item of RadiopharmaceuticalInformationSequence, where the PET IOD keeps it; v0 read the top level only |
| `radiopharmaceutical_volume` | RadiopharmaceuticalVolume, then RadiopharmaceuticalInformationSequence[0].RadiopharmaceuticalVolume | double | technical | addition: the first item of RadiopharmaceuticalInformationSequence, where the PET IOD keeps it; v0 read the top level only |
| `radiopharmaceutical_route` | RadiopharmaceuticalRoute, then RadiopharmaceuticalInformationSequence[0].RadiopharmaceuticalRoute | text | technical | addition: the first item of RadiopharmaceuticalInformationSequence, where the PET IOD keeps it; v0 read the top level only |
| `decay_correction` | DecayCorrection (0054,1102) | text | technical |  |
| `decay_factor` | DecayFactor (0054,1321) | double | technical |  |
| `reconstruction_method` | ReconstructionMethod (0054,1103) | text | technical |  |
| `scatter_correction_method` | ScatterCorrectionMethod (0054,1105) | text | technical |  |
| `attenuation_correction_method` | AttenuationCorrectionMethod (0054,1101) | text | technical |  |
| `randoms_correction_method` | RandomsCorrectionMethod (0054,1100) | text | technical |  |
| `dose_calibration_factor` | DoseCalibrationFactor (0054,1322) | double | technical |  |
| `activity_concentration_scale` | none | double | technical | v0 named a keyword that is no DICOM element; always null, kept for parity |
| `suv_type` | SUVType (0054,1006) | text | technical |  |
| `suvbw` | none | double | technical | v0 named a keyword that is no DICOM element; always null, kept for parity |
| `suvlbm` | none | double | technical | v0 named a keyword that is no DICOM element; always null, kept for parity |
| `suvbsa` | none | double | technical | v0 named a keyword that is no DICOM element; always null, kept for parity |
| `counts_source` | CountsSource (0054,1002) | text | technical |  |
| `units` | Units (0054,1001) | text | technical |  |
| `frame_reference_time` | FrameReferenceTime (0054,1300) | double | technical |  |
| `actual_frame_duration` | ActualFrameDuration (0018,1242) | double | technical |  |
| `patient_gantry_relationship_code` | PatientGantryRelationshipCodeSequence (0054,0414) | json | technical | DICOM JSON of the items |
| `slice_progression_direction` | SliceProgressionDirection (0054,0500) | text | technical |  |
| `series_type` | SeriesType (0054,1000) | text | technical |  |
| `units_type` | Units (0054,1001) | text | technical | Units once more, the way v0 has it |
| `counts_included` | CountsIncluded (0054,1400) | text | technical |  |

## stack (14)

| column | source | converter | class | note |
|---|---|---|---|---|
| `inversion_time` | InversionTime (0018,0082) | double | technical |  |
| `echo_time` | EchoTime, then fg MREchoSequence.EffectiveEchoTime, then private per-frame .EchoTime | double | technical | MREchoSequence.EffectiveEchoTime, then the private sequences (v0) |
| `echo_numbers` | EchoNumbers (0018,0086) | text | technical |  |
| `echo_train_length` | EchoTrainLength, then fg MRTimingAndRelatedParametersSequence.EchoTrainLength, then private per-frame .EchoTrainLength | int | technical | MRTimingAndRelatedParametersSequence, then the private sequences (v0) |
| `repetition_time` | RepetitionTime, then fg MRTimingAndRelatedParametersSequence.RepetitionTime, then private per-frame .RepetitionTime | double | technical | MRTimingAndRelatedParametersSequence, then the private sequences (v0) |
| `flip_angle` | FlipAngle, then fg MRTimingAndRelatedParametersSequence.FlipAngle, then private per-frame .FlipAngle | double | technical | MRTimingAndRelatedParametersSequence, then the private sequences (v0) |
| `receive_coil_name` | ReceiveCoilName, then fg MRReceiveCoilSequence.ReceiveCoilName | text | technical | MRReceiveCoilSequence (v0) |
| `image_orientation_patient` | ImageOrientationPatient, then fg PlaneOrientationSequence.ImageOrientationPatient | text | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `image_type` | ImageType (0008,0008) | text | technical |  |
| `xray_exposure` | Exposure (0018,1152) | double | technical | Exposure |
| `kvp` | KVP (0018,0060) | double | technical |  |
| `tube_current` | XRayTubeCurrent (0018,1151) | double | technical | XRayTubeCurrent |
| `pet_bed_index` | NumberOfSlices (0054,0081) | int | technical | NumberOfSlices, v0's name |
| `pet_frame_type` | SeriesType (0054,1000) | text | technical | SeriesType, v0's name |

## instance (33)

| column | source | converter | class | note |
|---|---|---|---|---|
| `instance_number` | InstanceNumber (0020,0013) | int | technical |  |
| `acquisition_number` | AcquisitionNumber (0020,0012) | int | technical |  |
| `acquisition_date` | AcquisitionDate (0008,0022) | date | quasi-identifying |  |
| `acquisition_time` | AcquisitionTime (0008,0032) | time | quasi-identifying |  |
| `content_date` | ContentDate (0008,0023) | date | quasi-identifying |  |
| `instance_creation_date` | InstanceCreationDate (0008,0012) | date | quasi-identifying | addition: a date the vote reads, and one an anonymiser often rewrites to a first of January (Wave 3 §4.2) |
| `presentation_creation_date` | PresentationCreationDate (0070,0082) | date | quasi-identifying | addition: the same, and weaker |
| `content_time` | ContentTime (0008,0033) | time | quasi-identifying |  |
| `slice_location` | SliceLocation (0020,1041) | double | technical |  |
| `pixel_spacing` | PixelSpacing, then fg PixelMeasuresSequence.PixelSpacing | text | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `rows` | Rows (0028,0010) | int | technical |  |
| `columns` | Columns (0028,0011) | int | technical |  |
| `bits_allocated` | BitsAllocated (0028,0100) | int | technical |  |
| `bits_stored` | BitsStored (0028,0101) | int | technical |  |
| `high_bit` | HighBit (0028,0102) | int | technical |  |
| `pixel_representation` | PixelRepresentation (0028,0103) | int | technical |  |
| `window_center` | WindowCenter (0028,1050) | text | technical |  |
| `window_width` | WindowWidth (0028,1051) | text | technical |  |
| `rescale_intercept` | RescaleIntercept (0028,1052) | double | technical |  |
| `rescale_slope` | RescaleSlope (0028,1053) | double | technical |  |
| `number_of_frames` | NumberOfFrames (0028,0008) | int | technical |  |
| `lossy_image_compression` | LossyImageCompression (0028,2110) | text | technical |  |
| `derivation_description` | DerivationDescription (0008,2111) | text | quasi-identifying |  |
| `image_comments` | ImageComments (0020,4000) | text | quasi-identifying |  |
| `transfer_syntax_uid` | TransferSyntaxUID (0002,0010), else meta TransferSyntaxUID | text | technical | the file meta; for a bare data set the syntax it was read with (v0 stored null there) |
| `charset` | SpecificCharacterSet (0008,0005) | text | technical | addition: SpecificCharacterSet as written |
| `diffusion_b_value` | DiffusionBValue, then fg MRDiffusionSequence.DiffusionBValue | text | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `diffusion_gradient_orientation` | DiffusionGradientOrientation (0018,9089) | text | technical |  |
| `diffusion_directionality` | DiffusionDirectionality, then fg MRDiffusionSequence.DiffusionDirectionality | text | technical | Enhanced MR fallback: the functional groups, shared then per-frame (v0) |
| `dwi_siemens_b_value` | (0019,xx0C) SIEMENS MR HEADER | int | technical | private, by creator block; bytes of an implicit VR file read as IS |
| `dwi_siemens_directionality` | (0019,xx0D) SIEMENS MR HEADER | text | technical | private, by creator block |
| `dwi_ge_b_value` | (0043,xx39) GEMS_PARM_01, first value | int | technical | the first of the four values |
| `dwi_philips_b_value` | (2001,xx03) Philips Imaging DD 001, sentinel above 1e37 is null | double | technical | the sentinel above 1e37 is null (v0); bytes read as FL |

177 columns.
