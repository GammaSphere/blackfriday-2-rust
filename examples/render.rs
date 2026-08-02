//! Renders Markdown from stdin to HTML on stdout.
//!
//! ```text
//! cargo run --release --example render < README.md
//! cargo run --release --example render -- --footnotes --toc < doc.md
//! ```
//!
//! Small on purpose: it exists so the library can be driven from a shell
//! without writing a program, and so a reproducer from `BUGS.md` can be tried
//! directly against the port rather than through the parity harness.

use std::io::{Read, Write};

use blackfriday::html::{HtmlRenderer, HtmlRendererParameters};
use blackfriday::markdown::{run_with, Options};
use blackfriday::{Extensions, HtmlFlags};

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
                println!("usage: render [--footnotes] [--titleblock] [--definition-lists]");
                println!("              [--auto-heading-ids] [--no-extensions] [--toc]");
                println!("              [--complete-page] [--skip-html] [--title TEXT]");
                println!("reads Markdown on stdin, writes HTML on stdout");
                return;
            }
            other => {
                eprintln!("render: unknown option {other}");
                std::process::exit(2);
            }
        }
    }

    let mut input = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut input) {
        eprintln!("render: {e}");
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
        eprintln!("render: {e}");
        std::process::exit(1);
    }
}
