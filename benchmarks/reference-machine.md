# Reference Machine

Collected on 2026-09-01 in the Codex workspace.

- Hostname: `DESKTOP-KBNRDAF`
- OS build: `Microsoft Windows [Version 10.0.19045.6466]`
- OS version API: `10.0.19045.0`
- Architecture: `AMD64`
- 64-bit OS: `True`
- CPU identifier: `Intel64 Family 6 Model 60 Stepping 3, GenuineIntel`
- Logical processors: `4`
- Rust toolchain: `rustc 1.98.0 (88d9e12ae 2026-08-18)`, host `x86_64-pc-windows-msvc`, LLVM `22.1.8`

## Startup Evidence

- `cargo run --release --bin fastpad -- --diagnostic` reached:

  ```text
  Running `target\release\fastpad.exe --diagnostic`
  ```

  The GUI process remained active until interrupted for cleanup, which matches the expected message-loop behavior for the current bootstrap.

- Caret and input confirmation came from the real smoke test:

  ```text
  cargo test --test startup_smoke --target x86_64-pc-windows-msvc -- --test-threads=1
  ```

  That test located the top-level FastPad window, found the `Scintilla` child, verified focus on that child, posted `x`, and read back `x` through a marshaled `WM_GETTEXT` path.

## Limitations

- `Get-CimInstance`, `wmic`, `systeminfo`, and `tasklist` were denied in this sandbox, so manufacturer, model, and physical memory could not be collected from this host.
