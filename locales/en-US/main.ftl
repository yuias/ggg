# Main UI strings - English
# App title, tabs, status bar

# Header
app-title = 📚 GGG
app-version = v{$version} 📥

# 3-Pane Layout
pane-folders = 📂 Folders
pane-downloads = 📥 Downloads
pane-history = 📋 History
pane-details = 📄 Details

# Folder Tree
tree-completed-node = History

# Download list columns
column-sel = Sel
column-status = Status
column-filename = Filename
column-size = Size
column-progress = Progress
column-speed = Speed
column-eta = ETA

# Status bar - Normal mode
status-normal-folder = 📁 [{$folder}]
status-normal-actions-download = ⏯ {$toggle}:toggle | ✓ {$select}:select | ➕ {$add}:add | 🗑 {$delete}:delete | 📋 {$menu}:menu
status-normal-actions-folder = ▶ {$toggle}:start all | ➕ {$add}:add | 📋 {$menu}:menu
status-normal-actions-details = ⏯ {$toggle}:toggle | 📋 {$menu}:menu
status-normal-undo = {$key}: undo({$count})
status-normal-busy = ⚠ {$count} busy, try again
status-normal-right = 🔄 {$switch}:folder | ❓ {$help}:help | ❌ {$quit}:quit

# Status bar - Other modes
status-add-download = 📥 Enter URL and press Enter to add
status-editing-field = ✏️  Enter value and press Enter to save
status-download-preview = 📋 Download Preview
status-search = 🔍 Search: {$query}
status-change-folder = 📁 Enter folder path
status-switch-folder = 📁 Select Folder
status-help = ❓ Help
status-settings = ⚙️  Settings
status-folder-edit = 📂 Edit Folder
status-confirm-delete = ⚠️  Confirm Delete
status-context-menu = 📋 Actions Menu

# Status bar - Right side hints. Args are display-formatted configured keys
# (see KeybindingResolver::display_key). status-hint-cancel and
# status-hint-finish stay literal (free-text input keys are not
# configurable).
status-hint-cancel = Esc: cancel
status-hint-confirm-cancel = {$confirm}: confirm | {$cancel}: cancel
status-hint-finish = Enter/Esc: finish
status-hint-navigate = {$down}/{$up}: navigate | {$select}: select | {$cancel}: cancel
status-hint-close = {$back}: close
status-hint-settings = {$back}: close | {$reload}: reload config
status-hint-folder-edit = {$down}/{$up}: navigate | {$select}: edit | {$back}: back
status-hint-confirm-yn = {$yes}: confirm | {$no}/{$back}: cancel
status-hint-menu = {$down}/{$up}: navigate | {$select}: execute | {$back}: cancel

# Quick Actions bar title
quick-actions-title = ⚡ Quick Actions
