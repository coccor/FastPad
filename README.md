<div align="center">

<img src="assets/fastpad-icon.svg" alt="FastPad" width="96" height="96">

# FastPad

**The text editor that's ready before you are.**

Click it, type. No splash screen, no spinner, no "loading extensions".
A native Windows editor for the notes, logs, configs, JSON and Markdown you open fifty times a day.

[![Latest release](https://img.shields.io/github/v/release/coccor/FastPad?label=release)](https://github.com/coccor/FastPad/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
![Windows 10 | 11 x64](https://img.shields.io/badge/Windows-10%20%7C%2011%20x64-0078D4)
[![Downloads](https://img.shields.io/github/downloads/coccor/FastPad/total)](https://github.com/coccor/FastPad/releases)

[**Download**](https://github.com/coccor/FastPad/releases/latest) · [Install](#install-in-10-seconds) · [Features](#everything-you-reach-for-nothing-you-dont) · [Shortcuts](#keyboard-shortcuts)

<img src="docs/images/hero.png" alt="FastPad editing Markdown with the live preview open side by side" width="900">

</div>

## Why FastPad

Most editors make you wait for everything before you can do anything. FastPad turns that around:
the window takes your keystrokes first, and settings, file loading, syntax highlighting and crash
recovery catch up behind you.

- **Typing in about 60 ms.** From launch to your first character on screen, measured over repeated
  warm launches on a 2014 Intel Core i5-4590 desktop ([how we measure](benchmarks/README.md)).
- **About 2 MB of memory at idle.** Leave it open all day and forget it's there.
- **A 1.7 MB download.** One executable and two DLLs. No runtime, no framework, no Electron.
- **Your text never leaves your PC.** No network access, no telemetry, no account, no background
  service. Links you click open in your browser; nothing else goes online.
- **Never lose a word.** Unsaved work is snapshotted while you type and comes back after a crash or
  power cut.

Written in Rust directly on Win32, with the battle-tested
[Scintilla](https://www.scintilla.org/) editing engine underneath.

## Install in 10 seconds

```powershell
winget install coccor.FastPad
```

<details>
<summary><b>Other ways to install</b>: Scoop, installer, portable ZIP</summary>

| Method | How | What you get |
|---|---|---|
| **winget** | `winget install coccor.FastPad` | Per-user install; `winget upgrade` keeps it current |
| **Scoop** | `scoop bucket add fastpad https://github.com/coccor/FastPad`<br>`scoop install fastpad/fastpad` | Portable install, Start menu shortcut, `fastpad` command |
| **Installer** | [`FastPad-<version>-windows-x64-setup.exe`](https://github.com/coccor/FastPad/releases/latest) | Per-user install, no administrator prompt |
| **Portable** | [`FastPad-<version>-windows-x64.zip`](https://github.com/coccor/FastPad/releases/latest) | Extract anywhere, run `FastPad.exe`; nothing written to the registry |

Every release publishes `SHA256SUMS.txt` so you can verify what you downloaded.

**The installer** puts FastPad in `%LocalAppData%\Programs\FastPad` and only touches your own user
registry: an uninstall entry, a Start menu shortcut, `fastpad` for Win+R, and FastPad in the
"Open with" list for `.txt`, `.md`, `.markdown`, `.json`, `.log` and `.ini`. Your default apps are
never changed. Optional extras: a desktop shortcut and "Edit with FastPad" on every file's right-click
menu. Uninstall from Settings > Apps. Unattended install:

```powershell
FastPad-<version>-windows-x64-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /TASKS="contextmenu"
```

`/ALLUSERS` installs for every user into Program Files instead (requires elevation).

**The portable ZIP** holds `FastPad.exe`, `Scintilla.dll` and `Lexilla.dll` (keep them together),
plus `README.md`, `LICENSE`, `LICENSES.md` and `licenses\`.

Settings and recovery snapshots live in `%LocalAppData%\FastPad` whichever way you install, so you
can switch methods without losing them, and uninstalling keeps them.

</details>

> **"Windows protected your PC"?** Current releases aren't code-signed yet, so SmartScreen may warn
> on first run. Choose **More info > Run anyway**, or check the file against `SHA256SUMS.txt` first.

## Everything you reach for. Nothing you don't.

### Markdown with a live preview

<img src="docs/images/markdown-preview.png" alt="Markdown source and rendered preview side by side" width="800">

Write on the left, see it rendered on the right. FastPad draws GitHub-flavored Markdown natively
(tables, task lists, strikethrough, code blocks and local images) and updates moments after you stop
typing. Both panes scroll together.

- **Ctrl+Shift+V** cycles between no preview, side by side and full width.
- Drag the divider to resize; double-click it to reset.
- Links just work: web links open in your browser, `#headings` jump inside the preview, and links to
  local files open in a new FastPad tab.
- Privacy by design: only local images are shown, and nothing is fetched from the internet.
- The preview's graphics stack loads the first time you open a preview, so it never slows startup.

### JSON you can trust

<img src="docs/images/json.png" alt="A formatted JSON document with syntax highlighting" width="800">

Syntax highlighting as soon as you open a `.json` file. **Validate JSON** points to the exact line
and column of a mistake; **Format JSON** (Ctrl+Shift+F) pretty-prints the whole document in one step you can undo. Neither will touch a
file that doesn't parse.

### Tabs, done right

Open as many files as you like in one window. **Ctrl+Tab** and **Ctrl+1…9** to jump, double-click
the empty tab bar for a new tab, scroll the wheel over the tabs to browse them. Open a file from
Explorer or the command line and it lands as a tab in the FastPad window you already have open, not
in a new window.

### A command palette for everything

<img src="docs/images/command-palette.png" alt="The command palette filtering theme commands" width="800">

**Ctrl+Shift+P** and start typing. Switch theme, toggle word wrap or line numbers, change font size
or tab width, all without leaving the keyboard. Every change is saved instantly.

### Themes that match your desk

<img src="docs/images/themes.png" alt="FastPad in light, dark and Catppuccin Mocha themes" width="800">

Light, dark, or **System** to follow Windows as it switches. Plus all four
[Catppuccin](https://catppuccin.com/) flavors (Latte, Frappé, Macchiato, Mocha), or plain
`catppuccin` to pick Latte by day and Mocha by night. Windows high contrast is always respected.

### Crash recovery that just works

FastPad quietly snapshots unsaved documents every 30 seconds while you edit. If your PC crashes,
loses power or reboots for an update, the next launch brings your work back as unsaved tabs. Saving
or discarding a recovered tab cleans up after itself.

### Pick up where you left off

Close FastPad and the next launch reopens every tab, including unsaved edits and untitled notes,
with the tab you were on active. Closing never nags about unsaved changes while this is on. Turn
it off with **File > Restore session on startup** or `restore_session=false`, and FastPad asks
before closing again.

### And the details you'd expect

- **Safe saves.** Files are written to a temporary copy and swapped into place, so a failed save never
  leaves you with a half-written file.
- **Encodings preserved.** UTF-8, UTF-8 with BOM, UTF-16 LE and UTF-16 BE are detected on open and
  kept on save.
- **Find and replace**, zoom, word wrap, line numbers, and left-to-right or right-to-left text.
- **Screen-reader friendly links.** Links in the Markdown preview are exposed to assistive
  technology and can be followed from it.

## Keyboard shortcuts

| Action | Shortcut | | Action | Shortcut |
|---|---|---|---|---|
| New tab | `Ctrl+N` or `Ctrl+T` | | Command palette | `Ctrl+Shift+P` |
| Open | `Ctrl+O` | | Markdown preview modes | `Ctrl+Shift+V` |
| Save | `Ctrl+S` | | Format JSON | `Ctrl+Shift+F` |
| Save as | `Ctrl+Shift+S` | | Word wrap | `Alt+Z` |
| Find | `Ctrl+F` | | Zoom in / out / reset | `Ctrl++` / `Ctrl+-` / `Ctrl+0` |
| Replace | `Ctrl+H` | | Next / previous tab | `Ctrl+Tab` / `Ctrl+Shift+Tab` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Y` | | Go to tab 1–9 | `Ctrl+1` … `Ctrl+9` |
| Left-to-right text | `Ctrl+L` | | Right-to-left text | `Ctrl+R` |

## Make it yours

Everything in the command palette is saved to `%LocalAppData%\FastPad\fastpad.ini`. You can also
edit it by hand: one `key=value` per line. A typo never blocks startup; FastPad points out the bad
line in a notification and applies the rest.

| Key | Values | Default |
|---|---|---|
| `font_face` | Any installed font name | `Consolas` |
| `font_size` | Points (positive integer) | `11` |
| `tab_width` | 1–255 | `4` |
| `word_wrap` | `true`/`false`, `1`/`0`, `yes`/`no`, `on`/`off` | `false` |
| `line_numbers` | `true`/`false`, `1`/`0`, `yes`/`no`, `on`/`off` | `true` |
| `theme` | `system`, `light`, `dark`, `catppuccin`, `catppuccin-latte`, `catppuccin-frappe`, `catppuccin-macchiato`, `catppuccin-mocha` | `system` |
| `recovery_interval_seconds` | Seconds between recovery snapshots | `30` |
| `restore_session` | `true`/`false`, `1`/`0`, `yes`/`no`, `on`/`off` | `true` |

Hand edits keep your comments and other lines; the palette rewrites only the line it changes.

## Command line

```text
FastPad.exe [--new-window] [path]
```

- `FastPad.exe notes.md` opens the file in your running FastPad window, or starts FastPad if none is
  open.
- `--new-window` always starts a separate, independent window.
- `--diagnostic` is for the startup benchmark harness; you don't need it.

## FAQ

**Is it really free?** Yes. FastPad is open source under the MIT License, with no paid tier, no ads
and no data collection.

**Does it replace Notepad?** It sits alongside it. The installer adds FastPad to "Open with" for
common text formats without taking over your defaults, and you can make it the default for any file
type from Windows Settings.

**Why the SmartScreen warning?** New, unsigned apps have no reputation with Microsoft yet. Code signing
is on the way; until then, every release publishes SHA-256 checksums you can verify.

**Can I use it on Windows on ARM or 32-bit Windows?** Not yet. Current builds are Windows 10 and 11
x64.

The preview renders GitHub-flavored Markdown natively (tables, task lists, strikethrough, code
blocks, images) and the HTML GitHub allows in READMEs: centred `<div>` and `<p>` blocks, sized
`<img>` tags, `<picture>` images that follow the light or dark theme, collapsible
`<details>` sections, and inline tags such as `<kbd>`, `<sup>`, and `<br>`. Local PNG, JPEG, GIF,
and SVG images are shown; remote images show a placeholder with their alt text, because nothing is
loaded from the network. It updates shortly after you stop typing, and scrolling either pane scrolls
the other. Links open when clicked: web and mail links in your default browser, `#anchors` inside
the preview (opening any collapsed section around them), and local files in a FastPad tab.
Files larger than 10 MB pause live updates; click the bar at the top of the preview to refresh it.

If FastPad saves you a few seconds a day, **[star it on GitHub](https://github.com/coccor/FastPad)**.
It's the single best way to help others find it. Found a bug or missing something?
[Open an issue](https://github.com/coccor/FastPad/issues).

## Building from source

Requirements: Rust stable (see `rust-toolchain.toml`), Visual Studio 2022 with the C++ x64 build
tools, and PowerShell 7.

```powershell
pwsh -File tools/fetch-native.ps1           # download and SHA-256-verify Scintilla and Lexilla
pwsh -File tools/build-native.ps1           # build the DLLs into native\out\x64
cargo build --release
cargo test -- --test-threads=1              # Windows integration tests must run serially
pwsh -File tools/audit-dependencies.ps1     # allow only the windows-sys/serde_json closure
pwsh -File tools/package.ps1                # dist\FastPad-<version>-windows-x64.zip
pwsh -File tools/verify-package.ps1         # add -RequireSignature for signed release builds
pwsh -File tools/package-installer.ps1      # dist\FastPad-<version>-windows-x64-setup.exe (Inno Setup 6)
pwsh -File tools/verify-installer.ps1       # silent install, registration checks, uninstall
```

The version comes from `Cargo.toml`. Releasing, code signing, and the Scoop and winget manifests are
covered in [`docs/distribution.md`](docs/distribution.md); startup benchmarking in
[`benchmarks/README.md`](benchmarks/README.md).

## License

FastPad is released under the [MIT License](LICENSE). Scintilla, Lexilla and the Rust crates it
links are listed with their licenses in [`LICENSES.md`](LICENSES.md).
