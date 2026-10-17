# Add to ggg — Chrome Extension

Sends URLs from the browser to ggg via the right-click context menu.

```
[Chrome Extension] --(Native Messaging stdio)--> [ggg-bridge.exe] --(Named Pipe)--> [ggg]
```

## Architecture

| Component        | Role                                                  |
|------------------|-------------------------------------------------------|
| `background.js`  | MV3 service worker. Registers menu items, sends `add_url` to the native host. |
| `manifest.json`  | Extension manifest (Manifest V3).                     |
| `ggg-bridge.exe` | Native Messaging host. Translates stdio frames to `\\.\pipe\ggg`. |
| `ggg`            | TUI app. Accepts URLs on the named pipe.              |

The native host name is **`com.ggg.bridge`** and must match the host
manifest installed by `ggg bridge install`.

## Install

1. Put `ggg-bridge.exe` next to `ggg.exe`. The release archive already has it
   there; from source, build it with `cargo build --release -p ggg-bridge`.
2. Load the extension:
   - Chrome → `chrome://extensions` → enable Developer Mode → "Load unpacked"
   - Select this directory (`windows/ggg-extension`).
   - Note the assigned **Extension ID** (e.g. `abcdefghijklmnopabcdefghijklmnop`).
3. Register the native host, allowing that extension:
   - `ggg bridge install --extension-id <ID>` (`--extension-id` is required;
     re-running install is fine).
   - This writes `%LOCALAPPDATA%\ggg\com.ggg.bridge.json` and a registry
     value at `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.ggg.bridge`.
4. Right-click any link or page and pick "Add link to ggg". If ggg is not
   running, the bridge starts the GUI (see below).

## When ggg is not running

If nothing serves the pipe, `ggg-bridge.exe` starts `ggg-gui.exe` from its own
directory, waits up to 10 seconds for it to answer, then delivers the link. Put
`ggg-gui.exe` next to `ggg-bridge.exe` for this to work; the bridge does not
search `PATH` and starts the GUI on the default configuration directory. When
`ggg-gui.exe` is missing or does not answer in time, the extension reports that
ggg is not running. The TUI is never started this way.

## Uninstall

```
ggg bridge uninstall
```

Removes the registry value and the host manifest, then unload the
extension from `chrome://extensions`.

## Development notes

- `sendNativeMessage` (one-shot) is used instead of `connectNative` so
  the bridge process exits between requests. The pipe client opens a
  fresh connection per call, matching the existing ggg-dnd pattern.
- The selection-context menu only fires if the highlighted text parses
  as a valid URL. Otherwise it silently does nothing — use "Add this
  page" for non-URL selections.
- Icons are intentionally omitted; notifications use an inline 1×1 PNG
  to avoid a separate icon asset. Add real icons under `icons/` and
  reference them from `manifest.json` when ready to ship.
