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
