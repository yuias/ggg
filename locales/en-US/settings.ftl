# Settings screen strings - English
# Labels and descriptions for settings

# Application Settings Fields
settings-app-max-concurrent = Max Concurrent Downloads
settings-app-max-concurrent-desc = Application-wide concurrent download limit
settings-app-max-concurrent-per-folder = Max Concurrent Per Folder
settings-app-max-concurrent-per-folder-desc = Default concurrent downloads per folder
settings-app-max-active-folders = Max Active Folders
settings-app-max-active-folders-desc = Maximum folders active simultaneously
settings-app-max-redirects = Max Redirects
settings-app-max-redirects-desc = Maximum number of HTTP redirects to follow
settings-app-retry-count = Retry Count
settings-app-retry-count-desc = Number of retry attempts on download failure
settings-app-scripts-enabled = Scripts Enabled
settings-app-scripts-enabled-desc = Enable/disable JavaScript script hooks
settings-app-language = Language
settings-app-language-desc = UI language (requires restart to apply)
settings-app-skip-download-preview = Skip Download Preview
settings-app-skip-download-preview-desc = Skip preview dialog and add downloads immediately
settings-app-auto-launch-dnd = Auto Launch ggg-dnd
settings-app-auto-launch-dnd-desc = Auto-launch ggg-dnd drag & drop helper on startup (Windows only)
settings-app-user-agent = User Agent
settings-app-user-agent-desc = Default User-Agent header for HTTP requests
settings-app-referrer-policy = Referrer Policy
settings-app-referrer-policy-desc = Default Referrer header policy for downloads
settings-app-default-directory = Default Directory
settings-app-default-directory-desc = Base save path for new folders and folder-less downloads

# Folder Settings Fields
settings-folder-name = Name
settings-folder-name-desc = Display name for this folder
settings-folder-save-path = Save Path
settings-folder-save-path-desc = Directory where downloaded files are saved
settings-folder-auto-date = Auto-Date Directory
settings-folder-auto-date-desc = Automatically create date-based subdirectories
settings-folder-auto-start = Auto-Start Downloads
settings-folder-auto-start-desc = Automatically start downloads when added to this folder
settings-folder-prevent-duplicate-url = Prevent Duplicate URL
settings-folder-prevent-duplicate-url-desc = Reject downloads with URLs already in this folder's queue
settings-folder-scripts = Scripts
settings-folder-scripts-desc = Script hook override for this folder
settings-folder-max-concurrent = Max Concurrent
settings-folder-max-concurrent-desc = Concurrent download limit for this folder
settings-folder-user-agent = User Agent
settings-folder-user-agent-desc = Custom User-Agent header for this folder
settings-folder-referrer-policy = Referrer Policy
settings-folder-referrer-policy-desc = Referrer header policy override for this folder
settings-folder-headers = Headers
settings-folder-headers-desc = Custom HTTP headers for this folder

# Context Menu Actions
context-menu-start-pause = Start/Pause Download
context-menu-retry = Retry Failed Download
context-menu-delete = Delete Download
context-menu-change-folder = Change Folder
context-menu-change-save-path = Change Save Path
context-menu-copy-url = Copy URL
context-menu-open-folder = Open Download Folder
context-menu-cancel = Cancel

# Settings UI Labels
settings-value-enabled = Enabled
settings-value-disabled = Disabled
settings-value-not-set = Not set
settings-value-inherit = Inherit from app
settings-value-enabled-override = Enabled (override)
settings-value-disabled-override = Disabled (override)

# Referrer Policy Values
settings-referrer-none = None
settings-referrer-same-as-url = Same as URL
settings-referrer-url-path = URL Path
settings-referrer-url-origin = URL Origin
settings-referrer-custom = Custom

# Settings Screen Sections
settings-new-folder = New Folder
settings-no-folders = No folders configured

# Script file rows (Scripts tab and folder editor)
settings-scripts-header = Scripts ({ $count } files)
settings-scripts-none = No script files found

# Settings tabs
settings-tab-general = General
settings-tab-downloads = Downloads
settings-tab-scripts = Scripts
settings-tab-folders = Folders
# Args are display-formatted configured keys (see KeybindingResolver::display_key).
settings-hint-switch-tab = {$next}/{$prev}: switch tabs
settings-hint-close = {$back}/{$toggle}: close

# Restart-required fields
settings-restart-marker = (restart required)
settings-restart-notice = { $label } will take effect after you restart the application.
