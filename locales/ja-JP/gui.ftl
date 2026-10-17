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

# Add Downloads dialog
gui-add-title = ダウンロードの追加
gui-add-urls = URL (1 行に 1 つ)
gui-add-expand = [1-10] のような URL の範囲を展開する
gui-add-preview = { $count } 件の URL を追加します
gui-add-invalid-range = 範囲指定が正しくありません: { $url }
gui-add-truncated = 先頭の { $max } 件のみ追加されます
gui-add-folder = フォルダー
gui-add-referrer = リファラー
gui-add-referrer-hint = 空欄の場合はリファラーポリシーに従います
gui-add-headers = 追加のヘッダー (名前: 値、1 行に 1 つ)
gui-add-save-as = 名前を付けて保存
gui-add-save-as-hint = 空欄の場合はフォルダーの保存先とサーバーのファイル名を使います
gui-add-save-as-disabled = 「名前を付けて保存」は追加する URL がちょうど 1 件のときだけ使えます (現在 { $count } 件)
gui-add-start-now = すぐに開始する
gui-add-error-no-urls = URL を 1 つ以上入力してください。
gui-add-error-referrer = リファラーに制御文字は使用できません。
gui-add-error-header = ヘッダーの { $line } 行目が「名前: 値」の形式になっていません。
gui-add-save-as-empty-name = フォルダーの後ろにファイル名を入力してください。
gui-add-save-as-invalid-name = 有効なファイル名を入力してください（\ / : * ? " < > | は使用不可、末尾のドットと空白も不可）
gui-add-save-as-relative-dir = 「名前を付けて保存」のフォルダーは、ドライブまたはネットワーク共有を含む完全なパスで指定してください。
gui-add-save-as-invalid-dir = 「名前を付けて保存」のフォルダーに使用できない文字 (* ? " < > | と制御文字) が含まれています。

# Confirmations
gui-confirm-title = 削除の確認
gui-confirm-delete-folder = 「{ $name }」の { $count } 件のダウンロードをすべて削除しますか？
gui-confirm-clear-history = 履歴の { $count } 件をすべて削除しますか？

# Edit item dialog
gui-edit-filename = ファイル名
gui-edit-filename-locked = ダウンロードを開始した後はファイル名を変更できません
gui-edit-filename-invalid = 有効なファイル名を入力してください（\ / : * ? " < > | は使用不可、末尾のドットと空白も不可）

# Move to folder dialog
gui-move-title = フォルダーへ移動
gui-move-current = (現在)

# Shortcut help and About dialogs
gui-shortcuts-title = キーボード ショートカット
gui-shortcuts-move = フォーカスを移動 (Shift で選択範囲を拡張)
gui-shortcuts-edge = 先頭 / 末尾の行
gui-shortcuts-page = 1 ページ上 / 下
gui-shortcuts-open = 詳細を表示、完了したダウンロードはフォルダーを開く
gui-shortcuts-toggle = フォーカス中の行の選択を切り替え
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

# Menu bar (an ampersand marks the mnemonic letter; shortcut text is added in code)
gui-menu-file = ファイル(&F)
gui-menu-edit = 編集(&E)
gui-menu-download = ダウンロード(&D)
gui-menu-view = 表示(&V)
gui-menu-tools = ツール(&T)
gui-menu-help = ヘルプ(&H)
gui-menu-new-download = 新しいダウンロード(&N)...
gui-menu-paste-and-add = 貼り付けて追加(&A)
gui-menu-exit = 終了(&X)
gui-menu-undo = 削除を元に戻す(&U)
gui-menu-copy-urls = URL をコピー(&C)
gui-menu-paste-urls = URL を貼り付け(&P)...
gui-menu-delete = 削除(&D)...
gui-menu-select-all = すべて選択(&A)
gui-menu-select-none = 選択を解除(&N)
gui-menu-find = 履歴を検索(&F)
gui-menu-start-pause = 開始/一時停止(&S)
gui-menu-retry = 再試行(&R)
gui-menu-edit-item = 編集(&E)...
gui-menu-move-to-folder = フォルダーへ移動(&M)...
gui-menu-open-containing-folder = 保存先フォルダーを開く(&O)
gui-menu-start-all = すべて開始(&A)
gui-menu-pause-all = すべて一時停止(&P)
gui-menu-details-pane = 詳細ペイン(&D)
gui-menu-refresh = 最新の情報に更新(&R)
gui-menu-settings = 設定(&S)...
gui-menu-shortcuts = キーボード ショートカット(&K)
gui-menu-about = ggg について(&A)
