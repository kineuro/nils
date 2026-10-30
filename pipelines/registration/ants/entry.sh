# SPDX-License-Identifier: AGPL-3.0-only
set -u; cd [InputDataset];
flair() { local g s q; for g in "$1"/*_T2w.nii.gz "$1"/*_FLAIR.nii.gz; do [ -e "$g" ] || continue; s=$(basename "$g" .nii.gz);
case "$s" in *_FLAIR) echo "$g"; return;; *_acq-*) q=${s#*_acq-};; *) continue;; esac;
case "+${q%%_*}+" in *+FLAIR+*) echo "$g"; return;; esac; done; };
t2w() { local g s q; for g in "$1"/*_T2w.nii.gz; do [ -e "$g" ] || continue; s=$(basename "$g" .nii.gz);
case "$s" in *_acq-*) q=${s#*_acq-}; case "+${q%%_*}+" in *+FLAIR+*) continue;; esac;; esac; echo "$g"; return; done; };
sanity() { ConvertTransformFile 3 "$1" "$2/xfm.txt" >/dev/null 2>&1 || return 1;
awk -v out="$2/sanity.txt" "/^Parameters:/ {for (i = 2; i <= 13; i++) p[i - 2] = \$i} /^FixedParameters:/ {for (i = 2; i <= 4; i++) c[i - 2] = \$i}
END {det = p[0] * (p[4] * p[8] - p[5] * p[7]) - p[1] * (p[3] * p[8] - p[5] * p[6]) + p[2] * (p[3] * p[7] - p[4] * p[6]);
tr = p[0] + p[4] + p[8]; a = (tr - 1) / 2; if (a > 1) a = 1; if (a < -1) a = -1; rot = atan2(sqrt(1 - a * a), a) * 180 / 3.14159265358979;
for (k = 0; k < 3; k++) o[k] = p[9 + k] + c[k] - (p[3 * k] * c[0] + p[3 * k + 1] * c[1] + p[3 * k + 2] * c[2]);
tl = sqrt(o[0] * o[0] + o[1] * o[1] + o[2] * o[2]); flip = (det < 0) ? 1 : 0;
printf \"%.4f %d %.2f %.2f\n\", det, flip, tl, rot > out}" "$2/xfm.txt"; };
inputs=/inputs;
for f in sub-*/ses-*/anat/*_T1w.nii.gz; do
[ -e "$f" ] || continue;
a=$(dirname "$f"); d=[OutputLocation]/$a; mkdir -p "$d"; b=$(basename "$f" .nii.gz); b=${b%_T1w};
w=$(mktemp -d [OutputLocation]/.work-XXXXXX); mask=""; m=$(find "$inputs"/brain_mask -path "*/$a/${b}_desc-brain_mask.nii.gz" 2>/dev/null | sort | head -n 1);
[ -n "$m" ] && mask=$m; masked=0; [ -n "$mask" ] && masked=1; xm=(); [ -n "$mask" ] && xm=(-x "[$mask,NULL]"); xn=(); [ -n "$mask" ] && xn=(-x "$mask");
if [ "[TASK]" = within_session ]; then
for role in flair t2w; do
if [ $role = flair ]; then mv=$(flair "$a"); con=FLAIR; else mv=$(t2w "$a"); con=T2w; fi;
[ -z "$mv" ] && continue;
if [ $role = t2w ] && [ -n "$(flair "$a")" ]; then echo "$a: the session has a FLAIR, so the FLAIR is registered onto the T1w and its T2w is not (the rules judged the FLAIR)"; continue; fi;
antsRegistration -d 3 --float 0 -u 0 -w "[0.005,0.995]" -r "[$f,$mv,1]" -t "Rigid[0.1]" -m "Mattes[$f,$mv,1,32,Regular,0.25]" -c "[1000x500x250x100,1e-6,10]" -f 8x4x2x1 -s 3x2x1x0vox ${xm[@]+"${xm[@]}"} -o "[$w/${con}_,]" -v 0 || { echo "$a: the rigid registration of the $con failed"; continue; };
antsApplyTransforms -d 3 -i "$mv" -r "$f" -t "$w/${con}_0GenericAffine.mat" -n Linear -o "$d/${b}_space-T1w_desc-aligned_${con}.nii.gz" || continue;
cp "$w/${con}_0GenericAffine.mat" "$d/${b}_from-${con}_to-T1w_mode-image_xfm.mat";
sanity "$w/${con}_0GenericAffine.mat" "$w" && read det flip tl rot < "$w/sanity.txt" || { det=""; flip=""; tl=""; rot=""; };
dice=""; note=""; mm=$(find "$inputs"/brain_mask -path "*/$a/$(basename "$mv" .nii.gz)_desc-brain_mask.nii.gz" 2>/dev/null | sort | head -n 1);
if [ -z "$mask" ]; then note="no brain mask of the T1w"; elif [ -z "$mm" ]; then note="no brain mask of the moving image";
else antsApplyTransforms -d 3 -i "$mm" -r "$f" -t "$w/${con}_0GenericAffine.mat" -n NearestNeighbor -o "$w/mask_moving.nii.gz" >/dev/null 2>&1 && dice=$(ImageMath 3 "$w/dice" DiceAndMinDistSum "$mask" "$w/mask_moving.nii.gz" >/dev/null 2>&1 && awk -F, "NR == 2 {printf \"%.3f\", \$2}" "$w/dice.csv"); [ -n "$dice" ] || note="the brain-mask Dice could not be computed"; fi;
printf "{\"moving\": \"%s\", \"method\": \"ants-rigid-mattes\", \"brain_masked\": %d%s%s%s%s%s%s}\n" "$con" "$masked" "${det:+, \"rigid_determinant\": $det}" "${flip:+, \"rigid_flip\": $flip}" "${tl:+, \"rigid_translation_mm\": $tl}" "${rot:+, \"rigid_rotation_deg\": $rot}" "${dice:+, \"alignment_dice\": $dice}" "${note:+, \"alignment_dice_note\": \"$note\"}" > "$d/${b}_from-${con}_to-T1w_desc-registration_metrics.json";
echo "$a: $con onto the T1w, rigid (determinant ${det:-?}, translation ${tl:-?} mm, rotation ${rot:-?} degrees, brain-mask Dice ${dice:-?}${note:+: $note})"; done;
else
tpl=/opt/templates/MNI152NLin2009cAsym_T1w.nii.gz;
if [ ! -f "$tpl" ]; then echo "$a: no template at $tpl: install this member into the software place (nils software install registration-ants@ants-2.6.2), which fetches TemplateFlow tpl-MNI152NLin2009cAsym_res-01_T1w.nii.gz by its sha256"; rm -rf "$w"; continue; fi;
N4BiasFieldCorrection -d 3 -i "$f" -o "$w/n4.nii.gz" -s 4 -b "[180]" -c "[50x50x50x50,0.0]" ${xn[@]+"${xn[@]}"} >/dev/null || { rm -rf "$w"; continue; };
if [ -n "$mask" ]; then ImageMath 3 "$w/brain.nii.gz" m "$w/n4.nii.gz" "$mask"; stripped=1; else cp "$w/n4.nii.gz" "$w/brain.nii.gz"; stripped=0; echo "$a: no brain mask registered for the T1w, so SyN ran on the whole head"; fi;
lm=""; xl=(); if [ "[LESIONS]" = on ]; then l=$(find "$inputs"/lesion_mask -path "*/$a/*_mask.nii.gz" -o -path "*/$a/*_dseg.nii.gz" 2>/dev/null | sort | head -n 1);
if [ -n "$l" ]; then ThresholdImage 3 "$l" "$w/notlesion.nii.gz" 0 0 1 0 >/dev/null; lm="$w/notlesion.nii.gz"; xl=(-x "NULL,$lm"); else echo "$a: lesions are on but no lesion mask is registered for the session: SyN ran without the lesion mask"; fi; fi;
antsRegistrationSyN.sh -d 3 -f "$tpl" -m "$w/brain.nii.gz" -o "$w/tpl_" -t s -n [THREADS] -j 0 ${xl[@]+"${xl[@]}"} >/dev/null || { echo "$a: SyN to the template failed"; rm -rf "$w"; continue; };
antsApplyTransforms -d 3 -r "$tpl" -t "$w/tpl_1Warp.nii.gz" -t "$w/tpl_0GenericAffine.mat" -o "[$d/${b}_from-T1w_to-MNI152NLin2009cAsym_mode-image_xfm.h5,1]" || continue;
antsApplyTransforms -d 3 -r "$w/brain.nii.gz" -t "[$w/tpl_0GenericAffine.mat,1]" -t "$w/tpl_1InverseWarp.nii.gz" -o "[$d/${b}_from-MNI152NLin2009cAsym_to-T1w_mode-image_xfm.h5,1]" || continue;
cp "$w/tpl_Warped.nii.gz" "$d/${b}_space-MNI152NLin2009cAsym_desc-aligned_T1w.nii.gz";
ThresholdImage 3 "$w/brain.nii.gz" "$w/bm.nii.gz" Otsu 1 >/dev/null;
CreateJacobianDeterminantImage 3 "$w/tpl_1InverseWarp.nii.gz" "$w/jac.nii.gz" 0 0 >/dev/null; ThresholdImage 3 "$w/jac.nii.gz" "$w/fold.nii.gz" -1e9 0 >/dev/null; ThresholdImage 3 "$tpl" "$w/bmt.nii.gz" Otsu 1 >/dev/null;
antsApplyTransforms -d 3 -r "$w/brain.nii.gz" -t "$w/tpl_1InverseWarp.nii.gz" -t "$w/tpl_1Warp.nii.gz" -o "[$w/round.nii.gz,1]" >/dev/null;
fold=$(ImageIntensityStatistics 3 "$w/fold.nii.gz" "$w/bmt.nii.gz" 2>/dev/null | awk "NR == 2 {printf \"%.4f\", \$2 * 100}");
for k in 0 1 2; do ImageMath 3 "$w/c$k.nii.gz" ExtractVectorComponent "$w/round.nii.gz" $k; ImageMath 3 "$w/c$k.nii.gz" m "$w/c$k.nii.gz" "$w/c$k.nii.gz"; done;
ImageMath 3 "$w/sum.nii.gz" + "$w/c0.nii.gz" "$w/c1.nii.gz"; ImageMath 3 "$w/sum.nii.gz" + "$w/sum.nii.gz" "$w/c2.nii.gz"; ImageMath 3 "$w/mag.nii.gz" ^ "$w/sum.nii.gz" 0.5;
ic=$(ImageIntensityStatistics 3 "$w/mag.nii.gz" "$w/bm.nii.gz" 2>/dev/null | awk "NR == 2 {printf \"%.3f\", \$2}");
sanity "$w/tpl_0GenericAffine.mat" "$w" && read det flip tl rot < "$w/sanity.txt" || { det=""; flip=""; tl=""; rot=""; };
printf "{\"moving\": \"T1w\", \"method\": \"ants-syn\", \"brain_masked\": %d, \"lesion_masked\": %d%s%s%s%s}\n" "$stripped" "$([ -n "$lm" ] && echo 1 || echo 0)" "${det:+, \"affine_determinant\": $det}" "${flip:+, \"rigid_flip\": $flip}" "${fold:+, \"folding_pct\": $fold}" "${ic:+, \"inverse_consistency_mm\": $ic}" > "$d/${b}_from-T1w_to-MNI152NLin2009cAsym_desc-registration_metrics.json";
echo "$a: T1w onto MNI152NLin2009cAsym, SyN (folding ${fold:-?} % of brain voxels, inverse consistency ${ic:-?} mm)"; fi;
rm -rf "$w"; done
