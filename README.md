<p align="center"><img src="assets/unlatch.png" width="128" alt="Unlatch icon"></p>

# Unlatch

**Find out what is locking a file or folder on Windows, and release it.**

Right-click a file, folder or drive, choose **Unlock with Unlatch**, and you get a list of every
process holding it open. From there you can end a process, close a single handle, or send the item
to the Recycle Bin once it is free.

Written in Rust. A single self-contained `.exe`, no runtime to install, no admin rights needed to
install.

## Features

- **Right-click integration** for files, folders, folder backgrounds and drives, including the
  Windows 11 top-level menu (no "Show more options" needed).
- **Three detection methods**, merged into one list per process:
  1. *Open handles*: every file and directory handle in the system, so it also catches programs
     whose working directory is inside a folder (a common cause of "folder in use" that the
     Restart Manager does not report).
  2. *Loaded modules*: `.exe` / `.dll` files a process is running from.
  3. *Restart Manager*: the official Windows API, which adds friendly app names and service names.
- **End process**: one process, or all of them at once, with a confirmation that warns about
  services and Explorer. Explorer is restarted if it does not come back on its own.
- **Close handle**: release one lock without ending the program (the program is not told, so use
  with care).
- **Move to Recycle Bin**: delete the item once it is unlocked (undoable).
- **Run as administrator** button, to see and act on services and other users' processes.
- Drag and drop, or *Choose file / Choose folder*, when started on its own.
- `--list` mode for scripts.

## Install

Download **`Unlatch-<version>-Setup.exe`** from the
[Releases](https://github.com/AdriaanBoshoff/Unlatch/releases) page and run it. It installs for
your user account only, so no administrator prompt. Setup is not code-signed yet, so Windows
SmartScreen may warn you first; choose *More info → Run anyway*.

To update, run the newer Setup over the old install. To remove Unlatch, use
*Settings → Apps → Installed apps*.

### Portable / manual install

The release also has a `.zip` with `Unlatch.exe` and `unlatch_shell.dll`. Keep the two files
together and run:

```
unlatch.exe --install
```

This copies both to `%LOCALAPPDATA%\Programs\Unlatch`, adds classic right-click entries under
`HKEY_CURRENT_USER\Software\Classes`, and registers an entry in *Settings → Apps → Installed apps*.
Nothing is written outside your user profile.

**Windows 11 top-level menu.** Windows 11 only shows packaged `IExplorerCommand` handlers in its
compact menu. `--install` writes a small sparse package manifest (embedded in the exe) to
`%LOCALAPPDATA%\Programs\Unlatch\package` and registers it with `Add-AppxPackage -Register`,
pointing at `unlatch_shell.dll`. Registering an unsigned package this way needs **Developer Mode**
(*Settings → System → For developers*). Without it, or without the DLL, you still get the classic
entry under **Show more options**, and the install message says which one you got.

In the top-level menu, choosing *Unlock with Unlatch* on several selected items opens one window
for all of them. Drives only get the classic entry, because package manifests cannot target drives.

To remove it, uninstall from *Installed apps* or run `unlatch.exe --uninstall`.

## Usage

| Command | What it does |
|---|---|
| `unlatch [PATH...]` | Open the window for these paths |
| `unlatch --install` | Add *Unlock with Unlatch* to the right-click menu |
| `unlatch --uninstall` | Remove it again |
| `--quiet` (with either of the above) | No message box; exit code 0 = done, 1 = failed, 2 = installed with the classic menu only |
| `unlatch --list PATH...` | Print what is holding each path to the console |

## Building

Needs Rust 1.85+ (edition 2024). The Windows SDK resource compiler (`rc.exe`) is used, if
present, to embed the icon, manifest and version info; without it the build still works.

**MSVC toolchain** (Visual Studio Build Tools installed):

```
cargo build --release --workspace
```

**Without Visual Studio**, using [llvm-mingw](https://github.com/mstorsjo/llvm-mingw):

```
winget install MartinStorsjo.LLVM-MinGW.UCRT
rustup toolchain install stable-x86_64-pc-windows-gnullvm
# put llvm-mingw's bin folder on PATH, then:
cargo +stable-x86_64-pc-windows-gnullvm build --release --workspace
```

`--workspace` also builds `unlatch_shell.dll` (the Windows 11 menu handler) next to the exe.
Both are linked statically (see `.cargo/config.toml`), so the resulting exe has no DLL dependencies
beyond Windows itself.

**Installer.** With [Inno Setup 6](https://jrsoftware.org/isinfo.php) installed:

```
ISCC.exe /DAppVersion=0.1.0 installer\Unlatch.iss
```

This writes `target\installer\Unlatch-0.1.0-Setup.exe`. Setup unpacks the two files to a temporary
folder and runs `Unlatch.exe --install --quiet`, so it always installs exactly the way
`--install` does.

The icon is generated by `tools/make_icon.py` (needs Pillow) and the outputs are committed in
`assets/`.

## How it works

| File | Role |
|---|---|
| `src/handles.rs` | System handle table via `NtQuerySystemInformation`; each file handle is duplicated and resolved with `GetFinalPathNameByHandleW` on a watchdog thread, because some handle types (pipes) can hang that call |
| `src/rstmgr.rs` | Restart Manager session (`RmRegisterResources` / `RmGetList`) |
| `src/procs.rs` | Process names, image paths, module lists, `TerminateProcess`, remote handle close (`DuplicateHandle` with `DUPLICATE_CLOSE_SOURCE`), elevation checks |
| `src/scan.rs` | Normalises paths, runs the three detectors and merges results per process |
| `src/shell.rs` | Registry install/uninstall, Recycle Bin, UAC relaunch |
| `src/app.rs` | The egui window |
| `shellext/` | `unlatch_shell.dll`: COM class factory and `IExplorerCommand` for the Windows 11 menu; starts `Unlatch.exe` with the selected paths |
| `packaging/` | Sparse package manifest and logos, embedded into the exe and registered on install |

## Clean-room statement

Unlatch was written from scratch using only Microsoft's public Windows API documentation. No code,
resources or design assets were taken from PowerToys File Locksmith or any other similar tool.

## Roadmap

- A signed `.msix` so the Windows 11 top-level menu works without Developer Mode.
- One window for a multi-selection in the classic menu too (the top-level menu already does this).

## License

MIT, see [LICENSE](LICENSE).
