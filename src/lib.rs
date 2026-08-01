//! A Rust port of [blackfriday](https://github.com/russross/blackfriday) v2,
//! the Go Markdown processor by Russ Ross.
//!
//! The goal is behavioural equivalence, not a new Markdown library. Where a
//! choice existed between "what Rust would do" and "what blackfriday does",
//! blackfriday wins — including its quirks. Blackfriday predates CommonMark
//! and implements Markdown.pl-descended semantics plus its own extension set,
//! so its output deliberately differs from crates like `pulldown-cmark` or
//! `comrak`.
//!
//! Ported from upstream commit `4c9bf9512682b995722660a4196c0013228e2049`
//! (branch `v2`).
//!
//! # Safety
//!
//! This crate sets `#![forbid(unsafe_code)]`. The only `unsafe` in the
//! repository lives in the `ffi/` crate, which exists purely so the original
//! Go test suite can call into this code; it is not part of the library.
//!
//! # Structure
//!
//! | Module | Ported from |
//! |---|---|
//! | [`flags`] | the `Extensions` / `HTMLFlags` / `ListType` const blocks |
//! | [`node`] | `node.go` — the syntax tree, as an arena rather than a pointer graph |
//!
//! Further modules — the block and inline parsers, HTML renderer and
//! smartypants — land as the port proceeds. See `DECISIONS.md` for the
//! architectural differences from the Go original and why each was necessary.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod flags;
pub mod node;

pub use flags::{
    CellAlignFlags, Extensions, HtmlFlags, ListType, TAB_SIZE_DEFAULT, TAB_SIZE_DOUBLE, VERSION,
};
pub use node::{Arena, Node, NodeId, NodeType, WalkStatus, Walker};
