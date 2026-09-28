# Dialog strings - English
# Titles, prompts, and messages for dialogs

# Dialog titles
dialog-details-toggle = Details (i to toggle)
dialog-details = Details
dialog-edit-field = Edit Field
dialog-edit-label = Edit {$label}
dialog-add-download = Add Download (Tab: switch field, Shift+Enter to expand [n-m] pattern)
dialog-add-download-url = URL
dialog-add-download-referrer = Referrer
dialog-item-edit = Edit item
dialog-item-edit-multi = Edit {$count} items
dialog-item-edit-save-path = Save path
dialog-item-edit-referrer = Referrer
dialog-item-edit-referrer-hint = Empty = use the referrer policy
dialog-item-edit-headers = Other headers (read-only):
dialog-item-edit-invalid-referrer = Referrer must not contain control characters
status-item-edit-skipped = {$count} item(s) skipped: pause them first
hint-item-edit = Tab: next field  Enter: apply  Esc: cancel
dialog-change-folder = 📁 Change Folder ({$down}/{$up} to navigate, {$select} to select)
dialog-confirm-delete = Confirm Delete
dialog-switch-folder = 📁 Switch Folder ({$down}/{$up} to navigate, {$select} to select)
dialog-actions = Actions ({$down}/{$up} to navigate, {$select} to select)
dialog-folder-actions = Folder Actions
dialog-history-actions = History Actions

# Folder context menu
folder-menu-start-all = Start All
folder-menu-stop-all = Stop All
folder-menu-delete-all = Delete All
folder-menu-clear-history = Clear History
dialog-download-preview = 📋 Download Preview
dialog-help = Help
dialog-settings = Settings
dialog-folders = Folders
dialog-folder-details = Folder Details

# Input prompts
prompt-url = URL:
prompt-save-path = Save Path:
prompt-value = Value:

# Confirmation messages
confirm-delete-single = Delete this download?
confirm-delete-multiple = Delete {$count} downloads?
confirm-delete-folder-config-note = This also removes its config directory.
confirm-delete-hint = Press {$select} or {$confirm} to confirm, {$cancel} or {$back} to cancel

# Help text in dialogs. Args are display-formatted configured keys (see
# KeybindingResolver::display_key).
help-edit-field = {$select}: Edit value
help-folder-edit-navigate = {$down}/{$up}: navigate | {$select}: edit field | {$back}: back to settings
help-folder-list = {$select}: edit folder
help-script-toggle = {$down}/{$up}: Navigate  {$toggle}/{$select}: Toggle  {$reload}: Reload all
