// Command repro runs one input through one implementation and exits.
//
// It exists so a candidate reproducer can be run under an external timeout: a
// Go program cannot kill its own wedged goroutine, so "does this input finish"
// has to be answered by a process that either exits or gets killed.
//
// Usage:
//
//	repro -side go   -ext footnotes < input
//	repro -side rust -ext footnotes < input
package main

import (
	"flag"
	"fmt"
	"io"
	"os"

	rust "github.com/GammaSphere/blackfriday-2-rust/adapter"
	gobf "github.com/russross/blackfriday/v2"
)

func main() {
	side := flag.String("side", "go", "go or rust")
	ext := flag.String("ext", "footnotes", "common, footnotes, or all")
	literal := flag.String("input", "", "input as a Go-quoted string; stdin if empty")
	show := flag.Bool("show", false, "print the rendered output instead of a summary")
	toc := flag.Bool("toc", false, "enable the TOC renderer flag and AutoHeadingIDs")
	file := flag.String("file", "", "read the input from this path instead of stdin")
	flag.Parse()

	var input []byte
	if *file != "" {
		// A path rather than stdin, so the same demo runs in PowerShell,
		// which reserves '<' and re-encodes anything piped through it.
		b, err := os.ReadFile(*file)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(2)
		}
		input = b
	} else if *literal != "" {
		unquoted, err := unquote(*literal)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(2)
		}
		input = unquoted
	} else {
		b, err := io.ReadAll(os.Stdin)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(2)
		}
		input = b
	}

	var goExt gobf.Extensions
	switch *ext {
	case "common":
		goExt = gobf.CommonExtensions
	case "footnotes":
		goExt = gobf.CommonExtensions | gobf.Footnotes
	case "all":
		goExt = gobf.CommonExtensions | gobf.Footnotes | gobf.Titleblock |
			gobf.DefinitionLists | gobf.AutoHeadingIDs | gobf.Tables
	default:
		fmt.Fprintln(os.Stderr, "unknown -ext")
		os.Exit(2)
	}

	goFlags := gobf.CommonHTMLFlags
	if *toc {
		goFlags |= gobf.TOC
		goExt |= gobf.AutoHeadingIDs
	}

	var out []byte
	switch *side {
	case "go":
		r := gobf.NewHTMLRenderer(gobf.HTMLRendererParameters{Flags: goFlags})
		out = gobf.Run(input, gobf.WithRenderer(r), gobf.WithExtensions(goExt))
	case "rust":
		r := rust.NewHTMLRenderer(rust.HTMLRendererParameters{Flags: rust.HTMLFlags(goFlags)})
		out = rust.Run(input, rust.WithRenderer(r), rust.WithExtensions(rust.Extensions(goExt)))
	default:
		fmt.Fprintln(os.Stderr, "unknown -side")
		os.Exit(2)
	}

	if *show {
		os.Stdout.Write(out)
		return
	}
	fmt.Printf("OK side=%s in=%d out=%d\n", *side, len(input), len(out))
}

// unquote accepts a Go-quoted string so a reproducer can be pasted from a log.
func unquote(s string) ([]byte, error) {
	if len(s) >= 2 && s[0] == '"' {
		var v string
		if _, err := fmt.Sscanf(s, "%q", &v); err != nil {
			return nil, err
		}
		return []byte(v), nil
	}
	return []byte(s), nil
}
