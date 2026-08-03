# A demo document

This file exists to be rendered on camera. It exercises most of what
blackfriday does, so one `bf` invocation produces visibly *interesting* HTML
rather than a single paragraph.

## Inline things

Emphasis: *single*, **double**, ***triple***, and ~~struck through~~.
Code spans like `run_with(input, options, &mut renderer)`.
An [inline link](https://github.com/russross/blackfriday), a
[reference link][bf], and a bare URL: https://www.rust-lang.org

Smart punctuation is on by default --- "quoted text", 'single quotes',
an ellipsis..., a fraction 1/2, and (c) 2026.

Entities pass through: AT&T, 5 < 6, and `&amp;` stays escaped.

## Blocks

> A blockquote, which may contain
>
> > a nested one,
>
> and a list:
>
> - first
> - second

1. An ordered list
2. with a second item
3. and a third

```rust
// A fenced code block, with an info string that becomes a class.
let html = blackfriday::run(b"# Hello");
```

    An indented code block,
    which keeps its   spacing.

Term
:   A definition list, which is a blackfriday extension rather than Markdown.

| left | centre | right |
|:-----|:------:|------:|
| a    |   b    |     c |
| 1    |   2    |     3 |

---

## Footnotes

Blackfriday supports Pandoc-style footnotes[^why], which the port reproduces
including the parts that are wrong[^cycle].

[^why]: They are a good test: they mutate the tree after parsing, which is
    where two of this port's own bugs lived.

[^cycle]: See `BUGS.md` #4 — a footnote that references itself makes both
    implementations loop forever.

[bf]: https://github.com/russross/blackfriday "the original"
