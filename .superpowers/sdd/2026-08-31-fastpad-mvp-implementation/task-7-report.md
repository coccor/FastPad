# Task 7 report — Startup benchmark harness

## Status

`DONE_WITH_CONCERNS`

Task 7 is implemented and its focused, regression, live benchmark, wrapper, compare, formatting,
lint, explicit Windows-target check, and release-build verification all pass. The fresh live run
did not meet the optional documented-reference TTI thresholds on this host, so performance remains
a release-gate concern rather than a harness correctness failure.

## Scope and architecture

Base commit: `d51a476c2784fed932a58b50b909370a77938dcb`.

Task 7 commit: the final SHA is reported in the completion handoff. It cannot be embedded in this
file because this file is itself part of that content-addressed commit.

Files changed:

- `src/perf/protocol.rs` adds the fixed `FPB1` record, strict encode/decode and diagnostic config
  validation, diagnostic shared-memory/event attachment, and diagnostic-only Scintilla/main-window
  subclass tracking.
- `src/bin/fastpad-bench.rs` adds release-process launch and cleanup, known-character injection and
  scalar cross-process Scintilla verification, JSONL output, percentile/reference enforcement, and
  deterministic bootstrap comparison.
- `src/perf/mod.rs` exposes the protocol module.
- `src/perf/startup.rs` publishes record-once milestones to an optional diagnostic session while
  preserving the existing `Eq` API for timestamp state.
- `src/bootstrap.rs` attaches transport and installs input hooks only for a benchmark-backed
  diagnostic launch, with the existing `WindowIdentity` checks retained around reentrant work.
- `src/window/main_window.rs` and `src/window/messages.rs` publish settings, file-load, and
  fully-ready completion at their existing deferred transitions.
- `Cargo.toml` enables only the additional Win32 console and memory feature groups needed by the
  harness and shared-memory transport.
- `tools/benchmark.ps1` builds the release application and invokes the harness.
- `benchmarks/README.md` documents run, enforcement, output, and comparison behavior.

The harness owns inheritable mapping/event handles and passes their numeric values plus the QPC
origin in diagnostic-only environment variables. A normal launch does not read these variables or
install subclasses. Accepted input is timestamped before forwarding the benchmark `WM_CHAR` to
Scintilla. A matching text-change notification arms rendering, and the timestamp/event are emitted
only after the following Scintilla `WM_PAINT` completes. The harness waits for `FullyReady`, then
waits two input-free seconds before sampling private working set and closes/reaps every child.

## RED evidence

The implementation and its test-first history were inherited as uncommitted work. No raw RED log
artifact was present in the workspace, so this completion pass did not pretend to reproduce RED by
discarding valid implementation. The preserved initial RED contract was:

```text
cargo test perf::protocol::tests --lib
Expected pre-implementation result: FAIL — `perf::protocol` and `BenchmarkRecord` did not exist.
```

The preserved behavioral regression tests identify the production break each caught, including:

- truncated frames being accepted;
- normal launches reading stale diagnostic environment state;
- a plain `--diagnostic` launch failing when no harness variables are present;
- rendered input being recorded without the benchmark character, modification, and following paint;
- reference values exactly at 25,000/40,000 microseconds being accepted;
- materiality/bootstrap comparison mistakes;
- selecting a process-owned IME helper instead of `FastPadMainWindow`;
- using cross-process pointer-based Scintilla text retrieval;
- console interruption leaving the active child registered;
- omitted deferred settings/file milestones; and
- diagnostic transport accidentally changing `StartupMetrics` equality behavior.

## Fresh GREEN and verification evidence

Focused protocol:

```text
cargo test perf::protocol::tests --lib
6 passed; 0 failed; 29 filtered out
```

Harness unit tests:

```text
cargo test --bin fastpad-bench
9 passed; 0 failed
```

Serialized regression suite:

```text
cargo test --lib -- --test-threads=1
35 passed; 0 failed

cargo test --bin fastpad-bench -- --test-threads=1
9 passed; 0 failed

cargo test --test editor_control -- --test-threads=1
5 passed; 0 failed

cargo test --test startup_smoke -- --test-threads=1
6 passed; 0 failed
```

The 11 integration-target executions include the real Scintilla editor test and the live startup
smoke tests. The latter also verifies that `--diagnostic` remains usable without benchmark
environment variables.

Quality gates:

```text
cargo fmt --check
PASS

cargo clippy --all-targets -- -D warnings
PASS

cargo check --all-targets --target x86_64-pc-windows-msvc
PASS

cargo build --release --bins --target x86_64-pc-windows-msvc
PASS
```

The direct live run also freshly built `fastpad` release before launching the harness:

```text
cargo build --release --bin fastpad
cargo run --release --bin fastpad-bench -- --runs 20 --warmup 5 --output %TEMP%\fastpad-task7-verification.jsonl
```

Result: 20 measured records after 5 warmups, 20 JSONL lines, 0 malformed JSON lines, exit code 0.

```text
process_start: p50=9440us p95=15979us
window_created: p50=14538us p95=30444us
editor_created: p50=23553us p95=44715us
first_paint: p50=34291us p95=54599us
first_input_accepted: p50=33964us p95=54165us
first_input_rendered: p50=35147us p95=55506us
settings_loaded: p50=34302us p95=54611us
file_loaded: p50=34310us p95=54619us
fully_ready: p50=34337us p95=54647us
idle_private_working_set_bytes: p50=2297856 p95=2347008
valid_records=20
```

Compare-mode smoke:

```text
cargo run --release --bin fastpad-bench -- compare %TEMP%\fastpad-task7-verification.jsonl %TEMP%\fastpad-task7-verification.jsonl
PASS (exit 0; no milestones marked REGRESSION)
```

PowerShell wrapper smoke:

```text
pwsh -NoProfile -File tools/benchmark.ps1 -Runs 1 -Warmup 0 -Output %TEMP%\fastpad-task7-wrapper.jsonl
PASS (1 valid record; 1 JSONL line; 0 malformed lines)
```

Process cleanup was checked before the focused run and after the 20/5 live run, serialized tests,
compare smoke, and wrapper smoke. Every check reported `fastpad_processes=0`. No child exited early
and no startup error-dialog condition was reported during the live runs.

## Self-review

Reviewed the complete diff from the base commit and checked every Task 7 requirement against the
plan, task brief, and performance specification.

Findings:

- The fixed frame contains magic, version, PID, all nine microsecond fields, and idle private bytes;
  decoding rejects wrong length, magic, and version.
- Diagnostic config rejects absent partial state, malformed/zero/equal handles, nonpositive or
  future origins, invalid inherited handles, mapping failures, and event-reset failures.
- Normal startup neither reads benchmark environment values nor installs benchmark subclasses.
- The stable-window-identity checks from Task 6 remain in place before and after hook installation.
- Scintilla input tracking follows accepted -> matching modification -> completed subsequent paint;
  hooks remove their retained references during `WM_NCDESTROY`.
- The harness records QPC immediately before `CreateProcessW`, validates PID/version/nonzero and
  applicable milestone ordering, verifies U+E000 through scalar messages, waits for readiness and
  idle time, samples memory, and owns bounded cleanup on success, error, and console interruption.
- Percentiles use the required ceiling index. Comparison uses 10,000 deterministic resamples with
  seed `0xFA57_0A0D` and requires both the absolute/relative materiality gate and a positive lower
  confidence bound.
- No runtime dependency or Task 8 UI behavior was added.

Remaining concerns:

- Fresh rendered-input TTI was p50 35,147 microseconds and p95 55,506 microseconds, above the
  documented strict reference limits of 25,000 and 40,000. The required non-enforcing Task 7 live
  run passes, but `--enforce-reference` would correctly return nonzero on this host. Prior handoff
  evidence (p50 34,153 / p95 44,221) also missed the limits, so this needs reference-machine
  investigation before release.
- Raw RED stdout from the prior agent was not archived; only the test-first source and expected RED
  failure contract were available for audit.
- GUI windows may briefly flash during live benchmarking by design.

## Fix round 1 — independent review

Review base: `5173e30e0dc3514f0267488b0db60bcc5b61721d`.

Status: `DONE_WITH_CONCERNS` — all seven Important findings were validated and fixed. The report-only
minor is also addressed below. The remaining concerns are measured performance, not harness
correctness.

### Findings and resolutions

1. **Private-memory semantics — validated.** `PROCESS_MEMORY_COUNTERS_EX::PrivateUsage` is private
   commit charge, not resident private working set. The harness now prefers
   `PROCESS_MEMORY_COUNTERS_EX2::PrivateWorkingSetSize`. Because EX2 requires a fully updated Windows
   10 22H2/Windows 11 22H2 or newer system, older supported Windows 10 builds fall back to
   enumerating committed regions with `VirtualQueryEx`, querying page residency/sharing with
   `QueryWorkingSetEx`, and counting only valid pages whose Shared bit is clear.
2. **Shared-frame coherence and premature signaling — validated.** The mapped transport now uses an
   aligned generation plus atomic header/record fields. The single writer marks an odd generation,
   updates all fields, then publishes an even generation with sequentially consistent operations;
   readers accept only identical, nonzero even generations around their atomic snapshot. Rendered
   input signals the event only when QPC capture, record-once insertion, and coherent publication
   all succeed.
3. **Console cleanup handle race/lifecycle gap — validated; suggested mechanism refined.** The old
   control handler and borrowed global process handle were removed. Each run creates a job with
   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Rather than create suspended, then assign (which still has a
   post-create/pre-assignment gap), Windows 10's `PROC_THREAD_ATTRIBUTE_JOB_LIST` assigns the child
   atomically during `CreateProcessW`. `IsProcessInJob` verifies the association. Closing the job is
   the primary error/termination cleanup, with bounded wait and direct termination only as a final
   same-thread fallback.
4. **Unchecked `WM_NOTIFY` payload size — validated.** The parent subclass now reads only `NMHDR`,
   validates `hwndFrom` and `SCN_MODIFIED`, and only then lazily casts to the larger Scintilla
   notification prefix to read `modification_type`.
5. **Broad inherited-handle set — validated.** Process creation now uses `STARTUPINFOEXW` and
   `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` containing exactly the mapping and event. The child requires
   `HANDLE_FLAG_INHERIT` on both adopted handles and clears the bit before using them.
6. **Unbounded scalar Scintilla reads — validated.** `SCI_GETLENGTH` and every `SCI_GETCHARAT` now use
   `SendMessageTimeoutW` with `SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT` and a five-second bound. Timeout
   or target-exit errors propagate through benchmark-character verification.
7. **Incomplete validation and integer narrowing — validated.** Records require a nonzero PID;
   child records separately match the launched PID; persisted records no longer perform a
   tautological PID comparison. Ordering requires process -> window -> editor, editor before paint
   and accepted input, accepted before rendered input, and paint -> settings -> file -> ready.
   Comparison output and bootstrap deltas use `i128`, preserving the complete JSON `u64` range.

### Focused RED/GREEN evidence

Memory semantics:

```text
cargo test --bin fastpad-bench private_working_set_uses_resident_private_pages_not_commit_charge
RED: unresolved private-working-set selection/page-accounting helpers
GREEN: 1 passed; 0 failed
```

Shared publication and successful-write signaling:

```text
cargo test perf::protocol::tests --lib
RED: SharedBenchmarkFrame absent; set_milestone returned () rather than write success
GREEN: 8 passed at that cycle; 0 failed
```

Safe notification discrimination:

```text
cargo test perf::protocol::tests::unrelated_notification_does_not_read_scintilla_only_payload --lib
RED: lazy Scintilla notification discriminator absent
GREEN: 1 passed; 0 failed
```

Bounded scalar verification:

```text
cargo test --bin fastpad-bench benchmark_character_verification
RED: verifier closure could not return/propagate timeout errors
GREEN: 2 passed; 0 failed
```

Record validation and full-range deltas:

```text
cargo test --bin fastpad-bench comparison_delta_preserves_the_full_u64_timing_range
RED: expected (i128, i128), implementation returned (i64, i64)
GREEN: 1 passed; 0 failed

# Mutation check: temporarily removed the new PID/order guards
cargo test --bin fastpad-bench record_validation_rejects_missing_or_misordered_milestones
RED: assertion failed because the impossible frame was accepted
# Restored guards
GREEN: 1 passed; 0 failed
```

Inherited handle validation and allowlisting:

```text
cargo test perf::protocol::tests::diagnostic_handles_must_arrive_with_inheritance_enabled --lib
RED: inherited-handle flag validator absent
GREEN: 1 passed; 0 failed

cargo test --bin fastpad-bench diagnostic_handle_allowlist_contains_only_mapping_and_event
RED: diagnostic handle allowlist absent
GREEN: 1 passed; 0 failed
```

Kill-on-close job:

```text
cargo test --bin fastpad-bench cleanup_job_is_configured_to_kill_children_when_harness_closes
RED: JobObjects feature and cleanup-job constructor absent
GREEN: 1 passed; 0 failed
```

An abrupt-lifecycle integration check launched the release harness, observed FastPad child PID
7276, force-terminated the harness, and confirmed `abrupt_cleanup_reaped_child=true` and
`fastpad_processes_after_abrupt_test=0`.

### Fix-round verification

Focused aggregate suites:

```text
cargo test --bin fastpad-bench
13 passed; 0 failed

cargo test perf::protocol::tests --lib
10 passed; 0 failed
```

Serialized full suite:

```text
cargo test --lib -- --test-threads=1
39 passed; 0 failed

cargo test --bin fastpad-bench -- --test-threads=1
13 passed; 0 failed

cargo test --test editor_control -- --test-threads=1
5 passed; 0 failed

cargo test --test startup_smoke -- --test-threads=1
6 passed; 0 failed
```

Fresh live release distribution:

```text
cargo build --release --bin fastpad
cargo run --release --bin fastpad-bench -- --runs 20 --warmup 5 --output %TEMP%\fastpad-task7-fix-round1.jsonl
20 measured records; 20 valid JSONL lines; 0 malformed lines; exit 0

process_start: p50=9525us p95=10683us
window_created: p50=14369us p95=15999us
editor_created: p50=23552us p95=27479us
first_paint: p50=33092us p95=36652us
first_input_accepted: p50=32764us p95=36067us
first_input_rendered: p50=34014us p95=38672us
settings_loaded: p50=33119us p95=36832us
file_loaded: p50=33126us p95=36966us
fully_ready: p50=33154us p95=36996us
idle_private_working_set_bytes: p50=1675264 p95=1720320
valid_records=20
fastpad_processes_after_benchmark=0
```

Quality gates were rerun after implementation:

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo check --all-targets --target x86_64-pc-windows-msvc
cargo build --release --bins --target x86_64-pc-windows-msvc
```

### Updated performance concern

The corrected fresh run's rendered-input TTI p95 (`38,672` microseconds) was below the 40 ms limit,
but p50 (`34,014` microseconds) still exceeded the 25 ms limit. First paint p50 was `33,092`
microseconds, also above the design's 20 ms goal; the original run's first-paint p50 (`34,291`
microseconds) missed it as well. These are explicit performance concerns for reference-machine
investigation, not correctness failures in the distribution harness. Correct resident-private
memory was 1.68 MB p50 / 1.72 MB p95, comfortably below the 20 MB budget.
