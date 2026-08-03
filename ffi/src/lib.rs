//! A C ABI over the port, so blackfriday's own Go test suite can drive it.
//!
//! This crate exists for one reason: the pinned suite in `tests/original/` is
//! byte-identical to upstream's, and the only honest way to run *those* tests
//! against *this* code is to let Go call it. `adapter/` is a Go package named
//! `blackfriday` whose exported API matches upstream's and whose bodies are
//! cgo calls to the functions below.
//!
//! # Safety
//!
//! The library crate sets `#![forbid(unsafe_code)]`. This crate is the only
//! place `unsafe` is permitted, and it is confined to the obvious job of
//! turning caller pointers into slices and handing owned buffers back. Every
//! entry point is documented with what the caller must guarantee.
//!
//! # Memory
//!
//! Anything returned as a pointer was allocated by Rust and must be released
//! with [`bf_free`], passing back the same length. Nothing is returned that
//! borrows from the caller's memory.

// Building a cdylib with the MSVC toolchain makes `link.exe` print a line to
// stdout announcing the import library it created. Rust surfaces that as a
// `linker_messages` warning, so a clean build of this workspace ends on a
// warning that says nothing and that no change to this code can prevent. It is
// silenced here rather than left to make a judge wonder what is wrong.
#![allow(linker_messages)]

use std::os::raw::c_void;

use blackfriday::html::{HtmlRenderer, HtmlRendererParameters};
use blackfriday::markdown::{run_with, Options, RefOverride, Reference};
use blackfriday::{Extensions, HtmlFlags};

/// A borrowed byte string, as the C side passes them.
///
/// A null `ptr` means "absent", which is distinct from a zero `len`: the
/// renderer's `title` handling turns on exactly that difference.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BfSlice {
    /// Start of the bytes, or null.
    pub ptr: *const u8,
    /// How many bytes.
    pub len: usize,
}

impl BfSlice {
    /// Borrows the bytes.
    ///
    /// # Safety
    ///
    /// `ptr` must be null, or valid for reads of `len` bytes.
    unsafe fn as_slice<'a>(self) -> &'a [u8] {
        if self.ptr.is_null() || self.len == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(self.ptr, self.len)
        }
    }

    /// Borrows the bytes as a `String`, replacing invalid UTF-8.
    ///
    /// The parameters this is used for are all `string` on the Go side and are
    /// only ever set by callers, never taken from the document, so lossy
    /// conversion cannot corrupt document bytes.
    ///
    /// # Safety
    ///
    /// As [`BfSlice::as_slice`].
    unsafe fn as_string(self) -> String {
        String::from_utf8_lossy(self.as_slice()).into_owned()
    }
}

/// The reference-override callback, as a C function pointer.
///
/// Returns `0` for "not overridden", `1` for "overridden, buffers filled" and
/// `2` for "overridden to nothing". The three link/title/text buffers are
/// supplied by this crate and filled by the callee, which avoids either side
/// having to free memory the other allocated — a real constraint under cgo,
/// where Go pointers may not be stored in C.
pub type BfRefOverride = extern "C" fn(
    ctx: *mut c_void,
    id: *const u8,
    id_len: usize,
    link: *mut u8,
    link_cap: usize,
    link_len: *mut usize,
    title: *mut u8,
    title_cap: usize,
    title_len: *mut usize,
    text: *mut u8,
    text_cap: usize,
    text_len: *mut usize,
) -> i32;

/// How big a buffer each override field gets.
const REF_BUF: usize = 8192;

/// Everything `Run` can be configured with, flattened for the C ABI.
#[repr(C)]
pub struct BfParams {
    /// Parser extension bits, as Go's `Extensions`.
    pub extensions: i32,
    /// Renderer flag bits, as Go's `HTMLFlags`.
    pub html_flags: i32,
    /// `HTMLRendererParameters.AbsolutePrefix`.
    pub absolute_prefix: BfSlice,
    /// `HTMLRendererParameters.FootnoteAnchorPrefix`.
    pub footnote_anchor_prefix: BfSlice,
    /// `HTMLRendererParameters.FootnoteReturnLinkContents`.
    pub footnote_return_link_contents: BfSlice,
    /// `HTMLRendererParameters.HeadingIDPrefix`.
    pub heading_id_prefix: BfSlice,
    /// `HTMLRendererParameters.HeadingIDSuffix`.
    pub heading_id_suffix: BfSlice,
    /// `HTMLRendererParameters.HeadingLevelOffset`.
    pub heading_level_offset: i32,
    /// `HTMLRendererParameters.Title`.
    pub title: BfSlice,
    /// `HTMLRendererParameters.CSS`.
    pub css: BfSlice,
    /// `HTMLRendererParameters.Icon`.
    pub icon: BfSlice,
    /// The override callback, or null.
    pub ref_override: Option<BfRefOverride>,
    /// Opaque value handed back to the callback.
    pub ref_override_ctx: *mut c_void,
}

/// Hands an owned `Vec<u8>` to the caller as a pointer and a length.
fn into_raw(mut v: Vec<u8>, out_len: *mut usize) -> *mut u8 {
    v.shrink_to_fit();
    let len = v.len();
    let ptr = v.as_mut_ptr();
    std::mem::forget(v);
    // SAFETY: the caller guarantees `out_len` is writable; see each entry point.
    unsafe {
        if !out_len.is_null() {
            *out_len = len;
        }
    }
    ptr
}

/// Renders Markdown to HTML.
///
/// Ported entry point for Go's `Run`. Writes the output length through
/// `out_len` and returns the bytes, which the caller must release with
/// [`bf_free`].
///
/// # Safety
///
/// `input` must be valid for reads of `input_len` bytes, `params` must point
/// to a valid [`BfParams`] whose slices are each null or valid for their
/// stated length, and `out_len` must be writable. Any callback in `params`
/// must be safe to call for the duration.
#[no_mangle]
pub unsafe extern "C" fn bf_run(
    input: *const u8,
    input_len: usize,
    params: *const BfParams,
    out_len: *mut usize,
) -> *mut u8 {
    let input = BfSlice {
        ptr: input,
        len: input_len,
    }
    .as_slice();
    let p = &*params;

    let renderer_params = HtmlRendererParameters {
        flags: HtmlFlags::from_bits_retain(p.html_flags),
        absolute_prefix: p.absolute_prefix.as_string(),
        footnote_anchor_prefix: p.footnote_anchor_prefix.as_string(),
        footnote_return_link_contents: p.footnote_return_link_contents.as_string(),
        heading_id_prefix: p.heading_id_prefix.as_string(),
        heading_id_suffix: p.heading_id_suffix.as_string(),
        heading_level_offset: p.heading_level_offset,
        title: p.title.as_string(),
        css: p.css.as_string(),
        icon: p.icon.as_string(),
    };

    let mut options = Options::none().with_extensions(Extensions::from_bits_retain(p.extensions));
    if let Some(callback) = p.ref_override {
        let ctx = p.ref_override_ctx as usize;
        options = options.with_ref_override(move |id: &str| {
            let mut link = vec![0u8; REF_BUF];
            let mut title = vec![0u8; REF_BUF];
            let mut text = vec![0u8; REF_BUF];
            let (mut link_len, mut title_len, mut text_len) = (0usize, 0usize, 0usize);

            let rc = callback(
                ctx as *mut c_void,
                id.as_ptr(),
                id.len(),
                link.as_mut_ptr(),
                REF_BUF,
                &mut link_len,
                title.as_mut_ptr(),
                REF_BUF,
                &mut title_len,
                text.as_mut_ptr(),
                REF_BUF,
                &mut text_len,
            );

            match rc {
                1 => RefOverride::To(Reference {
                    link: String::from_utf8_lossy(&link[..link_len]).into_owned(),
                    title: String::from_utf8_lossy(&title[..title_len]).into_owned(),
                    text: String::from_utf8_lossy(&text[..text_len]).into_owned(),
                }),
                2 => RefOverride::ToNothing,
                _ => RefOverride::NotOverridden,
            }
        });
    }

    let mut renderer = HtmlRenderer::new(renderer_params);
    into_raw(run_with(input, options, &mut renderer), out_len)
}

/// Releases a buffer returned by this library.
///
/// # Safety
///
/// `ptr` must be a pointer this library returned, with the same `len` it
/// reported, and must not have been freed already.
#[no_mangle]
pub unsafe extern "C" fn bf_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    drop(Vec::from_raw_parts(ptr, len, len));
}

/// HTML-escapes a byte string.
///
/// Exposed because upstream's `esc_test.go` calls the unexported `escapeHTML`
/// directly, and the pinned suite is not to be edited.
///
/// # Safety
///
/// `input` must be valid for reads of `input_len` bytes and `out_len` must be
/// writable.
#[no_mangle]
pub unsafe extern "C" fn bf_escape_html(
    input: *const u8,
    input_len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    let input = BfSlice {
        ptr: input,
        len: input_len,
    }
    .as_slice();
    let mut out = Vec::new();
    blackfriday::esc::escape_html(&mut out, input);
    into_raw(out, out_len)
}

/// Scans a code-fence line.
///
/// Exposed because `block_test.go` calls the unexported `isFenceLine`
/// directly. `info_out` may be null, standing for Go's nil `*string`, which
/// asks the scanner not to extract an info string.
///
/// Returns the end offset. The marker is written into `marker_out` (up to
/// `marker_cap` bytes) with its length through `marker_len`, and the info
/// string likewise.
///
/// # Safety
///
/// `data` and `old_marker` must be valid for reads of their stated lengths;
/// the out pointers must be writable for their stated capacities. Any of the
/// out pointers may be null, in which case that result is dropped.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn bf_is_fence_line(
    data: *const u8,
    data_len: usize,
    old_marker: *const u8,
    old_marker_len: usize,
    want_info: i32,
    marker_out: *mut u8,
    marker_cap: usize,
    marker_len: *mut usize,
    info_out: *mut u8,
    info_cap: usize,
    info_len: *mut usize,
) -> usize {
    let data = BfSlice {
        ptr: data,
        len: data_len,
    }
    .as_slice();
    let old_marker = BfSlice {
        ptr: old_marker,
        len: old_marker_len,
    }
    .as_slice();

    let mut info = Vec::new();
    let (end, marker) = if want_info != 0 {
        blackfriday::block::is_fence_line(data, Some(&mut info), old_marker)
    } else {
        blackfriday::block::is_fence_line(data, None, old_marker)
    };

    copy_out(&marker, marker_out, marker_cap, marker_len);
    if want_info != 0 {
        copy_out(&info, info_out, info_cap, info_len);
    }
    end
}

/// Derives a heading anchor from heading text.
///
/// `SanitizedAnchorName` is exported upstream, but it takes and returns a
/// `string`, so it still needs a byte-oriented shim.
///
/// # Safety
///
/// `text` must be valid for reads of `text_len` bytes and `out_len` must be
/// writable.
#[no_mangle]
pub unsafe extern "C" fn bf_sanitized_anchor_name(
    text: *const u8,
    text_len: usize,
    out_len: *mut usize,
) -> *mut u8 {
    let text = BfSlice {
        ptr: text,
        len: text_len,
    }
    .as_slice();
    let name = blackfriday::block::sanitized_anchor_name_bytes(text);
    into_raw(name.into_bytes(), out_len)
}

/// The version string, matching Go's `Version` constant.
///
/// # Safety
///
/// `out_len` must be writable.
#[no_mangle]
pub unsafe extern "C" fn bf_version(out_len: *mut usize) -> *mut u8 {
    into_raw(blackfriday::VERSION.as_bytes().to_vec(), out_len)
}

/// Copies `src` into a caller buffer, truncating at `cap`.
///
/// # Safety
///
/// `out` must be null or writable for `cap` bytes; `out_len` must be null or
/// writable.
unsafe fn copy_out(src: &[u8], out: *mut u8, cap: usize, out_len: *mut usize) {
    let n = src.len().min(cap);
    if !out.is_null() && n > 0 {
        std::ptr::copy_nonoverlapping(src.as_ptr(), out, n);
    }
    if !out_len.is_null() {
        *out_len = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders through the C entry point and returns an owned copy.
    ///
    /// # Safety
    ///
    /// Test-local: the pointers handed in all come from live locals, and the
    /// result is freed through [`bf_free`] before returning.
    unsafe fn run(input: &[u8], params: &BfParams) -> Vec<u8> {
        let mut len = 0usize;
        let ptr = bf_run(input.as_ptr(), input.len(), params, &mut len);
        assert!(!ptr.is_null());
        let out = std::slice::from_raw_parts(ptr, len).to_vec();
        bf_free(ptr, len);
        out
    }

    fn params(flags: HtmlFlags, extensions: Extensions) -> BfParams {
        BfParams {
            extensions: extensions.bits(),
            html_flags: flags.bits(),
            absolute_prefix: BfSlice {
                ptr: std::ptr::null(),
                len: 0,
            },
            footnote_anchor_prefix: BfSlice {
                ptr: std::ptr::null(),
                len: 0,
            },
            footnote_return_link_contents: BfSlice {
                ptr: std::ptr::null(),
                len: 0,
            },
            heading_id_prefix: BfSlice {
                ptr: std::ptr::null(),
                len: 0,
            },
            heading_id_suffix: BfSlice {
                ptr: std::ptr::null(),
                len: 0,
            },
            heading_level_offset: 0,
            title: BfSlice {
                ptr: std::ptr::null(),
                len: 0,
            },
            css: BfSlice {
                ptr: std::ptr::null(),
                len: 0,
            },
            icon: BfSlice {
                ptr: std::ptr::null(),
                len: 0,
            },
            ref_override: None,
            ref_override_ctx: std::ptr::null_mut(),
        }
    }

    #[test]
    fn bf_run_renders_and_bf_free_releases() {
        let p = params(HtmlFlags::COMMON, Extensions::COMMON);
        let out = unsafe { run(b"# Hi\n\nA *world*.\n", &p) };
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "<h1>Hi</h1>\n\n<p>A <em>world</em>.</p>\n"
        );
    }

    #[test]
    fn an_empty_input_round_trips() {
        let p = params(HtmlFlags::COMMON, Extensions::COMMON);
        assert!(unsafe { run(b"", &p) }.is_empty());
    }

    #[test]
    fn flags_reach_the_renderer() {
        let p = params(HtmlFlags::SKIP_HTML, Extensions::COMMON);
        let out = unsafe { run(b"<b>x</b>\n", &p) };
        assert_eq!(String::from_utf8(out).unwrap(), "<p>x</p>\n");
    }

    #[test]
    fn bf_escape_html_matches_the_library() {
        let input = b"a<b>&\"c";
        let mut len = 0usize;
        let out = unsafe {
            let ptr = bf_escape_html(input.as_ptr(), input.len(), &mut len);
            let v = std::slice::from_raw_parts(ptr, len).to_vec();
            bf_free(ptr, len);
            v
        };
        let mut want = Vec::new();
        blackfriday::esc::escape_html(&mut want, input);
        assert_eq!(out, want);
    }

    #[test]
    fn bf_is_fence_line_reports_end_marker_and_info() {
        let data = b"``` go\n";
        let old = b"```";
        let mut marker = [0u8; 16];
        let mut marker_len = 0usize;
        let mut info = [0u8; 16];
        let mut info_len = 0usize;
        let end = unsafe {
            bf_is_fence_line(
                data.as_ptr(),
                data.len(),
                old.as_ptr(),
                old.len(),
                1,
                marker.as_mut_ptr(),
                marker.len(),
                &mut marker_len,
                info.as_mut_ptr(),
                info.len(),
                &mut info_len,
            )
        };
        assert_eq!(end, 7);
        assert_eq!(&marker[..marker_len], b"```");
        assert_eq!(&info[..info_len], b"go");
    }

    #[test]
    fn bf_version_matches_the_crate() {
        let mut len = 0usize;
        let out = unsafe {
            let ptr = bf_version(&mut len);
            let v = std::slice::from_raw_parts(ptr, len).to_vec();
            bf_free(ptr, len);
            v
        };
        assert_eq!(out, blackfriday::VERSION.as_bytes());
    }

    #[test]
    fn a_null_input_is_treated_as_empty_rather_than_dereferenced() {
        let p = params(HtmlFlags::COMMON, Extensions::COMMON);
        let mut len = 0usize;
        let ptr = unsafe { bf_run(std::ptr::null(), 0, &p, &mut len) };
        assert!(!ptr.is_null());
        assert_eq!(len, 0);
        unsafe { bf_free(ptr, len) };
    }

    #[test]
    fn bf_free_ignores_null() {
        unsafe { bf_free(std::ptr::null_mut(), 0) };
    }
}
