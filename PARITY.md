# Parity

**65 of 65 tests pass. Zero failures, zero skips.**

The suite is blackfriday v2.1.0's own, unmodified. Not adapted, not
transliterated, not "equivalent" — the same bytes upstream ships, compiled and
run against this port.

| | |
|---|---|
| Upstream | `russross/blackfriday` `v2` @ `4c9bf9512682b995722660a4196c0013228e2049` (tag **v2.1.0**) |
| Suite | `tests/original/` — 52 files, 6 `_test.go` plus `testdata/` |
| Kickoff hash | `dd277c901490fa6f0c57053b927784bb5b5257f9f3d1103fae777122591818a9` |
| Result | 65 run, **65 pass**, 0 fail |
| Run time | 24.7 s (dominated by the pipe, see below) |

Reproduce with:

```bash
make parity
```

`make verify-hashes` runs first and refuses to continue if a single byte of the
pinned suite has changed. The manifest digest above is the one recorded at
kickoff in `.port-mortem.toml`, before any porting began.

## What passed

```text
TestAutoLink                          TestPrefixHeaderIdExtension
TestBlockComments                     TestPrefixHeaderIdExtensionWithPrefixAndSuffix
TestCodeSpan                          TestPrefixHeaderLevelOffset
TestCompletePage                      TestPrefixHeaderNoExtensions
TestConsecutiveLists                  TestPrefixHeaderSpaceExtension
TestDefinitionList                    TestPrefixMultipleHeaderExtensions
TestDisableSmartDashes                TestPreformattedHtml
TestDocument                          TestPreformattedHtmlLax
TestEmphasis                          TestReference
TestEmphasisLink                      TestReferenceLink
TestEmphasisMix                       TestReferenceOverride
TestEsc                               TestReference_EXTENSION_NO_EMPTY_LINE_BEFORE_BLOCK
TestFencedCodeBlock                   TestRelAttrLink
TestFencedCodeBlock_EXTENSION_NO_EMPTY_LINE_BEFORE_BLOCK
TestFencedCodeInsideBlockquotes       TestSafeInlineLink
TestFootnotes                         TestSanitizedAnchorName
TestFootnotesWithParameters           TestSkipHTML
TestHorizontalRule                    TestSkipImages
TestHrefTargetBlank                   TestSkipLinks
TestInlineComments                    TestSmartAngledDoubleQuotes
TestInlineLink                        TestSmartAngledDoubleQuotesNBSP
TestIsFenceLine                       TestSmartDoubleQuotes
TestLineBreak                         TestSmartDoubleQuotesNBSP
TestListWithFencedCodeBlock           TestSmartFractions
TestListWithFencedCodeBlockNoExtensions
TestListWithMalformedFencedCodeBlock  TestStrikeThrough
TestNestedFootnotes                   TestStrong
TestOrderedList                       TestTOC
TestOrderedList_EXTENSION_NO_EMPTY_LINE_BEFORE_BLOCK
TestPrefixAutoHeaderIdExtension       TestTable
TestPrefixAutoHeaderIdExtensionWithPrefixAndSuffix
                                      TestTags
                                      TestTitleBlock_EXTENSION_TITLEBLOCK
                                      TestUnderlineHeaders
                                      TestUnderlineHeadersAutoIDs
                                      TestUnorderedList
                                      TestUnorderedListWith_EXTENSION_NO_EMPTY_LINE_BEFORE_BLOCK
                                      TestUseXHTML
```

`TestReference` alone replays Markdown 1.0.3's twenty-three reference documents
from `testdata/`, comparing whole rendered files.

## How the suite reaches Rust code

`adapter/blackfriday.go` is a Go package named `blackfriday` whose exported API
matches upstream's signature for signature. Its bodies marshal to a Rust helper
(`harness/src/main.rs`, built as `bf-serve`) over a pipe.

It also carries two **unexported** names, `escapeHTML` and `isFenceLine`,
because `esc_test.go` and `block_test.go` call them directly. Those two are the
only unexported identifiers the suite reaches — which is why the pinned files
need no edits at all.

`TestReferenceOverride` passes a Go closure as `ReferenceOverrideFunc`, so the
protocol has to call *back* into Go from the middle of a render. The helper
sends a `NEED_REF` frame, the adapter answers on the same pipe, and the render
resumes. Go's `(*Reference, bool)` has three meanings and all three survive the
round trip: not overridden, overridden to a reference, overridden to nothing.

### It was going to be cgo

The plan was a cgo adapter over `ffi/`'s C ABI. The machine this was built on
has `CGO_ENABLED=0`, no gcc, and an MSVC-hosted Rust toolchain — an MSVC `.lib`
would have had to link into a MinGW build that is not installed.

The pipe costs about 24 seconds of process round-trips for the whole suite, and
buys something worth more than that: reproducing this run needs only Go and
Rust, no C toolchain, on any platform. `ffi/` is still built and still the
embedding path for callers who do have a C compiler; it is simply not what the
parity number rests on.

## Divergences

**One, deliberate and declared.**

`smartLeftAngle` (`smartypants.go:369`) writes `text[:i+1]` after a scan that
can leave `i == len(text)`. Go permits slicing past `len` up to `cap`, so
upstream either emits a byte the caller never wrote or panics outright,
depending on what the allocator left spare. It is reachable from `Run` through
an unescaped document title — written up as bug 3 in `BUGS.md`.

This port writes the bytes it was given. Reproducing upstream faithfully would
mean reading uninitialised memory, which the crate forbids and which no caller
can depend on: the stray byte is zero only because Go zeroes fresh allocations,
and the panic threshold tracks Go's size classes rather than anything about the
input.

No test in the pinned suite reaches it, so this divergence costs nothing above.
It is recorded here because a parity claim with a silent exception is not a
parity claim.

**Three upstream bugs are reproduced rather than fixed**, since equivalence is
the goal — `reBackslashOrAmp`, `titleBlock`, and the `smartLeftAngle` behaviour
above insofar as it is observable. Each is pinned by a test, so an upstream fix
would fail loudly here instead of drifting. See `BUGS.md`.

## The port's own tests

Separate from the suite above, and green in both debug and release:

| Suite | Count | What it checks |
|---|---|---|
| Unit tests | 211 | Per-function, against fixtures measured from Go |
| `tests/end_to_end.rs` | 1,449 cases | `run_with` against Go's `Run`, whole documents |
| `tests/render_parity.rs` | 1,449 cases | The renderer alone, over trees Go's parser built |
| Doctests | 11 | The examples in the public documentation |

The two 1,449-case suites bracket the pipeline deliberately: `render_parity`
feeds the renderer a syntax tree dumped from Go, holding the parser constant,
so a failure in one but not the other says which half is wrong without
bisecting. The corpus is 69 documents — 46 written for this, 23 taken from
upstream's `testdata/` — across 21 renderer configurations.
