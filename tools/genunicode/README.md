# genunicode

Generates `src/unicode_tables.rs` — the code-point sets
`SanitizedAnchorName` depends on, read out of Go's own `unicode` package.

```bash
cd tools/genunicode && go run .
```

Writes 747 inclusive ranges and 1,407 lowercase mappings, from Go 1.23.5 /
Unicode 15.0.0.

## Why not just use Rust's std

Because it gives a different answer, and the port has to match Go.

**Letters.** Go tests `unicode.IsLetter(r) || unicode.IsNumber(r)`, which is
General_Category L or N. Rust's `char::is_alphabetic` is the *Alphabetic
property*, a superset that also covers Nl and Other_Alphabetic. Dumping both
predicates across the entire code space and diffing them gives:

| | |
|---|---|
| Code points where only Rust says yes | **11,171** |
| Code points where only Go says yes | 0 |

Go's set is a strict subset. The extras are mostly combining marks, and the
difference is directly observable in output:

```text
sanitized_anchor_name("a\u{345}b")
  Go   -> "a-b"      the mark is not a letter, so it becomes a separator
  std  -> "a\u{345}b"  the mark is Alphabetic, so it survives
```

**Lowercasing.** Go's `unicode.ToLower` is *simple* case mapping. Rust's
`char::to_lowercase` is *full* case mapping and returns an iterator that may
yield more than one char. Inside this set there is exactly one divergence:
U+0130 (LATIN CAPITAL LETTER I WITH DOT ABOVE) maps to `i` in Go and to
`i` + U+0307 in Rust.

**Version drift.** Embedding the tables also decouples the port from whichever
Unicode version a given Rust toolchain happens to ship. Without that, the same
source could produce different anchor names on different toolchains — a
reproducibility problem, not just a correctness one.

## Re-measuring

The 11,171 figure is a constant in `main.go` because Go cannot compute Rust's
answer. To re-derive it, dump both predicates and diff:

```go
// Go side
for r := rune(0); r <= 0x10FFFF; r++ {
    if unicode.IsLetter(r) || unicode.IsNumber(r) { fmt.Printf("%X\n", r) }
}
```

```rust
// Rust side
for cp in 0u32..=0x10FFFF {
    if let Some(c) = char::from_u32(cp) {
        if c.is_alphabetic() || c.is_numeric() { println!("{cp:X}") }
    }
}
```

Skip the surrogate range `D800..=DFFF` on the Go side — those are not scalar
values and Rust has no `char` for them.
