// Command bf-fuzz is a differential fuzzer: it feeds the same input to real
// blackfriday and to the Rust port and reports any byte that differs.
//
// Both sides are the genuine article. The Go side is
// github.com/russross/blackfriday/v2 v2.1.0 straight from the module proxy,
// unpatched, wrapped in cmd/goserve. The Rust side is bf-serve, the same
// binary the pinned test suite runs against. There is no shared code between
// them and no reimplementation of either — the only thing this program
// contributes is inputs, a comparison, and supervision.
//
// # Why both sides are subprocesses
//
// Some inputs make blackfriday loop forever, and the port reproduces that
// faithfully (BUGS.md #4 and #5). Neither language can interrupt a wedged
// computation from inside the same process, so an in-process fuzzer stops at
// the first such input. Behind a pipe, a hang is a finding: the supervisor
// kills the child, restarts it, records the input, and carries on. That is the
// difference between a fuzzer that reports two bugs and one that reports the
// first and dies.
//
// Usage:
//
//	go run . -duration 60s -seed 1
//
// Exit status is non-zero if any divergence is found. Hangs are reported but
// do not fail the run, because both implementations hanging together is
// agreement — it is upstream's bug, faithfully reproduced.
package main

import (
	"flag"
	"fmt"
	"math/rand"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	gobf "github.com/russross/blackfriday/v2"
)

func main() {
	duration := flag.Duration("duration", 60*time.Second, "how long to run")
	seed := flag.Int64("seed", 1, "PRNG seed, for reproducibility")
	logPath := flag.String("log", "", "where to write the run log")
	corpus := flag.String("corpus", "../tests/original/testdata", "seed corpus directory")
	limit := flag.Duration("limit", 5*time.Second, "per-input time limit before calling it a hang")
	flag.Parse()

	out := os.Stdout
	if *logPath != "" {
		f, err := os.Create(*logPath)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(2)
		}
		defer f.Close()
		out = f
	}

	logf := func(format string, args ...interface{}) {
		fmt.Fprintf(out, "[%s] ", time.Now().UTC().Format(time.RFC3339))
		fmt.Fprintf(out, format+"\n", args...)
		if out != os.Stdout {
			fmt.Printf(format+"\n", args...)
		}
	}

	goPath, err := serverPath("BF_GOSERVE", "goserve.exe")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	rsPath, err := serverPath("BF_SERVE", "../target/release/bf-serve.exe")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}

	goSide, err := newImpl("go", goPath, *limit)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	defer goSide.close()
	rsSide, err := newImpl("rust", rsPath, *limit)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	defer rsSide.close()

	seeds := loadCorpus(*corpus)
	logf("bf-fuzz starting: duration=%s seed=%d limit=%s corpus=%d files",
		*duration, *seed, *limit, len(seeds))
	logf("go side:   github.com/russross/blackfriday/v2 v2.1.0 (unpatched), via %s", goPath)
	logf("rust side: the port, via %s", rsPath)

	rng := rand.New(rand.NewSource(*seed))
	configs := configurations()
	deadline := time.Now().Add(*duration)

	var (
		iterations int
		divergent  int
		hangs      int
		bothHang   int
		errored    int
		byConfig   = map[string]int{}
		started    = time.Now()
		lastReport = time.Now()
	)

	for time.Now().Before(deadline) {
		input := generate(rng, seeds)
		cfg := configs[rng.Intn(len(configs))]
		req := buildRequest(input, cfg)

		wantOut, wantPanic, wantErr := goSide.render(req)
		gotOut, gotPanic, gotErr := rsSide.render(req)
		iterations++

		goHung := wantErr == errHang
		rsHung := gotErr == errHang

		switch {
		case goHung && rsHung:
			// Agreement: both implementations refuse to finish. Upstream's
			// bug, faithfully reproduced. Recorded, not counted as a defect.
			bothHang++
			logf("HANG on both sides config=%s\n  input: %q", cfg.name, input)

		case goHung != rsHung:
			hangs++
			divergent++
			byConfig[cfg.name]++
			side := "rust"
			if goHung {
				side = "go"
			}
			logf("DIVERGENCE (only %s hangs) config=%s\n  input: %q", side, cfg.name, input)

		case wantErr != nil || gotErr != nil:
			errored++
			logf("HARNESS ERROR config=%s go=%v rust=%v\n  input: %q",
				cfg.name, wantErr, gotErr, input)

		case wantPanic != "" && gotPanic != "":
			// Both refused the input. The port reproduces upstream's panics
			// deliberately, so this is agreement too.

		case wantPanic != "" || gotPanic != "":
			divergent++
			byConfig[cfg.name]++
			logf("DIVERGENCE (panic) config=%s\n  input:    %q\n  go panic: %s\n  rs panic: %s",
				cfg.name, input, orNone(wantPanic), orNone(gotPanic))

		case string(wantOut) != string(gotOut):
			divergent++
			byConfig[cfg.name]++
			logf("DIVERGENCE config=%s\n  input: %q\n  go:    %q\n  rust:  %q",
				cfg.name, input, wantOut, gotOut)
		}

		if time.Since(lastReport) > 15*time.Second {
			logf("progress: %d inputs, %d divergences, %d shared hangs, %.0f inputs/s",
				iterations, divergent, bothHang,
				float64(iterations)/time.Since(started).Seconds())
			lastReport = time.Now()
		}
	}

	elapsed := time.Since(started)
	logf("finished: %d inputs in %s (%.0f/s) across %d configurations",
		iterations, elapsed.Round(time.Millisecond),
		float64(iterations)/elapsed.Seconds(), len(configs))
	logf("shared hangs: %d (both implementations, see BUGS.md #4 and #5)", bothHang)
	if errored > 0 {
		logf("harness errors: %d", errored)
	}

	if divergent == 0 {
		logf("RESULT: 0 divergences")
		return
	}
	keys := make([]string, 0, len(byConfig))
	for k := range byConfig {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		logf("  %s: %d", k, byConfig[k])
	}
	logf("RESULT: %d divergences out of %d", divergent, iterations)
	os.Exit(1)
}

func orNone(s string) string {
	if s == "" {
		return "(none)"
	}
	return s
}

// config is one (extensions, renderer parameters) pair to test under.
type config struct {
	name   string
	ext    gobf.Extensions
	params gobf.HTMLRendererParameters
}

func configurations() []config {
	return []config{
		{"common", gobf.CommonExtensions, gobf.HTMLRendererParameters{Flags: gobf.CommonHTMLFlags}},
		{"none", gobf.NoExtensions, gobf.HTMLRendererParameters{}},
		{"all", gobf.CommonExtensions | gobf.Titleblock | gobf.DefinitionLists |
			gobf.Footnotes | gobf.HeadingIDs | gobf.AutoHeadingIDs | gobf.Tables |
			gobf.Strikethrough | gobf.BackslashLineBreak | gobf.SpaceHeadings |
			gobf.HardLineBreak | gobf.LaxHTMLBlocks | gobf.NoEmptyLineBeforeBlock |
			gobf.TabSizeEight | gobf.FencedCode | gobf.Autolink | gobf.NoIntraEmphasis,
			gobf.HTMLRendererParameters{Flags: gobf.CommonHTMLFlags}},
		{"smarty-all", gobf.CommonExtensions, gobf.HTMLRendererParameters{
			Flags: gobf.Smartypants | gobf.SmartypantsFractions | gobf.SmartypantsDashes |
				gobf.SmartypantsLatexDashes | gobf.SmartypantsAngledQuotes |
				gobf.SmartypantsQuotesNBSP}},
		{"skip", gobf.CommonExtensions, gobf.HTMLRendererParameters{
			Flags: gobf.SkipHTML | gobf.SkipImages | gobf.SkipLinks | gobf.Safelink}},
		{"attrs", gobf.CommonExtensions, gobf.HTMLRendererParameters{
			Flags:          gobf.NofollowLinks | gobf.NoreferrerLinks | gobf.NoopenerLinks | gobf.HrefTargetBlank,
			AbsolutePrefix: "http://host"}},
		{"footnotes", gobf.CommonExtensions | gobf.Footnotes, gobf.HTMLRendererParameters{
			Flags: gobf.CommonHTMLFlags | gobf.FootnoteReturnLinks, FootnoteAnchorPrefix: "fn-"}},
		{"toc", gobf.CommonExtensions | gobf.AutoHeadingIDs, gobf.HTMLRendererParameters{
			Flags: gobf.CommonHTMLFlags | gobf.TOC}},
		{"page", gobf.CommonExtensions, gobf.HTMLRendererParameters{
			Flags: gobf.CommonHTMLFlags | gobf.CompletePage, Title: "T", CSS: "a.css", Icon: "i.ico"}},
		{"headings", gobf.CommonExtensions | gobf.AutoHeadingIDs, gobf.HTMLRendererParameters{
			Flags: gobf.CommonHTMLFlags, HeadingIDPrefix: "p-", HeadingIDSuffix: "-s",
			HeadingLevelOffset: 2}},
	}
}

func loadCorpus(dir string) []string {
	var out []string
	files, _ := filepath.Glob(filepath.Join(dir, "*.text"))
	sort.Strings(files)
	for _, f := range files {
		b, err := os.ReadFile(f)
		if err == nil {
			out = append(out, string(b))
		}
	}
	return out
}

// fragments are Markdown constructs worth combining. Purely random bytes find
// very little in a Markdown parser -- almost everything is a paragraph -- so
// most inputs are assembled from these instead, and mutated afterwards.
var fragments = []string{
	"# ", "## ", "###### ", "####### ", "#", "=====", "-----", "***", "---", "___",
	"> ", ">> ", "- ", "* ", "+ ", "1. ", "10) ", "    ", "\t", "\n", "\n\n", "\r\n",
	"`", "``", "```", "```go", "~~~", "*a*", "**b**", "***c***", "~~d~~", "_e_", "__f__",
	"[t](u)", "[t][r]", "[t][]", "[t]", "![a](i)", "![a][r]", "[r]: http://x \"ti\"",
	"[^1]", "[^1]: note", "^[inline]", "%%title", "% t", "<div>", "</div>", "<b>", "</b>",
	"<!-- c -->", "<!--", "-->", "<http://x.com>", "<a@b.c>", "http://x.com", "https://y.z/a(b)",
	"mailto:a@b.c", "ftp://q", "&amp;", "&#38;", "&#x26;", "&notanentity;", "&", ";",
	"\\", "\\*", "\\\n", "|a|b|", "|---|---|", "|:-:|--:|", ": def", "term", "  \n",
	"'", "\"", "``q''", "--", "---", "...", ". . .", "1/2", "3/4", "(c)", "(tm)", "(r)",
	"a", "b ", " c", "text with words", "éèê", "\U0001F600", "\x00", "\x7f",
	"    indented code", "<script>", "</script>", "<?php ?>", "<![CDATA[x]]>", "<!DOCTYPE h>",
}

func generate(rng *rand.Rand, seeds []string) []byte {
	switch n := rng.Intn(100); {
	case n < 25 && len(seeds) > 0:
		// Mutate a real document.
		return mutate(rng, []byte(seeds[rng.Intn(len(seeds))]))
	case n < 90:
		// Assemble one from fragments.
		var b strings.Builder
		parts := 1 + rng.Intn(40)
		for i := 0; i < parts; i++ {
			b.WriteString(fragments[rng.Intn(len(fragments))])
		}
		if rng.Intn(3) == 0 {
			return mutate(rng, []byte(b.String()))
		}
		return []byte(b.String())
	default:
		// Occasionally, raw bytes -- including invalid UTF-8, which is the
		// point: both sides handle documents as bytes, not text.
		b := make([]byte, rng.Intn(200))
		rng.Read(b)
		return b
	}
}

func mutate(rng *rand.Rand, in []byte) []byte {
	out := append([]byte(nil), in...)
	edits := 1 + rng.Intn(8)
	for i := 0; i < edits && len(out) > 0; i++ {
		switch rng.Intn(4) {
		case 0: // flip a byte
			out[rng.Intn(len(out))] = byte(rng.Intn(256))
		case 1: // delete a run
			at := rng.Intn(len(out))
			end := at + rng.Intn(len(out)-at) + 1
			out = append(out[:at], out[end:]...)
		case 2: // splice in a fragment
			at := rng.Intn(len(out))
			frag := []byte(fragments[rng.Intn(len(fragments))])
			out = append(out[:at], append(frag, out[at:]...)...)
		case 3: // truncate
			out = out[:rng.Intn(len(out))]
		}
	}
	return out
}
