# genentities

Two scratch generators, both of which have to run *inside* the upstream package
because `entities`, `escapeHTML` and `escapeAllHTML` are all unexported. The
sources are stored with a `.txt` suffix so they do not compile as part of this
repo.

```bash
git -c core.autocrlf=false clone -b v2 https://github.com/russross/blackfriday.git /tmp/bf

# src/entities.rs
cp tools/genentities/gen_test.go.txt /tmp/bf/zz_entities_gen_test.go
cd /tmp/bf && go test -run TestZZGenEntities -v -count=1 .

# tests/fixtures/go-esc.txt
cp tools/genentities/gen_esc_test.go.txt /tmp/bf/zz_esc_gen_test.go
cd /tmp/bf && go test -run TestZZGenEsc -count=1 .
```

The clone is a throwaway. Nothing under `tests/original/` is touched.

## The table

2,231 entries, extracted upstream from
<https://html.spec.whatwg.org/multipage/entities.json>: **2,125** ending in `;`
and **106** bare forms such as `&amp` and `&GT`.

Go stores it as `map[string]bool` where every value is `true`, so membership is
all that is ever asked. The port uses a byte-sorted array with a binary search —
same information, no hashing, no dependency. Go's `sort.Strings` and Rust's
`Ord for str` are both byte-wise, so the ordering agrees across the two
languages and the search is valid.

## Two things worth knowing about the table

**Half of it is unreachable.** The only consumer is `nodeIsEntity`
(`esc.go:52`), which performs a lookup solely after seeing a `;`:

```go
if s[endEntityPos] == ';' {
    if entities[string(s[end:endEntityPos+1])] {
```

Every key it can construct therefore ends in `;`, and the 106 bare entries can
never match. They are kept anyway — the table is a faithful copy, and pruning
entries because today's call site cannot reach them would be a divergence
waiting to matter.

**Case is load-bearing, not cosmetic.** `&gt;` is U+003E `>` while `&Gt;` is
U+226B `≫`. Both are in the table and they are different characters. A
case-insensitive lookup would silently conflate them.

## Why the escaper fixture exists

`escapeEntities` is easy to port incorrectly because `start` and `end` advance
at different rates: `end` moves one byte at a time, but a recognised entity
jumps `start` past the entire entity, so `start` sits *ahead* of `end` for
several iterations afterwards.

`&s[start..end]` panics when `start > end`, so the loop's safety rests on a
matched entity never containing an escapable byte. That holds — every key is
`&`, then ASCII alphanumerics, then `;`, with no interior `&`, `<`, `>` or `"`
across all 2,231 entries — and the fixture covers the near-misses (`&&amp;`,
`&a&amp;`, `&&&`) that probe it.
