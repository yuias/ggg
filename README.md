# ggg

A terminal-based download manager, built in Rust.

## Installation

### From GitHub Releases

TBD

### From Source (Recommended)

```bash
# Clone repository
git clone https://github.com/yuiAs/ggg.git
cd ggg

# Build release version
cargo build --release

# Binary will be at: target/release/ggg.exe (Windows) or target/release/ggg (Linux/macOS)
```

### Requirements

- **Rust** 1.85+ (edition 2024)
- An ANSI-compatible terminal (Windows Terminal, cmd, PowerShell, WSL, SSH, etc.)

## Usage

### Launch TUI

```bash
# Run from source
cargo run

# Or run compiled binary
./target/release/ggg
```

### Keybindings

For a complete keybindings reference, see the [KeyBindings Guide](docs/KeyBindings.md) or press `?` in the TUI for the help screen.

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

### Chrome Extension (Add to ggg)

A Chrome extension can push URLs into ggg from the browser's context menu.
URLs are routed through `ggg-bridge.exe` (a Chrome Native Messaging host)
into the same Named Pipe used by `ggg-dnd`.

#### Register the bridge

```bash
# 1. Build the bridge binary alongside ggg
cargo build --release -p ggg-bridge

# 2. Load windows/ggg-extension/ as an unpacked extension
#    in chrome://extensions and copy the assigned Extension ID.

# 3. Install the Native Messaging host manifest + registry entry.
#    --extension-id is required (repeatable for multiple IDs).
ggg bridge install --extension-id <EXTENSION_ID>

# Verify
ggg bridge status
```

#### Unregister

```bash
ggg bridge uninstall
```

This removes both `%LOCALAPPDATA%\ggg\com.ggg.bridge.json` and
`HKCU\Software\Google\Chrome\NativeMessagingHosts\com.ggg.bridge`.
The extension itself must be removed manually from `chrome://extensions`.

See [`windows/ggg-extension/README.md`](windows/ggg-extension/README.md)
for extension-side details.

## Configuration

ggg uses a TOML-based configuration system with application-wide and folder-specific settings.

For a complete, fully documented configuration example, see `examples/settings.toml.example` in the release archive or repository. Every setting is explained inline, with folder-specific overrides covered at the bottom.

## Project Structure

Top-level modules under `src/`:

| Module | Role |
|--------|------|
| `app`     | Config, keybindings, and shared application state. |
| `cli`     | Argument parsing, CLI command handlers, and daemon routing/control (`src/cli/remote.rs`). |
| `download` | The download manager, queue, and task types. |
| `file`    | Filesystem helpers (paths, naming). |
| `ipc`     | The Windows Named Pipe control protocol shared with the CLI, `ggg-dnd`, and `ggg-bridge`. |
| `script`  | The JavaScript hook engine (see [Script_UserGuide.md](docs/Script_UserGuide.md)). |
| `tui`     | The terminal UI (screens, keybinding resolution, rendering). |
| `ui`      | Low-level rendering helpers shared across the TUI. |
| `util`    | Small cross-cutting helpers (path resolution, redaction, ...). |

`windows/` holds the three sibling crates described above
(`ggg-ipc`, `ggg-dnd`, `ggg-bridge`).

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
```

## Helper Components (Windows)

The workspace ships three Windows-only sibling crates under `windows/`
that complement the main `ggg` binary:

| Crate          | Kind   | Role                                                                           |
|----------------|--------|--------------------------------------------------------------------------------|
| `ggg-ipc`      | lib    | Shared IPC protocol types and the synchronous Named Pipe client.               |
| `ggg-dnd`      | binary | Win32 GUI helper that accepts browser drag & drop and forwards URLs to ggg.    |
| `ggg-bridge`   | binary | Chrome Native Messaging host. Forwards URLs from the Chrome extension to ggg.  |

All three speak the same line-delimited JSON protocol on `\\.\pipe\ggg-dnd`,
defined in `ggg-ipc::protocol`. `ggg-dnd` and `ggg-bridge` are clients;
`ggg` itself is the server.

### ggg-dnd

Lightweight Win32 GUI helper that accepts browser drag & drop and forwards
URLs to the TUI via Named Pipes.

- Entire window is an OLE drop target — drag a URL from your browser onto it
- Communicates with `ggg` over Named Pipe (`\\.\pipe\ggg-dnd`)
- Can be auto-launched from `ggg` via the `Auto Launch ggg-dnd` setting

### ggg-bridge

Chrome Native Messaging host (console subsystem). Reads
length-prefixed JSON frames from stdin (per the Chrome Native Messaging
spec) and forwards each request to the same Named Pipe. Stays alive
until Chrome closes stdin, so both `sendNativeMessage` (one-shot) and
`connectNative` (persistent port) work. See the *Chrome Extension*
section above for installation.

### ggg-ipc

Internal library. Both `ggg-dnd` and `ggg-bridge` depend on it for
`IpcRequest` / `IpcResponse` definitions and the `send_url` / `ping`
helpers, so the wire protocol is defined exactly once.

### Building

```bash
# Build everything (default workspace members)
cargo build --release

# Build a single helper
cargo build --release -p ggg-dnd
cargo build --release -p ggg-bridge
```

Place the resulting `.exe` files next to `ggg.exe` so auto-launch
(`ggg-dnd`) and `ggg bridge install` (`ggg-bridge`) can find them
without an explicit path.

## Known Issues

### Limitations

- No GUI version (by design — TUI is the primary interface)
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
