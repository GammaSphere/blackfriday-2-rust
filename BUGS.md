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
