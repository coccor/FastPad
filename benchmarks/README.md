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
`fastpad-bench compare`.
