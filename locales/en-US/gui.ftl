# GUI-only strings - English
# Shared strings stay in the other files; add a key here only when the GUI needs its own.

gui-app-title = ggg

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

# Dialog buttons
gui-button-ok = OK
gui-button-cancel = Cancel
gui-button-delete = Delete
gui-button-move = Move
gui-button-close = Close

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
gui-shortcuts-placeholder = The shortcut list is not available yet.
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
