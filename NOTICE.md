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

Port code under `src/`, `ffi/`, `tests/port/`, `fuzz/`, and `bench/` is
original work written for this port.
