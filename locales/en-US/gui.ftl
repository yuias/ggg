# GUI-only strings - English
# Shared strings stay in the other files; add a key here only when the GUI needs its own.

gui-app-title = ggg
# Main window title: the folder that new downloads go to, then the application name
gui-window-title = { $folder } - { $app }

# Folder tree
gui-tree-folder = { $name } ({ $count })
gui-tree-completed = Completed ({ $count })

# Table columns that have no shared string
gui-column-added = Added
gui-column-completed = Completed
gui-column-url = URL
gui-history-search-hint = Search completed downloads

# Details panel
gui-details-tab-info = Info
gui-details-tab-log = Log
gui-details-label-error = Error:
gui-details-label-progress = Progress:

# Status bar summary
gui-statusbar-items = { $count ->
    [one] { $count } item
   *[other] { $count } items
}
gui-statusbar-selected = { $count } selected
gui-statusbar-active = { $count } active
gui-statusbar-pipe-tooltip = Receiving links from ggg-dnd and the browser extension on { $name }

# Status bar messages for finished actions
gui-status-toggled = { $count ->
    [one] Started or paused { $count } download.
   *[other] Started or paused { $count } downloads.
}
gui-status-retried = { $count ->
    [one] Retrying { $count } download.
   *[other] Retrying { $count } downloads.
}
gui-status-deleted = { $count ->
    [one] Deleted { $count } download.
   *[other] Deleted { $count } downloads.
}
gui-status-restored = { $count ->
    [one] Restored { $count } download.
   *[other] Restored { $count } downloads.
}
gui-status-moved = { $count ->
    [one] Moved { $count } download.
   *[other] Moved { $count } downloads.
}
gui-status-added = { $count ->
    [one] Added { $count } download.
   *[other] Added { $count } downloads.
}
gui-status-add-duplicates = { $count ->
    [one] { $count } duplicate was skipped.
   *[other] { $count } duplicates were skipped.
}
gui-status-add-start-errors = { $count ->
    [one] { $count } download could not be started.
   *[other] { $count } downloads could not be started.
}
gui-status-edited = { $count ->
    [one] Updated { $count } download.
   *[other] Updated { $count } downloads.
}
gui-status-edit-skipped = { $count ->
    [one] { $count } download was skipped.
   *[other] { $count } downloads were skipped.
}
gui-status-folder-started = { $count ->
    [one] Started { $count } download in the folder.
   *[other] Started { $count } downloads in the folder.
}
gui-status-folder-stopped = { $count ->
    [one] Stopped { $count } download in the folder.
   *[other] Stopped { $count } downloads in the folder.
}
gui-status-folder-emptied = { $count ->
    [one] Removed { $count } download from the folder.
   *[other] Removed { $count } downloads from the folder.
}
gui-status-resumed-all = { $count ->
    [one] Resumed { $count } download.
   *[other] Resumed { $count } downloads.
}
gui-status-paused-all = { $count ->
    [one] Paused { $count } download.
   *[other] Paused { $count } downloads.
}
gui-status-history-cleared = { $count ->
    [one] Cleared { $count } history entry.
   *[other] Cleared { $count } history entries.
}
gui-status-skipped = ({ $busy } busy, { $failed } refused)
gui-status-done = Done.
gui-status-settings-applied = Settings applied.
gui-status-settings-failed = Settings were not fully applied: { $error }
gui-status-command-failed = { $error }
gui-status-folder-created = Folder created.
gui-status-folder-deleted = Folder removed.
gui-status-scripts-reloaded = Scripts reloaded.
gui-status-config-reloaded = Settings reloaded from disk.
gui-status-close-dialog-first = Close the open dialog first.
gui-status-no-url = No URL found.
gui-status-add-truncated = Only the first { $max } URLs were added.

# Dialog buttons
gui-button-ok = OK
gui-button-cancel = Cancel
gui-button-delete = Delete
gui-button-move = Move
gui-button-close = Close

# Add Downloads dialog
gui-add-title = Add Downloads
gui-add-urls = URLs (one per line)
gui-add-expand = Expand URL ranges such as [1-10]
gui-add-preview = { $count ->
    [one] { $count } URL will be added
   *[other] { $count } URLs will be added
}
gui-add-invalid-range = Invalid range pattern: { $url }
gui-add-truncated = Only the first { $max } URLs will be added
gui-add-folder = Folder
gui-add-referrer = Referrer
gui-add-referrer-hint = Empty = use the referrer policy
gui-add-headers = Additional headers (Name: value, one per line)
gui-add-save-as = Save as
gui-add-save-as-hint = Leave empty to use the folder's location and the server's file name
gui-add-save-as-disabled = Save as is available when exactly one URL will be added ({ $count } now)
gui-add-start-now = Start immediately
gui-add-error-no-urls = Enter at least one URL.
gui-add-error-referrer = The referrer must not contain control characters.
gui-add-error-header = Line { $line } of the headers is not a valid "Name: value" pair.
gui-add-save-as-empty-name = Enter a file name after the folder.
gui-add-save-as-invalid-name = Enter a valid file name (no \ / : * ? " < > |, no trailing dot or space)
gui-add-save-as-relative-dir = The folder in "Save as" must be a full path, including the drive or network share.
gui-add-save-as-invalid-dir = The folder in "Save as" contains a character that is not allowed: * ? " < > | or a control character.

# Confirmations
gui-confirm-title = Confirm Delete
gui-confirm-delete-folder = { $count ->
    [one] Delete the { $count } download in "{ $name }"?
   *[other] Delete all { $count } downloads in "{ $name }"?
}
gui-confirm-clear-history = { $count ->
    [one] Remove { $count } entry from the history?
   *[other] Remove all { $count } entries from the history?
}

# Edit item dialog
gui-edit-filename = File name
gui-edit-filename-locked = The file name is locked once the download has started
gui-edit-filename-invalid = Enter a valid file name (no \ / : * ? " < > |, no trailing dot or space)

# Move to folder dialog
gui-move-title = Move to Folder
gui-move-current = (current)

# Shortcut help and About dialogs
gui-shortcuts-title = Keyboard Shortcuts
gui-shortcuts-move = Move the focus (Shift extends the selection)
gui-shortcuts-edge = First / last row
gui-shortcuts-page = Page up / down
gui-shortcuts-open = Show details, or open the folder of a finished download
gui-shortcuts-toggle = Toggle the focused row
gui-about-title = About ggg
gui-about-version = Version { $version }
gui-about-description = Download manager

# Context menu entries
gui-cmd-start-pause = Start/Pause
gui-cmd-retry = Retry
gui-cmd-edit-item = Edit...
gui-cmd-move-to-folder = Move to Folder...
gui-cmd-copy-urls = Copy URLs
gui-cmd-open-containing-folder = Open Containing Folder
gui-cmd-delete = Delete...
gui-cmd-paste-urls = Paste URLs...
gui-cmd-start-folder = Start All
gui-cmd-stop-folder = Stop All
gui-cmd-delete-folder-tasks = Delete All...
gui-cmd-clear-history = Clear History...
gui-cmd-folder-settings = Folder Settings...

# Menu bar (an ampersand marks the mnemonic letter; shortcut text is added in code)
gui-menu-file = &File
gui-menu-edit = &Edit
gui-menu-download = &Download
gui-menu-view = &View
gui-menu-tools = &Tools
gui-menu-help = &Help
gui-menu-new-download = &New Download...
gui-menu-paste-and-add = Paste and &Add
gui-menu-exit = E&xit
gui-menu-undo = &Undo Delete
gui-menu-copy-urls = &Copy URLs
gui-menu-paste-urls = &Paste URLs...
gui-menu-delete = &Delete...
gui-menu-select-all = Select &All
gui-menu-select-none = Select &None
gui-menu-find = &Find in History
gui-menu-start-pause = &Start/Pause
gui-menu-retry = &Retry
gui-menu-edit-item = &Edit...
gui-menu-move-to-folder = &Move to Folder...
gui-menu-open-containing-folder = &Open Containing Folder
gui-menu-start-all = Start &All
gui-menu-pause-all = &Pause All
gui-menu-details-pane = &Details Pane
gui-menu-refresh = &Refresh
gui-menu-settings = &Settings...
gui-menu-shortcuts = &Keyboard Shortcuts
gui-menu-about = &About ggg

# Settings window
gui-settings-title = Settings
gui-settings-page-connection = Connection
gui-button-apply = Apply
gui-status-settings-applied-restart = Settings applied. Some changes take effect after you restart the application.
gui-settings-restart-banner = Some of the changes take effect after you restart the application.
gui-settings-tui-only-suffix = (TUI only)
gui-settings-tui-only-preview = The GUI never shows a download preview; this only affects the TUI.
gui-settings-tui-only-dnd = The GUI does not launch ggg-dnd; this only affects the TUI.
gui-settings-language-en = English
gui-settings-language-ja = 日本語
gui-settings-expand-ranges = Expand URL ranges when adding without the dialog
gui-settings-expand-ranges-desc = Whether a range such as [1-10] becomes several downloads when URLs are added without the dialog (Ctrl+Shift+V, drops, browser links).
gui-settings-reload-config = Reload settings from disk
gui-settings-reload-config-desc = Discard the edits in this window and read the settings file again. Not possible while downloads are active.
gui-settings-reload-scripts = Reload scripts
gui-settings-reload-scripts-desc = Load the script files again without restarting.
gui-settings-limit = Limit
gui-settings-override = Override
gui-settings-referrer-custom-hint = Referrer value
gui-settings-inherit-with = Inherit from app ({ $value })
gui-settings-folder-add = Add
gui-settings-folder-remove = Remove...
gui-settings-folder-remove-button = Remove
gui-settings-folder-immediate = Adding or removing a folder takes effect immediately.
gui-settings-folder-remove-confirm = Remove the folder "{ $name }"? This takes effect immediately; Cancel does not undo it.
