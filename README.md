# blackfriday-rs

A Rust port of [blackfriday](https://github.com/russross/blackfriday) v2, the
Go Markdown processor by Russ Ross.

**Port Mortem 2026 — Track E (Go → Rust)**

| | |
|---|---|
| Source repository | <https://github.com/russross/blackfriday> |
| Source branch | `v2` |
| Source commit | `4c9bf9512682b995722660a4196c0013228e2049` |
| Source license | BSD-2-Clause |
| Source size | 7,263 Go LOC (5,028 logic + 2,235 generated entity table) |
| Original suite | 65 test functions, 2,744 LOC, 46 golden fixtures |

---

## Status

🚧 **Port in progress.** This section is replaced with measured parity,
benchmark, and fuzzing results as the port lands. Nothing below is claimed
until it is reproducible from a clean clone.

- [ ] Original Go suite running against the Rust port via the cgo adapter
- [ ] Parity results, per test and per file
- [ ] Differential fuzz harness + 60s log
- [ ] Benchmark report (p99, RSS, startup, throughput)
- [ ] `unsafe` count
- [ ] `DECISIONS.md`

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

## Prerequisites

- Rust 1.97+ (`rustup`)
- Go 1.21+ — **only** to build the test adapter and run the original suite. The
  shipped library and CLI are pure Rust and link nothing from Go.

## Build

```bash
cargo build --release
```

## Test

```bash
cargo test
```

Running the **original, unmodified** Go suite against the Rust port:

```bash
make parity
```

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

## License

Port code: see `LICENSE`. Vendored upstream tests and fixtures remain under
blackfriday's BSD-2-Clause license — see `NOTICE.md` and
`LICENSE-blackfriday.txt`.
