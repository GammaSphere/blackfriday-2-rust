// Command bf-bench-go times real blackfriday over the same corpus, the same
// number of times, reporting the same fields as bench/rust/main.go.
//
// It is deliberately the same program written twice rather than one program
// with two backends: any shared harness would sit on one side of the pipe and
// charge its cost to the other. The output format is identical so the two runs
// can be compared field by field.
//
// Peak memory and startup are measured from outside, by bench/run.ps1.
package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"time"

	bf "github.com/russross/blackfriday/v2"
)

var sink []byte

func main() {
	corpusDir := flag.String("corpus", "../../tests/original/testdata", "corpus directory")
	iterations := flag.Int("n", 200, "timed samples")
	batch := flag.Int("batch", 25, "corpus passes per timed sample")
	mode := flag.String("mode", "bench", "bench or startup")
	flag.Parse()

	if *mode == "startup" {
		sink = render([]byte("x"))
		if len(sink) == 0 {
			os.Exit(1)
		}
		return
	}

	corpus := loadCorpus(*corpusDir)
	if len(corpus) == 0 {
		fmt.Fprintf(os.Stderr, "no corpus found under %s\n", *corpusDir)
		os.Exit(2)
	}
	totalBytes := 0
	for _, d := range corpus {
		totalBytes += len(d)
	}

	for i := 0; i < 10; i++ {
		for _, doc := range corpus {
			sink = render(doc)
		}
	}

	// One sample times `batch` passes, not one; see the note in
	// bench/rust/main.rs. A single pass is under a millisecond and this
	// clock quantises to roughly that, which made every percentile here come
	// back as the same number.
	if *batch < 1 {
		*batch = 1
	}
	samples := make([]float64, 0, *iterations)
	wall := time.Now()
	for i := 0; i < *iterations; i++ {
		start := time.Now()
		for b := 0; b < *batch; b++ {
			for _, doc := range corpus {
				sink = render(doc)
			}
		}
		samples = append(samples, float64(time.Since(start).Nanoseconds())/1e6/float64(*batch))
	}
	elapsed := time.Since(wall).Seconds()

	sort.Float64s(samples)
	throughput := float64(totalBytes**iterations**batch) / elapsed / (1024 * 1024)

	fmt.Printf("impl=go\n")
	fmt.Printf("documents=%d\n", len(corpus))
	fmt.Printf("corpus_bytes=%d\n", totalBytes)
	fmt.Printf("iterations=%d\n", *iterations)
	fmt.Printf("batch=%d\n", *batch)
	fmt.Printf("min_ms=%.4f\n", samples[0])
	fmt.Printf("p50_ms=%.4f\n", percentile(samples, 0.50))
	fmt.Printf("p90_ms=%.4f\n", percentile(samples, 0.90))
	fmt.Printf("p99_ms=%.4f\n", percentile(samples, 0.99))
	fmt.Printf("max_ms=%.4f\n", samples[len(samples)-1])
	fmt.Printf("total_s=%.4f\n", elapsed)
	fmt.Printf("throughput_mib_s=%.2f\n", throughput)
}

func render(input []byte) []byte {
	r := bf.NewHTMLRenderer(bf.HTMLRendererParameters{Flags: bf.CommonHTMLFlags})
	return bf.Run(input, bf.WithRenderer(r), bf.WithExtensions(bf.CommonExtensions))
}

// percentile is nearest-rank over an already-sorted sample, matching the Rust
// side exactly -- there are several defensible definitions and a mismatch here
// would show up as a difference in the implementations.
func percentile(sorted []float64, q float64) float64 {
	if len(sorted) == 0 {
		return 0
	}
	rank := int(q*float64(len(sorted)) + 0.999999)
	if rank < 1 {
		rank = 1
	}
	if rank > len(sorted) {
		rank = len(sorted)
	}
	return sorted[rank-1]
}

func loadCorpus(dir string) [][]byte {
	files, _ := filepath.Glob(filepath.Join(dir, "*.text"))
	sort.Strings(files)
	var out [][]byte
	for _, f := range files {
		if b, err := os.ReadFile(f); err == nil {
			out = append(out, b)
		}
	}
	return out
}
