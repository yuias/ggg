# Settings screen strings - Japanese
# Labels and descriptions for settings

# Application Settings Fields
settings-app-max-concurrent = 最大同時ダウンロード数
settings-app-max-concurrent-desc = アプリケーション全体の同時ダウンロード数の上限
settings-app-max-concurrent-per-folder = フォルダごとの最大同時数
settings-app-max-concurrent-per-folder-desc = フォルダごとのデフォルト同時ダウンロード数
settings-app-max-active-folders = 最大アクティブフォルダ数
settings-app-max-active-folders-desc = 同時にアクティブにできるフォルダの最大数
settings-app-max-redirects = 最大リダイレクト数
settings-app-max-redirects-desc = HTTPリダイレクトを追跡する最大回数
settings-app-retry-count = リトライ回数
settings-app-retry-count-desc = ダウンロード失敗時のリトライ試行回数
settings-app-scripts-enabled = スクリプト有効化
settings-app-scripts-enabled-desc = JavaScriptスクリプトフックの有効/無効
settings-app-language = 言語
settings-app-language-desc = UI言語（適用には再起動が必要）
settings-app-skip-download-preview = ダウンロードプレビューをスキップ
settings-app-skip-download-preview-desc = プレビューダイアログをスキップして即座にダウンロードを追加
settings-app-auto-launch-dnd = ggg-dnd 自動起動
settings-app-auto-launch-dnd-desc = 起動時にドラッグ＆ドロップ補助ツール ggg-dnd を自動起動（Windows専用）
settings-app-user-agent = ユーザーエージェント
settings-app-user-agent-desc = HTTPリクエストのデフォルトUser-Agentヘッダー
settings-app-referrer-policy = リファラーポリシー
settings-app-referrer-policy-desc = ダウンロードのデフォルトReferrerヘッダーポリシー
settings-app-default-directory = デフォルトディレクトリ
settings-app-default-directory-desc = 新規フォルダおよびフォルダなしダウンロードの基準保存先パス

# Folder Settings Fields
settings-folder-name = 名前
settings-folder-name-desc = このフォルダの表示名
settings-folder-save-path = 保存パス
settings-folder-save-path-desc = ダウンロードファイルの保存先ディレクトリ
settings-folder-auto-date = 日付ディレクトリ自動作成
settings-folder-auto-date-desc = 日付ベースのサブディレクトリを自動作成
settings-folder-auto-start = ダウンロード自動開始
settings-folder-auto-start-desc = このフォルダに追加されたダウンロードを自動的に開始
settings-folder-prevent-duplicate-url = 重複URL防止
settings-folder-prevent-duplicate-url-desc = このフォルダのキューに既に存在するURLのダウンロードを拒否
settings-folder-scripts = スクリプト
settings-folder-scripts-desc = このフォルダのスクリプトフック上書き設定
settings-folder-max-concurrent = 最大同時数
settings-folder-max-concurrent-desc = このフォルダの同時ダウンロード数の上限
settings-folder-user-agent = ユーザーエージェント
settings-folder-user-agent-desc = このフォルダ用のカスタムUser-Agentヘッダー
settings-folder-referrer-policy = リファラーポリシー
settings-folder-referrer-policy-desc = このフォルダのReferrerヘッダーポリシー上書き
settings-folder-headers = ヘッダー
settings-folder-headers-desc = このフォルダ用のカスタムHTTPヘッダー

# Context Menu Actions
context-menu-start-pause = ダウンロードの開始/停止
context-menu-retry = 失敗したダウンロードを再試行
context-menu-delete = ダウンロードを削除
context-menu-change-folder = フォルダを変更
context-menu-edit-item = 編集...
context-menu-copy-url = URLをコピー
context-menu-open-folder = ダウンロードフォルダを開く
context-menu-cancel = キャンセル

# Settings UI Labels
settings-value-enabled = 有効
settings-value-disabled = 無効
settings-value-not-set = 未設定
settings-value-inherit = アプリから継承
settings-value-enabled-override = 有効 (上書き)
settings-value-disabled-override = 無効 (上書き)

# Referrer Policy Values
settings-referrer-none = なし
settings-referrer-same-as-url = URLと同じ
settings-referrer-url-path = URLパス部分
settings-referrer-url-origin = URLサーバー部分
settings-referrer-custom = カスタム

# Settings Screen Sections
settings-new-folder = 新しいフォルダ
settings-no-folders = フォルダが設定されていません

# Script file rows (Scripts tab and folder editor)
settings-scripts-header = スクリプト ({ $count } 件)
settings-scripts-none = スクリプトファイルが見つかりません

# Settings tabs
settings-tab-general = 一般
settings-tab-downloads = ダウンロード
settings-tab-scripts = スクリプト
settings-tab-folders = フォルダ
# Args are display-formatted configured keys (see KeybindingResolver::display_key).
settings-hint-switch-tab = {$next}/{$prev}: タブ切り替え
settings-hint-close = {$back}/{$toggle}: 閉じる

# Restart-required fields
settings-restart-marker = (再起動が必要)
settings-restart-notice = { $label } の変更はアプリケーションを再起動すると反映されます。
