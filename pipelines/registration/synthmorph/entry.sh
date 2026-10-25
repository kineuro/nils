set -u; cd [InputDataset];
flair() { local g s q; for g in "$1"/*_T2w.nii.gz "$1"/*_FLAIR.nii.gz; do [ -e "$g" ] || continue; s=$(basename "$g" .nii.gz);
case "$s" in *_FLAIR) echo "$g"; return;; *_acq-*) q=${s#*_acq-};; *) continue;; esac;
case "+${q%%_*}+" in *+FLAIR+*) echo "$g"; return;; esac; done; };
t2w() { local g s q; for g in "$1"/*_T2w.nii.gz; do [ -e "$g" ] || continue; s=$(basename "$g" .nii.gz);
case "$s" in *_acq-*) q=${s#*_acq-}; case "+${q%%_*}+" in *+FLAIR+*) continue;; esac;; esac; echo "$g"; return; done; };
qc() { mri_synthseg --i "$1" --o "$2/seg_$3.nii.gz" --qc "$2/qc_$3.csv" --robust --cpu --threads [THREADS] >/dev/null 2>&1 || return 1;
fspython -c "import sys, csv; r = list(csv.reader(open(sys.argv[1])))[-1]; v = [float(x) for x in r[1:] if x.strip()]; print(\"%.3f\" % min(v))" "$2/qc_$3.csv"; };
dice() { fspython -c "import sys, numpy as np, nibabel as n; a = np.asarray(n.load(sys.argv[1]).dataobj).astype(int); b = np.asarray(n.load(sys.argv[2]).dataobj).astype(int);
if a.shape != b.shape: print(\"\"); sys.exit(); L = [l for l in np.unique(a) if l > 0]; d = [2.0 * np.sum((a == l) & (b == l)) / max(np.sum(a == l) + np.sum(b == l), 1) for l in L]; print(\"%.3f\" % (np.mean(d) if d else 0))" "$1" "$2"; };
sanity() { fspython -c "import sys, numpy as np;
rows = [l.split() for l in open(sys.argv[1]) if len(l.split()) == 4 and all(x.replace(\".\", \"\").replace(\"-\", \"\").replace(\"e\", \"\").replace(\"+\", \"\").isdigit() for x in l.split())][:4];
M = np.array(rows, float); R = M[:3, :3]; det = float(np.linalg.det(R)); a = max(-1.0, min(1.0, (np.trace(R) - 1) / 2)); rot = float(np.degrees(np.arccos(a))); tl = float(np.linalg.norm(M[:3, 3]));
print(\"%.4f %d %.2f %.2f\" % (det, 1 if det < 0 else 0, tl, rot))" "$1"; };
warpstats() { fspython -c "import sys, numpy as np, nibabel as n; f = n.load(sys.argv[1]); u = np.asarray(f.dataobj, dtype=np.float32); u = u.reshape(u.shape[:3] + (-1,))[..., :3]; m = np.asarray(n.load(sys.argv[2]).dataobj) > 0;
sp = f.header.get_zooms()[:3]; g = [np.gradient(u[..., k], *sp) for k in range(3)]; J = np.stack([np.stack(g[k], -1) for k in range(3)], -2); J[..., 0, 0] += 1; J[..., 1, 1] += 1; J[..., 2, 2] += 1;
det = np.linalg.det(J); fold = 100.0 * np.mean(det[m] <= 0) if m.any() else 0.0;
import scipy.ndimage as d; v = n.load(sys.argv[3]); w = np.asarray(v.dataobj, dtype=np.float32).reshape(v.shape[:3] + (-1,))[..., :3]; idx = np.indices(u.shape[:3]).astype(np.float32);
pos = idx + np.stack([u[..., k] / sp[k] for k in range(3)]); back = np.stack([d.map_coordinates(w[..., k], pos, order=1, mode=\"nearest\") for k in range(3)]); rt = np.sqrt(sum(((u[..., k] + back[k]) ** 2) for k in range(3)));
print(\"%.4f %.3f\" % (fold, float(rt[m].mean()) if m.any() else 0.0))" "$1" "$2" "$3"; };
rigid=0; mri_synthmorph register -h 2>&1 | grep -q rigid && rigid=1;
tpl=/opt/templates/MNI152NLin2009cAsym_T1w.nii.gz; [ -f "$tpl" ] || tpl=$FREESURFER_HOME/average/mni_icbm152_nlin_asym_09c/mni_icbm152_t1_tal_nlin_asym_09c.nii;
for f in sub-*/ses-*/anat/*_T1w.nii.gz; do
[ -e "$f" ] || continue;
a=$(dirname "$f"); d=[OutputLocation]/$a; mkdir -p "$d"; b=$(basename "$f" .nii.gz); b=${b%_T1w};
w=$(mktemp -d [OutputLocation]/.work-XXXXXX);
qf=$(qc "$f" "$w" fixed) || { echo "$a: SynthSeg could not read the T1w, so its quality was not scored"; qf=""; };
if [ "[TASK]" = within_session ]; then
for role in flair t2w; do
if [ $role = flair ]; then mv=$(flair "$a"); con=FLAIR; else mv=$(t2w "$a"); con=T2w; fi;
[ -z "$mv" ] && continue;
qm=$(qc "$mv" "$w" moving_$con) || qm="";
if [ -n "$qf" ] && [ -n "$qm" ] && fspython -c "import sys; sys.exit(0 if min(float(sys.argv[1]), float(sys.argv[2])) < 0.65 else 1)" "$qf" "$qm"; then
echo "$a: SynthSeg scores an input below 0.65 (T1w $qf, $con $qm): the pair is refused"; printf "{\"moving\": \"%s\", \"method\": \"synthmorph\", \"qc_fixed\": %s, \"qc_moving\": %s, \"refused\": 1}\n" "$con" "$qf" "$qm" > "$d/${b}_from-${con}_to-T1w_desc-registration_metrics.json"; continue; fi;
model=affine; affine=1; if [ $rigid = 1 ]; then model=rigid; affine=0; fi;
mri_synthmorph register -m $model -t "$w/${con}.lta" -o "$w/${con}_aligned.nii.gz" -j [THREADS] "$mv" "$f" >/dev/null 2>&1 || { echo "$a: SynthMorph $model of the $con failed"; continue; };
[ $affine = 1 ] && echo "$a: the pinned SynthMorph has no rigid model, so the $con was aligned affinely (scale and shear allowed), flagged";
cp "$w/${con}.lta" "$d/${b}_from-${con}_to-T1w_mode-image_xfm.lta"; cp "$w/${con}_aligned.nii.gz" "$d/${b}_space-T1w_desc-aligned_${con}.nii.gz";
read det flip tl rot < <(sanity "$w/${con}.lta") || { det=""; flip=""; tl=""; rot=""; };
mri_synthseg --i "$w/${con}_aligned.nii.gz" --o "$w/seg_aligned.nii.gz" --robust --cpu --threads [THREADS] >/dev/null 2>&1 && ld=$(dice "$w/seg_fixed.nii.gz" "$w/seg_aligned.nii.gz") || ld="";
printf "{\"moving\": \"%s\", \"method\": \"synthmorph-%s\", \"affine_used\": %d%s%s%s%s%s%s%s}\n" "$con" "$model" "$affine" "${qf:+, \"qc_fixed\": $qf}" "${qm:+, \"qc_moving\": $qm}" "${det:+, \"rigid_determinant\": $det}" "${flip:+, \"rigid_flip\": $flip}" "${tl:+, \"rigid_translation_mm\": $tl}" "${rot:+, \"rigid_rotation_deg\": $rot}" "${ld:+, \"alignment_dice\": $ld}" > "$d/${b}_from-${con}_to-T1w_desc-registration_metrics.json";
echo "$a: $con onto the T1w, SynthMorph $model (quality T1w ${qf:-?}, $con ${qm:-?}; determinant ${det:-?}, translation ${tl:-?} mm, rotation ${rot:-?} degrees; label Dice ${ld:-?})"; done;
else
if [ -n "$qf" ] && fspython -c "import sys; sys.exit(0 if float(sys.argv[1]) < 0.65 else 1)" "$qf"; then echo "$a: SynthSeg scores the T1w below 0.65 ($qf): refused"; printf "{\"moving\": \"T1w\", \"method\": \"synthmorph-joint\", \"qc_fixed\": %s, \"refused\": 1}\n" "$qf" > "$d/${b}_from-T1w_to-MNI152NLin2009cAsym_desc-registration_metrics.json"; rm -rf "$w"; continue; fi;
[ -f "$tpl" ] || { echo "$a: no template: neither the site file mni_template nor the mni_icbm152_nlin_asym_09c of FreeSurfer is here"; rm -rf "$w"; continue; };
mri_synthmorph register -m joint -r 0.5 -t "$w/fwd.nii.gz" -T "$w/inv.nii.gz" -o "$w/aligned.nii.gz" -j [THREADS] "$f" "$tpl" >/dev/null 2>&1 || { echo "$a: SynthMorph joint to the template failed"; rm -rf "$w"; continue; };
cp "$w/fwd.nii.gz" "$d/${b}_from-T1w_to-MNI152NLin2009cAsym_mode-image_xfm.nii.gz"; cp "$w/inv.nii.gz" "$d/${b}_from-MNI152NLin2009cAsym_to-T1w_mode-image_xfm.nii.gz"; cp "$w/aligned.nii.gz" "$d/${b}_space-MNI152NLin2009cAsym_desc-aligned_T1w.nii.gz";
mri_synthstrip -i "$f" -m "$w/brain.nii.gz" --threads [THREADS] >/dev/null 2>&1 || cp "$w/seg_fixed.nii.gz" "$w/brain.nii.gz";
read fold ic < <(warpstats "$w/fwd.nii.gz" "$w/brain.nii.gz" "$w/inv.nii.gz") || { fold=""; ic=""; };
mri_synthseg --i "$tpl" --o "$w/seg_tpl.nii.gz" --robust --cpu --threads [THREADS] >/dev/null 2>&1 && mri_synthseg --i "$w/aligned.nii.gz" --o "$w/seg_aligned.nii.gz" --robust --cpu --threads [THREADS] >/dev/null 2>&1 && ld=$(dice "$w/seg_tpl.nii.gz" "$w/seg_aligned.nii.gz") || ld="";
printf "{\"moving\": \"T1w\", \"method\": \"synthmorph-joint\", \"affine_used\": 0%s%s%s%s}\n" "${qf:+, \"qc_fixed\": $qf}" "${fold:+, \"folding_pct\": $fold}" "${ic:+, \"inverse_consistency_mm\": $ic}" "${ld:+, \"alignment_dice\": $ld}" > "$d/${b}_from-T1w_to-MNI152NLin2009cAsym_desc-registration_metrics.json";
echo "$a: T1w onto MNI152NLin2009cAsym, SynthMorph joint (quality ${qf:-?}; folding ${fold:-?} % of brain voxels, inverse consistency ${ic:-?} mm, label Dice ${ld:-?})"; fi;
rm -rf "$w"; done
