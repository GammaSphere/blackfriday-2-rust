# Five-minute demo: shot list

The brief asks for "a five-minute demo video showing the original suite running
against the port and explaining the evidence." The parity run is the required
content; everything else earns its place only if it fits around it.

Five minutes is less than it sounds. This plan is budgeted to **4:50** so there
is room to breathe.

## Before you hit record

Run all of this first, so nothing compiles or downloads on camera:

```bash
cd E:\projects\raptors-v2
cargo build --release                    # ~5s, and warms the cache
cd fuzz && go build -o goserve.exe ./cmd/goserve && go build -o bf-fuzz.exe . && cd ..
cd bench/go && go build -o bf-bench-go.exe . && cd ../..
```

Then:

- Terminal font at **18–20pt**, window about 100×30. Judges may watch on a laptop.
- `clear` between segments.
- Close anything that could pop a notification.
- Have `README.md`, `BUGS.md` and `fuzz/log.txt` open in tabs to cut to.
- Record at 1080p. OBS or Windows Game Bar (`Win+G`) are both fine.

One rehearsal run end to end. The parity segment is the only one with real
waiting in it, and knowing exactly how long it takes is the difference between
filling the silence and apologising for it.

---

## 0:00 – 0:25 — What this is

**On screen:** the top of `README.md`.

> "This is blackfriday — Russ Ross's Markdown processor, about seven thousand
> lines of Go — ported to Rust. Track E. The claim I want to make is not that
> it's rewritten, it's that it *behaves the same*, and everything I'm about to
> show is reproducible from a clean clone."

Point at the results table. Do not read it aloud; it is on screen.

## 0:25 – 1:00 — One command, and it runs

**Terminal:**

```bash
cargo clean && cargo build --release
```

Comes back in about five seconds with four binaries.

```bash
./target/release/bf --footnotes < docs/demo.md | head -30
```

> "Zero dependencies — `cargo tree` is one line. Zero `unsafe` in everything
> that ships. And no Go anywhere in this binary; Go only shows up later, when
> the *original test suite* calls into it."

That last sentence matters. Say it here, once, clearly. It is the fastest way
to kill the "is this a wrapper?" question before it forms.

## 1:00 – 2:20 — The original suite, unmodified ← *the required shot*

**Terminal:**

```bash
make parity
```

While `verify-hashes` prints, talk:

> "These are upstream's own test files. Not adapted, not transliterated — the
> same bytes blackfriday ships. That hash is the manifest digest recorded at
> kickoff, before any porting started, and it hasn't moved."

Point at `dd277c90…818a9` on screen.

The Go test run takes **about 22 seconds**. Fill it:

> "What's happening now is Go's test binary running against Rust. There's a Go
> package here called `blackfriday` whose exported API matches upstream's
> signature for signature, and whose bodies talk to the Rust port over a pipe.
> Two unexported functions are in there too — `escapeHTML` and `isFenceLine` —
> because the suite calls them directly. Those are the only two it reaches,
> which is why none of the pinned files needed an edit."

Land on:

```text
ok  github.com/GammaSphere/blackfriday-2-rust/adapter  21.6s
```

> "Sixty-five of sixty-five."

Cut to `PARITY.md` for two seconds to show the per-test list exists.

## 2:20 – 3:05 — Differential fuzzing

**On screen:** `fuzz/log.txt`, top and bottom.

> "Real blackfriday on one side, the port on the other, same inputs, byte
> comparison. Eight hundred and four thousand inputs, zero divergences."

Then, briefly:

> "Both sides run as supervised child processes, and that wasn't a design
> flourish — some inputs make blackfriday loop forever, and neither language
> can interrupt its own wedged computation. An in-process fuzzer stops at the
> first one. Mine kills the child and carries on, which is why it found the
> second hang instead of dying on the first."

Optional if the pace allows — start a live run and let it scroll for ten
seconds:

```bash
cd fuzz && ./bf-fuzz.exe -duration 30s -limit 2s
```

## 3:05 – 4:05 — Five bugs, one of them live ← *the memorable shot*

**Terminal.** Do this one live; it is eight bytes and it is unarguable.

```bash
printf '\r\n\t+ \n: ' | timeout 5 ./target/release/bf ; echo "exit=$?"
```

`exit=124`. Then the same input against the real thing:

```bash
cd fuzz && printf '\r\n\t+ \n: ' | timeout 5 ./repro.exe -side go -ext common ; echo "exit=$?"
```

`exit=124` again.

> "Eight bytes. No options — that's `Run(input)` with the defaults, which is the
> one-line way the README tells you to use the library. `paragraph` hands off to
> the definition-list parser and returns a count measured from the wrong origin,
> and that count can be zero, so the block loop re-parses the same slice
> forever. Upstream hangs. The port hangs in exactly the same place, because
> the goal is equivalence, not repair."

**Cut to `BUGS.md`**, scroll the five headings.

> "Five defects, all with public-API reproducers. Two of them are denial of
> service. The fuzzer found the last two; the first three came out of measuring
> the Go rather than reading it."

## 4:05 – 4:35 — Benchmarks, honestly

**On screen:** the table in `BENCHMARKS.md`.

> "I'm not going to claim a speedup, because there isn't one. Throughput is a
> dead heat. Memory is two and a half times better and startup is one point
> four. The p99 is *worse* — 1.17 against 0.89 — and I know why: Go's node
> literals are slices of the input and mine are owned copies. Fixing it means
> putting a lifetime on the arena, which changes the public API, and that's not
> a decision to make in the last hour of a port."

One sentence on method:

> "Go's clock quantised every percentile to the same number until I made each
> sample time forty passes instead of one. That's in the write-up too."

## 4:35 – 4:50 — Where to look

> "One declared divergence, and it's upstream reading past the end of a slice —
> I couldn't reproduce that without reading uninitialised memory. `DECISIONS.md`
> has nineteen entries. Everything is in the repo. Thanks."

---

## If you overrun

Cut in this order:

1. The live fuzz run at 2:20 — the log says the same thing.
2. The `PARITY.md` cutaway at 2:15.
3. The method sentence at 4:30.

**Never cut:** `make parity` finishing, or the eight-byte hang. Those two are
the submission.

## If a command misbehaves on camera

- `make` is not installed on the dev machine used for this port. Either install
  it, or use the spelled-out body in `docs/PORT-STATUS.md` under "Running the
  parity suite" — it is the same commands.
- If `bf-serve` is missing, `cargo build --release` makes it.
- `timeout` comes from Git Bash on Windows. In PowerShell, run the hang demo
  from a `bash` shell, or just let it hang and press Ctrl-C on camera — that is
  arguably a better shot anyway.
