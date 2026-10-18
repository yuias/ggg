# ggg

A terminal-based download manager, built in Rust.

## Installation

### From GitHub Releases

Download `ggg-<version>-<hash>.zip` from the Releases page, extract it
anywhere and run `ggg.exe`. The archive is built for Windows and contains:

- `ggg.exe`, `ggg-dnd.exe`, `ggg-bridge.exe` (the helpers must stay next to `ggg.exe`)
- `docs/`: keybindings, scripting and language guides
- `examples/`: a commented `settings.toml.example` and example scripts
- `locales/`: translation files, for [overriding the built-in language](docs/i18n.md)
- `windows/ggg-extension/`: the Chrome extension

The archive has no `config/` folder. If you create one next to `ggg.exe`, ggg
uses it as its config directory (portable mode); otherwise the config lives in
`~/.config/ggg/`. See `examples/settings.toml.example` for the full search order.

### From Source

```bash
git clone https://github.com/yuias/ggg.git
cd ggg
cargo build --release
```

The binary is `target/release/ggg.exe` on Windows and `target/release/ggg`
on Linux/macOS.

### Requirements

- **Rust** 1.85+ (edition 2024), only to build from source
- An ANSI-compatible terminal (Windows Terminal, cmd, PowerShell, WSL, SSH, etc.)

## Usage

### Launch TUI

```bash
# From a release archive or a release build
ggg

# From the source tree
cargo run
```

### Keybindings

See the [Keybindings guide](docs/Keybindings.md), or press `?` in the TUI for the help screen.

### URL Range Expansion

A URL containing a numeric range `[start-end]` is expanded into one download
per number when it is added (TUI add dialog, `ggg add`, and each line of
`ggg batch-add`):

```bash
ggg add "https://example.com/img[9-11].jpg"
# -> img9.jpg, img10.jpg, img11.jpg

ggg add "https://example.com/img[009-011].jpg"
# -> img009.jpg, img010.jpg, img011.jpg (a leading zero keeps the width)
```

- Only the first `[start-end]` in a URL is expanded.
- A range whose end is below its start, or that would produce more than
  1000 URLs, adds nothing.
- When the brackets are part of the real URL, pass `--no-expand` to `add` or
  `batch-add` to take each URL literally:

```bash
ggg add --no-expand "https://example.com/archive[1-2].zip"
ggg batch-add --no-expand urls.txt
```

### Scripts

JavaScript hooks can modify requests and react to downloads. See the
[Script guide](docs/Script.md); examples are in `examples/scripts/`.

### Languages

The UI is available in English and Japanese. Only English is built into the
binary; for Japanese, copy `locales/ja-JP/` from the release archive (or the
source tree) into the override directory (`%APPDATA%\ggg\locales\` on
Windows). See the [language guide](docs/i18n.md) for other platforms.

## Configuration

ggg uses TOML configuration with application-wide and per-folder settings.
`examples/settings.toml.example` documents every setting inline, including
the per-folder overrides at the bottom.

To use an example script, copy it from `examples/scripts/` into
`<config_dir>/scripts/`.

## Windows helpers

Three Windows-only crates under `windows/` complement `ggg`. In a release
archive the `.exe` files are already next to `ggg.exe`, which is where
auto-launch (`ggg-dnd`) and `ggg bridge install` (`ggg-bridge`) look for them.

| Crate        | Kind   | Role                                                                          |
|--------------|--------|-------------------------------------------------------------------------------|
| `ggg-ipc`    | lib    | Shared IPC protocol types and the synchronous Named Pipe client.              |
| `ggg-dnd`    | binary | Win32 GUI helper that accepts browser drag & drop and forwards URLs to ggg.   |
| `ggg-bridge` | binary | Chrome Native Messaging host. Forwards URLs from the Chrome extension to ggg. |

### Drag and drop (ggg-dnd)

- The whole window is an OLE drop target: drag a URL from your browser onto it.
- It talks to `ggg` over the Named Pipe `\\.\pipe\ggg`.
- It can be auto-launched from `ggg` with the `Auto Launch ggg-dnd` setting.

### Chrome extension (ggg-bridge)

The extension adds "Add ... to ggg" context-menu entries (link, page, image)
in Chrome and sends the URL to ggg through `ggg-bridge.exe`.

1. Make sure `ggg-bridge.exe` is next to `ggg.exe`. It is in the release
   archive, or build it with `cargo build --release -p ggg-bridge`.
2. In `chrome://extensions`, load `windows/ggg-extension/` as an unpacked
   extension and copy the Extension ID it gets.
3. Register the Native Messaging host. `--extension-id` is required and can be
   repeated for multiple IDs:

   ```bash
   ggg bridge install --extension-id <EXTENSION_ID>
   ggg bridge status
   ```

To unregister, run `ggg bridge uninstall`. It removes
`%LOCALAPPDATA%\ggg\com.ggg.bridge.json` and
`HKCU\Software\Google\Chrome\NativeMessagingHosts\com.ggg.bridge`.
The extension itself must be removed manually from `chrome://extensions`.

See [`windows/ggg-extension/README.md`](windows/ggg-extension/README.md) for
extension details.

#### Starting the GUI from the extension

When nothing serves the link pipe, `ggg-bridge.exe` starts `ggg-gui.exe` and
delivers the link once the GUI answers (it waits up to 10 seconds). The
bridge looks only for `ggg-gui.exe` in its own directory, so keep the two files
side by side. It starts the GUI without arguments, so the GUI uses the default
configuration directory. If `ggg-gui.exe` is missing, or the GUI does not
answer in time, the extension reports that ggg is not running. Nothing is
started when a process already owns the pipe.

### GUI instances (ggg-gui)

```bash
ggg-gui [--config DIR] [URL]...
```

Only one GUI runs per configuration directory. Starting `ggg-gui` again for the
same directory brings the running window to the foreground (the Settings window
if it is open), adds any URLs given on the command line, and exits with
status 0. GUIs started with different `--config` directories run
independently.

The GUI and the TUI share the link pipe `\\.\pipe\ggg`, and the first one to
start owns it. If the TUI owns it, a GUI that starts later listens on
`\\.\pipe\ggg-<pid>` (shown in its status-bar tooltip), and the bridge keeps
talking to the TUI. After the TUI quits, a GUI on such a fallback pipe takes
over `\\.\pipe\ggg` the next time it is activated; a link sent from the
extension triggers that, because the bridge then starts a second GUI that
activates the first.

Set `GGG_PIPE` to a pipe name of the form `\\.\pipe\<name>` to move the link
pipe for every ggg program started with that variable, for example in tests
or scripts that must not meet a running ggg. Values that are not of that form
are ignored. Use it together with `--config` or `GGG_CONFIG_DIR` to keep such
a run away from your real configuration.

### Internals

`ggg` is the server; `ggg-dnd` and `ggg-bridge` are clients. All three speak a
line-delimited JSON protocol over the Named Pipe, defined in
`ggg-ipc::protocol` (`IpcRequest` / `IpcResponse`, plus the `send_url` and
`ping` helpers). `ggg-bridge` reads length-prefixed JSON frames from stdin, as
the Chrome Native Messaging spec requires, and stays alive until Chrome closes
stdin, so both `sendNativeMessage` (one-shot) and `connectNative` (persistent
port) work.

## Project Structure

Top-level modules under `src/`:

| Module | Role |
|--------|------|
| `app`     | Config, keybindings, and shared application state. |
| `cli`     | Argument parsing, CLI command handlers, and daemon routing/control (`src/cli/remote.rs`). |
| `download` | The download manager, queue, and task types. |
| `file`    | Filesystem helpers (paths, naming). |
| `ipc`     | The Windows Named Pipe control protocol shared with the CLI, `ggg-dnd`, and `ggg-bridge`. |
| `script`  | The JavaScript hook engine (see the [Script guide](docs/Script.md)). |
| `tui`     | The terminal UI (screens, keybinding resolution, rendering). |
| `ui`      | Low-level rendering helpers shared across the TUI. |
| `util`    | Small cross-cutting helpers (path resolution, redaction, ...). |

`windows/` holds the helper crates and the Chrome extension described in
[Windows helpers](#windows-helpers).

## Development

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test suite
cargo test --test download_manager_tests

# Run with output
cargo test -- --nocapture
```

### Building

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release

# Check without building
cargo check

# A single helper (Windows)
cargo build --release -p ggg-dnd
cargo build --release -p ggg-bridge
```

### Release archive

`tools/build-release.ps1` (requires PowerShell 7) builds in release mode and
writes `dist/ggg-<version>-<hash>.zip`:

```powershell
pwsh tools/build-release.ps1
pwsh tools/build-release.ps1 -SkipBuild -OutputDir out
```

- `-OutputDir` is where the archive goes (default `dist`; a relative path is
  resolved against the repository root).
- `-SkipBuild` packages the binaries already in the cargo target directory.
- On non-Windows hosts only `ggg` is built; the Windows helpers and the
  extension are left out.

## Known Issues

### Limitations

- `ggg-gui` is Windows-only for now; it builds elsewhere but has no menu bar, drop target or link pipe there
- Single-threaded downloads per file (chunk-based multi-connection acceleration not yet implemented)
- `ggg-dnd` and `ggg-bridge` are Windows-only

## License

[MIT License](LICENSE)

## Acknowledgments

Inspired by classic download managers:
- **Iria** / **Irvine** (Windows download manager)
- **ReGet** (Windows download manager)

Built with:
- [ratatui](https://github.com/ratatui-org/ratatui) - Terminal UI framework
- [crossterm](https://github.com/crossterm-rs/crossterm) - Terminal manipulation
- [deno_core](https://github.com/denoland/deno_core) - V8 JavaScript runtime
- [tokio](https://tokio.rs/) - Async runtime
- [reqwest](https://github.com/seanmonstar/reqwest) - HTTP client
- [serde](https://serde.rs/) - Serialization
