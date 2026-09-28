# Command response messages

# Error messages
cmd-error-invalid-uuid = 無効なUUID
cmd-error-invalid-config = 無効な設定フォーマット
cmd-error-scripts-disabled = スクリプトが有効になっていない
cmd-error-start-download = ダウンロード開始に失敗: { $error }
cmd-error-pause-download = ダウンロード一時停止に失敗: { $error }
cmd-error-remove-download = ダウンロード削除に失敗: { $error }
cmd-error-change-folder = フォルダ変更に失敗: { $error }
cmd-error-validation-failed = バリデーションエラー: { $error }
cmd-error-save-config = 設定の保存に失敗: { $error }
cmd-error-folder-not-found = フォルダ '{ $folder }' が見つからない
cmd-error-reload-scripts = スクリプトの再読み込みに失敗: { $error }
cmd-error-reload-config = 設定のリロードに失敗: { $error }
cmd-error-reload-active-downloads = ダウンロード実行中は設定をリロードできない。全てのダウンロードを一時停止してから再試行すること。
cmd-error-empty-folder-name = フォルダ名を空にすることはできない
cmd-error-empty-default-directory = デフォルトのダウンロードディレクトリを空にすることはできない
cmd-error-duplicate-folder-name = フォルダ '{ $name }' は既に存在する
cmd-error-default-folder-rename = デフォルトフォルダの名前は変更できない
cmd-error-script-send = 送信エラー: { $error }
cmd-error-script-recv = 受信エラー: { $error }
cmd-error-script-communication = 送受信エラー: { $error }
cmd-error-blocking-task = ブロッキングタスク失敗: { $error }

# Success messages
cmd-success-scripts-reloaded = スクリプトを再読み込みした
cmd-success-config-reloaded = 設定を正常にリロードした
