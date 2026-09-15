# Release Profile Comparison

## Status: PROVISIONAL, NON-REFERENCE HOST

These numbers were collected on 2026-09-15 on the development host (Windows 10 Pro 10.0.19045,
x64), not on the documented reference machine, with 30 measured warm launches (5 warmup launches)
per profile instead of 100. They must not be used to select the release profile or to claim the
startup targets. The `release` profile is the packaging default only until the reference run below
is completed.

Method: `cargo build --locked --profile <profile> --features release-package --bins`, the pinned
source-built `Scintilla.dll`/`Lexilla.dll` from `tools/build-native.ps1` copied beside each
`target\<profile>\fastpad.exe`, then
`target\<profile>\fastpad-bench.exe --runs 30 --warmup 5 --output <file>` (no
`--enforce-reference`). TTI is `first_input_rendered`; all times are microseconds from the harness
QPC origin taken immediately before `CreateProcessW`. Binaries are unsigned.

### Static CRT (current packaging configuration)

The portable package links the C runtime statically (`-Ctarget-feature=+crt-static`, passed with the
path remaps through `CARGO_ENCODED_RUSTFLAGS`). Only `release` has been re-measured this way so far.

| Profile | `fastpad.exe` bytes | Warm TTI p50 | Warm TTI p95 | First paint p50 | First paint p95 | Idle private WS p50 |
|---|---:|---:|---:|---:|---:|---:|
| `release` (fat LTO, opt 3, static CRT) | 486,400 | 59,559 us | 80,258 us | 56,037 us | 75,885 us | 2,027,520 B |

### Dynamic CRT (superseded; not the shipped configuration)

These earlier measurements linked `VCRUNTIME140.dll` and the `api-ms-win-crt-*` UCRT dynamically. They
are kept for reference only and are not directly comparable with the static-CRT row above.

| Profile | `fastpad.exe` bytes | Warm TTI p50 | Warm TTI p95 | First paint p50 | First paint p95 | Idle private WS p50 |
|---|---:|---:|---:|---:|---:|---:|
| `release` (fat LTO, opt 3, dynamic CRT) | 389,632 | 57,103 us | 62,376 us | 53,280 us | 58,372 us | 2,011,136 B |
| `release-size` (thin LTO, opt z, dynamic CRT) | 321,024 | 56,330 us | 59,466 us | 52,099 us | 54,733 us | 1,998,848 B |
| `release-thin` (thin LTO, opt 3, dynamic CRT) | 400,384 | 57,610 us | 61,608 us | 54,035 us | 58,326 us | 1,994,752 B |

Observations on this host: the dynamic-CRT profiles were within about 1.3 ms of each other at p50,
which is inside run-to-run noise for 30 samples. The static-CRT `release` p50 is about 2.5 ms slower
than its dynamic-CRT measurement, and its p95 about 18 ms slower. Those samples were taken in a
separate session, so the p95 difference may be host noise and needs the reference-machine run to
confirm. No configuration meets the 25 ms p50 / 40 ms p95 warm TTI or 20 ms first paint targets on
this host. Idle private working set (about 2 MB) is well under 20 MB. Roughly 10-12 ms elapses
before `process_start`, and about 28 ms between `editor_created` and `first_paint`.

## Outstanding owner and reference-hardware gates

- [ ] Sign the package with `FASTPAD_SIGNING_CERTIFICATE` set, then run
      `pwsh -File tools/verify-package.ps1 -RequireSignature`.
- [ ] On the reference machine (`benchmarks/reference-machine.md`), build signed static-CRT artifacts
      for each profile and run 100 measured warm iterations (10 warmup) per profile; record executable
      size, TTI p50/p95, first paint, and idle private working set here.
- [ ] Input-before-settings ordering is not deterministically proven by acceptance. The empty-launch
      acceptance test proves first accepted input precedes settings loading only for a launch whose
      input arrived before first paint (retried up to 5 times). Nothing proves rendered input precedes
      settings: the editor repaints after the posted settings message.
- [ ] Select the profile with the lowest compliant TTI (not assumed to be fat LTO), set it as the
      `tools/package.ps1 -BuildProfile` default, and run
      `cargo run --release --bin fastpad-bench -- --runs 100 --warmup 10 --enforce-reference`.
- [ ] Record cold TTI from the first launch after each of ten documented clean reboots of the
      reference machine (no cache-purge utility); p50 must be below 100 ms.
- [ ] Validate the signed portable package and acceptance tests on Windows 11 x64 (and Windows 10
      x64 reference hardware).
- [ ] Run the full serial suite: `cargo test --target x86_64-pc-windows-msvc -- --test-threads=1`.
