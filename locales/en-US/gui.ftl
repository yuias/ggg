# GUI-only strings - English
# Shared strings stay in the other files; add a key here only when the GUI needs its own.

gui-app-title = ggg
gui-snapshot-summary = { $tasks } tasks in { $folders } folders

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
