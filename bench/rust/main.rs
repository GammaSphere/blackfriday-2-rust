//! Timing harness for the port.
//!
//! Deliberately boring: no criterion, no statistical machinery, no
//! dependencies. It renders one corpus a fixed number of times, records every
//! individual duration, and prints percentiles. `bench/go/main.go` does the
//! identical thing with real blackfriday, so the two numbers are comparable
//! because the two programs are the same program.
//!
//! Percentiles come from the full sorted sample rather than from a mean and a
//! standard deviation: rendering latency is not normally distributed, and a
//! p99 is the number that matters for anything serving requests.
//!
//! Peak memory and process startup are *not* measured in here. Asking a
//! process how much memory it is using needs either `unsafe` or a dependency,
//! and either would compromise what this repository is claiming. They are
//! measured from outside instead — see `bench/run.ps1`.

use std::time::Instant;

use blackfriday::html::{HtmlRenderer, HtmlRendererParameters};
use blackfriday::markdown::{run_with, Options};
use blackfriday::HtmlFlags;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut corpus_dir = String::from("../../tests/original/testdata");
    let mut iterations = 200usize;
    let mut batch = 25usize;
    let mut mode = String::from("bench");

    while let Some(a) = args.next() {
        match a.as_str() {
            "-corpus" => corpus_dir = args.next().unwrap_or(corpus_dir),
            "-n" => {
                iterations = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(iterations)
            }
            "-batch" => {
                batch = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(batch)
                    .max(1)
            }
            "-mode" => mode = args.next().unwrap_or(mode),
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
    }

    // `startup` renders the smallest possible document and exits, so timing
    // the whole process from outside measures process start plus one render.
    if mode == "startup" {
        let out = render(b"x");
        std::process::exit(i32::from(out.is_empty()));
    }

    let corpus = load_corpus(&corpus_dir);
    if corpus.is_empty() {
        eprintln!("no corpus found under {corpus_dir}");
        std::process::exit(2);
    }
    let total_bytes: usize = corpus.iter().map(Vec::len).sum();

    // Warm up, so the first timed pass is not paying for cold caches or a
    // first-touch page fault on every allocation.
    for _ in 0..10 {
        for doc in &corpus {
            std::hint::black_box(render(doc));
        }
    }

    // One sample times `batch` passes over the corpus, not one. A single pass
    // takes under a millisecond, and Go's clock on Windows quantises to about
    // that -- its p50 came back as exactly 0.9995 ms on three consecutive runs
    // while this side's varied in the fourth decimal. Timing a batch puts the
    // sample well above the quantum on both sides; dividing by `batch`
    // afterwards restores per-pass units.
    let mut samples: Vec<f64> = Vec::with_capacity(iterations);
    let wall = Instant::now();
    for _ in 0..iterations {
        let start = Instant::now();
        for _ in 0..batch {
            for doc in &corpus {
                std::hint::black_box(render(doc));
            }
        }
        samples.push(start.elapsed().as_secs_f64() * 1e3 / batch as f64);
    }
    let elapsed = wall.elapsed().as_secs_f64();

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let throughput = (total_bytes * iterations * batch) as f64 / elapsed / (1024.0 * 1024.0);

    println!("impl=rust");
    println!("documents={}", corpus.len());
    println!("corpus_bytes={total_bytes}");
    println!("iterations={iterations}");
    println!("batch={batch}");
    println!("min_ms={:.4}", samples[0]);
    println!("p50_ms={:.4}", percentile(&samples, 0.50));
    println!("p90_ms={:.4}", percentile(&samples, 0.90));
    println!("p99_ms={:.4}", percentile(&samples, 0.99));
    println!("max_ms={:.4}", samples[samples.len() - 1]);
    println!("total_s={elapsed:.4}");
    println!("throughput_mib_s={throughput:.2}");
}

/// One pass: parse and render with the defaults `Run` uses.
fn render(input: &[u8]) -> Vec<u8> {
    let mut renderer = HtmlRenderer::new(HtmlRendererParameters {
        flags: HtmlFlags::COMMON,
        ..Default::default()
    });
    run_with(input, Options::common(), &mut renderer)
}

/// Nearest-rank percentile over an already-sorted sample.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn load_corpus(dir: &str) -> Vec<Vec<u8>> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "text"))
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths.iter().filter_map(|p| std::fs::read(p).ok()).collect()
}
