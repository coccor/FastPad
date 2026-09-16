# FastPad

FastPad is a startup-latency-first text editor for Windows 10 and Windows 11 x64. It is a native
Win32 application written in Rust on top of Scintilla and Lexilla, with no UI framework, no browser
runtime, no background service, and no network access. The window accepts typing before optional
work such as settings, file loading, syntax highlighting, and crash recovery has finished.

## Portable ZIP

`FastPad-0.1.0-windows-x64.zip` contains:

- `FastPad.exe`
- `Scintilla.dll` and `Lexilla.dll` (built from the pinned Scintilla 5.6.6 and Lexilla 5.5.3 sources)
- `README.md`, `LICENSES.md`, and `licenses\` (Scintilla, Lexilla, and linked Rust crate license texts)

Extract the ZIP to any folder and run `FastPad.exe`. Keep the two DLLs beside the executable;
FastPad loads them only from its own folder. Nothing is installed and no registry keys are written.

## Command line

```text
FastPad.exe [--new-window] [path]
```

- With no flags, a launch hands its file (if any) to an already-running FastPad window in the same
  Windows session and exits. If no FastPad is running, it becomes the new primary window.
- `--new-window` always starts an independent FastPad process and window.
- `path` opens one file after the window is ready for input. JSON (`.json`) and Markdown (`.md`)
  files get syntax highlighting.
- `--diagnostic` is used by the startup benchmark harness and is not needed for normal use.

## Settings

Settings are read after the window appears from `%LocalAppData%\FastPad\fastpad.ini`. The file is
optional; each line is `key=value`. Invalid or unknown lines are reported in a non-blocking
notification and every valid line still applies.

| Key | Values | Default |
|---|---|---|
| `font_face` | Any non-empty font name | `Consolas` |
| `font_size` | Positive integer (points) | `11` |
| `tab_width` | Integer 1-255 | `4` |
| `word_wrap` | `true`/`false`, `1`/`0`, `yes`/`no`, `on`/`off` | `false` |
| `line_numbers` | `true`/`false`, `1`/`0`, `yes`/`no`, `on`/`off` | `true` |
| `theme` | `system`, `light`, `dark`, `catppuccin`, `catppuccin-latte`, `catppuccin-frappe`, `catppuccin-macchiato`, `catppuccin-mocha` | `system` |
| `recovery_interval_seconds` | Positive integer | `30` |

`system` and `catppuccin` follow the Windows light/dark app setting (`catppuccin` uses Latte when
light and Mocha when dark); the other themes are fixed. Windows high contrast always overrides the
configured theme.

The command palette (Ctrl+Shift+P) changes the theme, word wrap (also Alt+Z), line numbers, font
size (6-72 pt) and tab width (2, 4 or 8) while FastPad runs. Each change is written back to
`fastpad.ini` immediately, rewriting only that key's line; comments and other lines are kept.

## Crash recovery

While you edit, FastPad periodically writes snapshots of unsaved documents to
`%LocalAppData%\FastPad\Recovery` (`*.fps` files). After a crash, the next launch reopens them as
unsaved tabs. Saving or discarding a recovered tab removes its snapshot; malformed snapshots are
renamed with an `.invalid` suffix instead of being opened.

## JSON tools

Validate JSON and Format JSON run only when you choose them. They never modify a document that does
not parse, and formatting is a single undo step.

## Building from source

Requirements: Rust stable (see `rust-toolchain.toml`), Visual Studio 2022 with the C++ x64 build
tools, and PowerShell 7.

```powershell
pwsh -File tools/fetch-native.ps1           # download and SHA-256-verify Scintilla and Lexilla
pwsh -File tools/build-native.ps1           # build the DLLs into native\out\x64
cargo build --release
cargo test -- --test-threads=1              # Windows integration tests must run serially
pwsh -File tools/audit-dependencies.ps1     # allow only the windows-sys/serde_json closure
pwsh -File tools/package.ps1                # dist\FastPad-0.1.0-windows-x64.zip
pwsh -File tools/verify-package.ps1         # add -RequireSignature for signed release builds
```

`tools/package.ps1` signs the binaries when `FASTPAD_SIGNING_CERTIFICATE` is set to a certificate
thumbprint or PFX path (`FASTPAD_SIGNING_PASSWORD` and `FASTPAD_TIMESTAMP_URL` are optional).
Startup benchmarking is described in `benchmarks/README.md`.
