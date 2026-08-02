# blackfriday-rs

A Rust port of [blackfriday](https://github.com/russross/blackfriday) v2, the
Go Markdown processor by Russ Ross.

**Port Mortem 2026 — Track E (Go → Rust)**

| | |
|---|---|
| Source repository | <https://github.com/russross/blackfriday> |
| Source branch | `v2` |
| Source commit | `4c9bf9512682b995722660a4196c0013228e2049` (tag v2.1.0) |
| Source license | BSD-2-Clause |
| Source size | 7,263 Go LOC (5,028 logic + 2,235 generated entity table) |
| Original suite | 65 test functions, 2,744 LOC, 46 golden fixtures |

---

## Results

| | |
|---|---|
| **Original Go suite, unmodified** | **65 of 65 pass** — [PARITY.md](PARITY.md) |
| **Differential fuzzing** | **804,261 inputs, 0 divergences** — [docs/fuzz-run.log](docs/fuzz-run.log) |
| **Port's own tests** | 211 unit + 1,449 end-to-end + 1,449 renderer + 11 doctests |
| **Dependencies** | **zero** |
| **`unsafe` in the library** | **zero** (`#![forbid(unsafe_code)]`) |
| **Upstream bugs found** | **5**, two of them infinite loops — [BUGS.md](BUGS.md) |
| **Declared divergences** | **1**, and it is upstream reading past the end of a slice |
| Performance | throughput level, memory 2.5× lower, tail worse — [BENCHMARKS.md](BENCHMARKS.md) |

The suite in `tests/original/` is byte-identical to upstream's — `make
verify-hashes` recomputes every digest and the manifest's own, which still
matches the hash recorded before any porting began. Nothing here is claimed
that is not reproducible from a clean clone.

## Why blackfriday, and why Rust

Blackfriday is a self-contained Markdown processor: bytes in, bytes out, no
network, no clock, no filesystem, no concurrency in the hot path. That makes it
almost ideal for proving behavioural equivalence — every observable difference
between the original and the port is a real difference, not environmental noise.

It is also *pre-CommonMark*. Blackfriday implements Markdown.pl-descended
behaviour plus its own extension set, and those semantics do not match any
existing Rust Markdown crate: `pulldown-cmark` and `markdown` target CommonMark,
`comrak` is a port of C's cmark, and `markdown-it` is a port of the JavaScript
library. No Rust port of blackfriday exists, so this is a genuine migration
rather than a re-derivation of someone else's port — and differential testing
against it is meaningful rather than tautological.

## The method

Every module is checked against **measured Go output**, never against a reading
of the Go source. Generators live in `tools/`; several have to run *inside* the
upstream package because the functions they measure are unexported. Fixtures
are hex-encoded, since much of this code handles bytes that are not valid UTF-8.

It was worth it. The fixtures corrected a wrong hand-written expectation **five
separate times**, and three of the five upstream bugs below were found by
measuring rather than by reading. The two most interesting defects in the port
itself — an attribute path that mangled non-UTF-8 bytes, and a footnote queue
that lost Go's pointer aliasing — were found by the differential fuzzer on
inputs no hand-written corpus contained.

## Five bugs in blackfriday

Found while porting, written up with public-API reproducers in
[BUGS.md](BUGS.md). All five are **reproduced, not fixed**: equivalence is the
goal, and a port that silently corrects its original is a port whose output you
cannot predict from the original's.

1. **`reBackslashOrAmp` never matches a backslash.** `[\&]` inside an RE2
   character class contains only `&`.
2. **`titleBlock` emits a stray empty heading and loses the title** when every
   line starts with `%`.
3. **`smartLeftAngle` reads past the end of its input**, and `Run` panics on a
   `CompletePage|Smartypants` document title of 8, 16, 24 or 32 bytes ending in
   an unclosed `<`. Other lengths inject a NUL into `<title>`.
4. **A footnote cycle makes `Run` loop forever**, with unbounded allocation.
   Twenty-four bytes, needs the `Footnotes` extension.
5. **Eight bytes wedge `Run` with the default options.** `Run([]byte("\r\n\t+ \n: "))`
   never returns — a definition-list handoff in `paragraph` returns a count
   measured from the wrong origin, and that count can be zero.

Numbers 4 and 5 are denial-of-service bugs in the documented one-line way to
use the library. Both are reproduced faithfully by the port, which is why the
fuzzer reports them as *shared hangs* rather than divergences.

## The one declared divergence

`smartLeftAngle` writes `text[:i+1]` after a scan that can leave `i` at the end
of the slice. Go permits that up to `cap`, so upstream either emits a byte the
caller never wrote or panics, decided by what the allocator happened to leave
spare. This port writes the bytes it was given.

Reproducing upstream faithfully would mean reading uninitialised memory, which
the crate forbids and which no caller can depend on. It is recorded in
[PARITY.md](PARITY.md) and [BUGS.md](BUGS.md) rather than buried, because a
parity claim with a silent exception is not a parity claim.

## Prerequisites

- Rust 1.97+ (`rustup`)
- Go 1.21+ — **only** to run the original suite and the fuzzer. The shipped
  library links nothing from Go.

No C toolchain is required. The parity harness drives the port over a pipe
rather than through cgo; see [PARITY.md](PARITY.md) for why.

## Build

```bash
cargo build --release
```

## Use

```rust
let html = blackfriday::run(b"# Hello\n\nA *world*.\n");
assert_eq!(html, b"<h1>Hello</h1>\n\n<p>A <em>world</em>.</p>\n");
```

There is a CLI for trying things by hand:

```bash
cargo run --release --example render -- --footnotes --toc < doc.md
```

## Test

```bash
cargo test
```

The **original, unmodified** Go suite against the Rust port:

```bash
make parity
```

Differential fuzzing, both implementations supervised as child processes so a
hang is a finding rather than the end of the run:

```bash
cd fuzz && go build -o goserve.exe ./cmd/goserve && go build -o bf-fuzz.exe . && ./bf-fuzz.exe -duration 180s
```

## Layout

| path | what |
|---|---|
| `src/` | the library — zero dependencies, zero `unsafe` |
| `tests/original/` | upstream's suite, byte-identical, hash-pinned |
| `tests/fixtures/` | measured Go output, hex-encoded |
| `tools/` | the generators that produced those fixtures |
| `adapter/` | a Go package named `blackfriday`, implemented by the port |
| `harness/` | `bf-serve`, the port behind a pipe |
| `ffi/` | a C ABI — the only `unsafe` in the repository |
| `fuzz/` | the differential fuzzer and its two supervised children |
| `bench/` | the timing harnesses, one per language |
| `examples/` | `render`, a small CLI |

## A note on line endings

The upstream golden fixtures under `tests/original/testdata/` are byte-for-byte
comparison targets. If Git is allowed to translate their line endings on
checkout — the default on Windows, where `core.autocrlf=true` — the upstream
suite fails against *upstream itself*, with diffs like:

```text
Expected["<blockquote>\r\n<p>A list within a blockquote:</p>\r\n..."]
Actual  ["<blockquote>\n<p>A list within a blockquote:</p>\n..."]
```

That is a checkout artifact, not a defect in blackfriday or in this port. This
repository pins the fixtures with `-text` in `.gitattributes` so they survive
checkout unmodified on every platform.

## Further reading

- [PARITY.md](PARITY.md) — the parity run, per test, and the one divergence
- [BUGS.md](BUGS.md) — five upstream defects, with reproducers
- [BENCHMARKS.md](BENCHMARKS.md) — measurements, method, and what is slower
- [DECISIONS.md](DECISIONS.md) — every place Rust and blackfriday disagreed
- [docs/PORT-STATUS.md](docs/PORT-STATUS.md) — working notes

## License

Port code: see `LICENSE`. Vendored upstream tests and fixtures remain under
blackfriday's BSD-2-Clause license — see `NOTICE.md` and
`LICENSE-blackfriday.txt`.
