# Five-minute demo: the script

Everything you need to record, in order. **🎙 lines are read aloud verbatim.**
**⌨ blocks are copy-paste.** **🖥 says what should be on screen.**

**593 words of narration.** At 145 words a minute that is 4:05 of speech; at a
slower 130 it is 4:33. Add roughly twenty seconds of watching commands run and
you land between **4:25 and 4:55**. The required content is the original test
suite running against the port; everything else fits around it.

---

## Before you press record

Run all of this once, so nothing compiles or downloads on camera:

```powershell
cd E:\projects\raptors-v2
cargo build --release
cd fuzz; go build -o goserve.exe ./cmd/goserve; go build -o bf-fuzz.exe .; go build -o repro.exe ./cmd/repro; cd ..
cd bench\go; go build -o bf-bench-go.exe .; cd ..\..
.\scripts\parity.ps1
```

That last line warms the Go build cache, so the take runs in ~23 seconds
instead of ~40.

Then:

- Terminal font **18–20pt**, window about 100×30.
- One PowerShell window for everything. No shell switching on camera.
- `clear` between segments.
- Turn off notifications.
- Have these open in editor tabs to cut to: `README.md`, `BUGS.md`,
  `fuzz/log.txt`, `BENCHMARKS.md`.
- Record 1080p. OBS or `Win+G` both fine.

Do one rehearsal end to end. The parity segment is the only real wait, and
knowing its length is the difference between filling the silence and
apologising for it.

---

# 1 · Opening — 0:00 to 0:20

🖥 **The top of `README.md`**, results table visible.

🎙
> This is blackfriday, a Markdown processor written in Go by Russ Ross. About
> seven thousand lines. I've ported it to Rust, for Track E.
>
> My claim isn't that I rewrote it. It's that it behaves the same. So the next
> four minutes are evidence, and all of it reproduces from a clean clone.

---

# 2 · One command, and it runs — 0:20 to 0:55

⌨ **Build from nothing.**

```powershell
cargo clean; cargo build --release
```

🎙 *(while it builds, about five seconds)*
> One command, from an empty target directory. A library and three binaries.

⌨ **Render a document.**

```powershell
.\target\release\bf --footnotes --toc docs\demo.md
```

🖥 Let the HTML scroll. Don't read it.

🎙
> Zero dependencies. Zero unsafe in everything that ships.
>
> And there's no Go in that binary. Go appears exactly once in this project, in
> a minute, when the *original test suite* calls into the Rust.

🖥 **Pause on that last sentence.** It kills the "is this just a wrapper?"
question before it forms.

---

# 3 · The original suite — 0:55 to 2:15 ← *required*

⌨ **The one that matters.**

```powershell
.\scripts\parity.ps1
```

🖥 Hash verification prints first, then a ~23 second wait, then `ok`.

🎙 *(as the hashes print)*
> These are upstream's own test files. Not adapted, not translated. The same
> bytes blackfriday ships.
>
> That hash is the manifest digest recorded at kickoff, before I wrote a line
> of port code. It hasn't moved.

🖥 **Point at `dd277c90…818a9`.**

🎙 *(during the 23-second wait — this is your filler, take it slowly)*
> What's running now is Go's test binary, driving Rust.
>
> There's a Go package in here called blackfriday. Its exported API matches
> upstream's, signature for signature. But the bodies don't implement Markdown.
> They send the input to the Rust port over a pipe and hand back what comes out.
>
> Two unexported functions are in there too, `escapeHTML` and `isFenceLine`,
> because the suite calls those directly. They're the only two it reaches. That
> is why none of the pinned files needed an edit.
>
> It was going to be cgo. This machine has no C compiler, so a pipe it is. The
> upside is that reproducing this needs only Go and Rust, on any platform.

🖥 **Land on:**

```text
PARITY: 65 of 65 pass
```

🎙
> Sixty-five of sixty-five.

🖥 Cut to `PARITY.md` for two seconds — show the per-test list exists.

---

# 4 · Differential fuzzing — 2:15 to 2:55

🖥 **`fuzz/log.txt`.** Show the header, then jump to the last three lines.

🎙
> Real blackfriday on one side, the port on the other. Same inputs, compared
> byte for byte. Eight hundred and four thousand of them. Zero divergences.
>
> Both sides run as supervised child processes, and that wasn't decoration.
> Some inputs make blackfriday loop forever, and neither language can interrupt
> its own wedged computation. So an in-process fuzzer stops dead at the first
> one. Mine kills the child and carries on.

⌨ *(optional, only if you're ahead of time — let it scroll ten seconds)*

```powershell
cd fuzz; .\bf-fuzz.exe -duration 30s -limit 2s; cd ..
```

---

# 5 · Five bugs, one of them live — 2:55 to 3:55 ← *the memorable one*

⌨ **Show the input first.**

```powershell
Format-Hex docs\repro\bug5-hang.md
```

🖥 Eight bytes: `0D 0A 09 2B 20 0A 3A 20`.

🎙
> Eight bytes. Carriage return, newline, tab, plus, space, newline, colon,
> space.

⌨ **Feed it to the port.**

```powershell
.\target\release\bf docs\repro\bug5-hang.md
```

🖥 **Nothing happens. Let it sit three or four seconds. Then Ctrl-C on camera.**

🎙
> That's the port. It never returns.

⌨ **Now the original.**

```powershell
.\fuzz\repro.exe -side go -ext common -file docs\repro\bug5-hang.md
```

🖥 **Also nothing. Let it sit. Ctrl-C again.**

🎙
> And that's real blackfriday, 2.1.0, straight from the module proxy. Same
> eight bytes, same result.
>
> No options either. That's `Run(input)` with the defaults, which is the
> one-line way the README tells you to use it.
>
> The paragraph parser hands off to the definition-list parser and returns a
> byte count measured from the wrong starting point. That count can be zero.
> When it is, the block loop parses the same slice again, forever.
>
> The port does it too, in the same place, because the goal is equivalence, not
> repair.

🖥 **Cut to `BUGS.md`.** Scroll the five headings.

🎙
> Five defects, each with a public-API reproducer. Two are denial of service.
> The fuzzer found the last two. The first three came out of measuring the Go
> rather than reading it.

---

# 6 · Benchmarks, honestly — 3:55 to 4:30

🖥 **The table in `BENCHMARKS.md`.**

🎙
> I'm not going to claim a speedup, because there isn't one.
>
> Throughput is a dead heat. Memory is two and a half times better, startup
> about one and a half.
>
> And the p99 is worse. 1.17 milliseconds against 0.89. Go's node literals are
> slices of the input; mine are owned copies. Fixing it means putting a lifetime
> on the arena, which changes the public API. Not a decision for the last hour
> of a port, so I measured it and wrote it down.
>
> One note on method. Go's clock reported every percentile as the same number
> until I made each sample time forty passes instead of one.

---

# 7 · Close — 4:30 to 4:45

🖥 **Back to `README.md`.**

🎙
> One declared divergence, and it's upstream reading past the end of a slice.
> I couldn't reproduce that without reading uninitialised memory.
>
> Nineteen entries in the decision log. It's all in the repo. Thanks.

---

## If you overrun

Cut in this order:

1. The live fuzz run in segment 4. The log says the same thing.
2. The `PARITY.md` cutaway in segment 3.
3. The methodology note in segment 6.

**Never cut:** `.\scripts\parity.ps1` finishing, or the eight-byte hang. Those
two are the submission.

## If something misbehaves

| symptom | fix |
|---|---|
| `bf < file.md` fails | PowerShell reserves `<`. Use the file argument, as every command above does. |
| `bf-serve not found` | `cargo build --release` |
| `repro.exe not found` | `cd fuzz; go build -o repro.exe ./cmd/repro` |
| parity takes 40s+ | The Go build cache is cold. Run it once before recording. |
| Ctrl-C doesn't stop `bf` | Close the tab. On a rerecord, wrap it: `Start-Process .\target\release\bf -ArgumentList docs\repro\bug5-hang.md` and kill it. |

## Word budget, per segment

| segment | words | speech at 145 wpm |
|---|---:|---:|
| 1 · Opening | 53 | 0:22 |
| 2 · Build and run | 46 | 0:19 |
| 3 · The original suite | 150 | 1:02 |
| 4 · Fuzzing | 68 | 0:28 |
| 5 · The bugs | 136 | 0:56 |
| 6 · Benchmarks | 107 | 0:44 |
| 7 · Close | 33 | 0:14 |
| **total** | **593** | **4:05** |

Segment 3 is the longest on purpose: most of it is spoken over the parity run,
not before it. If you finish talking before `go test` finishes, stop and let it
land in silence — the result on screen is the point, not the commentary.

If you naturally read faster than this, slow down rather than adding material.
