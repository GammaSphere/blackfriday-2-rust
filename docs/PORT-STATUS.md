# Port status and handoff

Working notes for the blackfriday → Rust port. Not part of the submission
deliverable; this exists so the work can be picked up cleanly in a new session.

| | |
|---|---|
| Repo | `E:\projects\raptors-v2` → `git@github.com:GammaSphere/blackfriday-2-rust.git` (`main`) |
| Upstream | `russross/blackfriday` `v2` @ `4c9bf9512682b995722660a4196c0013228e2049` (= tag **v2.1.0**) |
| Freeze | **2026-08-03 18:00 UTC** |
| Tests | 211 unit + 1,449 end-to-end + 1,449 renderer + 11 doctests, green in debug **and** release |
| Parity | **65 of 65** of upstream's own suite |
| Fuzzing | **804,261 inputs, 0 divergences** |
| Dependencies | **zero** |
| `unsafe` in `src/` | **0** (`#![forbid(unsafe_code)]` on the crate) |
| Kickoff hash | `dd277c901490fa6f0c57053b927784bb5b5257f9f3d1103fae777122591818a9` — unchanged |

## The port is complete

Everything on the original checklist has landed. What remains is not code:

- [ ] **File the five upstream bugs** at `github.com/russross/blackfriday`.
      Written up in `BUGS.md` with public-API reproducers. **Needs the user** —
      filing an issue is an outward-facing action.
- [ ] **Five-minute demo video.** Cannot be produced from here.

## How to resume

1. `cargo` is not on `PATH`; prefix commands with
   `$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"`.
2. An upstream reference checkout is needed only to regenerate fixtures:
   ```bash
   git -c core.autocrlf=false clone -b v2 https://github.com/russross/blackfriday.git /tmp/bf
   ```
   `core.autocrlf=false` is not optional — see "CRLF" below.

### Verification gate

Run this before every commit. **Do not pipe cargo into `Select-String`** — that
makes `$LASTEXITCODE` report the pipeline rather than cargo, so a failing clippy
can look like a pass. This was a real defect in an earlier version of the loop.

```powershell
$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"; cd E:\projects\raptors-v2
cargo fmt --all *> $null
$fail=0
foreach($c in @('build','test','test --release')){ Invoke-Expression "cargo $c" *> $null; if($LASTEXITCODE -ne 0){"$c FAIL"; $fail=1} }
cargo fmt --all -- --check *> $null; if($LASTEXITCODE -ne 0){"fmt FAIL"; $fail=1}
cargo clippy --all-targets -- -D warnings *> $null; if($LASTEXITCODE -ne 0){"clippy FAIL"; $fail=1}
$doc = cargo doc --no-deps 2>&1 | Select-String -Pattern '^warning|^error'
if($doc){ $doc | ForEach-Object {"DOC: $_"}; $fail=1 }
if($fail -eq 0){"ALL GATES PASS"} else {"GATES FAILED"}
```

`cargo test --release` matters: the unescaper relies on wrapping integer
overflow, which differs between debug and release.

### Running the parity suite

`make` is not installed here. The target's body, spelled out:

```bash
cd /e/projects/raptors-v2
rm -rf target/parity && mkdir -p target/parity
cp adapter/go.mod adapter/blackfriday.go target/parity/
cp tests/original/*_test.go target/parity/
cp -r tests/original/testdata target/parity/
cd target/parity && BF_SERVE=../release/bf-serve.exe go test ./...
```

Takes about 26 seconds. Rebuild `bf-serve` first if `src/` changed:
`cargo build --release -p blackfriday-harness`.

### Running the fuzzer

```bash
cd /e/projects/raptors-v2/fuzz
go build -o goserve.exe ./cmd/goserve && go build -o bf-fuzz.exe .
./bf-fuzz.exe -duration 180s -seed 20260803 -limit 2s -log log.txt
```

Both implementations run as supervised children, so a hang is reported and the
run continues. Shared hangs are expected — they are `BUGS.md` #4 and #5.

`fuzz/repro.exe` (built from `./cmd/repro`) runs one input through one side;
combine it with `timeout` to check whether something hangs.

## The method that has been working

Every module is checked against **measured Go output**, not against a reading of
the Go source. Generators live in `tools/`; several must run *inside* the
upstream package because the functions they measure are unexported (those are
stored as `gen_*.go.txt` and copied into a throwaway clone). Fixtures land in
`tests/fixtures/` and are hex-encoded, since much of this code handles bytes
that are not valid UTF-8.

This has been worth it: **the fixtures corrected a wrong hand-written
expectation five separate times**, including twice in a single commit. Assume
your reading of the Go is wrong until measured.

The differential fuzzer earns its place separately. It found two real port bugs
that every other layer of testing missed, on inputs no hand-written corpus
would contain.

## Things that will bite whoever picks this up

- **CRLF.** A default clone on Windows makes the *upstream* suite fail against
  *itself*, because `core.autocrlf` rewrites the golden `.html` fixtures while
  the renderer emits LF. This repo pins them with `-text` in `.gitattributes`.
  Always clone upstream with `-c core.autocrlf=false`.
- **Go's `iota`.** `NoExtensions = 0` occupies iota 0, so `NoIntraEmphasis` is
  `1 << 1 == 2` and bit 0 is unused. Same for `HTMLFlags`. But `ListType` and
  `CellAlignFlags` genuinely start at `1 << 0`. The four blocks disagree.
- **Upstream panics on some inputs, and the port matches.** Six measured cases
  (`isHRule` on `""`/`" "`/`"  "`/`"   "`, `isPrefixHeading("")`,
  `isUnderlinedHeading("")`). Tests assert the panics; returning `false` instead
  would be a silent divergence.
- **Upstream hangs on some inputs, and the port matches.** `BUGS.md` #4 and #5.
  Do not "fix" either without deciding to diverge deliberately.
- **`cargo test --release` is not redundant.** The unescaper depends on wrapping
  `i32` overflow, which panics in debug and wraps in release.
- **Bytes, not `String`.** Anything lifted out of the document is `Vec<u8>`.
  `String::from_utf8_lossy` rewrites invalid bytes as U+FFFD and Go's
  `string([]byte)` does not; that was a real bug in attributes and heading ids.
- **Reference identity matters.** `p.refs` and `p.notes` share `*reference` in
  Go, and the aliasing is observable. `RefHandle` is `Rc<RefCell<…>>` for that
  reason; see `DECISIONS.md` #17.
- **Dead code is preserved deliberately** in several places — `dliPrefix`'s
  trailing loop never runs, `endsWithBlankLine` is stubbed to `false` behind
  upstream's own TODO, `html()`'s entire first search pass is commented out, a
  cluster of five helpers in `html.go` has no callers at all, and `Softbreak` is
  never constructed anywhere in v2. All noted in module docs.

## Where things are written down

| document | contents |
|---|---|
| `README.md` | the headline results and how to reproduce them |
| `PARITY.md` | the parity run, per test, and the one declared divergence |
| `BUGS.md` | five upstream defects with public-API reproducers |
| `BENCHMARKS.md` | measurements, method, and an honest account of what is slower |
| `DECISIONS.md` | 19 entries: every place Rust and blackfriday disagreed |
| `fuzz/log.txt` | the timestamped fuzz run behind the README's number |
