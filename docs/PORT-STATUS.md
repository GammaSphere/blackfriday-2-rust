# Port status and handoff

Working notes for the blackfriday → Rust port. Not part of the submission
deliverable; this exists so the work can be picked up cleanly in a new session.

| | |
|---|---|
| Repo | `E:\projects\raptors-v2` → `git@github.com:GammaSphere/blackfriday-2-rust.git` (`main`) |
| Upstream | `russross/blackfriday` `v2` @ `4c9bf9512682b995722660a4196c0013228e2049` (= tag **v2.1.0**) |
| Freeze | **2026-08-03 18:00 UTC** |
| Last commit | `00883c6` — hand-coded HTML tag matcher |
| Tests | 189 unit + 9 doctests, all green in debug **and** release |
| Dependencies | **zero** (`cargo tree` shows only the crate itself) |
| `unsafe` in `src/` | **0** (`#![forbid(unsafe_code)]` on the crate) |
| Kickoff hash | `dd277c901490fa6f0c57053b927784bb5b5257f9f3d1103fae777122591818a9` — unchanged since commit 02 |

## How to resume

1. Upstream reference checkout is needed for the generators. Recreate it with:
   ```bash
   git -c core.autocrlf=false clone -b v2 https://github.com/russross/blackfriday.git /tmp/bf
   ```
   `core.autocrlf=false` is not optional — see "CRLF" below.
2. `cargo` is not on `PATH`; prefix commands with
   `$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"`.
3. Pick the first unchecked item under "Remaining work".

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

## The method that has been working

Every module is checked against **measured Go output**, not against a reading of
the Go source. Generators live in `tools/`; several must run *inside* the
upstream package because the functions they measure are unexported (those are
stored as `gen_*.go.txt` and copied into a throwaway clone). Fixtures land in
`tests/fixtures/` and are hex-encoded, since much of this code handles bytes
that are not valid UTF-8.

This has been worth it: **the fixtures have corrected a wrong hand-written
expectation five separate times**, including twice in a single commit. Assume
your reading of the Go is wrong until measured.

## What is done

| Module | Contents |
|---|---|
| `src/flags.rs` | `Extensions`, `HtmlFlags`, `ListType`, `CellAlignFlags` |
| `src/node.rs` | arena AST + borrow-free `Walker` |
| `src/util.rs` | byte predicates, `slugify`, `is_indented` |
| `src/unicode_tables.rs` | generated Unicode tables (747 ranges + 1,407 lowercase mappings) |
| `src/entities.rs` | blackfriday's 2,231-entry entity table |
| `src/esc.rs` | `escape_html`, `escape_all_html`, `esc_link` |
| `src/html_entities.rs` | Go stdlib's entity tables (2,138 + 91 two-code-point) |
| `src/unescape.rs` | port of Go's `html.UnescapeString` |
| `src/markdown.rs` | `Renderer` trait, `Options`, `Markdown`, reference scanners |
| `src/block.rs` | **the entire block layer**, dispatcher included |
| `src/html.rs` | renderer config, helpers, tag matcher, `out`/`cr` |

## Remaining work

- [ ] **`render_node`** — `html.go:508-836` (328 lines), plus `render_header`,
      `render_footer` and `writeTOC` (`html.go:837-940`).
- [ ] **`src/smartypants.rs`** — `smartypants.go` (398 lines). `HtmlRenderer`
      needs an `sr: SPRenderer` field; `NewHTMLRenderer` constructs one.
- [ ] **`src/inline.rs`** — `inline.go` (1,049 lines). The largest module left.
      Note `inline_html_comment` is currently parked in `block.rs` and should
      move here when this lands.
- [ ] **`run()` / `run_with()`** — wire parse → render. Then **remove the
      `#![allow(dead_code)]` from `src/block.rs` and `src/html.rs`, and the
      `#[allow(dead_code)]` on `Markdown` in `src/markdown.rs`** — they are only
      there because nothing outside the crate can reach the internals yet.
- [ ] `ffi/` — `cdylib` exposing `bf_run`, `bf_free`, `bf_is_fence_line`,
      `bf_esc`. The **only** place `unsafe` is permitted.
- [ ] `adapter/` — Go package `blackfriday` with cgo wrappers matching
      upstream's exported API **plus `isFenceLine` and `esc`**, which the pinned
      suite reaches directly (they are the only two unexported names it uses).
- [ ] Drop the pinned `tests/original/*_test.go` in unmodified, run `make parity`,
      fix divergences, re-verify the kickoff hash.
- [ ] `PARITY.md` — per-test pass rate, honest failure list.
- [ ] `fuzz/` — differential harness, ≥60s continuous run, timestamped log.
- [ ] `bench/` — methodology + results (p99, RSS, startup, throughput).
- [ ] `DECISIONS.md` — ≥10 substantive entries (material is in "Decisions" below).
- [ ] Final `README.md` with measured results.
- [ ] **Five-minute demo video** — cannot be produced from here; needs the user.

## Two upstream bugs found, both with public-API reproducers

Written up in `BUGS.md`. **Neither has been filed upstream yet** — doing so
during the event is worth +3 (Bug Catcher).

1. **`reBackslashOrAmp` never matches a backslash** (`block.go:30`). The pattern
   `[\&]` is a character class containing only `&`, because a backslash inside
   an RE2 class escapes the next byte. A backslash escape is therefore expanded
   only when an unrelated `&` appears elsewhere in the same string.
   `Run("```\-go\ncode\n```\n")` gives `class="language-\-go"`.
2. **`titleBlock` emits a stray empty heading and loses the title**
   (`block.go:294`). When every line starts with `%`, the scan index never
   leaves zero, so nothing is consumed — but `addBlock` runs unconditionally.
   `Run("% a", Titleblock)` gives `<h1 class="title"></h1>` followed by
   `<p>% a</p>`. Needs input whose last line lacks a trailing newline.

A third unguarded index (`renderParagraph("   ")` panics) is documented as a
**latent hazard, not claimed as a bug** — no path to it from `Run` was found.

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
- **`cargo test --release` is not redundant.** The unescaper depends on wrapping
  `i32` overflow, which panics in debug and wraps in release.
- **Dead code is preserved deliberately** in several places — `dliPrefix`'s
  trailing loop never runs, `endsWithBlankLine` is stubbed to `false` behind
  upstream's own TODO (so `finalizeList` can never clear `tight`), and `html()`'s
  entire first search pass is commented out. All noted in module docs.

## Decisions, for `DECISIONS.md`

1. **v2 over v1.** v1's `Renderer` takes `out *bytes.Buffer` plus `func() bool`
   closures writing to the same buffer as the caller — aliasing that fights the
   borrow checker on every renderer method. v2 builds an AST and walks it.
2. **Arena AST.** Go's `Node` is a cyclic pointer graph. `Vec<Node>` +
   `NodeId(usize)` gives `Copy` handles with no `Rc`, no `RefCell`, no `unsafe`.
3. **Borrow-free `Walker`.** Upstream mutates the tree *during* traversal
   (`markdown.go:410`), which a closure holding `&Arena` cannot express.
4. **`Vec<u8>` instead of `io::Write`.** Upstream discards every write error, so
   the observable contract is "append bytes, never fail".
5. **`Options` builder instead of variadic functional options**, preserving
   last-writer-wins.
6. **`RefOverride` enum instead of `Option<Reference>`.** Go's
   `(*Reference, bool)` has three meanings; collapsing loses one.
7. **Unicode tables generated from Go**, not `char::is_alphabetic` — 11,171 code
   points differ, and embedding decouples the port from Rust's Unicode version.
8. **Hand-coded HTML tag matcher** rather than a regex dependency.
9. **Zero dependencies, zero `unsafe`** outside the planned `ffi/`.
10. **cgo adapter rather than translated tests**, keeping the pinned `_test.go`
    files byte-identical.
11. **Bugs reproduced, not fixed** — equivalence is the goal; tests pin the buggy
    behaviour so an upstream fix fails loudly.
12. **`expandTabs` not ported** — unreachable dead code in v2.
