# Tauri Windows port — working notes

Working document for bringing `apps/linows/` to Windows parity. Updated as we go.

## Goal

Ship Look on Windows from the same Tauri v2 codebase as Linux, replacing the WinUI3 app at `apps/windows/`. macOS (`apps/macos/`) remains the design source of truth; Linux UI is the current parity baseline for non-macOS look-and-feel.

## Constraints

- **Do not regress Linux UI.** Linux is in good shape. Windows-specific UI tweaks must be scoped (e.g. `<html data-platform="windows">` selector, or a class set from Rust at boot via Tauri OS plugin). Do not edit shared CSS selectors that already render correctly on Linux.
- **Backend restructure is fine.** Reorganising Rust modules under `src-tauri/src/platform/{linux,windows,shared}/` is the planned approach and is not affected by the UI rule above.
- **WinUI3 app (`apps/windows/`) is reference-only.** Keep in-tree, unmaintained. Mine it for behaviour (autostart, focus-existing-window, icon pipeline, AUMID handling, theme presets) but do not ship it.
- **Packaging target: NSIS, per-user install, no admin.** Matches the current WinUI3 ship path (`%LOCALAPPDATA%\Programs\Look`).

## Current state (audit, 2026-05-16)

`apps/linows/` is Linux-biased. Compiles for Windows in principle but most platform plumbing is missing.

| Subsystem | Windows status | Source |
|---|---|---|
| Window effects (Mica/Acrylic) | done | `src-tauri/src/platform.rs:421-452` |
| Reveal in `explorer.exe` | done | `src-tauri/src/commands.rs:184` |
| Config dir via `LOCALAPPDATA` | done | `src-tauri/src/state.rs:334` |
| `autostart.rs` | missing — XDG-only, needs `HKCU\…\Run` | `src-tauri/src/autostart.rs` |
| Icon extraction | missing — XDG/.desktop only, needs Shell APIs | `src-tauri/src/platform.rs` |
| Process list / kill | missing — `/proc`-based | `src-tauri/src/process.rs` |
| Clipboard file copy | missing — `xclip`/`wl-clipboard` only | `src-tauri/src/commands.rs` |
| Window focus / focus-existing-app | missing — 5 `linux_*` modules (~1.1k LOC), no Windows analogue | `src-tauri/src/linux_*.rs` |
| Global hotkey | unknown — Tauri plugin should suffice on Windows; verify | — |
| `tauri.conf.json` bundle targets | missing — only `deb`+`appimage` | `src-tauri/tauri.conf.json:37` |
| `Cargo.toml` Windows target deps | missing — only Linux `[target.cfg]` block | `src-tauri/Cargo.toml:24` |
| CI workflow for linows on Windows | missing | `.github/workflows/` |

42 `cfg(target_os="linux")` gates across 5 files. Five top-level `linux_*` modules. Adding Windows backends without restructure would double the noise.

## Proposed restructure (no behaviour change)

```
src-tauri/src/
├── main.rs, state.rs, config.rs, commands.rs, shell.rs,
│   calc.rs, music.rs, clipboard.rs, sysinfo.rs, translate.rs, files.rs
└── platform/
    ├── mod.rs                 # cfg-gated re-exports
    ├── shared.rs              # read_icon_file, shared helpers
    ├── linux/
    │   ├── gnome_ext.rs       (was linux_gnome_ext.rs)
    │   ├── transparency.rs    (was linux_transparency.rs)
    │   ├── wayland_shortcut.rs, window_focus.rs, wlr_focus.rs
    │   ├── autostart.rs       (XDG .desktop part of autostart.rs)
    │   ├── icons.rs           (XDG/.desktop scan from platform.rs)
    │   ├── process.rs         (Linux half of process.rs)
    │   └── clipboard.rs       (xclip/wl-clipboard fallback)
    └── windows/
        ├── autostart.rs       (HKCU\…\Run)
        ├── icons.rs           (SHGetFileInfo / IShellItemImageFactory)
        ├── process.rs         (Toolhelp32 + TerminateProcess)
        ├── clipboard.rs       (CF_HDROP file copy)
        ├── window_focus.rs    (SetForegroundWindow + AUMID match)
        └── effects.rs         (extracted Mica/Acrylic)
```

## Plan

### Step 0 — Verify current state compiles on Windows
- `rustc --version`, `cargo tauri --version`, `where.exe cl.exe`, `rustup target list --installed`
- `cargo check` in `apps/linows/src-tauri`
- If green: proceed. If red: fix compile errors first as a precursor PR.

### Step 1 — Restructure (no-op refactor)

**Concrete move/extract plan** (audited 2026-05-16):

**Pure file moves (rename only):**
- `src/linux_gnome_ext.rs`        → `src/platform/linux/gnome_ext.rs`
- `src/linux_transparency.rs`     → `src/platform/linux/transparency.rs`
- `src/linux_wayland_shortcut.rs` → `src/platform/linux/wayland_shortcut.rs`
- `src/linux_window_focus.rs`     → `src/platform/linux/window_focus.rs`
- `src/linux_wlr_focus.rs`        → `src/platform/linux/wlr_focus.rs`

**Split files (Linux block → `platform/linux/`, Windows block → `platform/windows/`, shared → keep):**

- `src/platform.rs` (481 lines) is the messiest — three concerns mixed:
  1. `get_icon`, `IconCache`, `IconResult`, `resolve_icon` (dispatcher) — **stays at top of platform/mod.rs** (cross-platform entry; dispatches to platform-specific resolver)
  2. `xdg_data_dirs`, `resolve_app_icon`, `parse_desktop_icon`, `resolve_themed_icon`, `resolve_file_icon`, scoring helpers — **move to `platform/linux/icons.rs`**
  3. `set_window_effect` (Windows-only Mica/Acrylic) — **move to `platform/windows/effects.rs`** (re-exported as `#[tauri::command]` from `platform/mod.rs`)
  4. `detect_compositor`, `is_tiling_wm` — **move to `platform/linux/wm.rs`** (or fold into `transparency.rs`)
  5. `read_icon_file` (base64-encode any image file, no OS calls) — **move to `platform/shared.rs`** (used by Linux today; Windows will reuse for cached PNGs)

- `src/autostart.rs` (52 lines) — XDG-only. Today the `set_autostart`/`get_autostart` Tauri commands hardcode `~/.config/autostart/look.desktop`.
  - **Move current body → `platform/linux/autostart.rs`**
  - **New `platform/windows/autostart.rs`** writing `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
  - **`src/autostart.rs` becomes the dispatch shell** with `#[tauri::command]`s that delegate via `cfg(target_os = …)` to the platform module

- `src/process.rs` (390 lines) — no cfg gates today, reads `/proc` directly. Will silently return empty Vec on Windows.
  - **Move current body → `platform/linux/process.rs`**
  - **New `platform/windows/process.rs`** using Toolhelp32 + `TerminateProcess`
  - **`src/process.rs` becomes dispatch shell** — keeps `RunningApp` struct, `list_processes`/`kill_process` commands, delegates impl per-cfg

- `src/state.rs` — `look_db_path()` has Windows + Linux branches (`state.rs:334-352`).
  - Keep helper function in `state.rs`. It's already neatly cfg-gated and only ~20 lines; not worth splitting.

- `src/commands.rs` — reveal-in-Explorer has cross-platform branches (`commands.rs:184-…`); clipboard file-copy uses `xclip`/`wl-clipboard` (Linux-only).
  - Keep `#[tauri::command]` wrappers in `commands.rs`.
  - **Extract Linux clipboard-file-copy helper → `platform/linux/clipboard.rs`**
  - **New `platform/windows/clipboard.rs`** with CF_HDROP file copy

**New files:**

- `src/platform/mod.rs` — module tree; cfg-gated `pub use platform::{linux,windows}::*;` lines.
- `src/platform/shared.rs` — `read_icon_file`, any other genuinely platform-shared helpers.
- `src/platform/linux/mod.rs`, `src/platform/windows/mod.rs` — sub-tree roots.

**main.rs touch points:**
- Replace 5 `mod linux_xxx;` declarations (lines 11, 13, 15, 17, 19) with `mod platform;`.
- Replace 8 call sites (`linux_transparency::has_compositor`, `linux_window_focus::activate_self`, `linux_gnome_ext::ensure_installed`, `linux_wayland_shortcut::start`, etc.) with `platform::xxx` or keep direct paths through `cfg(target_os="linux") { use crate::platform::linux::… }`. Decide once we land Step 1.

**Verification gates (must pass before declaring Step 1 done):**
1. `cargo check --target x86_64-pc-windows-msvc` from `apps/linows/src-tauri` — same result as baseline.
2. `cargo check` on Linux (NixOS dev shell or Ubuntu VM) — same result as baseline.
3. `cargo tauri dev` on Linux — UI loads identically (no UI changes in this step).
4. Diff is move-only — `git log -p --follow` should show file relocations with no semantic edits beyond `mod` paths.

### Step 2 — Windows backends (milestone-paced, full parity is the goal)
- **M1 — runnable:** window opens, global hotkey, search results render, open file/folder/url. (Most of this is already cross-platform; M1 is mostly Step 1 + verification.)
- **M2 — icons + reveal:** `platform/windows/icons.rs` (Shell APIs), reveal-in-Explorer already works.
- **M3 — autostart + window focus:** `HKCU\…\Run`, `SetForegroundWindow` + AUMID-aware focus-existing-app.
- **M4 — process + clipboard file copy:** Toolhelp32 process enum, `TerminateProcess`, `CF_HDROP`.
- **M5 — packaging:** NSIS bundle target in `tauri.conf.json`, Windows CI workflow, signing story.

### Step 3 — UI scoping (only where needed)
- Add platform-class attribute at boot (Rust → JS) so any Windows-specific CSS can be scoped without touching Linux selectors.
- Identify any Windows-specific UI needs (Mica blur tuning, title-bar absence handling, taskbar/tray icon, font fallbacks). Defer until M1 surfaces actual gaps.

## Open questions (resolved 2026-05-16)

1. **WinUI3 app fate?** → Keep in-tree as unmaintained reference. Don't ship.
2. **First-runnable scope?** → Full parity day-one is the goal; we'll stage M1–M4 so there's a runnable build at every step.
3. **Packaging target?** → NSIS, per-user, no admin. Match WinUI3's `%LOCALAPPDATA%\Programs\Look` UX.
4. **Restructure first or backends first?** → Restructure first as a no-op PR.
5. **Verify-first?** → Yes — run toolchain checks + `cargo check` on this Win11 host before planning further.

## Decisions log

- **2026-05-16** — Linux UI is the parity baseline among non-macOS; do not regress it while doing Windows work. Windows-only UI tweaks must be platform-scoped.
- **2026-05-16** — WinUI3 stays in-tree as reference; not shipped, not maintained.
- **2026-05-16** — Windows packaging will use NSIS (per-user, no admin) to match WinUI3's existing UX.
- **2026-05-16** — Restructure first as a no-op PR, then Windows backends incrementally.
- **2026-05-16** — Windows Rust builds **must run under VS 2022 BT `vcvarsall.bat x64` env**, not a bare shell. Rustc autodetects VS 2026 Community's `link.exe` but that install lacks the Windows SDK, so the linker can't find `msvcrt.lib` (LNK1104). Existing `build-ffi.bat` already follows this pattern. For ad-hoc dev: wrap cargo calls in `cmd /c 'call "<vs2022bt>\VC\Auxiliary\Build\vcvarsall.bat" x64 && cargo …'`. Same applies to `cargo install tauri-cli` — without the env, build scripts fail to link.
- **2026-05-16** — M1.5 (UWP indexing) is its own milestone, inserted before M2. Reason: without UWP, the launcher is missing most user-visible Win11 apps (Notepad, Weather, Mail, Calculator, Photos…) and the M2/M3 fixes won't be testable against them. Plan: port WinUI3's `UwpAppService.EnumerateAppsFolder` to Rust under `platform/windows/apps.rs` (or a new `platform/windows/uwp.rs`) using the `windows` crate's IShellItem/IEnumShellItems bindings; feed candidates into the indexing pipeline alongside the existing Start Menu walk in `core/engine/src/platform/windows/apps.rs`. AUMID captured during enumeration also unlocks M3 focus-existing-app.
- **2026-05-16** — Per-page Settings icons are unobtainable from a public Windows API; WinUI3's `SettingsIconCatalog.cs` resorts to Segoe Fluent Icons glyphs for the same reason. We chose Lucide (already in tree, theme-aware via `currentColor`, cross-platform) over Segoe for the catalog. Catalog lives at `apps/linows/src/js/settings-icons/windows.js`; render path in `results.js` skips `loadIcon` when a glyph hits so the backend gear doesn't clobber it.
- **2026-05-16** — Classic Control Panel applets (env vars dialog, Device Manager, Services, Registry, Task Manager, …) get their own `look-cmd://program[?args]` scheme rather than being shoehorned into `ms-settings:`. Reason: `ShellExecuteW` can't argv-parse rundll32 commands. The Tauri `open_path` splits on `?` and spawns via `Command::new`. Catalog at `core/engine/src/platform/windows/control_panel.rs`.
- **2026-05-16** — Real per-page Settings PNG icons are deliberately out of scope. They live in the SystemSettings UWP package's protected MRT resource bundles; extracting them needs a fragile MRT projection or PRI parser and breaks across Windows builds. Fluent/Lucide glyph mapping is the documented WinUI3 workaround and we follow suit.
- **2026-05-16** — Dev launcher must run under `vcvarsall.bat x64` env via `scripts/windows/with-vcvars.bat`. The new `Makefile.win` invokes every cargo target through it. Old `Makefile.win` (WinUI3 dotnet build) renamed to `Makefile.winui3`. Top-level `Makefile` dispatches: Windows → `Makefile.win`, macOS → `Makefile.mac`, legacy via explicit `-f Makefile.winui3`.
- **2026-05-16** — Tauri dev's file watcher only watches `apps/linows/src-tauri/`. Changes under `core/engine/` need a touch of any `src-tauri/` file to trigger rebuild; frontend HTML/CSS/JS changes need a manual `Ctrl+R` in the webview (no HMR — `beforeDevCommand` is empty by design since `frontendDist` is static).
- **2026-05-16** — Dev paths fixed for Windows: `setup_dev_env()` was using POSIX env vars only (`HOME`, `XDG_DATA_HOME`) so both fell back to `.` on cmd/PowerShell, landing the dev DB and config inside the repo. Now falls back to `USERPROFILE` and uses `LOCALAPPDATA` on Windows; both write under `%LOCALAPPDATA%\look\look.dev.db` and `%USERPROFILE%\.look.dev.config`.

## Status

- [x] Step 0 — verify Windows toolchain + `cargo check` (passes on `x86_64-pc-windows-msvc` under VS 2022 BT `vcvarsall.bat x64`)
- [x] Step 1 — restructure to `platform/{linux,windows,shared}/` (merged to main as PR #140, commit 9925f34)
- [x] Step 2 M1 — runnable Windows build (window + search + open)
- [x] Step 2 M1.5 — UWP indexing via `shell:AppsFolder`
- [x] Step 2 M1.6 — `.lnk` target dedup vs fallback `.exe` walk
- [x] Step 2 M1.7 — classic Control Panel applets catalog (env vars, Device Manager, Services, …)
- [x] Step 2 M2 — Windows icon resolver (`IShellItemImageFactory`)
- [x] Step 2 M2 polish — Lucide settings-icon catalog (per-page glyphs for `ms-settings:` + `look-cmd://`)
- [x] Step 2 M2 polish — Windows-only drive picker UI in settings
- [~] Step 2 M3 — focus-existing-app shipped (`SetForegroundWindow` + AUMID match via `platform/windows/window_focus.rs`); autostart (`HKCU\…\Run`) still pending
- [ ] Step 2 M4 — process list/kill + clipboard file copy
- [ ] Step 2 M5 — NSIS packaging + CI
- [ ] Step 3 — Windows UI scoping (rounded corners pending — see open issues)
- [x] Tooling — `Makefile.win` (Tauri) + `Makefile.winui3` (legacy WinUI3) + `scripts/windows/with-vcvars.bat`

### Progress recap (2026-05-16)

Working on Windows: launcher window, global hotkey, search, Esc hide, ms-settings panels resolve and open, `.lnk` and `.exe` Start Menu entries resolve and open, UWP apps appear in search (Notepad, Weather, Calculator, …), classic Control Panel applets are searchable (env vars, Device Manager, Services, Registry, Task Manager, Event Viewer, Disk Management, Programs and Features, MSConfig, MSInfo32, Resource Monitor, DxDiag, Performance Monitor), shell icons render for files/apps/UWP, settings entries get per-page Lucide glyphs.

Build/run is one command: `make app-run` (Tauri path) or `make -f Makefile.winui3 …` for the legacy WinUI3 reference.

Key implementation notes:
- `commands.rs::open_path` gates the Linux-only `launch_app` chain (`gtk-launch` / `gio launch` / direct exec) on `cfg(target_os = "linux")`. Windows falls through to the generic else-branch (`open::that`) which uses `ShellExecuteW`.
- A new `look-cmd://program[?args]` synthetic scheme handles classic Win32 applets that ShellExecute can't argv-parse (`rundll32.exe sysdm.cpl,EditEnvironmentVariables`). `open_path` splits on `?` and spawns via `Command::new`.
- UWP enumeration lives at `core/engine/src/platform/windows/uwp.rs`, mirroring `apps/windows/LauncherApp/Services/UwpAppService.cs`: `SHCreateItemFromParsingName("shell:AppsFolder")` + `IEnumShellItems` walk → `(title, AUMID)` pairs → candidates with `shell:AppsFolder\{AUMID}` paths.
- `.lnk` target dedup at `core/engine/src/platform/windows/lnk.rs`: each Start Menu shortcut's target is resolved via `IShellLinkW::GetPath` and stamps a `target_path:{normalized}` lock so the fallback `.exe` walker skips the same binary.
- Icons via `apps/linows/src-tauri/src/platform/windows/icons.rs`: `IShellItemImageFactory::GetImage` → HBITMAP → `GetDIBits` BGRA → `png` encode → base64 data URL. Path separators are normalized (`/` → `\`) before handing to `SHCreateItemFromParsingName`.
- Per-page settings glyphs via `apps/linows/src/js/settings-icons/windows.js`: maps `ms-settings:*` and `look-cmd://*` keys to Lucide SVGs (no real Settings icons exist; Windows hides them in the SystemSettings package's MRT resources).

Open issues:
- **`file_scan_drives_dismissed` config key is dead.** Inherited from WinUI3; engine doesn't read it. Either honor it or drop it from `default_config.txt`.

Resolved:
- **Focus-existing-app on Enter** — ported `apps/windows/.../ActionDispatcher.cs::TryActivateExistingAppWindow` to `apps/linows/src-tauri/src/platform/windows/window_focus.rs`. UWP entries match by AUMID (`GetApplicationUserModelId` over processes under `\WindowsApps\`); `.lnk`/`.exe` entries resolve the shortcut target then match by full exe path. `commands.rs::open_path` runs the focus probe before `window.hide()` (SetForegroundWindow needs us to still hold foreground), then falls through to `open::that` on miss.
- **Rounded corners** — DWM rounding via `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND)` in `effects.rs`, paired with `window.set_background_color(Color(0,0,0,0))` in `main.rs` so WebView2's default background goes transparent and DWM's clip has nothing opaque to fight with. **Critical:** the frontend calls `setWindowEffect('mica')` after mount, which goes through `window.set_effects()` → `window-vibrancy::apply_mica`. That call reconfigures `DWMWA_SYSTEMBACKDROP_TYPE` and silently resets the corner preference — so we re-apply corners inside `apply()` after every `set_effects`. Initial set in `main.rs::setup` is still needed because the frontend doesn't call `set_window_effect` until the JS bundle loads. Two dead-end attempts: (a) `SetWindowRgn(CreateRoundRectRgn(…))` — GDI edges are aliased, visibly jagged; (b) `DWMWA_BORDER_COLOR = DWMWA_COLOR_NONE` — on a borderless transparent window DWM ties the corner clip to the same frame pass, so NONE collapses the window back to square corners.

### Linux verification needed for Phase B

Phase B was developed on a Windows host. Linux compilation was not verified locally. Before merging, run on a Linux dev machine:

```bash
cd apps/linows
cargo check --manifest-path src-tauri/Cargo.toml
cargo tauri dev    # smoke test: launcher opens, search/open/reveal/clipboard/autostart still work
```

If anything Linux-only breaks, the most likely culprits are stale `crate::linux_*` paths that grep missed (none expected — grep was clean), or visibility issues with `pub(crate)` vs the old default `pub`.
