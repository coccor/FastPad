# Licenses

## FastPad

FastPad source code and `FastPad.exe`: no license has been declared yet (`Cargo.toml` has no
`license` field). All rights are reserved by the FastPad authors until the project owner publishes a
license.

## Native components

| Component | Version | License | Text |
|---|---|---|---|
| Scintilla (`Scintilla.dll`) | 5.6.6 | License for Lexilla, Scintilla, and SciTE (Historical Permission Notice and Disclaimer style), Copyright 1998-2021 Neil Hodgson | `licenses/Scintilla.txt` |
| Lexilla (`Lexilla.dll`) | 5.5.3 | License for Lexilla, Scintilla, and SciTE (Historical Permission Notice and Disclaimer style), Copyright 1998-2021 Neil Hodgson | `licenses/Lexilla.txt` |

## Rust crates

FastPad depends directly on `windows-sys 0.61.2` and `serde_json 1.0.151`. The complete locked
dependency closure, with the license expressions reported by `cargo metadata --locked`, is:

| Crate | Version | License | Role |
|---|---|---|---|
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | Win32 bindings (linked) |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 | Import linking for `windows-sys` (linked) |
| `serde_json` | 1.0.151 | MIT OR Apache-2.0 | Explicit JSON commands (linked) |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | Locked but not linked (no normal dependency edge for this target) |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | `serde_json` dependency (linked) |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | `serde_json` dependency (linked) |
| `memchr` | 2.8.3 | Unlicense OR MIT | `serde_json` dependency (linked) |
| `zmij` | 1.0.23 | MIT | `serde_json` dependency (linked) |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | Build-time procedural macro |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | Build-time procedural macro support |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | Build-time procedural macro support |
| `syn` | 3.0.4 | MIT OR Apache-2.0 | Build-time procedural macro support |
| `unicode-ident` | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 | Build-time procedural macro support |

FastPad uses these crates under the MIT license option where a choice is offered; `unicode-ident`
additionally carries the Unicode-3.0 license for its Unicode data tables. Build-time procedural
macro crates are not linked into `FastPad.exe`.

The portable package includes `licenses/rust-crates.txt`, generated during packaging by
`tools/rust-crate-licenses.ps1` from `cargo metadata --locked`. It lists every crate linked into
`FastPad.exe` with its version and SPDX license, followed by the verbatim `LICENSE*`/`COPYING`
files, including copyright notices, from that crate's source.
