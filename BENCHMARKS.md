# Benchmarks

**The short version: the port is not faster.** Throughput is a dead heat, the
median is a hair ahead, the tail is meaningfully behind, and memory is less than
half. If you are looking for a "Rust rewrite is 10× faster" headline, this is
not one, and the numbers below are the reason rather than a caveat bolted onto
one.

## Results

Corpus: upstream's own `tests/original/testdata`, 23 documents, 42,543 bytes.
Configuration: `Run`'s defaults — `CommonExtensions`, `CommonHTMLFlags`.

| metric | rust | go | rust is |
|---|---:|---:|---:|
| min | 0.635 ms | 0.727 ms | **1.14× faster** |
| p50 | 0.754 ms | 0.777 ms | **1.03× faster** |
| p90 | 0.958 ms | 0.814 ms | 0.85× — *slower* |
| p99 | 1.170 ms | 0.888 ms | 0.76× — *slower* |
| throughput | 51.9 MiB/s | 52.1 MiB/s | 1.00× — level |
| peak RSS | 5.52 MiB | 13.59 MiB | **2.46× less** |
| startup | 5.48 ms | 7.76 ms | **1.42× faster** |

Latency is per pass over the whole corpus. Reproduce with:

```bash
pwsh bench/run.ps1 -Iterations 50 -Batch 40 -Repeats 5
```

## What the numbers say

**Throughput is level.** Neither implementation is doing anything the other is
not; both walk the same bytes through the same algorithms, and it shows.

**Memory is where the port wins**, by 2.5×. Go allocates each `Node`
separately and lets the collector deal with it; the port allocates one arena
and indexes into it. The tree survives the parse either way, so this is not a
lifetime trick — it is one allocation instead of thousands, and no collector
metadata.

**Startup is 1.4× faster**, which is mostly the Go runtime starting rather than
anything about Markdown. It matters for a CLI invoked per file and not at all
for a long-lived server.

**The tail is worse, and that is the interesting number.** The port's p99 is
1.17 ms against Go's 0.89 ms while its median is *better*. Something occasional
is expensive here that is not expensive there.

The cause is the same design that wins on memory. Go's `Node.Literal` is a
**slice of the input** — assigning it copies no bytes. The port's is a
`Vec<u8>`, so every text node copies its contents out of the document. Most of
those copies are small and the allocator handles them from a warm free list,
which is why the median is fine; occasionally one is not, and that shows up at
p99.

Fixing it properly means giving the arena a lifetime parameter so nodes can
borrow the input, which changes the public API — `Markdown<'a>`, `Node<'a>`,
and a `parse` whose result cannot outlive its argument. That is a real design
decision, not a micro-optimisation, and it is not one to make in a hurry at the
end of a port whose whole claim is behavioural equivalence.

One thing *was* worth fixing. The arena's backing `Vec` grew during parsing,
and `Node` is a wide struct, so each doubling was a large memcpy landing
mid-parse. `Markdown::parse` now reserves from the input length up front, which
took p99 from about 1.25 ms to about 1.05 ms in isolation. The divisor is a
measured guess documented at the call site; being wrong costs a little memory,
never correctness.

## Method

`bench/rust/main.rs` and `bench/go/main.go` are deliberately the same program
written twice, rather than one harness with two backends — a shared harness
would sit on one side of the language boundary and charge its costs to the
other. Both parse and render the corpus, record every individual sample, sort
it, and report nearest-rank percentiles from the full sample. No criterion, no
dependencies, no statistical machinery.

Percentiles come from the sorted sample rather than from a mean and a standard
deviation, because rendering latency is not normally distributed and the p99 is
the number that matters to anything serving requests.

### One sample times forty passes, not one

The first run reported Go's p50 as **exactly 0.9995 ms on three consecutive
runs** while the Rust side varied in the fourth decimal. That is not a fast
implementation, it is a coarse clock: a single pass over the corpus takes under
a millisecond, and Go's clock on this platform quantises to roughly that.

So a sample times `-batch` passes (40 by default) and divides afterwards. The
sample is then well above the quantum on both sides, and the percentiles mean
something. Anyone reproducing this should keep the batch large enough that the
reported `min_ms` is not suspiciously round.

### Memory and startup are measured from outside

Asking a process how much memory it is using needs either `unsafe` or a
dependency on the Rust side, and this repository claims neither. `bench/run.ps1`
polls the child's working set from the parent instead, and times a process that
starts, renders one byte, and exits.

Polling rather than reading `PeakWorkingSet64` after exit is deliberate: that
property came back as zero here, and a number that is silently zero is worse
than no number at all.

### Caveats worth stating

- **One machine, one platform.** Windows 11, MSVC-hosted Rust 1.97, Go 1.23.
  The allocator and the clock are both platform-specific, and both matter here.
- **One corpus, and a small one.** 42 KB of hand-written Markdown from 2004,
  which is what upstream ships. A corpus of large documents would weight the
  arena's advantages differently, and one of pathological documents differently
  again.
- **Best of five repeats**, to suppress scheduler noise on a machine that is
  not otherwise quiet. The spread between repeats was a few percent.
- The port is compiled with `lto = true` and `codegen-units = 1`. Go is built
  with its defaults, because that is how Go is built.
