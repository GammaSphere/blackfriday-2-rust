# Attribution

This repository is a Rust port of **blackfriday**, a Markdown processor written
in Go by Russ Ross.

- Upstream: <https://github.com/russross/blackfriday>
- Branch: `v2`
- Commit: `4c9bf9512682b995722660a4196c0013228e2049`
- License: Simplified BSD License (BSD-2-Clause)

The original test suite and the 46 golden fixture files under
`tests/original/` are copied verbatim from that commit and remain under the
upstream license. They are preserved unmodified so that the port can be
measured against them; see `tests/original/BASELINE.md` for the recorded
hashes.

The upstream license text is reproduced in `LICENSE-blackfriday.txt`.

Port code under `src/`, `ffi/`, `harness/`, `adapter/`, `examples/`, `tools/`,
`fuzz/`, `bench/`, and the port's own tests in `tests/*.rs` is original work
written for this port. Only `tests/original/` is upstream's.

## Go standard library

Blackfriday's `escLink` calls `html.UnescapeString`, so reproducing its
behaviour required porting that function and the entity tables it reads.

- `src/unescape.rs` is a structural port of `$GOROOT/src/html/escape.go`.
- `src/html_entities.rs` is generated from the `entity` and `entity2` maps in
  `$GOROOT/src/html/entity.go`.

Both derive from the Go standard library, which is distributed under the
BSD-3-Clause license, © The Go Authors. The generator that extracts them is in
`tools/genhtmlent/`; no Go source is vendored into this repository.

Note that this is a *port*, not a link: the shipped artifact contains no Go
code and does not depend on a Go toolchain to build or run.
