//! The HTML renderer, ported from upstream `html.go`.
//!
//! This commit carries the renderer's configuration and its pure helpers. The
//! `render_node` dispatch and the `out`/`cr` output primitives follow: `out`
//! strips HTML tags via a package-level regexp when tag output is suppressed,
//! and that pattern is hand-coded separately rather than pulling in a regex
//! crate for one use.

// The helpers below are consumed by `render_node`, which lands in the next
// commit. Until then only the tests reach them.
#![allow(dead_code)]

use crate::flags::{CellAlignFlags, HtmlFlags};
use crate::node::{Arena, NodeId, NodeType};
use std::collections::HashMap;

/// How singleton tags are closed.
const XHTML_CLOSE: &str = " />";
/// How singleton tags are closed without XHTML.
const HTML_CLOSE: &str = ">";

/// Supplementary parameters tweaking the HTML renderer's behaviour.
///
/// Mirrors upstream's `HTMLRendererParameters`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HtmlRendererParameters {
    /// Prepended to each relative URL.
    pub absolute_prefix: String,
    /// Added to each footnote anchor, to keep them unique.
    pub footnote_anchor_prefix: String,
    /// Shown inside the `<a>` of a footnote return link when
    /// [`HtmlFlags::FOOTNOTE_RETURN_LINKS`] is set. Empty means the default.
    pub footnote_return_link_contents: String,
    /// Added to the front of each heading id, to keep them unique.
    pub heading_id_prefix: String,
    /// Added to the back of each heading id, to keep them unique.
    pub heading_id_suffix: String,
    /// Shifts heading levels: an offset of 1 turns `<h1>` into `<h2>`. May be
    /// negative. The result is clamped to 1..=6.
    pub heading_level_offset: i32,

    /// Document title, used when [`HtmlFlags::COMPLETE_PAGE`] is set.
    pub title: String,
    /// Optional CSS URL, used when [`HtmlFlags::COMPLETE_PAGE`] is set.
    pub css: String,
    /// Optional icon URL, used when [`HtmlFlags::COMPLETE_PAGE`] is set.
    pub icon: String,

    /// Renderer behaviour flags.
    pub flags: HtmlFlags,
}

/// Renders a document as HTML.
///
/// Build it with [`HtmlRenderer::new`] rather than constructing it directly, so
/// the derived state stays consistent — Go says the same about its own type.
pub struct HtmlRenderer {
    /// The parameters this renderer was built with.
    pub params: HtmlRendererParameters,

    /// How to end singleton tags: `" />"` or `">"`.
    close_tag: &'static str,

    /// Heading ids seen so far, to avoid collisions within one render.
    heading_ids: HashMap<String, usize>,

    /// Length of the last thing written; `cr` uses it to avoid double newlines.
    pub(crate) last_output_len: usize,
    /// When positive, tag output is suppressed (used for image alt text).
    pub(crate) disable_tags: i32,
}

impl HtmlRenderer {
    /// Creates and configures a renderer.
    ///
    /// Ported from `NewHTMLRenderer` (`html.go:127`).
    pub fn new(mut params: HtmlRendererParameters) -> Self {
        let close_tag = if params.flags.intersects(HtmlFlags::USE_XHTML) {
            XHTML_CLOSE
        } else {
            HTML_CLOSE
        };

        if params.footnote_return_link_contents.is_empty() {
            // U+FE0E is VARIATION SELECTOR-15. It suppresses the automatic
            // emoji presentation of the preceding U+21A9 on iOS and iPadOS.
            params.footnote_return_link_contents =
                "<span aria-label='Return'>\u{21a9}\u{fe0e}</span>".to_string();
        }

        HtmlRenderer {
            params,
            close_tag,
            heading_ids: HashMap::new(),
            last_output_len: 0,
            disable_tags: 0,
        }
    }

    /// How this renderer closes singleton tags.
    pub fn close_tag(&self) -> &'static str {
        self.close_tag
    }

    /// Makes `id` unique within this render, appending `-1`, `-2` and so on.
    ///
    /// Ported from `ensureUniqueHeadingID` (`html.go:251`). The counter is
    /// stored against the *original* id, so a document with `foo`, `foo`, `foo`
    /// yields `foo`, `foo-1`, `foo-2`. When a generated candidate is itself
    /// already taken, upstream falls back to appending `-1` repeatedly, which
    /// this reproduces.
    pub fn ensure_unique_heading_id(&mut self, id: &str) -> String {
        let mut id = id.to_string();
        while let Some(&count) = self.heading_ids.get(&id) {
            let tmp = format!("{id}-{}", count + 1);
            if !self.heading_ids.contains_key(&tmp) {
                self.heading_ids.insert(id.clone(), count + 1);
                id = tmp;
            } else {
                id = format!("{id}-1");
            }
        }
        self.heading_ids.entry(id.clone()).or_insert(0);
        id
    }

    /// Prefixes a relative link with [`HtmlRendererParameters::absolute_prefix`].
    ///
    /// Ported from `addAbsPrefix` (`html.go:270`). Links beginning with `.` are
    /// left alone even though `isRelativeLink` accepts them.
    pub fn add_abs_prefix(&self, link: &[u8]) -> Vec<u8> {
        if !self.params.absolute_prefix.is_empty()
            && !link.is_empty()
            && is_relative_link(link)
            && link[0] != b'.'
        {
            let mut new_dest = self.params.absolute_prefix.clone().into_bytes();
            if link[0] != b'/' {
                new_dest.push(b'/');
            }
            new_dest.extend_from_slice(link);
            return new_dest;
        }
        link.to_vec()
    }

    /// Writes an opening tag with optional attributes.
    ///
    /// Ported from `tag` (`html.go:333`). Sets `last_output_len` to 1 rather
    /// than the true length, which is deliberate upstream: only "did anything
    /// get written" matters to [`Self::cr`].
    pub(crate) fn tag(&mut self, out: &mut Vec<u8>, name: &[u8], attrs: &[String]) {
        out.extend_from_slice(name);
        if !attrs.is_empty() {
            out.push(b' ');
            out.extend_from_slice(attrs.join(" ").as_bytes());
        }
        out.push(b'>');
        self.last_output_len = 1;
    }
}

/// Whether `tag` is an HTML tag named `tagname`.
///
/// Ported from `isHTMLTag` (`html.go:151`).
pub(crate) fn is_html_tag(tag: &[u8], tagname: &str) -> bool {
    find_html_tag_pos(tag, tagname).0
}

/// Finds `char` in `html`, ignoring occurrences inside quotes.
///
/// Ported from `skipUntilCharIgnoreQuotes` (`html.go:158`). Single, double and
/// grave quotes all count, since the content may be JavaScript.
///
/// Returns `start` — not the length — when the character is never found, which
/// is what makes the `rightAngle >= i` test in [`find_html_tag_pos`] meaningful.
pub(crate) fn skip_until_char_ignore_quotes(html: &[u8], start: usize, ch: u8) -> usize {
    let mut in_single = false;
    let mut in_double = false;
    let mut in_grave = false;
    let mut i = start;
    while i < html.len() {
        if html[i] == ch && !in_single && !in_double && !in_grave {
            return i;
        } else if html[i] == b'\'' {
            in_single = !in_single;
        } else if html[i] == b'"' {
            in_double = !in_double;
        } else if html[i] == b'`' {
            in_grave = !in_grave;
        }
        i += 1;
    }
    start
}

/// Locates the closing `>` of a tag named `tagname`.
///
/// Ported from `findHTMLTagPos` (`html.go:179`). Tag names are matched
/// case-insensitively, one byte at a time.
pub(crate) fn find_html_tag_pos(tag: &[u8], tagname: &str) -> (bool, isize) {
    let mut i = 0usize;
    if i < tag.len() && tag[0] != b'<' {
        return (false, -1);
    }
    i += 1;
    i = skip_space(tag, i);

    if i < tag.len() && tag[i] == b'/' {
        i += 1;
    }

    i = skip_space(tag, i);
    let name = tagname.as_bytes();
    let mut j = 0usize;
    while i < tag.len() {
        if j >= name.len() {
            break;
        }
        if tag[i].to_ascii_lowercase() != name[j] {
            return (false, -1);
        }
        i += 1;
        j += 1;
    }

    if i == tag.len() {
        return (false, -1);
    }

    let right_angle = skip_until_char_ignore_quotes(tag, i, b'>');
    if right_angle >= i {
        return (true, right_angle as isize);
    }

    (false, -1)
}

/// Advances past whitespace.
///
/// Ported from `skipSpace` (`html.go:215`). Uses blackfriday's own whitespace
/// definition, which includes form feed and vertical tab.
pub(crate) fn skip_space(tag: &[u8], mut i: usize) -> usize {
    while i < tag.len() && crate::util::is_space(tag[i]) {
        i += 1;
    }
    i
}

/// Whether `link` is relative.
///
/// Ported from `isRelativeLink` (`html.go:222`). Note `//host/path` is *not*
/// relative — it is a protocol-relative URL.
///
/// # Panics
///
/// On empty input, matching upstream's unguarded `link[0]`.
pub(crate) fn is_relative_link(link: &[u8]) -> bool {
    // A fragment.
    if link[0] == b'#' {
        return true;
    }
    // Begins with '/' but not '//', which would be protocol-relative.
    if link.len() >= 2 && link[0] == b'/' && link[1] != b'/' {
        return true;
    }
    // Just the root.
    if link.len() == 1 && link[0] == b'/' {
        return true;
    }
    if link.starts_with(b"./") {
        return true;
    }
    if link.starts_with(b"../") {
        return true;
    }
    false
}

/// Adds `rel` and `target` attributes according to the link flags.
///
/// Ported from `appendLinkAttrs` (`html.go:282`). Relative links get nothing.
/// Note `target="_blank"` is appended even when no `rel` value applies, so the
/// early return below it only skips the `rel` attribute.
pub(crate) fn append_link_attrs(attrs: &mut Vec<String>, flags: HtmlFlags, link: &[u8]) {
    if is_relative_link(link) {
        return;
    }
    let mut val: Vec<&str> = Vec::new();
    if flags.intersects(HtmlFlags::NOFOLLOW_LINKS) {
        val.push("nofollow");
    }
    if flags.intersects(HtmlFlags::NOREFERRER_LINKS) {
        val.push("noreferrer");
    }
    if flags.intersects(HtmlFlags::NOOPENER_LINKS) {
        val.push("noopener");
    }
    if flags.intersects(HtmlFlags::HREF_TARGET_BLANK) {
        attrs.push("target=\"_blank\"".to_string());
    }
    if val.is_empty() {
        return;
    }
    attrs.push(format!("rel=\"{}\"", val.join(" ")));
}

/// Whether `link` is a `mailto:` URL.
pub(crate) fn is_mailto(link: &[u8]) -> bool {
    link.starts_with(b"mailto:")
}

/// Whether smart punctuation applies inside this node.
///
/// Ported from `isSmartypantable` (`html.go:317`): not inside links, code
/// blocks or code spans.
pub(crate) fn is_smartypantable(arena: &Arena, node: NodeId) -> bool {
    let Some(parent) = arena[node].parent() else {
        return true;
    };
    let pt = arena[parent].node_type;
    pt != NodeType::Link && pt != NodeType::CodeBlock && pt != NodeType::Code
}

/// Adds a `class="language-…"` attribute from a code fence's info string.
///
/// Ported from `appendLanguageAttr` (`html.go:322`). Only the first
/// whitespace-delimited word is used, so ```` ```go linenums ```` gives
/// `language-go`.
pub(crate) fn append_language_attr(attrs: &mut Vec<String>, info: &[u8]) {
    if info.is_empty() {
        return;
    }
    let end_of_lang = info
        .iter()
        .position(|&c| c == b'\t' || c == b' ')
        .unwrap_or(info.len());
    attrs.push(format!(
        "class=\"language-{}\"",
        String::from_utf8_lossy(&info[..end_of_lang])
    ));
}

/// Builds a footnote reference anchor.
///
/// Ported from `footnoteRef` (`html.go:343`).
pub(crate) fn footnote_ref(prefix: &str, destination: &[u8], note_id: i32) -> Vec<u8> {
    let url_frag = format!(
        "{prefix}{}",
        String::from_utf8_lossy(&crate::util::slugify(destination))
    );
    let anchor = format!("<a href=\"#fn:{url_frag}\">{note_id}</a>");
    format!("<sup class=\"footnote-ref\" id=\"fnref:{url_frag}\">{anchor}</sup>").into_bytes()
}

/// Builds a footnote list item's opening tag.
pub(crate) fn footnote_item(prefix: &str, slug: &[u8]) -> Vec<u8> {
    format!("<li id=\"fn:{prefix}{}\">", String::from_utf8_lossy(slug)).into_bytes()
}

/// Builds a footnote's return link.
pub(crate) fn footnote_return_link(prefix: &str, return_link: &str, slug: &[u8]) -> Vec<u8> {
    format!(
        " <a class=\"footnote-return\" href=\"#fnref:{prefix}{}\">{return_link}</a>",
        String::from_utf8_lossy(slug)
    )
    .into_bytes()
}

/// Whether a list item should be preceded by a newline.
///
/// Ported from `itemOpenCR` (`html.go:358`). False for the first item, and for
/// tight lists and definition lists.
pub(crate) fn item_open_cr(arena: &Arena, node: NodeId) -> bool {
    if arena[node].prev().is_none() {
        return false;
    }
    let Some(parent) = arena[node].parent() else {
        return false;
    };
    let ld = &arena[parent].list;
    !ld.tight && !ld.list_flags.intersects(crate::ListType::DEFINITION)
}

/// Whether a paragraph inside a list item should render without `<p>` tags.
///
/// Ported from `skipParagraphTags` (`html.go:366`).
pub(crate) fn skip_paragraph_tags(arena: &Arena, node: NodeId) -> bool {
    let Some(parent) = arena[node].parent() else {
        return false;
    };
    let Some(grandparent) = arena[parent].parent() else {
        return false;
    };
    if arena[grandparent].node_type != NodeType::List {
        return false;
    }
    arena[grandparent].list.tight
        || arena[parent]
            .list
            .list_flags
            .intersects(crate::ListType::TERM)
}

/// The `align` attribute value for a table cell.
///
/// Ported from `cellAlignment` (`html.go:375`). Anything other than the three
/// named alignments gives the empty string.
pub(crate) fn cell_alignment(align: CellAlignFlags) -> &'static str {
    match align {
        a if a == CellAlignFlags::LEFT => "left",
        a if a == CellAlignFlags::RIGHT => "right",
        a if a == CellAlignFlags::CENTER => "center",
        _ => "",
    }
}

/// The opening and closing tags for a heading level.
///
/// Ported from `headingTagsFromLevel` (`html.go:473`). Levels outside 1..=6
/// clamp to the nearest end.
pub(crate) fn heading_tags_from_level(level: i32) -> (&'static [u8], &'static [u8]) {
    match level {
        i32::MIN..=1 => (b"<h1", b"</h1>"),
        2 => (b"<h2", b"</h2>"),
        3 => (b"<h3", b"</h3>"),
        4 => (b"<h4", b"</h4>"),
        5 => (b"<h5", b"</h5>"),
        _ => (b"<h6", b"</h6>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer(flags: HtmlFlags) -> HtmlRenderer {
        HtmlRenderer::new(HtmlRendererParameters {
            flags,
            ..Default::default()
        })
    }

    /// Measured Go answers for the renderer helpers.
    const FIXTURE: &str = include_str!("../tests/fixtures/go-htmlhelp.txt");

    fn rows(tag: &str) -> impl Iterator<Item = Vec<String>> + '_ {
        FIXTURE.lines().filter_map(move |l| {
            let f: Vec<String> = l.split(' ').map(str::to_string).collect();
            (f.first().map(String::as_str) == Some(tag)).then_some(f)
        })
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("bad hex"))
            .collect()
    }

    #[test]
    fn helpers_match_go() {
        // ensure_unique_heading_id, applied in sequence to one renderer.
        let mut r = renderer(HtmlFlags::NONE);
        let mut n = 0;
        for f in rows("U") {
            assert_eq!(
                r.ensure_unique_heading_id(&f[1]),
                f[2],
                "ensure_unique_heading_id({:?})",
                f[1]
            );
            n += 1;
        }
        assert!(n >= 8);

        for f in rows("R") {
            let link = unhex(&f[1]);
            assert_eq!(
                is_relative_link(&link),
                f[2] == "true",
                "is_relative_link({:?})",
                String::from_utf8_lossy(&link)
            );
        }

        let rp = HtmlRenderer::new(HtmlRendererParameters {
            absolute_prefix: "http://x".into(),
            ..Default::default()
        });
        for f in rows("A") {
            let link = unhex(&f[1]);
            assert_eq!(
                rp.add_abs_prefix(&link),
                unhex(&f[2]),
                "add_abs_prefix({:?})",
                String::from_utf8_lossy(&link)
            );
        }

        for f in rows("L") {
            let flags = HtmlFlags::from_bits_retain(f[1].parse().unwrap());
            let link = unhex(&f[2]);
            let mut attrs = Vec::new();
            append_link_attrs(&mut attrs, flags, &link);
            let joined = if attrs.is_empty() {
                "-".to_string()
            } else {
                attrs.join("|")
            };
            assert_eq!(
                joined.as_bytes(),
                unhex(&f[3]),
                "append_link_attrs({flags:?}, {:?})",
                String::from_utf8_lossy(&link)
            );
        }

        for f in rows("G") {
            let info = unhex(&f[1]);
            let mut attrs = Vec::new();
            append_language_attr(&mut attrs, &info);
            let joined = if attrs.is_empty() {
                "-".to_string()
            } else {
                attrs.join("|")
            };
            assert_eq!(joined.as_bytes(), unhex(&f[2]));
        }

        for f in rows("T") {
            let tag = unhex(&f[1]);
            let (found, pos) = find_html_tag_pos(&tag, &f[2]);
            assert_eq!(
                (found, pos),
                (f[3] == "true", f[4].parse::<isize>().unwrap()),
                "find_html_tag_pos({:?}, {:?})",
                String::from_utf8_lossy(&tag),
                f[2]
            );
        }

        for f in rows("S") {
            let s = unhex(&f[1]);
            assert_eq!(
                skip_until_char_ignore_quotes(&s, 0, b'>'),
                f[2].parse::<usize>().unwrap()
            );
        }

        for f in rows("C") {
            let align = CellAlignFlags::from_bits_retain(f[1].parse().unwrap());
            assert_eq!(cell_alignment(align).as_bytes(), unhex(&f[2]));
        }

        for f in rows("H") {
            let (o, c) = heading_tags_from_level(f[1].parse().unwrap());
            assert_eq!(o, unhex(&f[2]).as_slice());
            assert_eq!(c, unhex(&f[3]).as_slice());
        }

        // The generated default footnote return link.
        let d = rows("D").next().unwrap();
        assert_eq!(
            r.params.footnote_return_link_contents.as_bytes(),
            unhex(&d[1])
        );
    }

    #[test]
    fn empty_input_matches_any_tag_name() {
        // Surprising, and measured: findHTMLTagPos("", _) returns (true, 1).
        // The `tag[0] != '<'` guard is skipped because the length test fails
        // first, and `i == len(tag)` compares 1 against 0, so nothing rejects
        // it. Preserved rather than "fixed" -- callers only reach it with a
        // non-empty tag, but the behaviour is upstream's.
        assert_eq!(find_html_tag_pos(b"", "div"), (true, 1));
        assert!(is_html_tag(b"", "div"));
        assert!(is_html_tag(b"", "anything"));
    }

    #[test]
    fn close_tag_depends_on_xhtml_flag() {
        assert_eq!(renderer(HtmlFlags::NONE).close_tag(), ">");
        assert_eq!(renderer(HtmlFlags::USE_XHTML).close_tag(), " />");
    }

    #[test]
    fn default_footnote_return_link_is_filled_in() {
        let r = renderer(HtmlFlags::NONE);
        assert_eq!(
            r.params.footnote_return_link_contents,
            "<span aria-label='Return'>\u{21a9}\u{fe0e}</span>"
        );
        // A supplied value is left alone.
        let r = HtmlRenderer::new(HtmlRendererParameters {
            footnote_return_link_contents: "back".into(),
            ..Default::default()
        });
        assert_eq!(r.params.footnote_return_link_contents, "back");
    }

    #[test]
    fn heading_ids_are_made_unique() {
        let mut r = renderer(HtmlFlags::NONE);
        assert_eq!(r.ensure_unique_heading_id("foo"), "foo");
        assert_eq!(r.ensure_unique_heading_id("foo"), "foo-1");
        assert_eq!(r.ensure_unique_heading_id("foo"), "foo-2");
        assert_eq!(r.ensure_unique_heading_id("bar"), "bar");
        // An explicit collision with a generated name still resolves.
        assert_eq!(r.ensure_unique_heading_id("foo-1"), "foo-1-1");
    }

    #[test]
    fn relative_links_are_classified_like_go() {
        assert!(is_relative_link(b"#frag"));
        assert!(is_relative_link(b"/root/path"));
        assert!(is_relative_link(b"/"));
        assert!(is_relative_link(b"./here"));
        assert!(is_relative_link(b"../up"));
        // Protocol-relative is NOT relative.
        assert!(!is_relative_link(b"//example.com/x"));
        assert!(!is_relative_link(b"http://example.com"));
        assert!(!is_relative_link(b"mailto:a@b.c"));
        assert!(
            !is_relative_link(b"plain.html"),
            "bare names are not relative"
        );
    }

    #[test]
    fn abs_prefix_skips_dot_relative_links() {
        let r = HtmlRenderer::new(HtmlRendererParameters {
            absolute_prefix: "http://x".into(),
            ..Default::default()
        });
        assert_eq!(r.add_abs_prefix(b"/a"), b"http://x/a");
        assert_eq!(r.add_abs_prefix(b"#f"), b"http://x/#f");
        // ./ and ../ are relative but deliberately excluded.
        assert_eq!(r.add_abs_prefix(b"./a"), b"./a");
        assert_eq!(r.add_abs_prefix(b"../a"), b"../a");
        // Absolute links are untouched.
        assert_eq!(r.add_abs_prefix(b"http://y/a"), b"http://y/a");
    }

    #[test]
    fn link_attrs_follow_the_flags() {
        let attrs = |flags: HtmlFlags, link: &[u8]| {
            let mut a = Vec::new();
            append_link_attrs(&mut a, flags, link);
            a
        };
        assert!(
            attrs(HtmlFlags::NOFOLLOW_LINKS, b"/rel").is_empty(),
            "relative"
        );
        assert_eq!(
            attrs(HtmlFlags::NOFOLLOW_LINKS, b"http://x"),
            vec!["rel=\"nofollow\""]
        );
        assert_eq!(
            attrs(
                HtmlFlags::NOFOLLOW_LINKS | HtmlFlags::NOREFERRER_LINKS,
                b"http://x"
            ),
            vec!["rel=\"nofollow noreferrer\""]
        );
        // target is appended even with no rel value.
        assert_eq!(
            attrs(HtmlFlags::HREF_TARGET_BLANK, b"http://x"),
            vec!["target=\"_blank\""]
        );
        // ...and comes before rel when both apply.
        assert_eq!(
            attrs(
                HtmlFlags::HREF_TARGET_BLANK | HtmlFlags::NOOPENER_LINKS,
                b"http://x"
            ),
            vec!["target=\"_blank\"", "rel=\"noopener\""]
        );
    }

    #[test]
    fn language_attr_uses_only_the_first_word() {
        let attr = |info: &[u8]| {
            let mut a = Vec::new();
            append_language_attr(&mut a, info);
            a
        };
        assert_eq!(attr(b"go"), vec!["class=\"language-go\""]);
        assert_eq!(attr(b"go linenums"), vec!["class=\"language-go\""]);
        assert_eq!(attr(b"go\ttabbed"), vec!["class=\"language-go\""]);
        assert!(attr(b"").is_empty());
    }

    #[test]
    fn quote_aware_scan_returns_start_when_not_found() {
        // Not the length -- that is what makes find_html_tag_pos's
        // `right_angle >= i` test able to fail.
        assert_eq!(skip_until_char_ignore_quotes(b"abc", 0, b'>'), 0);
        assert_eq!(skip_until_char_ignore_quotes(b"ab>c", 0, b'>'), 2);
        // Quoted occurrences are ignored, in all three quote styles.
        assert_eq!(skip_until_char_ignore_quotes(b"a'>'b>", 0, b'>'), 5);
        assert_eq!(skip_until_char_ignore_quotes(b"a\">\"b>", 0, b'>'), 5);
        assert_eq!(skip_until_char_ignore_quotes(b"a`>`b>", 0, b'>'), 5);
    }

    #[test]
    fn tag_names_match_case_insensitively() {
        assert!(is_html_tag(b"<div>", "div"));
        assert!(is_html_tag(b"<DIV>", "div"));
        assert!(is_html_tag(b"<DiV class=x>", "div"));
        assert!(is_html_tag(b"</div>", "div"), "closing tags count too");
        assert!(!is_html_tag(b"<span>", "div"));
        assert!(!is_html_tag(b"div>", "div"), "must start with <");
    }

    #[test]
    fn heading_tags_clamp_to_one_through_six() {
        assert_eq!(heading_tags_from_level(1), (&b"<h1"[..], &b"</h1>"[..]));
        assert_eq!(heading_tags_from_level(0), (&b"<h1"[..], &b"</h1>"[..]));
        assert_eq!(heading_tags_from_level(-5), (&b"<h1"[..], &b"</h1>"[..]));
        assert_eq!(heading_tags_from_level(6), (&b"<h6"[..], &b"</h6>"[..]));
        assert_eq!(heading_tags_from_level(99), (&b"<h6"[..], &b"</h6>"[..]));
    }

    #[test]
    fn cell_alignment_names_match_go() {
        assert_eq!(cell_alignment(CellAlignFlags::LEFT), "left");
        assert_eq!(cell_alignment(CellAlignFlags::RIGHT), "right");
        assert_eq!(cell_alignment(CellAlignFlags::CENTER), "center");
        assert_eq!(cell_alignment(CellAlignFlags::NONE), "");
        // CENTER is LEFT|RIGHT, so it must be tested before the others would
        // shadow it -- a naive bit test would report "left" here.
        assert_eq!(
            cell_alignment(CellAlignFlags::LEFT | CellAlignFlags::RIGHT),
            "center"
        );
    }

    #[test]
    fn smartypants_is_suppressed_inside_links_and_code() {
        let mut a = Arena::new();
        let doc = a.new_node(NodeType::Document);
        let text = a.new_node(NodeType::Text);
        a.append_child(doc, text);
        assert!(is_smartypantable(&a, text));

        for parent_type in [NodeType::Link, NodeType::CodeBlock, NodeType::Code] {
            let mut a = Arena::new();
            let p = a.new_node(parent_type);
            let t = a.new_node(NodeType::Text);
            a.append_child(p, t);
            assert!(!is_smartypantable(&a, t), "inside {parent_type}");
        }
    }

    #[test]
    fn mailto_detection() {
        assert!(is_mailto(b"mailto:a@b.c"));
        assert!(!is_mailto(b"http://x"));
        assert!(!is_mailto(b"MAILTO:a@b.c"), "case-sensitive, like Go");
    }
}
