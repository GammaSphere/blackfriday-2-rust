# genhelpers

Produces `tests/fixtures/go-helpers.txt`: measured Go answers for blackfriday's
byte-level helpers, which `src/util.rs` asserts against.

## Running it

`ispunct`, `isspace`, `slugify` and `isIndented` are unexported, so the
generator has to run *inside* the package. `gen_test.go.txt` is the source; it
is stored with a `.txt` suffix so it does not compile as part of this repo.

```bash
git -c core.autocrlf=false clone -b v2 https://github.com/russross/blackfriday.git /tmp/bf
cp tools/genhelpers/gen_test.go.txt /tmp/bf/zz_fixture_gen_test.go
cd /tmp/bf && go test -run TestZZGenFixture -count=1 .
cp /tmp/bf/helpers-fixture.txt "$OLDPWD/tests/fixtures/go-helpers.txt"
```

The clone is a throwaway. Nothing under `tests/original/` is touched — that
directory is the pinned kickoff copy and its hashes must not move.

## Encoding

Byte strings are hex-encoded rather than `%q`-quoted. `slugify` operates on
bytes and happily returns output that is not valid UTF-8 (feed it `\xff\xfe`),
so Go quoting would be both lossy and awkward to parse back in Rust.

## What the fixture pins

| Prefix | Contents |
|---|---|
| `B` | all 256 byte values against `ispunct`, `isspace`, `ishorizontalspace`, `isverticalspace`, `isletter`, `isalnum` |
| `S` | a `slugify` corpus, including punctuation-only, high-byte and multi-byte UTF-8 inputs |
| `I` | `isIndented` across tab, space, short-line and zero-size cases |

The `S` cases exist because `slugify` has two behaviours that read as bugs and
are not:

- `slugify("!")` is `"-"`, not `""`. Go's trailing-trim loop is `for b = len(out)-1; b > 0; b--`,
  so index 0 is never trimmed.
- Output can never contain two adjacent dashes, because a run of symbols emits
  only one. That invariant is what stops the leading and trailing trims from
  crossing each other.

Note that `slugify` **preserves case**. It is not `SanitizedAnchorName`, which
lowercases and is Unicode-aware; the two are easy to confuse and are used for
different things.
