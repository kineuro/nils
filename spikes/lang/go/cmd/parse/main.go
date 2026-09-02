// SPDX-License-Identifier: AGPL-3.0-only

// The language spike, Go side (docs/decisions/15, C1): walk a corpus, read every
// file's DICOM header up to the pixel data, extract a fixed set of technical tags,
// and write counts, rates and failure classes. Same semantics as rust/parse.
//
// N worker goroutines parse; one writer goroutine owns the output files, which mimics
// v0's single database writer. Output stays on the host: index.tsv holds one row per
// parsed file keyed by a sequence number, paths.tsv maps sequence numbers to paths,
// failures.tsv lists the files that failed with their class and the library's
// message, and summary.json holds the numbers the report may quote.
package main

import (
	"bufio"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/suyashkumar/dicom"
	"github.com/suyashkumar/dicom/pkg/tag"
)

const library = "suyashkumar/dicom v1.1.0"

// The technical tags the index keeps. Patient-level tags are never read.
var tags = []struct {
	name string
	tag  tag.Tag
}{
	{"SOPClassUID", tag.SOPClassUID},
	{"SOPInstanceUID", tag.SOPInstanceUID},
	{"StudyInstanceUID", tag.StudyInstanceUID},
	{"SeriesInstanceUID", tag.SeriesInstanceUID},
	{"Modality", tag.Modality},
	{"Manufacturer", tag.Manufacturer},
	{"ManufacturerModelName", tag.ManufacturerModelName},
	{"SeriesDescription", tag.SeriesDescription},
	{"ProtocolName", tag.ProtocolName},
	{"SeriesNumber", tag.SeriesNumber},
	{"InstanceNumber", tag.InstanceNumber},
	{"ImageType", tag.ImageType},
	{"EchoTime", tag.EchoTime},
	{"RepetitionTime", tag.RepetitionTime},
	{"InversionTime", tag.InversionTime},
	{"FlipAngle", tag.FlipAngle},
	{"SliceThickness", tag.SliceThickness},
	{"PixelSpacing", tag.PixelSpacing},
	{"ImageOrientationPatient", tag.ImageOrientationPatient},
	{"ImagePositionPatient", tag.ImagePositionPatient},
	{"Rows", tag.Rows},
	{"Columns", tag.Columns},
}

// The classes are the report's vocabulary, shared with Rust.
const (
	classOk            = "ok"             // Part 10 file, meta group present, header read to the pixel data
	classOkRaw         = "ok_raw"         // no preamble and no meta group; read as a raw dataset
	classNotDicom      = "not_dicom"      // the first bytes are neither Part 10 nor a raw dataset
	classParseError    = "parse_error"    // the library gave up before the pixel data
	classTruncated     = "truncated"      // the file ended before the header did
	classUnsupportedTs = "unsupported_ts" // transfer syntax the library does not read
	classMissingSop    = "missing_sop"    // parsed, but no SOP Instance UID
	classIoError       = "io_error"       // the operating system refused the read
)

type record struct {
	seq     uint64
	path    string
	size    int64
	class   string
	message string // the library's message for failures, without the path
	ts      string // transfer syntax UID from the meta group, or "raw" for the fallback
	values  []string
}

type summary struct {
	Implementation     string            `json:"implementation"`
	Library            string            `json:"library"`
	Label              string            `json:"label"`
	Workers            int               `json:"workers"`
	HostCPUs           int               `json:"host_cpus"`
	Files              uint64            `json:"files"`
	Bytes              uint64            `json:"bytes"`
	Parsed             uint64            `json:"parsed"`
	Failed             uint64            `json:"failed"`
	Classes            map[string]uint64 `json:"classes"`
	TransferSyntaxes   map[string]uint64 `json:"transfer_syntaxes"`
	WallSeconds        float64           `json:"wall_seconds"`
	FilesPerSecond     float64           `json:"files_per_second"`
	MegabytesPerSecond float64           `json:"megabytes_per_second"`
	UserCPUSeconds     float64           `json:"user_cpu_seconds"`
	SystemCPUSeconds   float64           `json:"system_cpu_seconds"`
	PeakRSSMegabytes   float64           `json:"peak_rss_megabytes"`
}

func main() {
	root := flag.String("root", "", "corpus root; every regular file below it is read, whatever its name")
	out := flag.String("out", "", "output directory (created); stays on the host")
	workers := flag.Int("workers", 8, "parser goroutines")
	limit := flag.Uint64("limit", 0, "stop after this many files (0 = all)")
	label := flag.String("label", "", "free-form label copied into summary.json")
	flag.Parse()
	if *root == "" || *out == "" {
		fmt.Fprintln(os.Stderr, "usage: parse --root DIR --out DIR [--workers N] [--limit N] [--label L]")
		os.Exit(2)
	}
	if err := os.MkdirAll(*out, 0o755); err != nil {
		panic(err)
	}
	started := time.Now()

	paths := make(chan record, 4096)
	records := make(chan record, 4096)

	var wg sync.WaitGroup
	for i := 0; i < *workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for rec := range paths {
				records <- parseOne(rec)
			}
		}()
	}
	go func() {
		wg.Wait()
		close(records)
	}()

	type totals struct {
		counts map[string]uint64
		ts     map[string]uint64
		bytes  uint64
	}
	done := make(chan totals, 1)
	go func() {
		counts, ts, bytes := writeAll(*out, records)
		done <- totals{counts, ts, bytes}
	}()

	// The walker feeds the workers from this goroutine: no extension filter, no
	// symlinks followed.
	var n uint64
	_ = filepath.WalkDir(*root, func(path string, d fs.DirEntry, err error) error {
		if err != nil || !d.Type().IsRegular() {
			return nil
		}
		n++
		paths <- record{seq: n, path: path}
		if *limit > 0 && n >= *limit {
			return filepath.SkipAll
		}
		return nil
	})
	close(paths)
	t := <-done

	wall := time.Since(started).Seconds()
	var ru syscall.Rusage
	_ = syscall.Getrusage(syscall.RUSAGE_SELF, &ru)
	parsed := t.counts[classOk] + t.counts[classOkRaw]
	s := summary{
		Implementation:     "go",
		Library:            library,
		Label:              *label,
		Workers:            *workers,
		HostCPUs:           runtime.NumCPU(),
		Files:              n,
		Bytes:              t.bytes,
		Parsed:             parsed,
		Failed:             n - parsed,
		Classes:            t.counts,
		TransferSyntaxes:   t.ts,
		WallSeconds:        wall,
		FilesPerSecond:     float64(n) / wall,
		MegabytesPerSecond: float64(t.bytes) / 1e6 / wall,
		UserCPUSeconds:     float64(ru.Utime.Sec) + float64(ru.Utime.Usec)/1e6,
		SystemCPUSeconds:   float64(ru.Stime.Sec) + float64(ru.Stime.Usec)/1e6,
		PeakRSSMegabytes:   float64(ru.Maxrss) / 1024.0,
	}
	js, _ := json.MarshalIndent(s, "", "  ")
	if err := os.WriteFile(filepath.Join(*out, "summary.json"), js, 0o644); err != nil {
		panic(err)
	}
	fmt.Println(string(js))
}

// The writer goroutine: owns the three TSV files and the counters.
func writeAll(out string, records <-chan record) (map[string]uint64, map[string]uint64, uint64) {
	open := func(name string) (*bufio.Writer, *os.File) {
		f, err := os.Create(filepath.Join(out, name))
		if err != nil {
			panic(err)
		}
		return bufio.NewWriterSize(f, 1<<20), f
	}
	index, indexF := open("index.tsv")
	paths, pathsF := open("paths.tsv")
	failures, failuresF := open("failures.tsv")
	header := []string{"seq", "size", "class", "ts"}
	for _, t := range tags {
		header = append(header, t.name)
	}
	fmt.Fprintln(index, strings.Join(header, "\t"))
	fmt.Fprintln(paths, "seq\tpath")
	fmt.Fprintln(failures, "seq\tclass\tmessage\tpath")

	counts := map[string]uint64{}
	ts := map[string]uint64{}
	var bytes uint64
	for rec := range records {
		bytes += uint64(rec.size)
		counts[rec.class]++
		fmt.Fprintf(paths, "%d\t%s\n", rec.seq, rec.path)
		switch rec.class {
		case classOk, classOkRaw:
			ts[rec.ts]++
			fmt.Fprintf(index, "%d\t%d\t%s\t%s", rec.seq, rec.size, rec.class, rec.ts)
			for _, v := range rec.values {
				index.WriteByte('\t')
				index.WriteString(clean(v))
			}
			index.WriteByte('\n')
		default:
			fmt.Fprintf(failures, "%d\t%s\t%s\t%s\n", rec.seq, rec.class, clean(rec.message), rec.path)
		}
	}
	for _, w := range []*bufio.Writer{index, paths, failures} {
		if err := w.Flush(); err != nil {
			panic(err)
		}
	}
	for _, f := range []*os.File{indexF, pathsF, failuresF} {
		_ = f.Close()
	}
	return counts, ts, bytes
}

// TSV cells carry no tabs, newlines or control characters.
func clean(s string) string {
	return strings.TrimSpace(strings.Map(func(r rune) rune {
		if r < 0x20 || r == 0x7f {
			return ' '
		}
		return r
	}, s))
}

type sniffResult int

const (
	sniffPart10 sniffResult = iota
	sniffRaw
	sniffOther
	sniffUnreadable
)

// A look at the first bytes decides which reader to use, the same look in both
// languages: the DICM magic after the preamble or at the start means Part 10; a
// group 0008 tag at the start means a raw dataset; anything else is not DICOM.
func sniff(path string) (sniffResult, error) {
	f, err := os.Open(path)
	if err != nil {
		return sniffUnreadable, err
	}
	defer f.Close()
	var buf [132]byte
	n, err := io.ReadFull(f, buf[:])
	if err != nil && !errors.Is(err, io.ErrUnexpectedEOF) && !errors.Is(err, io.EOF) {
		return sniffUnreadable, err
	}
	switch {
	case (n >= 132 && string(buf[128:132]) == "DICM") || (n >= 4 && string(buf[0:4]) == "DICM"):
		return sniffPart10, nil
	case n >= 8 && buf[0] == 0x08 && buf[1] == 0x00:
		return sniffRaw, nil
	default:
		return sniffOther, nil
	}
}

func parseOne(rec record) record {
	if st, err := os.Stat(rec.path); err == nil {
		rec.size = st.Size()
	}
	kind, err := sniff(rec.path)
	switch kind {
	case sniffOther:
		rec.class = classNotDicom
		return rec
	case sniffUnreadable:
		rec.class = classIoError
		rec.message = withoutPath(err.Error(), rec.path)
		return rec
	}

	f, err := os.Open(rec.path)
	if err != nil {
		rec.class = classIoError
		rec.message = withoutPath(err.Error(), rec.path)
		return rec
	}
	defer f.Close()

	// The library reads the file meta group itself for Part 10 files. For a raw
	// dataset it is told to skip that step; it then infers the transfer syntax from
	// the first element (implicit VR little endian is its default).
	opts := []dicom.ParseOption{dicom.SkipPixelData(), dicom.AllowMissingMetaElementGroupLength()}
	if kind == sniffRaw {
		opts = append(opts, dicom.SkipMetadataReadOnNewParserInit())
	}
	p, err := dicom.NewParser(f, rec.size, nil, opts...)
	if err != nil {
		rec.class, rec.message = classify(err, rec.path)
		return rec
	}
	rec.ts = "raw"
	if kind == sniffPart10 {
		meta := p.GetMetadata()
		if e, err := meta.FindElementByTag(tag.TransferSyntaxUID); err == nil {
			rec.ts = strings.TrimRight(firstString(e), "\x00")
		}
	}

	// Elements one by one until the pixel data (which the library reads past and
	// discards: it has no way to stop in front of it) or the end of the file.
	var ds dicom.Dataset
	for {
		e, err := p.Next()
		if err != nil {
			if errors.Is(err, dicom.ErrorEndOfDICOM) || errors.Is(err, io.EOF) {
				break
			}
			rec.class, rec.message = classify(err, rec.path)
			return rec
		}
		ds.Elements = append(ds.Elements, e)
		if e.Tag == tag.PixelData {
			break
		}
	}

	if _, err := ds.FindElementByTag(tag.SOPInstanceUID); err != nil {
		rec.class = classMissingSop
		return rec
	}
	if kind == sniffRaw {
		rec.class = classOkRaw
	} else {
		rec.class = classOk
	}
	rec.values = make([]string, len(tags))
	for i, t := range tags {
		if e, err := ds.FindElementByTag(t.tag); err == nil {
			rec.values[i] = valueString(e)
		}
	}
	return rec
}

func firstString(e *dicom.Element) string {
	if e == nil || e.Value == nil || e.Value.ValueType() != dicom.Strings {
		return ""
	}
	v := e.Value.GetValue().([]string)
	if len(v) == 0 {
		return ""
	}
	return v[0]
}

// One string per element, multiple values joined with a backslash as in the file.
func valueString(e *dicom.Element) string {
	if e == nil || e.Value == nil {
		return ""
	}
	switch v := e.Value.GetValue().(type) {
	case []string:
		parts := make([]string, len(v))
		for i, s := range v {
			parts[i] = strings.TrimRight(s, " \x00")
		}
		return strings.Join(parts, "\\")
	case []int:
		parts := make([]string, len(v))
		for i, x := range v {
			parts[i] = strconv.Itoa(x)
		}
		return strings.Join(parts, "\\")
	case []float64:
		parts := make([]string, len(v))
		for i, x := range v {
			parts[i] = strconv.FormatFloat(x, 'g', -1, 64)
		}
		return strings.Join(parts, "\\")
	default:
		return ""
	}
}

// Class from the error chain and its text; the library wraps with %w, so io errors
// stay reachable.
func classify(err error, path string) (string, string) {
	msg := withoutPath(err.Error(), path)
	var pathErr *fs.PathError
	switch {
	case errors.As(err, &pathErr) && !errors.Is(err, io.ErrUnexpectedEOF) && !errors.Is(err, io.EOF):
		return classIoError, msg
	case errors.Is(err, io.ErrUnexpectedEOF) || errors.Is(err, io.EOF) || strings.Contains(msg, "EOF"):
		return classTruncated, msg
	case strings.Contains(msg, "transfer syntax"):
		return classUnsupportedTs, msg
	default:
		return classParseError, msg
	}
}

// Messages never carry a path: the failure file has its own column for that.
func withoutPath(msg, path string) string {
	return strings.ReplaceAll(msg, path, "<path>")
}
