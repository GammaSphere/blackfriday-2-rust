# Kickoff baseline — original blackfriday suite

Everything in this directory is copied **verbatim** from upstream. It is the
measuring stick for the port, so it does not get edited. If a divergence shows
up later, the port changes, not these files.

## Provenance

| | |
|---|---|
| Repository | <https://github.com/russross/blackfriday> |
| Branch | `v2` |
| Commit | `4c9bf9512682b995722660a4196c0013228e2049` |
| Tag | **`v2.1.0`** — the pinned commit is exactly this release |
| Commit date | 2020-10-26 23:47:54 -0400 |
| License | BSD-2-Clause |
| Files vendored | 6 `_test.go` + 46 `testdata/` fixtures = **52** |

Verified byte-identical to upstream at vendoring time: 0 mismatches across all
52 files.

## Integrity

Per-file SHA-256 digests are in `SHA256SUMS` (coreutils format, LF-terminated).

**Manifest digest — SHA-256 of `SHA256SUMS` itself:**

```text
dd277c901490fa6f0c57053b927784bb5b5257f9f3d1103fae777122591818a9
```

Re-verify from a clean clone:

```bash
cd tests/original && sha256sum -c SHA256SUMS && sha256sum SHA256SUMS
```

Any change to that manifest digest means the original suite was touched, and
that would need disclosing in `DECISIONS.md`. The intent is that it never
changes.

## Recorded upstream result

Run against upstream itself, on a clean checkout, before any port code existed.

| | |
|---|---|
| Toolchain | `go1.23.5 windows/amd64` |
| Command | `go test -v -count=1 ./...` |
| Exit code | `0` |
| Test functions | **65 passed, 0 failed, 0 skipped** |
| Wall time | 10.712s |

```text
ok  github.com/russross/blackfriday/v2  10.712s
```

This 65/65 is the denominator for every parity number this port reports.

## Reproducing the baseline — read this first

A default clone on Windows **fails this suite against upstream itself**:

```text
Expected["<blockquote>\r\n<p>A list within a blockquote:</p>\r\n..."]
Actual  ["<blockquote>\n<p>A list within a blockquote:</p>\n..."]
```

That is not a blackfriday bug and not a port bug. `core.autocrlf=true` — the
Windows default — rewrites the golden `.html` fixtures to CRLF on checkout,
while the renderer emits LF. The fixtures are byte-for-byte comparison targets,
so translating them invalidates the comparison.

This repository pins them with `-text` in `.gitattributes`, so a clean clone
here is correct on every platform. To reproduce the baseline directly from
upstream instead, disable the translation at clone time:

```bash
git -c core.autocrlf=false clone -b v2 https://github.com/russross/blackfriday.git
cd blackfriday && go test -count=1 ./...
```

## What the tests reach into

The suite is `package blackfriday` — internal tests — so it can use unexported
identifiers. That constrains the adapter. Audited against this commit, the
tests touch exactly two non-public names:

| Identifier | Defined | Used by |
|---|---|---|
| `isFenceLine` | `block.go:566` | `block_test.go:1864` (`TestIsFenceLine`) |
| `esc` | `esc.go` | `esc_test.go` |

Everything else goes through the public API: `Run`, `New`, `WithRenderer`,
`WithExtensions`, `WithNoExtensions`, `WithRefOverride`, `NewHTMLRenderer`,
`HTMLRendererParameters`, `Extensions`, `HTMLFlags`, `Node`, `Reference`,
`SanitizedAnchorName`.

So the Go adapter has to re-export those two internal names alongside the
public surface. That is the whole reason the adapter can host these test files
unmodified.
