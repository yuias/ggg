# GUI-only strings - Japanese
# Every key defined in en-US/gui.ftl must also be defined here.

gui-app-title = ggg

# Folder tree
gui-tree-folder = { $name } ({ $count })
gui-tree-completed = 完了 ({ $count })

# Table columns that have no shared string
gui-column-added = 追加日時
gui-column-completed = 完了日時
gui-column-url = URL
gui-history-search-hint = 完了したダウンロードを検索

# Details panel
gui-details-tab-info = 情報
gui-details-tab-log = ログ
gui-details-label-error = エラー:
gui-details-label-progress = 進捗:

# Status bar summary
gui-statusbar-items = { $count } 項目
gui-statusbar-selected = { $count } 項目を選択
gui-statusbar-active = ダウンロード中 { $count } 件

# Status bar messages for finished actions
gui-status-toggled = { $count } 件のダウンロードを開始または一時停止しました。
gui-status-retried = { $count } 件のダウンロードを再試行しています。
gui-status-deleted = { $count } 件のダウンロードを削除しました。
gui-status-restored = { $count } 件のダウンロードを元に戻しました。
gui-status-moved = { $count } 件のダウンロードを移動しました。
gui-status-added = { $count } 件のダウンロードを追加しました。
gui-status-add-duplicates = { $count } 件は重複のためスキップしました。
gui-status-add-start-errors = { $count } 件のダウンロードを開始できませんでした。
gui-status-edited = { $count } 件のダウンロードを更新しました。
gui-status-edit-skipped = { $count } 件のダウンロードをスキップしました。
gui-status-folder-started = フォルダー内の { $count } 件のダウンロードを開始しました。
gui-status-folder-stopped = フォルダー内の { $count } 件のダウンロードを停止しました。
gui-status-folder-emptied = フォルダーから { $count } 件のダウンロードを削除しました。
gui-status-resumed-all = { $count } 件のダウンロードを再開しました。
gui-status-paused-all = { $count } 件のダウンロードを一時停止しました。
gui-status-history-cleared = { $count } 件の履歴を消去しました。
gui-status-skipped = (使用中 { $busy } 件、拒否 { $failed } 件)
gui-status-done = 完了しました。
gui-status-settings-applied = 設定を適用しました。
gui-status-settings-failed = 設定を一部適用できませんでした: { $error }
gui-status-command-failed = { $error }
gui-status-folder-created = フォルダーを作成しました。
gui-status-folder-deleted = フォルダーを削除しました。
gui-status-scripts-reloaded = スクリプトを再読み込みしました。
gui-status-config-reloaded = ディスクから設定を再読み込みしました。

# Dialog buttons
gui-button-ok = OK
gui-button-cancel = キャンセル
gui-button-delete = 削除
gui-button-move = 移動
gui-button-close = 閉じる

# Confirmations
gui-confirm-title = 削除の確認
gui-confirm-delete-folder = 「{ $name }」の { $count } 件のダウンロードをすべて削除しますか？
gui-confirm-clear-history = 履歴の { $count } 件をすべて削除しますか？

# Move to folder dialog
gui-move-title = フォルダーへ移動
gui-move-current = (現在)

# Shortcut help and About dialogs
gui-shortcuts-title = キーボード ショートカット
gui-shortcuts-placeholder = ショートカットの一覧はまだ表示できません。
gui-about-title = ggg について
gui-about-version = バージョン { $version }
gui-about-description = ダウンロード マネージャー

# Context menu entries
gui-cmd-start-pause = 開始/一時停止
gui-cmd-retry = 再試行
gui-cmd-edit-item = 編集...
gui-cmd-move-to-folder = フォルダーへ移動...
gui-cmd-copy-urls = URL をコピー
gui-cmd-open-containing-folder = 保存先フォルダーを開く
gui-cmd-delete = 削除...
gui-cmd-paste-urls = URL を貼り付け...
gui-cmd-start-folder = すべて開始
gui-cmd-stop-folder = すべて停止
gui-cmd-delete-folder-tasks = すべて削除...
gui-cmd-clear-history = 履歴をクリア...
gui-cmd-folder-settings = フォルダーの設定...
