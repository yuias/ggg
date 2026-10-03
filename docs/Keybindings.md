# Keybindings

Every keyboard shortcut in the TUI, the screens ("contexts") each one works in, and how to change them. The `?` help screen in the app lists the current bindings.

## Contexts

A key press is resolved against the screen or overlay that is currently shown, so the same key can mean different things in different places:

| Context    | Where it applies |
|------------|------------------|
| `Normal`   | The three-pane main screen. |
| `Settings` | The Settings tabs (General, Downloads, Scripts, Folders) and the folder editor. |
| `Menu`     | The download context menu and the folder context menu. |
| `Dialog`   | Change Folder, Switch Folder, Confirm Delete, and the download preview dialog. |
| `Help`     | The help overlay (opened with `?`). |

## Default bindings

The **Config key** column is the name used under `[keybindings]` in `settings.toml` (see `examples/settings.toml.example`).

| Action                             | Default key(s)        | Config key               | Contexts                             |
|------------------------------------|-----------------------|--------------------------|--------------------------------------|
| Move up                            | `k`, `Up`             | `move_up`                | Normal, Settings, Menu, Dialog, Help |
| Move down                          | `j`, `Down`           | `move_down`              | Normal, Settings, Menu, Dialog, Help |
| Move to top                        | `g`, `Home`           | `move_to_top`            | Normal, Settings, Dialog, Help       |
| Move to bottom                     | `G`, `End`            | `move_to_bottom`         | Normal, Settings, Dialog, Help       |
| Page up                            | `Ctrl+u`              | `page_up`                | Normal, Settings, Dialog, Help       |
| Page down                          | `Ctrl+d`              | `page_down`              | Normal, Settings, Dialog, Help       |
| Focus next pane / next tab         | `Tab`                 | `focus_next_pane`        | Normal, Settings                     |
| Focus previous pane / previous tab | `BackTab` (Shift+Tab) | `focus_prev_pane`        | Normal, Settings                     |
| Focus left                         | `h`, `Left`           | `focus_left`             | Normal, Settings                     |
| Focus right                        | `l`, `Right`          | `focus_right`            | Normal, Settings                     |
| Select item                        | `Enter`               | `select_item`            | Normal, Settings, Menu, Dialog       |
| Toggle selection                   | `Space`               | `toggle_selection`       | Normal, Settings                     |
| Select all                         | `V`                   | `select_all`             | Normal                               |
| Deselect all                       | `Escape`              | `deselect_all`           | Normal                               |
| Add download                       | `a`                   | `add_download`           | Normal, Settings                     |
| Delete download                    | `d`                   | `delete_download`        | Normal, Settings, Menu               |
| Start/pause download               | `s`                   | `toggle_download`        | Normal, Menu                         |
| Retry download                     | `r`                   | `retry_download`         | Normal, Menu                         |
| Resume all                         | `S`                   | `resume_all`             | Normal                               |
| Pause all                          | `P`                   | `pause_all`              | Normal                               |
| Open context menu                  | `m`                   | `open_context_menu`      | Normal                               |
| Edit item (save path/referrer)     | `e`                   | `edit_item`              | Normal, Menu                         |
| Toggle details pane                | `i`                   | `toggle_details`         | Normal                               |
| Cycle details pane position        | `D`                   | `cycle_details_position` | Normal                               |
| Open search                        | `/`                   | `open_search`            | Normal                               |
| Open/close help                    | `?`                   | `open_help`              | Normal, Help                         |
| Open settings                      | `x`                   | `open_settings`          | Normal, Settings                     |
| Switch folder                      | `F`                   | `switch_folder`          | Normal                               |
| Quit                               | `q`, `Ctrl+c`         | `quit`                   | Normal                               |
| Undo                               | `Ctrl+z`              | `undo`                   | Normal                               |
| Refresh                            | `R`                   | `refresh`                | Normal                               |
| Back / close                       | `Escape`, `q`         | `back`                   | Settings, Menu, Dialog, Help         |
| Confirm yes                        | `y`                   | `confirm_yes`            | Dialog                               |
| Confirm no                         | `n`                   | `confirm_no`             | Dialog                               |
| Reload settings.toml               | `R`                   | `reload_config`          | Settings                             |

Notes:

- In the Edit item (`e`) and Add Download dialogs, `Tab` and `Shift+Tab` move between the two fields (Save path and Referrer; URL and Referrer). The Edit item dialog also accepts `Down` and `Up`. These keys are fixed and cannot be changed in `[keybindings]`.
- On the Settings screen, `focus_next_pane` and `focus_prev_pane` switch tabs. On the Folders tab, `focus_right` opens the selected folder and `focus_left` leaves the folder editor.
- The help overlay scrolls with `move_up`, `move_down`, `page_up`, `page_down`, `move_to_top` and `move_to_bottom`. `back` or `open_help` closes it.
- `refresh` does nothing visible; the screen updates on its own.

## Customizing

Add a `[keybindings]` table to `settings.toml`. `examples/settings.toml.example` has a commented copy of the defaults. Rules:

- Only the actions you list are changed; all others keep their default keys.
- A listed action replaces all of its default keys instead of adding to them: `move_down = "j"` also removes the default `Down`.
- An unknown action name is ignored and a warning is logged, so a `settings.toml` written for another version still loads.
- If two actions use the same key in the same context, only one keeps it and a conflict warning is logged. The action whose name sorts first alphabetically wins (names compared as `MoveDown`-style names, so `PauseAll` beats `Quit`). The defaults have no conflicts.

A key is written as a string:

- A single character: `"a"`, `"j"`. An uppercase letter such as `"G"` is typed with Shift.
- A named key: `Enter`, `Space`, `Tab`, `BackTab`, `Escape`, `Backspace`, `Up`, `Down`, `Left`, `Right`, `Home`, `End`, `PageUp`, `PageDown`, `Delete`, `Insert`, `F1` to `F12`.
- A key with modifiers: `Ctrl+`, `Alt+` or `Shift+` in front, for example `"Ctrl+z"`.
- A list of keys to bind several keys to one action: `["k", "Up"]`.
