//! `bf` — renders Markdown from stdin to HTML on stdout.
//!
//! ```text
//! bf < README.md
//! bf --footnotes --toc < doc.md
//! ```
//!
//! Small on purpose. It exists so `cargo build --release` produces something
//! runnable rather than only a library, so the port can be driven from a shell
//! without writing a program, and so a reproducer from `BUGS.md` can be tried
//! against the port directly rather than through the parity harness.
//!
//! Exit status is `0` on success, `1` on an I/O failure, `2` on a bad option —
//! deliberately boring, because a demo that has to explain its own exit codes
//! is not demonstrating the library.

use std::io::{Read, Write};

use blackfriday::html::{HtmlRenderer, HtmlRendererParameters};
use blackfriday::markdown::{run_with, Options};
use blackfriday::{Extensions, HtmlFlags};

/// What `--help` prints.
const USAGE: &str = "\
usage: bf [OPTIONS] < input.md > output.html

Renders Markdown on stdin to HTML on stdout, with blackfriday's semantics.
Defaults match Go's Run(): CommonExtensions and CommonHTMLFlags.

Parser extensions:
  --footnotes           [^1] references and their definitions
  --titleblock          a leading %-prefixed title block
  --definition-lists    term / : definition
  --auto-heading-ids    derive an id from each heading's text
  --no-extensions       turn every extension off

Renderer flags:
  --toc                 emit a table of contents, rewriting heading ids
  --complete-page       wrap the output in <html><head><body>
  --skip-html           drop raw HTML instead of passing it through
  --title TEXT          document title, used with --complete-page

  -h, --help            print this and exit
  -V, --version         print the blackfriday version this tracks
";

fn main() {
    let mut extensions = Extensions::COMMON;
    let mut flags = HtmlFlags::COMMON;
    let mut title = String::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--footnotes" => extensions |= Extensions::FOOTNOTES,
            "--titleblock" => extensions |= Extensions::TITLEBLOCK,
            "--definition-lists" => extensions |= Extensions::DEFINITION_LISTS,
            "--auto-heading-ids" => extensions |= Extensions::AUTO_HEADING_IDS,
            "--no-extensions" => extensions = Extensions::from_bits_retain(0),
            "--toc" => flags |= HtmlFlags::TOC,
            "--complete-page" => flags |= HtmlFlags::COMPLETE_PAGE,
            "--skip-html" => flags |= HtmlFlags::SKIP_HTML,
            "--title" => title = args.next().unwrap_or_default(),
            "--help" | "-h" => {
                print!("{USAGE}");
                return;
            }
            "--version" | "-V" => {
                println!(
                    "bf (blackfriday-rs), tracking blackfriday {}",
                    blackfriday::VERSION
                );
                return;
            }
            other => {
                eprintln!("bf: unknown option {other}");
                eprintln!("try `bf --help`");
                std::process::exit(2);
            }
        }
    }

    let mut input = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut input) {
        eprintln!("bf: reading stdin: {e}");
        std::process::exit(1);
    }

    let mut renderer = HtmlRenderer::new(HtmlRendererParameters {
        flags,
        title,
        ..Default::default()
    });
    let out = run_with(
        &input,
        Options::none().with_extensions(extensions),
        &mut renderer,
    );

    if let Err(e) = std::io::stdout().write_all(&out) {
        eprintln!("bf: writing stdout: {e}");
        std::process::exit(1);
    }
}
