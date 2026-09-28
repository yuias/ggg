# Main UI strings - Japanese
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
status-normal-actions-download = ⏯ {$toggle}:Toggle | ✓ {$select}:Select | ➕ {$add}:Add | 🗑 {$delete}:Delete | 📋 {$menu}:Menu
status-normal-actions-folder = ▶ {$toggle}:Start All | ➕ {$add}:Add | 📋 {$menu}:Menu
status-normal-actions-details = ⏯ {$toggle}:Toggle | 📋 {$menu}:Menu
status-normal-undo = {$key}: Undo({$count})
status-normal-busy = ⚠ {$count}件処理中、もう一度試してください
status-normal-right = 🔄 {$switch}:Folder | ❓ {$help}:Help | ❌ {$quit}:Quit

# Status bar - Other modes
status-add-download = 📥 URLを入力してEnterで追加
status-editing-field = ✏️  値を入力してEnterで保存
status-download-preview = 📋 Download Preview
status-search = 🔍 Search: {$query}
status-change-folder = 📁 フォルダパスを入力
status-switch-folder = 📁 フォルダを選択
status-help = ❓ Help
status-settings = ⚙️  Settings
status-folder-edit = 📂 Folder Edit
status-confirm-delete = ⚠️  Confirm Delete
status-context-menu = 📋 Actions Menu

# Status bar - Right side hints. Args are display-formatted configured keys
# (see KeybindingResolver::display_key). status-hint-cancel and
# status-hint-finish stay literal (free-text input keys are not
# configurable).
status-hint-cancel = Esc: Cancel
status-hint-confirm-cancel = {$confirm}: Confirm | {$cancel}: Cancel
status-hint-finish = Enter/Esc: Finish
status-hint-navigate = {$down}/{$up}: Navigate | {$select}: Select | {$cancel}: Cancel
status-hint-close = {$back}: Close
status-hint-settings = {$back}: Close | {$reload}: Reload Config
status-hint-folder-edit = {$down}/{$up}: Navigate | {$select}: Edit | {$back}: Back
status-hint-confirm-yn = {$yes}: Confirm | {$no}/{$back}: Cancel
status-hint-menu = {$down}/{$up}: Navigate | {$select}: Execute | {$back}: Cancel

# Quick Actions bar title
quick-actions-title = ⚡ Quick Actions
