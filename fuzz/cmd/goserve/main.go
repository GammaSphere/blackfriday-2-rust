// Command goserve renders Markdown with real blackfriday, over the same pipe
// protocol bf-serve speaks.
//
// It exists so the fuzzer can supervise the Go side the way it supervises the
// Rust side. A Go program cannot interrupt its own wedged goroutine, so an
// input that makes blackfriday loop forever ends any fuzzer that calls it
// in-process — which is exactly what happened twice, and each time the run
// stopped at the first such input instead of continuing past it. Behind a pipe
// the supervisor can kill the process, restart it, record the hang, and carry
// on.
//
// The wire format is harness/src/main.rs's, so one client drives both sides.
package main

import (
	"encoding/binary"
	"fmt"
	"io"
	"os"

	bf "github.com/russross/blackfriday/v2"
)

const (
	opRun = 1

	statusResult = 0
	statusPanic  = 2
)

func main() {
	in := os.Stdin
	out := os.Stdout

	for {
		op, args, err := readFrame(in)
		if err == io.EOF || err == io.ErrUnexpectedEOF {
			return
		}
		if err != nil {
			fmt.Fprintln(os.Stderr, "goserve:", err)
			os.Exit(1)
		}
		if op != opRun {
			writeFrame(out, statusPanic, [][]byte{[]byte(fmt.Sprintf("unknown op %d", op))})
			continue
		}

		rendered, panicked := render(args)
		if panicked != "" {
			writeFrame(out, statusPanic, [][]byte{[]byte(panicked)})
			continue
		}
		writeFrame(out, statusResult, [][]byte{rendered})
	}
}

// render mirrors bf-serve's argument layout exactly, so the fuzzer builds one
// request and sends it to both.
func render(args [][]byte) (out []byte, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			panicked = fmt.Sprint(r)
		}
	}()

	params := bf.HTMLRendererParameters{
		Flags:                      bf.HTMLFlags(i32(args, 2)),
		HeadingLevelOffset:         int(i32(args, 3)),
		AbsolutePrefix:             str(args, 4),
		FootnoteAnchorPrefix:       str(args, 5),
		FootnoteReturnLinkContents: str(args, 6),
		HeadingIDPrefix:            str(args, 7),
		HeadingIDSuffix:            str(args, 8),
		Title:                      str(args, 9),
		CSS:                        str(args, 10),
		Icon:                       str(args, 11),
	}
	r := bf.NewHTMLRenderer(params)
	return bf.Run(arg(args, 0), bf.WithRenderer(r),
		bf.WithExtensions(bf.Extensions(i32(args, 1)))), ""
}

func arg(args [][]byte, n int) []byte {
	if n < len(args) {
		return args[n]
	}
	return nil
}

func i32(args [][]byte, n int) int32 {
	a := arg(args, n)
	if len(a) < 4 {
		return 0
	}
	return int32(binary.LittleEndian.Uint32(a))
}

func str(args [][]byte, n int) string { return string(arg(args, n)) }

func readFrame(r io.Reader) (byte, [][]byte, error) {
	var tag [1]byte
	if _, err := io.ReadFull(r, tag[:]); err != nil {
		return 0, nil, err
	}
	count, err := readU32(r)
	if err != nil {
		return 0, nil, err
	}
	vals := make([][]byte, count)
	for i := range vals {
		size, err := readU32(r)
		if err != nil {
			return 0, nil, err
		}
		buf := make([]byte, size)
		if _, err := io.ReadFull(r, buf); err != nil {
			return 0, nil, err
		}
		vals[i] = buf
	}
	return tag[0], vals, nil
}

func writeFrame(w io.Writer, status byte, vals [][]byte) {
	var n [4]byte
	buf := []byte{status}
	binary.LittleEndian.PutUint32(n[:], uint32(len(vals)))
	buf = append(buf, n[:]...)
	for _, v := range vals {
		binary.LittleEndian.PutUint32(n[:], uint32(len(v)))
		buf = append(buf, n[:]...)
		buf = append(buf, v...)
	}
	w.Write(buf)
}

func readU32(r io.Reader) (uint32, error) {
	var b [4]byte
	if _, err := io.ReadFull(r, b[:]); err != nil {
		return 0, err
	}
	return binary.LittleEndian.Uint32(b[:]), nil
}
