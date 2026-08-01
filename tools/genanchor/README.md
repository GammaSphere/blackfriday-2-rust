# genanchor

Generates `tests/fixtures/go-anchor.txt` — measured `SanitizedAnchorName`
output from real blackfriday, which `src/block.rs` asserts against.

```bash
cd tools/genanchor && go run . > ../../tests/fixtures/go-anchor.txt
```

`SanitizedAnchorName` is exported, so unlike `tools/genhelpers` this one can
link against the published `v2.1.0` module directly.

## The corpus

52 cases, chosen to land on the places where a naive port drifts:

| Group | Why |
|---|---|
| the 7 cases from the pinned `TestSanitizedAnchorName` | the suite's own expectations, restated as bytes |
| combining marks (U+0345, U+05B0, U+09BE, U+102B, U+0E31) | Other_Alphabetic — Rust's `is_alphabetic` keeps them, Go drops them |
| U+0130, U+00DF, U+1E9E | simple vs full case mapping |
| U+2160, U+00BD, U+2461 | Nl and No — numbers that are not digits |
| CJK, Cyrillic, Arabic, Hangul, emoji | multi-byte paths |
| leading/trailing/repeated separators | the `future_dash` state machine |
| malformed UTF-8 | Go yields one `U+FFFD` per bad byte; Rust's lossy conversion yields one per maximal invalid subsequence |

That last row is worth spelling out. The two produce a *different number* of
replacement characters for the same input, which sounds like a divergence and
is not: `U+FFFD` is General_Category So, so it is neither a letter nor a number
under either implementation, and its only effect is to set `future_dash`.
Whether it appears once or three times, the output is identical. The fixture
carries several malformed inputs so that argument stays tested rather than
merely asserted.

Output is hex-encoded on both sides so malformed input survives the round trip.
