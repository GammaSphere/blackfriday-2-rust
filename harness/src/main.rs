//! Serves the port over stdin/stdout, so Go can drive it.
//!
//! # Why a pipe and not cgo
//!
//! The plan was a cgo adapter over `ffi/`'s C ABI. cgo needs a C compiler and
//! an object format both toolchains agree on; the machine this was built on
//! has `CGO_ENABLED=0`, no gcc, and an MSVC-hosted Rust, so an MSVC `.lib`
//! would have had to link into a MinGW build that is not installed.
//!
//! What actually matters is unchanged: the pinned suite in `tests/original/`
//! stays byte-identical to upstream's and runs against this code. Only the
//! transport differs, and a pipe needs no C toolchain at all, which makes the
//! parity run reproducible on any machine with Go and Rust. `ffi/` remains the
//! embedding path for callers who do have a C compiler.
//!
//! # Protocol
//!
//! Frames are little-endian. A request is
//!
//! ```text
//! u8  op
//! u32 argc
//! argc times: u32 len, len bytes
//! ```
//!
//! and a response is
//!
//! ```text
//! u8  status      0 = result, 1 = the server is asking for a reference override
//! u32 valc
//! valc times: u32 len, len bytes
//! ```
//!
//! Status 1 interrupts a `Run` mid-render: the client answers with a frame in
//! the *request* shape carrying the override result, and then reads again.
//! Everything is synchronous on the one pipe, so there is no framing state
//! beyond this.
//!
//! # No threads, and why that took a rewrite
//!
//! The override callback has to read and write the same pipe the request loop
//! owns. Moving the render onto another thread does not work: the library's
//! override is a `Box<dyn Fn>` with no `Send` bound, matching Go's, and adding
//! one to suit this harness would be the tail wagging the dog. Instead nothing
//! holds a lock across a call — `io::stdin()` and `io::stdout()` are global
//! handles whose buffers survive being re-locked, so the closure simply locks
//! them again while the request loop is not.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};

use blackfriday::block::{is_fence_line, sanitized_anchor_name_bytes};
use blackfriday::esc::escape_html;
use blackfriday::html::{HtmlRenderer, HtmlRendererParameters};
use blackfriday::markdown::{run_with, Options, RefOverride, Reference};
use blackfriday::{Extensions, HtmlFlags};

/// Render Markdown to HTML.
const OP_RUN: u8 = 1;
/// HTML-escape a byte string.
const OP_ESCAPE_HTML: u8 = 2;
/// Scan a code-fence line.
const OP_IS_FENCE_LINE: u8 = 3;
/// Derive a heading anchor.
const OP_SANITIZED_ANCHOR_NAME: u8 = 4;
/// Report the version string.
const OP_VERSION: u8 = 5;

/// This frame carries a result.
const STATUS_RESULT: u8 = 0;
/// This frame is a request for a reference override.
const STATUS_NEED_REF: u8 = 1;
/// The port panicked on this input; the single value is the message.
const STATUS_PANIC: u8 = 2;

/// Runs `f`, turning a panic into a message instead of unwinding out of the
/// request loop.
///
/// The port reproduces upstream's panics deliberately — six measured inputs
/// make blackfriday panic, and matching that is the point. A harness that died
/// on the first of them would be useless, and worse, it would die *between*
/// reading a request and writing a response, leaving the pipe half a frame out
/// of step so the next exchange blocks forever. That is exactly what happened
/// the first time the fuzzer ran long enough to find one.
///
/// The Go side turns [`STATUS_PANIC`] back into a Go panic, so a caller's
/// `recover` sees what it would have seen from upstream.
fn catching<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    // The default hook would print a backtrace per input, which for a fuzzer
    // is megabytes of noise about behaviour that is expected.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(previous);

    result.map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_string())
    })
}

fn main() -> io::Result<()> {
    loop {
        let Some((op, args)) = read_frame()? else {
            return Ok(()); // the client closed the pipe
        };

        let outcome: Result<Vec<Vec<u8>>, String> = match op {
            OP_RUN => catching(|| vec![do_run(&args)]),
            OP_ESCAPE_HTML => catching(|| {
                let mut out = Vec::new();
                escape_html(&mut out, arg(&args, 0));
                vec![out]
            }),
            OP_IS_FENCE_LINE => catching(|| {
                let want_info = arg(&args, 2).first().copied().unwrap_or(0) != 0;
                let mut info = Vec::new();
                let (end, marker) = if want_info {
                    is_fence_line(arg(&args, 0), Some(&mut info), arg(&args, 1))
                } else {
                    is_fence_line(arg(&args, 0), None, arg(&args, 1))
                };
                vec![(end as u64).to_le_bytes().to_vec(), marker, info]
            }),
            OP_SANITIZED_ANCHOR_NAME => {
                catching(|| vec![sanitized_anchor_name_bytes(arg(&args, 0)).into_bytes()])
            }
            OP_VERSION => catching(|| vec![blackfriday::VERSION.as_bytes().to_vec()]),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown op {op}"),
                ))
            }
        };

        match outcome {
            Ok(vals) => write_frame(STATUS_RESULT, &vals)?,
            Err(message) => write_frame(STATUS_PANIC, &[message.into_bytes()])?,
        }
    }
}

/// Renders one document, servicing override callbacks as they arise.
fn do_run(args: &[Vec<u8>]) -> Vec<u8> {
    let params = HtmlRendererParameters {
        flags: HtmlFlags::from_bits_retain(i32_arg(args, 2)),
        heading_level_offset: i32_arg(args, 3),
        absolute_prefix: string_arg(args, 4),
        footnote_anchor_prefix: string_arg(args, 5),
        footnote_return_link_contents: string_arg(args, 6),
        heading_id_prefix: string_arg(args, 7),
        heading_id_suffix: string_arg(args, 8),
        title: string_arg(args, 9),
        css: string_arg(args, 10),
        icon: string_arg(args, 11),
    };
    let has_override = arg(args, 12).first().copied().unwrap_or(0) != 0;

    let mut options =
        Options::none().with_extensions(Extensions::from_bits_retain(i32_arg(args, 1)));

    if has_override {
        options = options.with_ref_override(|id: &str| ask_client_for_override(id));
    }

    let mut renderer = HtmlRenderer::new(params);
    run_with(arg(args, 0), options, &mut renderer)
}

/// Asks the client to resolve a reference, over the same pipe.
///
/// A failure here cannot be reported upwards — the callback's signature has no
/// room for one, matching Go's — so a broken pipe degrades to "not
/// overridden", which is the answer a caller with no override would give.
fn ask_client_for_override(id: &str) -> RefOverride {
    if write_frame(STATUS_NEED_REF, &[id.as_bytes().to_vec()]).is_err() {
        return RefOverride::NotOverridden;
    }
    let Ok(Some((_, reply))) = read_frame() else {
        return RefOverride::NotOverridden;
    };
    match arg(&reply, 0).first().copied().unwrap_or(0) {
        1 => RefOverride::To(Reference {
            link: String::from_utf8_lossy(arg(&reply, 1)).into_owned(),
            title: String::from_utf8_lossy(arg(&reply, 2)).into_owned(),
            text: String::from_utf8_lossy(arg(&reply, 3)).into_owned(),
        }),
        2 => RefOverride::ToNothing,
        _ => RefOverride::NotOverridden,
    }
}

/// Borrows argument `n`, or an empty slice when it is absent.
fn arg(args: &[Vec<u8>], n: usize) -> &[u8] {
    args.get(n).map_or(&[][..], Vec::as_slice)
}

/// Argument `n` as a little-endian `i32`.
fn i32_arg(args: &[Vec<u8>], n: usize) -> i32 {
    let a = arg(args, n);
    if a.len() < 4 {
        return 0;
    }
    i32::from_le_bytes([a[0], a[1], a[2], a[3]])
}

/// Argument `n` as a `String`, replacing invalid UTF-8.
///
/// Every field this is used for is already a `string` on the Go side, so no
/// arbitrary document bytes pass through it.
fn string_arg(args: &[Vec<u8>], n: usize) -> String {
    String::from_utf8_lossy(arg(args, n)).into_owned()
}

/// Reads one frame, or `None` at a clean end of input.
fn read_frame() -> io::Result<Option<(u8, Vec<Vec<u8>>)>> {
    let stdin = io::stdin();
    let mut r = stdin.lock();

    let mut tag = [0u8; 1];
    match r.read_exact(&mut tag) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }

    let count = read_u32(&mut r)? as usize;
    let mut args = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        let len = read_u32(&mut r)? as usize;
        let mut buf = vec![0u8; len];
        r.read_exact(&mut buf)?;
        args.push(buf);
    }
    Ok(Some((tag[0], args)))
}

/// Writes one frame and flushes it.
fn write_frame(status: u8, vals: &[Vec<u8>]) -> io::Result<()> {
    let stdout = io::stdout();
    let mut w = stdout.lock();
    w.write_all(&[status])?;
    w.write_all(&(vals.len() as u32).to_le_bytes())?;
    for v in vals {
        w.write_all(&(v.len() as u32).to_le_bytes())?;
        w.write_all(v)?;
    }
    w.flush()
}

/// Reads a little-endian `u32`.
fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
