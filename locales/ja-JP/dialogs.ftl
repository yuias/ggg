# Dialog strings - Japanese
# Titles, prompts, messages

# Dialog titles
dialog-details-toggle = Details (i: Toggle)
dialog-details = Details
dialog-edit-field = Edit Field
dialog-edit-label = Edit {$label}
dialog-add-download = Add Download (Tab: Switch Field, Shift+Enter: Expand [n-m])
dialog-add-download-url = URL
dialog-add-download-referrer = リファラー
dialog-change-save-path = Change Save Path (Enter: Confirm, Esc: Cancel)
dialog-item-edit = 項目を編集
dialog-item-edit-multi = {$count}件の項目を編集
dialog-item-edit-save-path = 保存先
dialog-item-edit-referrer = リファラー
dialog-item-edit-referrer-hint = 空欄 = リファラーポリシーを使用
dialog-item-edit-headers = その他のヘッダー（読み取り専用）:
dialog-item-edit-invalid-referrer = リファラーに制御文字は使用できない
status-item-edit-skipped = {$count}件をスキップした: 一時停止してから再試行
hint-item-edit = Tab: 次のフィールド  Enter: 適用  Esc: キャンセル
dialog-change-folder = 📁 Change Folder ({$down}/{$up}: Navigate, {$select}: Select)
dialog-confirm-delete = Confirm Delete
dialog-switch-folder = 📁 Select Folder ({$down}/{$up}: Navigate, {$select}: Select)
dialog-actions = Actions Menu ({$down}/{$up}: Navigate, {$select}: Select)
dialog-folder-actions = Folder Actions
dialog-history-actions = History Actions

# Folder context menu
folder-menu-start-all = すべて開始
folder-menu-stop-all = すべて停止
folder-menu-delete-all = すべて削除
folder-menu-clear-history = 履歴をクリア
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
confirm-delete-single = このダウンロードを削除しますか？
confirm-delete-multiple = {$count}個のダウンロードを削除しますか？
confirm-delete-folder-config-note = 設定ディレクトリも同時に削除される。
confirm-delete-hint = {$confirm}または{$select}で確定、{$cancel}または{$back}でキャンセル

# Help text in dialogs. Args are display-formatted configured keys (see
# KeybindingResolver::display_key).
help-edit-field = {$select}: Edit Value
help-folder-edit-navigate = {$down}/{$up}: Navigate | {$select}: Edit Field | {$back}: Back to Settings
help-folder-list = {$select}: Edit Folder
help-script-toggle = {$down}/{$up}: Navigate  {$toggle}/{$select}: Toggle  {$reload}: Reload All
