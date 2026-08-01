# walkorder

Prints blackfriday's `Node.Walk` traversal order for a fixed tree, so the Rust
port's walker can be checked against measured Go behaviour rather than against a
hand-derived expectation.

```bash
cd tools/walkorder && go run .
```

## Why this exists

`src/node.rs` replaces Go's pointer tree with an arena, and its [`Walker`] is a
borrow-free cursor rather than the obvious closure-taking-`&Arena` design. That
choice is forced: upstream mutates the tree *during* a walk. `markdown.go:410`
parses inline content into fresh child nodes from inside the visitor, and the
walk is expected to then descend into those new nodes.

Getting the traversal subtly wrong — visiting a container once instead of twice,
descending in the wrong order, or *not* descending into nodes the visitor just
created — would produce a parser that works on simple documents and corrupts
nested ones. The failure would surface far from its cause.

So the cases here are measured, and `src/node.rs` asserts exactly what this
prints:

| Case | What it pins down |
|---|---|
| plain walk | containers visited twice (enter/leave), leaves once |
| lone leaf | a leaf root is visited exactly once, not twice |
| skip children | `SkipChildren` on entering suppresses the subtree *and* the leaving visit |
| terminate | `Terminate` stops before advancing |
| mutation mid-walk | the walk descends into children appended by the visitor |
| unlink | first/last-child repair on the parent |
| `String()` | 16-byte truncation boundary is `>`, not `>=` |

The mutation case is the important one. Go's output for it is:

```text
Document true
Paragraph true
Text true
Emph true      <- appended during the Paragraph visit
Emph false
Paragraph false
...
```
