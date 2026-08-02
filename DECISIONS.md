# Decisions

Every place where "what Rust would do" and "what blackfriday does" pulled in
different directions, and which one won.

The rule throughout: **blackfriday wins.** The goal is behavioural equivalence,
so a nicer design that changes output is a failure, not an improvement. Where
the language genuinely cannot express the original, the difference is recorded
here rather than smoothed over.

---

## 1. v2 over v1

Blackfriday v1's `Renderer` interface takes `out *bytes.Buffer` on every method
*and* `func() bool` closures that write to the same buffer the caller is
writing to. That is aliasing by design: two live paths to one buffer, decided
at runtime. Expressing it in Rust means `RefCell` at minimum, and fighting the
borrow checker on every single renderer method.

v2 builds a syntax tree and walks it. The renderer is handed one node at a time
and appends to a buffer it alone owns. That is a shape Rust can express
directly, so v2 is what was ported.

## 2. An arena instead of a pointer graph

Go's `Node` is a cyclic doubly-linked tree: `Parent`, `FirstChild`, `LastChild`,
`Prev`, `Next`. Every node points at its neighbours and its neighbours point
back.

In Rust that is `Rc<RefCell<Node>>` with `Weak` back-edges — reference counting
on every traversal step, runtime borrow checks, and a panic waiting in any code
path that holds one borrow while taking another. Upstream has several.

Instead `Arena` owns `Vec<Node>` and `NodeId(usize)` is a `Copy` handle. Links
are `Option<NodeId>` in both directions, so the cycle is fine — it is data, not
ownership. No `Rc`, no `RefCell`, no `unsafe`, and a node id can be held across
a mutation without the borrow checker objecting.

## 3. A borrow-free `Walker`

`Arena::walk` takes a closure and is the obvious API, but it cannot do the job:
upstream **mutates the tree during traversal.** `Parse` walks the document
appending inline children to each block as it visits them (`markdown.go:410`),
and `writeTOC` rewrites every heading's id mid-walk (`html.go:908`). A closure
holding `&Arena` cannot append to it.

`Walker` holds a `NodeId` and a direction flag and *no borrow at all*. The arena
is passed to `advance` for the duration of one step. Both mutating walks then
fall out naturally, and the closure form stays for the read-only cases.

## 4. `Vec<u8>` instead of `io::Write`

Every renderer method upstream takes an `io.Writer` and **discards the error**:
`io.WriteString(w, ...)` with no check, everywhere, without exception. The
observable contract is "append bytes, cannot fail".

Modelling that as `io::Write` would put a `Result` on every call that no
implementation can ever return `Err` from, and force callers to handle a
failure that does not exist. `&mut Vec<u8>` says the true thing.

## 5. A builder instead of variadic functional options

Go configures the parser with `Run(input, WithNoExtensions(), WithExtensions(x),
WithRenderer(r))`. `Option` is `func(*Markdown)` and `New` applies each in turn,
so later options silently overwrite earlier ones — upstream documents this as a
feature: "you can use any number of With\* arguments, even contradicting ones."

Rust has no variadics, so the shape cannot be transcribed. `Options` is a
builder whose `with_*` methods consume and return `Self`, which gives the same
last-writer-wins ordering from ordinary method chaining and puts every knob in
one type instead of scattering them across free functions.

## 6. `RefOverride` is an enum, not an `Option`

`ReferenceOverrideFunc` returns `(*Reference, bool)`, and all three reachable
combinations mean different things:

| Go | Meaning |
|---|---|
| `(nil, false)` | not overridden — run the normal lookup |
| `(&r, true)` | overridden to `r` |
| `(nil, true)` | overridden to **nothing** — the reference does not resolve |

`Option<Reference>` has two states and would merge the last two, quietly turning
"explicitly unresolvable" into "look it up anyway". `RefOverride` spells out all
three. The distinction is live: `TestReferenceOverride` in the pinned suite
exercises it, and it survives the round trip through the parity harness.

## 7. Unicode tables generated from Go, not `char::is_alphabetic`

`SanitizedAnchorName` keeps runes for which `unicode.IsLetter` or
`unicode.IsDigit` holds. The obvious port is `char::is_alphabetic() ||
char::is_numeric()`.

That is wrong for **11,171 code points**, measured. Go's `IsLetter` is
categories L\* only; Rust's `is_alphabetic` is `Alphabetic`, which additionally
includes `Other_Alphabetic` — combining marks, vowel signs, and more. `IsDigit`
is Nd; `is_numeric` is N\*, adding Nl and No.

So `src/unicode_tables.rs` is generated from Go's own tables: 747 ranges and
1,407 lowercase mappings, emitted by `tools/genunicode`. It also decouples the
port from whichever Unicode version the local Rust happens to ship, which is
what an equivalence claim needs.

## 8. Two regexps, hand-coded

Upstream compiles four patterns at package level. Pulling in `regex` for them
would be the single largest dependency in the project, for four call sites, and
would make behaviour depend on a crate rather than on code that can be read.

`htmlTagRe` and `reBackslashOrAmp` were hand-coded earlier; `anchorRe` and
`htmlEntityRe` in `src/inline.rs`. None needs backtracking once the structure
is examined:

- `htmlEntityRe`'s named branch takes as many letters as it can, then as many
  digits as it can. Giving a letter back leaves a letter where a digit or `;`
  must be; giving a digit back leaves a digit where `;` must be. A shorter
  match can never succeed where the greedy one failed.
- `anchorRe`'s URL character class excludes `"` and `<`, which are exactly the
  characters that terminate a URL in the pattern, so its `+` never has to give
  anything back. Its two *optional groups* do need "try it, then fall back",
  since that is what leftmost-first means, and they are written that way.

Both are checked against real `regexp` output rather than against that
reasoning.

## 9. Zero dependencies, zero `unsafe`

`Cargo.toml` has an empty `[dependencies]` and the crate carries
`#![forbid(unsafe_code)]`. `cargo tree` shows the crate and nothing else.

A `bitflags` dependency was the tempting one, and declined: Go's flag types are
plain `int`s, so `Extensions(1 << 30)` is legal and meaningful even though it
names nothing. `bitflags` normalises unknown bits by default; a `go_flags!`
macro of about sixty lines keeps `from_bits_retain` semantics and the exact
`&`/`|` behaviour, and makes bit-level tests against Go possible.

The only `unsafe` in the repository is in `ffi/`, which is not part of the
library.

## 10. cgo was the plan; a pipe is what shipped

The parity harness was going to be a cgo adapter over `ffi/`'s C ABI. It could
not be: this machine reports `CGO_ENABLED=0`, has no gcc, and hosts Rust on
MSVC — an MSVC `.lib` would have had to link into a MinGW build that is not
installed.

The adapter drives a Rust helper over a pipe instead. That costs about 24
seconds of process round-trips for the whole suite and buys a parity run
reproducible with only Go and Rust, on any platform, with no C toolchain. What
actually matters is untouched: the pinned suite is byte-identical to upstream's
and runs unmodified.

`ffi/` is still built and still the embedding path for callers who do have a C
compiler. It is simply not what the parity number rests on.

## 11. Bugs are reproduced, not fixed

Five defects were found in upstream while porting (`BUGS.md`), two of them
infinite loops reachable from `Run`. None is fixed here. Equivalence is the
goal, and a port that silently corrects its original is a port whose output you
cannot predict from the original's.

Each is pinned by a test that asserts the *buggy* behaviour, so if upstream ever
fixes one, the port's tests fail loudly instead of drifting apart quietly — with
the exception of the two hangs, since a test that never returns is not a test.
Those are recorded by the fuzzer as *shared* hangs instead: both implementations
stop, which is agreement.

The same reasoning covers the six inputs on which upstream **panics** —
`isHRule("")`, `isPrefixHeading("")` and friends. The port panics on exactly
those inputs, and tests assert it. Returning `false` instead would look like
robustness and would be a silent divergence.

## 12. Dead code is preserved on purpose

Several things upstream cannot reach are ported anyway, each with a note saying
so:

- `dliPrefix`'s trailing loop, which no input reaches.
- `endsWithBlankLine`, stubbed to `false` behind upstream's own TODO — which is
  why `finalizeList` can never clear `tight`.
- `html()`'s entire first search pass, commented out upstream.
- A cluster of five helpers in `html.go` — `isHTMLTag`, `isSmartypantable`,
  `findHTMLTagPos`, `skipUntilCharIgnoreQuotes`, `skipSpace` — with no callers
  anywhere in v2 outside its own tests. `isSmartypantable` looks like a v1
  leftover: v2 decides smartypants from the renderer's flags at the `Text` arm
  rather than asking the node.
- The `Softbreak` node type, which is declared, named in `String()` and handled
  in `RenderNode`, but which **nothing in blackfriday v2 ever constructs**.

Deleting any of them would make the next diff against upstream harder to read,
and a future upstream change could wake them up.

## 13. `expandTabs` is not ported

Genuinely unreachable in v2 — no call site, not even a dead one. It is the only
function deliberately left out, and it is named here so the omission is a
decision on the record rather than something overlooked.

## 14. `title` is `Option<Vec<u8>>`, because nil is not empty

`LinkData.Title` is a `[]byte` that stays nil until something assigns it, and
the renderer's `Image` arm tests it **against nil**, not against its length:

```go
if node.LinkData.Title != nil {
    r.out(w, []byte(`" title="`))
```

An absent title emits nothing; an empty one emits `title=""`. Modelling it as
`Vec<u8>` collapses the two and puts `title=""` on every inline image — which
is exactly what happened, and what the end-to-end comparison caught: 40 of 1,449
cases. No unit test would have found it.

## 15. Attributes are bytes, not `String`

The HTML renderer builds `attrs []string` and joins them. Transcribing that as
`Vec<String>` compiles, passes 1,449 end-to-end cases, and is wrong: an
attribute can carry a link, a title, or a code-fence info string lifted straight
out of the document, and a document is bytes. `String::from_utf8_lossy` rewrites
anything invalid as U+FFFD; Go's `string([]byte)` conversion is a
reinterpretation and preserves it.

The differential fuzzer found this within seconds of first running, on inputs no
hand-written corpus contained. `attrs` is `Vec<Vec<u8>>`, `tag` joins bytes, and
`HeadingData.heading_id` became `Vec<u8>` for the same reason — a `{#id}` is
copied out of the document too.

## 16. `render_header` takes `&mut Arena`

`writeTOC` rewrites every heading's id to `toc_N` before the body pass reads
them, so the mutation cannot be deferred and cannot be hidden. Go passes a
pointer and says nothing about it; here it has to be in the signature, which at
least makes the surprising part visible.

## 17. The reference table is the one `Rc<RefCell<…>>`

Decision 2 avoided `Rc<RefCell<…>>` for the syntax tree because nothing there
needed it. The reference table does need it, and finding out cost a bug.

`p.refs` is `map[string]*reference` and `p.notes` is `[]*reference`, and the
aliasing between them is **observable**. Referencing one footnote id twice
queues the same pointer twice, so when `link` assigns `ref.footnote` on the
second pass it changes what the first queue entry points at. `parseRefsToAST`
then attaches one `Item` node twice and appends the body to it twice:

```html
<li id="fn:1">the notethe note</li>
```

The port queued *snapshots*, which cannot express that, and produced two `<li>`
elements instead. Snapshots also cannot express a second definition of an id
replacing the table entry while earlier queue entries keep the old one.

So `RefHandle = Rc<RefCell<InternalReference>>`, and `parse_refs_to_ast` reads
through the handle at the moment of use rather than from a copy taken when the
entry was queued — which is what dereferencing a pointer does.

The differential fuzzer found this. No unit test would have: it needs one id
referenced twice *and* a comparison against Go, and the shape looks correct
from either side alone.

## 18. The fuzzer supervises both implementations as subprocesses

The obvious differential fuzzer calls both implementations in-process. That
works until an input makes one of them loop forever — and two such inputs exist
(`BUGS.md` #4 and #5), both reproduced faithfully by the port.

Neither language can interrupt a wedged computation from inside the same
process. An in-process fuzzer therefore stops at the first hang, which is
exactly what happened: three separate runs ended at their first hanging input,
having found nothing else.

Filtering the inputs was tried and abandoned. A filter tight enough to be useful
was not sound — the 204-byte input that defeated it shrank to `[^1]
[^1]:[^1]`,
where the self-reference is a footnote *body*, not a line — and a filter loose
enough to be sound skipped most multi-footnote documents.

So both sides run behind the same pipe protocol: `bf-serve` for Rust,
`cmd/goserve` for Go. A hang becomes a finding. The supervisor kills the child,
restarts it, records the input, and carries on. That is the difference between a
fuzzer that reports two upstream denial-of-service bugs and one that reports the
first and dies — and it is why the run behind the numbers in `README.md` covers
804,261 inputs rather than a few hundred.

## 19. Method, not architecture: measure, do not read

Not a code decision, but the one that shaped every other. Every module is
checked against **measured Go output**, never against a reading of the Go
source. Generators live in `tools/`; several must run inside the upstream
package because the functions they measure are unexported. Fixtures are
hex-encoded, since much of this code handles bytes that are not valid UTF-8.

The fixtures corrected a wrong hand-written expectation **five separate times**,
including twice in a single commit. Two of the three upstream bugs were found
this way rather than by inspection. The habit is worth more than any of the
decisions above.
