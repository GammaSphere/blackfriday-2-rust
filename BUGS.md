# Upstream bugs found while porting

Defects in blackfriday v2.1.0 (`4c9bf95`) discovered by porting it. The port
**reproduces each of these deliberately** — behavioural equivalence is the goal,
so a divergence would be a failure even when the upstream behaviour is wrong.
Each is covered by a test that pins the buggy output, so that if upstream ever
fixes it the port's tests fail loudly rather than drifting.

---

## 1. `reBackslashOrAmp` never matches a backslash

**Severity:** low — wrong output, no crash or unsoundness.
**Status:** confirmed, minimal reproducer through the public API.
**Location:** `block.go:30`.

### The defect

```go
reBackslashOrAmp = regexp.MustCompile("[\\&]")
```

The Go string literal `"[\\&]"` is the pattern `[\&]`. Inside an RE2 character
class a backslash escapes the following byte, so this class contains exactly one
member: `&`. It does not match a backslash, despite the variable's name. The
intent was plainly `"[\\\\&]"`, giving the pattern `[\\&]`.

Measured:

```text
reBackslashOrAmp.String() == "[\\&]"
  Match("\\") = false     <-- the bug
  Match("&")  = true
  Match("\\#") = false
```

### Why it matters

The regexp is used as a fast-path guard:

```go
func unescapeString(str []byte) []byte {
	if reBackslashOrAmp.Match(str) {
		return reEntityOrEscapedChar.ReplaceAllFunc(str, unescapeChar)
	}
	return str
}
```

`reEntityOrEscapedChar` handles both entities *and* backslash escapes, but a
string containing only a backslash escape never reaches it. So whether an escape
is processed depends on whether an unrelated `&` happens to appear elsewhere in
the same string:

```text
unescapeString("\-go")      == "\-go"    escape left alone
unescapeString("\-go&amp;x") == "-go&x"  same escape, now processed
```

### Reproducer (public API)

`unescapeString` is reached from `finalizeCodeBlock`, so this surfaces in the
info string of a fenced code block:

```go
blackfriday.Run([]byte("```\\-go\ncode\n```\n"), blackfriday.WithExtensions(blackfriday.FencedCode))
// <pre><code class="language-\-go">code
// </code></pre>

blackfriday.Run([]byte("```\\-go&amp;x\ncode\n```\n"), blackfriday.WithExtensions(blackfriday.FencedCode))
// <pre><code class="language--go&x">code
// </code></pre>
```

The first should have produced `language--go`. The generator that measured this
is `tools/genblock/repro_test.go.txt`.

### Impact

Confined to code-fence info strings, which is the only caller of
`unescapeString`. A language tag containing a backslash escape keeps its
backslash and lands in the rendered `class` attribute. No crash, no injection —
the value still goes through `escapeHTML` downstream.

### In this port

`src/block.rs::unescape_string` reproduces the guard exactly, including the
class that contains only `&`. `unescape_string_matches_go_including_the_backslash_bug`
pins both behaviours.

---

## 2. `titleBlock` emits a stray empty heading and loses the title

**Severity:** medium — visibly wrong output through the public API.
**Status:** confirmed, minimal reproducer.
**Location:** `block.go:294`.

### The defect

```go
splitData := bytes.Split(data, []byte("\n"))
var i int
for idx, b := range splitData {
	if !bytes.HasPrefix(b, []byte("%")) {
		i = idx // - 1
		break
	}
}
data = bytes.Join(splitData[0:i], []byte("\n"))
consumed := len(data)
...
block := p.addBlock(Heading, data)   // unconditional
```

`i` is only ever assigned inside the loop, so if **every** line starts with `%`
the loop never breaks and `i` stays `0`. `splitData[0:0]` is empty, the joined
data is `""`, and `consumed` is `0`.

Splitting a string that ends in `\n` always yields a trailing `""` element,
which does not start with `%` — so the loop normally does break. The bad case is
input whose final line has **no trailing newline**, which is exactly what a
document consisting only of a title block looks like.

Two further problems compound it. `addBlock` is called unconditionally, so a
node is appended even on the zero-consumption path; and the `doRender` parameter
is never read at all, so the "don't render, just measure" contract that
`fencedCodeBlock` and `htmlHr` honour is silently violated here.

### Reproducer (public API)

```go
blackfriday.Run([]byte("% a"), blackfriday.WithExtensions(blackfriday.Titleblock|blackfriday.CommonExtensions))
```

```html
<h1 class="title"></h1>

<p>% a</p>
```

Expected `<h1 class="title">a</h1>` and nothing else. Instead the title is lost,
an empty heading is emitted, and the raw text is re-rendered as a paragraph —
because `titleBlock` returned `0`, so `block()` ignored it and fell through to
the paragraph handler, by which time the stray node had already been appended.

Measured directly: `titleBlock("% a", …)` returns `0` while leaving one
`Heading` node with empty content, `Level` 1 and `IsTitleblock` true. Identical
with `doRender` false.

`% a\n` (with the newline) behaves correctly: consumed 3, literal `a`.

### Impact

Requires the non-default `Titleblock` extension and input whose last line lacks
a trailing newline. No crash; the damage is a wrong document — a stray empty
`<h1 class="title">` plus duplicated body text.

### In this port

`src/block.rs::title_block` reproduces all of it: the `i = 0` fallback, the
unconditional `add_block`, and the ignored `do_render`. Pinned by
`title_block_reproduces_the_zero_consumption_bug`.

---

## 3. `smartLeftAngle` reads past the end of its input, and `Run` can panic

**Severity:** high — reachable from the public API; either corrupts output with
a NUL byte or crashes the process, chosen by the length of the input.
**Status:** confirmed, minimal reproducer through the public API.
**Location:** `smartypants.go:369`.

### The defect

```go
func (r *SPRenderer) smartLeftAngle(out *bytes.Buffer, previousChar byte, text []byte) int {
	i := 0

	for i < len(text) && text[i] != '>' {
		i++
	}

	out.Write(text[:i+1])
	return i
}
```

The loop exits either because it found a `>` — in which case `text[:i+1]`
correctly includes it — or because `i` reached `len(text)`, in which case
`text[:i+1]` is a slice one byte longer than the data. Go permits that whenever
`cap` exceeds `len`, so the write copies a byte the caller never supplied; when
`cap == len` it is a runtime panic instead. There is no `i < len(text)` guard.

Which of the two happens is decided by the allocator, not by the program:

```text
Process(w, []byte("<b"))          -> "<b\x00"    (cap 8 > len 2)
Process(w, exactCapSlice("<b"))   -> panic: slice bounds out of range
```

### Why it is reachable

`escapeHTML` turns `<` into `&lt;`, so text nodes never reach this code with a
bare `<`. But `writeDocumentHeader` does not escape the document title:

```go
if r.Flags&Smartypants != 0 {
	r.sr.Process(w, []byte(r.Title))     // html.go:869 -- unescaped
} else {
	escapeHTML(w, []byte(r.Title))
}
```

`[]byte(string)` allocates a rounded-up size class, so whether spare capacity
exists — and therefore whether the result is a corrupted document or a crash —
depends on the length of the title.

### Reproducer (public API)

```go
r := blackfriday.NewHTMLRenderer(blackfriday.HTMLRendererParameters{
	Flags: blackfriday.CompletePage | blackfriday.Smartypants,
	Title: "aaaaaaa<", // exactly 8 bytes
})
blackfriday.Run([]byte("hi"), blackfriday.WithRenderer(r))
```

```text
panic: runtime error: slice bounds out of range [:2] with capacity 1
```

Titles of 8, 16, 24 and 32 bytes ending in an unclosed `<` panic; measured
across every length from 1 to 40, the other 36 do not. They emit a NUL instead:

```html
  <title>aa<&#0;</title>     <!-- the byte is a literal 0x00, shown escaped -->
```

The panic offsets are exactly Go's small size classes, which is the tell that
this is allocator-dependent rather than input-dependent in any meaningful sense.

### Impact

A caller that renders a complete page with smart punctuation and takes the
title from user input has a remote crash. It needs no Markdown extension, only
`CompletePage|Smartypants`, and the title need not be attacker-controlled to be
corrupted — an ordinary title like `a < b` is fine, but `a <` is not.

### In this port

This is the port's **one deliberate divergence**, and it is declared rather than
hidden. `src/smartypants.rs::smart_left_angle` writes the bytes it was given:

```rust
out.extend_from_slice(&text[..text.len().min(i + 1)]);
```

Reproducing upstream faithfully would mean reading uninitialised memory, which
the crate forbids (`#![forbid(unsafe_code)]`) and which is not a behaviour any
caller can depend on: the extra byte is zero only because Go zeroes fresh
allocations, and the panic threshold tracks size classes rather than anything in
the input. Pinned by `unterminated_tag_writes_only_what_it_was_given`, and
recorded again in `PARITY.md`.
