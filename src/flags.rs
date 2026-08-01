//! Bit flags mirroring blackfriday's `Extensions`, `HTMLFlags`, `ListType` and
//! `CellAlignFlags`.
//!
//! # Why the values are not what they look like
//!
//! Upstream declares these with Go's `iota`:
//!
//! ```go
//! const (
//!     NoExtensions    Extensions = 0
//!     NoIntraEmphasis Extensions = 1 << iota  // iota is 1 here, not 0
//!     Tables                                  // 1 << 2
//!     ...
//! )
//! ```
//!
//! `iota` counts *ConstSpec lines within the block*, and `NoExtensions` occupies
//! line 0. So `NoIntraEmphasis` is `1 << 1 == 2`, and bit 0 is never used. The
//! same applies to `HTMLFlags`, where `HTMLFlagsNone = 0` pushes `SkipHTML` to
//! `1 << 1 == 2`.
//!
//! Transcribing these as `1 << 0, 1 << 1, ...` — the obvious reading — shifts
//! every flag by one position. The parser would still compile and most simple
//! documents would still render, because the error only shows up when a caller
//! passes a specific flag and gets a different feature. `ListType` and
//! `CellAlignFlags` have no zero-valued first line, so those genuinely do start
//! at `1 << 0`; the inconsistency is exactly what makes this easy to get wrong.
//!
//! Every value below was read out of the Go binary rather than off the source,
//! and the tests at the bottom of this file assert those measured values.

use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not};

/// Defines a Go-style flag newtype over `i32`.
///
/// Go's underlying types here are plain `int`, and callers are free to pass
/// values that do not correspond to any named flag. The representation is kept
/// as a transparent `i32` so that round-tripping an arbitrary value through the
/// FFI boundary cannot lose information.
macro_rules! go_flags {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$cmeta:meta])* const $cname:ident = $cvalue:expr; )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        #[repr(transparent)]
        pub struct $name(i32);

        impl $name {
            $( $(#[$cmeta])* pub const $cname: Self = Self($cvalue); )*

            /// The raw integer, as Go would see it.
            #[inline]
            pub const fn bits(self) -> i32 {
                self.0
            }

            /// Wraps a raw integer, preserving unknown bits.
            ///
            /// Mirrors Go's implicit conversion: any `int` is a valid value of
            /// the flag type, whether or not it names a defined flag.
            #[inline]
            pub const fn from_bits_retain(bits: i32) -> Self {
                Self(bits)
            }

            /// True when every bit in `other` is set in `self`.
            ///
            /// `contains(EMPTY)` is true, matching the Go idiom `f & X != 0`
            /// only for single-bit `X`; prefer this for multi-bit tests.
            #[inline]
            pub const fn contains(self, other: Self) -> bool {
                (self.0 & other.0) == other.0
            }

            /// True when `self` and `other` share at least one bit.
            ///
            /// This is the direct spelling of Go's `flags&Flag != 0`.
            #[inline]
            pub const fn intersects(self, other: Self) -> bool {
                (self.0 & other.0) != 0
            }

            /// True when no bits are set.
            #[inline]
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            /// Returns `self` with the bits in `other` cleared.
            #[inline]
            pub const fn without(self, other: Self) -> Self {
                Self(self.0 & !other.0)
            }
        }

        impl BitOr for $name {
            type Output = Self;
            #[inline]
            fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
        }

        impl BitOrAssign for $name {
            #[inline]
            fn bitor_assign(&mut self, rhs: Self) { self.0 |= rhs.0; }
        }

        impl BitAnd for $name {
            type Output = Self;
            #[inline]
            fn bitand(self, rhs: Self) -> Self { Self(self.0 & rhs.0) }
        }

        impl BitAndAssign for $name {
            #[inline]
            fn bitand_assign(&mut self, rhs: Self) { self.0 &= rhs.0; }
        }

        impl Not for $name {
            type Output = Self;
            #[inline]
            fn not(self) -> Self { Self(!self.0) }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!(stringify!($name), "(0x{:x})"), self.0)
            }
        }
    };
}

go_flags! {
    /// Parsing extensions, OR'ed together to select multiple.
    ///
    /// Bit 0 is unused; see the module docs.
    Extensions {
        /// No extensions enabled.
        const NONE = 0;
        /// Ignore emphasis markers inside words.
        const NO_INTRA_EMPHASIS = 1 << 1;
        /// Render tables.
        const TABLES = 1 << 2;
        /// Render fenced code blocks.
        const FENCED_CODE = 1 << 3;
        /// Detect embedded URLs that are not explicitly marked.
        const AUTOLINK = 1 << 4;
        /// Strikethrough text using `~~text~~`.
        const STRIKETHROUGH = 1 << 5;
        /// Loosen up HTML block parsing rules.
        const LAX_HTML_BLOCKS = 1 << 6;
        /// Be strict about prefix heading rules.
        const SPACE_HEADINGS = 1 << 7;
        /// Translate newlines into line breaks.
        const HARD_LINE_BREAK = 1 << 8;
        /// Expand tabs to eight spaces instead of four.
        const TAB_SIZE_EIGHT = 1 << 9;
        /// Pandoc-style footnotes.
        const FOOTNOTES = 1 << 10;
        /// No need for an empty line to start a code, quote or list block.
        const NO_EMPTY_LINE_BEFORE_BLOCK = 1 << 11;
        /// Specify heading IDs with `{#id}`.
        const HEADING_IDS = 1 << 12;
        /// Titleblock ala pandoc.
        const TITLEBLOCK = 1 << 13;
        /// Create the heading ID from the text.
        const AUTO_HEADING_IDS = 1 << 14;
        /// Translate trailing backslashes into line breaks.
        const BACKSLASH_LINE_BREAK = 1 << 15;
        /// Render definition lists.
        const DEFINITION_LISTS = 1 << 16;
    }
}

impl Extensions {
    /// The extension set used by upstream's `Run` when no options are given.
    ///
    /// Measured value `0x190be` (102590).
    pub const COMMON: Self = Self::from_bits_retain(
        Self::NO_INTRA_EMPHASIS.bits()
            | Self::TABLES.bits()
            | Self::FENCED_CODE.bits()
            | Self::AUTOLINK.bits()
            | Self::STRIKETHROUGH.bits()
            | Self::SPACE_HEADINGS.bits()
            | Self::HEADING_IDS.bits()
            | Self::BACKSLASH_LINE_BREAK.bits()
            | Self::DEFINITION_LISTS.bits(),
    );
}

go_flags! {
    /// HTML renderer configuration.
    ///
    /// Bit 0 is unused; see the module docs.
    HtmlFlags {
        /// No flags enabled.
        const NONE = 0;
        /// Skip preformatted HTML blocks.
        const SKIP_HTML = 1 << 1;
        /// Skip embedded images.
        const SKIP_IMAGES = 1 << 2;
        /// Skip all links.
        const SKIP_LINKS = 1 << 3;
        /// Only link to trusted protocols.
        const SAFELINK = 1 << 4;
        /// Only link with `rel="nofollow"`.
        const NOFOLLOW_LINKS = 1 << 5;
        /// Only link with `rel="noreferrer"`.
        const NOREFERRER_LINKS = 1 << 6;
        /// Only link with `rel="noopener"`.
        const NOOPENER_LINKS = 1 << 7;
        /// Add a blank target.
        const HREF_TARGET_BLANK = 1 << 8;
        /// Generate a complete HTML page.
        const COMPLETE_PAGE = 1 << 9;
        /// Generate XHTML output instead of HTML.
        const USE_XHTML = 1 << 10;
        /// Generate a link at the end of a footnote to return to the source.
        const FOOTNOTE_RETURN_LINKS = 1 << 11;
        /// Enable smart punctuation substitutions.
        const SMARTYPANTS = 1 << 12;
        /// Enable smart fractions (with `SMARTYPANTS`).
        const SMARTYPANTS_FRACTIONS = 1 << 13;
        /// Enable smart dashes (with `SMARTYPANTS`).
        const SMARTYPANTS_DASHES = 1 << 14;
        /// Enable LaTeX-style dashes (with `SMARTYPANTS`).
        const SMARTYPANTS_LATEX_DASHES = 1 << 15;
        /// Enable angled double quotes (with `SMARTYPANTS`).
        const SMARTYPANTS_ANGLED_QUOTES = 1 << 16;
        /// Enable French guillemets (with `SMARTYPANTS`).
        const SMARTYPANTS_QUOTES_NBSP = 1 << 17;
        /// Generate a table of contents.
        const TOC = 1 << 18;
    }
}

impl HtmlFlags {
    /// The flag set used by upstream's `Run` when no options are given.
    ///
    /// Measured value `0xf400` (62464).
    pub const COMMON: Self = Self::from_bits_retain(
        Self::USE_XHTML.bits()
            | Self::SMARTYPANTS.bits()
            | Self::SMARTYPANTS_FRACTIONS.bits()
            | Self::SMARTYPANTS_DASHES.bits()
            | Self::SMARTYPANTS_LATEX_DASHES.bits(),
    );
}

go_flags! {
    /// Flags for `List` and `Item` nodes.
    ///
    /// Unlike `Extensions` and `HtmlFlags`, this block has no zero-valued first
    /// line, so it really does start at `1 << 0`.
    ListType {
        /// No flags enabled.
        const NONE = 0;
        /// An ordered list.
        const ORDERED = 1 << 0;
        /// A definition list.
        const DEFINITION = 1 << 1;
        /// A definition-list term.
        const TERM = 1 << 2;
        /// The item contains a block element.
        const ITEM_CONTAINS_BLOCK = 1 << 3;
        /// The item begins the list.
        const ITEM_BEGINNING_OF_LIST = 1 << 4;
        /// The item ends the list.
        const ITEM_END_OF_LIST = 1 << 5;
    }
}

go_flags! {
    /// Alignment of a table cell.
    ///
    /// Only one of these is used at a time; they are not OR'ed by the renderer,
    /// though `CENTER` is itself `LEFT | RIGHT`.
    CellAlignFlags {
        /// No alignment specified.
        const NONE = 0;
        /// Left-aligned.
        const LEFT = 1 << 0;
        /// Right-aligned.
        const RIGHT = 1 << 1;
        /// Centred; equal to `LEFT | RIGHT`.
        const CENTER = (1 << 0) | (1 << 1);
    }
}

/// Default tab stop width.
pub const TAB_SIZE_DEFAULT: usize = 4;

/// Tab stop width when [`Extensions::TAB_SIZE_EIGHT`] is set.
pub const TAB_SIZE_DOUBLE: usize = 8;

/// Upstream's `Version` constant, reported unchanged.
pub const VERSION: &str = "2.0";

#[cfg(test)]
mod tests {
    use super::*;

    /// These are the values printed by a Go program linked against
    /// blackfriday v2 at commit 4c9bf95, not values transcribed from the
    /// source. See `bench/../tests/original/BASELINE.md` for provenance.
    #[test]
    fn extension_values_match_go() {
        assert_eq!(Extensions::NONE.bits(), 0);
        assert_eq!(Extensions::NO_INTRA_EMPHASIS.bits(), 2);
        assert_eq!(Extensions::TABLES.bits(), 4);
        assert_eq!(Extensions::FENCED_CODE.bits(), 8);
        assert_eq!(Extensions::AUTOLINK.bits(), 16);
        assert_eq!(Extensions::STRIKETHROUGH.bits(), 32);
        assert_eq!(Extensions::LAX_HTML_BLOCKS.bits(), 64);
        assert_eq!(Extensions::SPACE_HEADINGS.bits(), 128);
        assert_eq!(Extensions::HARD_LINE_BREAK.bits(), 256);
        assert_eq!(Extensions::TAB_SIZE_EIGHT.bits(), 512);
        assert_eq!(Extensions::FOOTNOTES.bits(), 1024);
        assert_eq!(Extensions::NO_EMPTY_LINE_BEFORE_BLOCK.bits(), 2048);
        assert_eq!(Extensions::HEADING_IDS.bits(), 4096);
        assert_eq!(Extensions::TITLEBLOCK.bits(), 8192);
        assert_eq!(Extensions::AUTO_HEADING_IDS.bits(), 16384);
        assert_eq!(Extensions::BACKSLASH_LINE_BREAK.bits(), 32768);
        assert_eq!(Extensions::DEFINITION_LISTS.bits(), 65536);
        assert_eq!(Extensions::COMMON.bits(), 102590);
    }

    #[test]
    fn html_flag_values_match_go() {
        assert_eq!(HtmlFlags::NONE.bits(), 0);
        assert_eq!(HtmlFlags::SKIP_HTML.bits(), 2);
        assert_eq!(HtmlFlags::SKIP_IMAGES.bits(), 4);
        assert_eq!(HtmlFlags::SKIP_LINKS.bits(), 8);
        assert_eq!(HtmlFlags::SAFELINK.bits(), 16);
        assert_eq!(HtmlFlags::NOFOLLOW_LINKS.bits(), 32);
        assert_eq!(HtmlFlags::NOREFERRER_LINKS.bits(), 64);
        assert_eq!(HtmlFlags::NOOPENER_LINKS.bits(), 128);
        assert_eq!(HtmlFlags::HREF_TARGET_BLANK.bits(), 256);
        assert_eq!(HtmlFlags::COMPLETE_PAGE.bits(), 512);
        assert_eq!(HtmlFlags::USE_XHTML.bits(), 1024);
        assert_eq!(HtmlFlags::FOOTNOTE_RETURN_LINKS.bits(), 2048);
        assert_eq!(HtmlFlags::SMARTYPANTS.bits(), 4096);
        assert_eq!(HtmlFlags::SMARTYPANTS_FRACTIONS.bits(), 8192);
        assert_eq!(HtmlFlags::SMARTYPANTS_DASHES.bits(), 16384);
        assert_eq!(HtmlFlags::SMARTYPANTS_LATEX_DASHES.bits(), 32768);
        assert_eq!(HtmlFlags::SMARTYPANTS_ANGLED_QUOTES.bits(), 65536);
        assert_eq!(HtmlFlags::SMARTYPANTS_QUOTES_NBSP.bits(), 131072);
        assert_eq!(HtmlFlags::TOC.bits(), 262144);
        assert_eq!(HtmlFlags::COMMON.bits(), 62464);
    }

    #[test]
    fn list_type_values_match_go() {
        assert_eq!(ListType::ORDERED.bits(), 1);
        assert_eq!(ListType::DEFINITION.bits(), 2);
        assert_eq!(ListType::TERM.bits(), 4);
        assert_eq!(ListType::ITEM_CONTAINS_BLOCK.bits(), 8);
        assert_eq!(ListType::ITEM_BEGINNING_OF_LIST.bits(), 16);
        assert_eq!(ListType::ITEM_END_OF_LIST.bits(), 32);
    }

    #[test]
    fn cell_align_values_match_go() {
        assert_eq!(CellAlignFlags::LEFT.bits(), 1);
        assert_eq!(CellAlignFlags::RIGHT.bits(), 2);
        assert_eq!(CellAlignFlags::CENTER.bits(), 3);
        assert_eq!(
            CellAlignFlags::CENTER,
            CellAlignFlags::LEFT | CellAlignFlags::RIGHT
        );
    }

    #[test]
    fn bit_zero_is_never_used_by_extensions_or_html_flags() {
        // The whole point of the iota quirk. If a future edit "tidies" these
        // to 1 << 0, this fails.
        assert_eq!(Extensions::COMMON.bits() & 1, 0);
        assert_eq!(HtmlFlags::COMMON.bits() & 1, 0);
    }

    #[test]
    fn set_operations_behave_like_go_int_masks() {
        let e = Extensions::TABLES | Extensions::FENCED_CODE;
        assert!(e.intersects(Extensions::TABLES));
        assert!(e.contains(Extensions::TABLES | Extensions::FENCED_CODE));
        assert!(!e.intersects(Extensions::AUTOLINK));
        assert_eq!(e.without(Extensions::TABLES), Extensions::FENCED_CODE);
        assert!(Extensions::NONE.is_empty());
    }

    #[test]
    fn unknown_bits_survive_a_round_trip() {
        // Go callers can pass any int; the FFI boundary must not silently
        // normalise values it does not recognise.
        let raw = 0x7FFF_FFFF;
        assert_eq!(Extensions::from_bits_retain(raw).bits(), raw);
        assert_eq!(HtmlFlags::from_bits_retain(-1).bits(), -1);
    }

    #[test]
    fn tab_sizes_match_go() {
        assert_eq!(TAB_SIZE_DEFAULT, 4);
        assert_eq!(TAB_SIZE_DOUBLE, 8);
        assert_eq!(VERSION, "2.0");
    }
}
