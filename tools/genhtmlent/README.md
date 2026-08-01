# genhtmlent

Extracts Go's standard-library HTML entity tables into `src/html_entities.rs`,
and emits `tests/fixtures/go-unescape.txt` — measured `html.UnescapeString`
output that `src/unescape.rs` asserts against.

```bash
cp "$(go env GOROOT)/src/html/entity.go" .
sed -i 's/^package html$/package main/' entity.go
go run .
```

`entity.go` imports only `sync` and is otherwise self-contained, so repackaging
it verbatim is enough to reach the maps. They are unexported and lazily
populated by `populateMaps`, which is why they cannot simply be imported.

The copied `entity.go` is a build input, not a committed artifact — only the
generated Rust table is checked in.

## Why blackfriday needs Go's table as well as its own

`escLink` (`esc.go:67`) runs link text through `html.UnescapeString` before
escaping it. Matching that means matching Go's standard library, and its table
is a different object from blackfriday's:

| | `src/entities.rs` | `src/html_entities.rs` |
|---|---|---|
| Source | blackfriday `entities.go` | Go stdlib `html/entity.go` |
| Keys | include leading `&` | do not |
| Values | `bool` | decoded code point(s) |
| Entries | 2,231 | 2,138 + 91 two-code-point |

## Behaviour that is easy to get wrong

Everything below was measured, and two of them contradicted what I first wrote
by hand:

- **`&#x;` decodes to U+FFFD, not the literal text.** It is four bytes, so it
  clears Go's `len(s) <= 3` guard; after consuming `x` and `;` the cursor sits
  at 4, clearing the `i <= 3` "no digits matched" check too. The accumulated
  value is 0, and 0 maps to U+FFFD. `&#;` is three bytes and *does* stay
  literal.
- **`&notarealentity;` is not left alone.** The longest-match fallback walks
  back through shorter prefixes, finds `not`, and yields `¬arealentity;`.
  Blackfriday's own `escape_html` has no such rule and leaves the same input
  untouched — the two tables must not be confused.
- **`0x80..=0x9F` are remapped for Windows-1252 compatibility.** `&#128;` is
  `€` (U+20AC), not U+0080.
- **106 named entities are valid without a semicolon**, and Go caps the
  fallback search at `longestEntityWithoutSemicolon = 6`. A test re-derives
  that 6 from the table so a regeneration against a newer Go cannot silently
  invalidate it.
- **Numeric overflow is silent.** Go accumulates into an `int32` and wraps, so
  `&#99999999999;` overflows to a negative value, fails every range check, and
  reaches `EncodeRune`, which emits U+FFFD. Rust panics on overflow in debug
  builds, so the port uses `wrapping_mul`/`wrapping_add` deliberately.

## Licensing

The tables are derived from Go's standard library, BSD-3-Clause, © The Go
Authors. See `NOTICE.md`.
