# dumpconst

Prints blackfriday v2's exported constant values by linking against the real Go
library, so the port's flag values can be checked against measured output rather
than against a reading of the source.

```bash
cd tools/dumpconst && go run .
```

## Why this exists

Upstream declares its flags with Go's `iota`, and the first line of each block
is a zero constant:

```go
const (
    NoExtensions    Extensions = 0
    NoIntraEmphasis Extensions = 1 << iota
    Tables
    ...
)
```

`iota` counts ConstSpec lines, and `NoExtensions` is line 0 — so
`NoIntraEmphasis` is `1 << 1 == 2`, not 1, and bit 0 is never used. The obvious
transcription (`1 << 0`, `1 << 1`, …) shifts every flag by one position and
produces a port that compiles, renders ordinary documents correctly, and gets
the wrong answer the moment a caller passes an explicit flag.

`ListType` and `CellAlignFlags` have no zero-valued first line and genuinely do
start at `1 << 0`. That inconsistency between the four blocks is what makes the
mistake easy to make in either direction.

The values this prints are asserted by `extension_values_match_go` and its
siblings in `src/flags.rs`.

## Pinning

`go.mod` requires `v2.1.0`, which is the same commit this port targets:

```text
$ git rev-list -n 1 v2.1.0
4c9bf9512682b995722660a4196c0013228e2049
```
