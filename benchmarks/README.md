# Startup benchmark

Run the Windows release harness from the repository root:

```powershell
./tools/benchmark.ps1 -Runs 100 -Warmup 10 -Output benchmarks/latest.jsonl
```

Each measured launch appends one fixed-version JSON object after the harness has delivered and
verified `U+E000`, observed the following Scintilla paint, waited two input-free seconds after
`FullyReady`, sampled private working set, and closed the child process. Warmup launches are not
written. The summary reports sorted p50 and p95 values for all nine startup milestones.

Use `-EnforceReference` on the documented reference machine. It fails when warm rendered-input TTI
p50 is at least 25,000 microseconds or p95 is at least 40,000 microseconds.

Compare two saved distributions with:

```powershell
cargo run --release --bin fastpad-bench -- compare baseline.jsonl candidate.jsonl
```

A milestone is reported as a regression only when candidate p95 increases by at least the larger
of 2,000 microseconds or 10 percent and the deterministic 10,000-resample bootstrap 95 percent
confidence interval for the p95 delta excludes zero.

## Markdown preview

Preview costs are measured in-process with ignored tests:

```powershell
cargo test --release --test markdown_preview -- --ignored --test-threads=1 --nocapture
```

Targets: preview open on 100 KB < 50 ms p95; one-paragraph update in a 1 MB document < 2 ms p95;
keystroke cost with the side-by-side preview open within 10% (+100 µs) of no preview; no private
memory growth across repeated open/close cycles (median of five closes within 2 MB of a reference
close taken after two warm-up cycles). The first open loads Direct2D, DirectWrite, Direct3D, and
the GPU driver for the rest of the session, about 45 MB of private bytes that closing does not
return.

Startup with a Markdown file is compared against the pre-preview baseline with
`./tools/benchmark.ps1 -LaunchFile benchmarks/fixtures/sample.md` on both builds and
`fastpad-bench compare`. A `--launch-file` run types its benchmark character into the initial
Untitled tab, not into the launch file, which opens in a tab of its own; the rendered-input event
is still required, but the buffer check is skipped, and the harness marks the Untitled tab saved
before closing so no save prompt blocks the exit.

Release binary size (`FastPad.exe`, release profile with `release-package`, as `tools/package.ps1`
builds it): 511,488 bytes before the preview, 957,440 bytes with it (+445,952 bytes). A plain
`cargo build --release` binary with the preview is 960,512 bytes.

HTML rendering (phase 1), measured 2026-09-17 on the same machine against `origin/main` (ef32e96):

- HTML-heavy 100 KB preview open: 16.9 ms p95 (the Markdown-only 100 KB open measured 17.1 ms in
  the same run).
- One-paragraph update in a 1 MB document: 1.67 ms and 1.50 ms p95 in two quiet runs; the baseline
  build measured 1.62 ms. A run while other work shared the machine measured 2.27 ms.
- Update inside a `<div>` spanning a 1 MB document: 1.38 s p95. No target: the element makes the
  whole document one block, so every edit parses it again and lays out that one block in full.
- SVG decode of `assets/fastpad-icon.svg` at 256 px: median 8.0 ms, max 9.6 ms (a second run
  measured median 13.1 ms, max 17.3 ms).
- Release binary with `release-package`: 1,008,128 bytes before, 1,111,552 bytes after
  (+103,424 bytes).
- Startup with `benchmarks/fixtures/sample.md` (100 runs each, `fastpad-bench compare`): no
  milestone regressed; every p95 delta was between -4.9 ms and -1.3 ms.

Deviations from the design spec, recorded without a ruling:

- The image cache is keyed by path only, not by path and modification time. An image edited while
  its document is shown keeps the decoded pixels; it refreshes when the preview is reopened.
