# KeyBindings Guide

Reference for every keyboard shortcut in the TUI, the screens ("contexts")
each one is active in, and how to customize them. This mirrors the
`?` help screen inside the app, which is generated from the same
configuration.

## Contexts

A key press is resolved against the context of the screen or overlay
currently shown, so the same physical key can mean different things (or
nothing) in different places:

| Context    | Where it applies |
|------------|-------------------|
| `Normal`   | The three-pane main screen. |
| `Settings` | The Settings tabs (General/Downloads/Scripts/Folders) and the folder editor. |
| `Menu`     | The download context menu and the folder context menu. |
| `Dialog`   | Change Folder, Switch Folder, Confirm Delete, and the download preview dialog. |
| `Help`     | The help overlay (opened with `?`). |

## Default bindings

The **Config key** column is the name used under `[keybindings]` in
`settings.toml` (see `config/settings.toml.example`).

| Action                  | Default key(s)   | Config key                | Contexts |
|--------------------------|------------------|----------------------------|----------|
| Move up                  | `k`, `Up`        | `move_up`                  | Normal, Settings, Menu, Dialog, Help |
| Move down                | `j`, `Down`      | `move_down`                | Normal, Settings, Menu, Dialog, Help |
| Move to top               | `g`, `Home`      | `move_to_top`               | Normal, Settings, Dialog, Help |
| Move to bottom             | `G`, `End`       | `move_to_bottom`            | Normal, Settings, Dialog, Help |
| Page up                  | `Ctrl+u`         | `page_up`                  | Normal, Settings, Dialog, Help |
| Page down                | `Ctrl+d`         | `page_down`                | Normal, Settings, Dialog, Help |
| Focus next pane           | `Tab`            | `focus_next_pane`          | Normal, Settings |
| Focus previous pane        | `Shift+Tab`      | `focus_prev_pane`          | Normal, Settings |
| Focus left / previous tab  | `h`, `Left`      | `focus_left`                | Normal, Settings |
| Focus right / next tab     | `l`, `Right`     | `focus_right`               | Normal, Settings |
| Select item                | `Enter`          | `select_item`               | Normal, Settings, Menu, Dialog |
| Toggle selection           | `Space`          | `toggle_selection`          | Normal, Settings |
| Select all                 | `V`              | `select_all`                | Normal |
| Deselect all                | `Escape`         | `deselect_all`              | Normal |
| Add download                | `a`              | `add_download`              | Normal, Settings |
| Delete download              | `d`              | `delete_download`           | Normal, Settings, Menu |
| Start/pause download         | `s`              | `toggle_download`           | Normal, Menu |
| Retry download               | `r`              | `retry_download`            | Normal, Menu |
| Resume all                  | `S`              | `resume_all`                | Normal |
| Pause all                   | `P`              | `pause_all`                 | Normal |
| Open context menu            | `m`              | `open_context_menu`         | Normal |
| Edit item (save path/referrer) | `e`            | `edit_item`                 | Normal, Menu |
| Toggle details pane           | `i`              | `toggle_details`            | Normal |
| Cycle details pane position     | `D`              | `cycle_details_position`    | Normal |
| Open search                 | `/`              | `open_search`                | Normal |
| Open/close help              | `?`              | `open_help`                 | Normal, Help |
| Open settings                | `x`              | `open_settings`              | Normal |
| Switch folder                | `F`              | `switch_folder`              | Normal |
| Quit                       | `q`, `Ctrl+c`    | `quit`                      | Normal |
| Undo                       | `Ctrl+z`         | `undo`                      | Normal |
| Refresh                    | `R`              | `refresh`                   | Normal |
| Back / close                 | `Escape`, `q`    | `back`                      | Settings, Menu, Dialog, Help |
| Confirm yes                  | `y`              | `confirm_yes`                | Dialog |
| Confirm no                   | `n`              | `confirm_no`                 | Dialog |
| Reload settings.toml          | `R`              | `reload_config`              | Settings |

Notes:

- **Edit item** (`e`) opens a small dialog to change the save path and/or
  referrer of the selected download(s) (Save path first, Referrer second;
  `Tab`/`focus_next_pane` and `focus_left`/`focus_right` move between the two
  fields).
- The Add Download dialog also has a Referrer field alongside the URL,
  reached the same way.
- The Settings screen has four tabs — General, Downloads, Scripts, Folders —
  switched with `focus_left`/`focus_right`.
- The help overlay sizes itself to the terminal and scrolls with the
  navigation keys above (`move_up`/`move_down`/`page_up`/`page_down`/
  `move_to_top`/`move_to_bottom`); `back` or `open_help` closes it.

## Customizing

Add a `[keybindings]` table to `settings.toml` (see
`config/settings.toml.example` for a fully commented copy of the defaults
above). Rules:

- Only the actions you list are overridden; every action left out keeps its
  default binding.
- Listing an action replaces its *entire* key list, it does not merge with
  the default: `move_down = "j"` drops the default `Down` binding too, it
  does not add "j" alongside it.
- An unrecognized action name is ignored (with a warning logged), so a
  settings.toml written for an older or newer build still loads.
- If two actions end up bound to the same key in the same context, the one
  that comes first alphabetically by its internal name wins; the loser is
  logged as a conflict warning. `ggg`'s own defaults never conflict.

A key string is either a single character (`"a"`, `"j"`, an uppercase
letter implies Shift), a named key (`Enter`, `Space`, `Tab`, `BackTab`,
`Escape`, `Up`/`Down`/`Left`/`Right`, `Home`/`End`, `PageUp`/`PageDown`,
`Delete`, `Insert`, `F1`-`F12`), optionally prefixed with modifiers
(`Ctrl+`, `Alt+`, `Shift+`, e.g. `"Ctrl+z"`), or a list of any of the above
(`["k", "Up"]`) to bind more than one key to the same action.
