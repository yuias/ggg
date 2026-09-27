use clap::{Parser, Subcommand};

pub mod error;
pub mod output;
pub mod handler;
pub mod daemon;
pub mod daemon_ops;
pub mod remote;
pub mod bridge;

/// A `--header "Name: value"` argument. Holds the raw, unparsed/unvalidated
/// string (that happens in `remote::parse_header_arg`); this type only
/// exists so `{:?}` on the parsed `Cli` (used by the `-v` trace log in
/// `main.rs`) never prints the value, which may be a secret such as a
/// `Cookie` or `Authorization` header. The name is kept visible since it is
/// useful for debugging and is not itself a secret.
#[derive(Clone, PartialEq, Eq)]
pub struct HeaderArg(pub String);

impl std::fmt::Debug for HeaderArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.split_once(':') {
            Some((name, _)) => write!(f, "{}: <redacted>", name.trim()),
            None => write!(f, "<redacted>"),
        }
    }
}

impl std::str::FromStr for HeaderArg {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(HeaderArg(s.to_string()))
    }
}

impl std::ops::Deref for HeaderArg {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

/// A `--referrer` argument. Same rationale as `HeaderArg`: a referrer URL
/// can itself carry a secret (a token in the query string, for example), so
/// `{:?}` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedArg(pub String);

impl std::fmt::Debug for RedactedArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<redacted>")
    }
}

impl std::str::FromStr for RedactedArg {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(RedactedArg(s.to_string()))
    }
}

impl std::ops::Deref for RedactedArg {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

/// GGG - A classic-style download manager
#[derive(Parser, Debug)]
#[command(name = "ggg")]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Override config directory path
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<std::path::PathBuf>,

    /// Run in headless mode (no GUI)
    #[arg(long, global = true)]
    pub headless: bool,

    /// Enable verbose logging (TRACE level)
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Subcommand to execute
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// CLI commands
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Add a new download
    Add {
        /// URL to download
        url: String,

        /// Folder ID to assign (default, images, videos, audio, archives)
        #[arg(long)]
        folder: Option<String>,

        /// Set the Referer header. Overrides any `Referer` header set via
        /// --header, regardless of the order the flags were given in.
        #[arg(long)]
        referrer: Option<RedactedArg>,

        /// Add a header as "Name: value" (repeatable). When the same name is
        /// given more than once, case-insensitively, the last value wins.
        #[arg(long = "header", value_name = "K:V")]
        header: Vec<HeaderArg>,

        /// Start the download right away instead of waiting on the folder's
        /// auto-start setting (requires a running daemon)
        #[arg(long)]
        immediate: bool,

        /// Print only the new task IDs, one per line
        #[arg(short = 'q', long, group = "add_output")]
        quiet: bool,

        /// Print the result as JSON
        #[arg(long, group = "add_output")]
        json: bool,
    },

    /// List all downloads
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Start a download
    Start {
        /// Download ID (UUID)
        id: String,

        /// Wait for download to complete and show progress
        #[arg(long)]
        wait: bool,
    },

    /// Pause a download
    Pause {
        /// Download ID (UUID)
        id: String,
    },

    /// Remove a download
    Remove {
        /// Download ID (UUID)
        id: String,
    },

    /// Show download status
    Status {
        /// Download ID (UUID)
        id: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Manage configuration
    Config {
        /// Configuration action
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Display application logs
    Logs {
        /// Follow log output (tail -f mode)
        #[arg(long, short)]
        follow: bool,

        /// Filter by log level (error, warn, info, debug, trace)
        #[arg(long)]
        level: Option<String>,

        /// Number of lines to show (default: 50)
        #[arg(long, short = 'n')]
        lines: Option<usize>,
    },

    /// Show download completion history
    History {
        /// Show only today's completions
        #[arg(long)]
        today: bool,

        /// Filter by folder ID
        #[arg(long)]
        folder: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show download statistics
    Stats {
        /// Filter by folder ID
        #[arg(long)]
        folder: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Debug and diagnostic commands
    Debug {
        /// Debug action
        #[command(subcommand)]
        action: DebugAction,
    },

    /// Manage scripts
    Script {
        /// Script action
        #[command(subcommand)]
        action: ScriptAction,
    },

    /// Manage folders
    Folder {
        /// Folder action
        #[command(subcommand)]
        action: FolderAction,
    },

    /// Start all downloads
    StartAll {
        /// Filter by folder ID
        #[arg(long)]
        folder: Option<String>,
    },

    /// Pause all downloads
    PauseAll {
        /// Filter by folder ID
        #[arg(long)]
        folder: Option<String>,
    },

    /// Clear downloads by status
    Clear {
        /// Comma-separated status list (completed,error,paused)
        #[arg(long)]
        status: String,

        /// Filter by folder ID
        #[arg(long)]
        folder: Option<String>,
    },

    /// Batch add downloads from file
    BatchAdd {
        /// File containing URLs (one per line)
        file: String,

        /// Folder ID to assign
        #[arg(long)]
        folder: Option<String>,
    },

    /// Set download priority
    Priority {
        /// Download ID (UUID)
        id: String,

        /// Priority value (0-255, higher = more priority)
        #[arg(long)]
        set: u8,
    },

    /// Move download in queue or to another folder
    Move {
        /// Download ID (UUID)
        id: String,

        // The four move operations are mutually exclusive; the `move_op` group
        // (ArgGroup, multiple = false by default) makes clap reject more than
        // one at parse time.
        /// Move to top of queue
        #[arg(long, group = "move_op")]
        to_top: bool,

        /// Move to bottom of queue
        #[arg(long, group = "move_op")]
        to_bottom: bool,

        /// Move before another download
        #[arg(long, group = "move_op")]
        before: Option<String>,

        /// Move to different folder
        #[arg(long, group = "move_op")]
        folder: Option<String>,
    },

    /// Export data
    Export {
        /// Export action
        #[command(subcommand)]
        action: ExportAction,
    },

    /// Import data
    Import {
        /// Import action
        #[command(subcommand)]
        action: ImportAction,
    },

    /// Test utilities
    Test {
        /// Test action
        #[command(subcommand)]
        action: TestAction,
    },

    /// Manage the Chrome Native Messaging bridge (Windows only)
    Bridge {
        /// Bridge action
        #[command(subcommand)]
        action: BridgeAction,
    },

    /// Control a running daemon for this config directory
    Daemon {
        /// Daemon action
        #[command(subcommand)]
        action: DaemonAction,
    },
}

/// Daemon control actions, sent over the control pipe.
#[derive(Subcommand, Debug)]
pub enum DaemonAction {
    /// Check whether a daemon is running for this config directory
    Ping,

    /// Show daemon status (pid, uptime, queue counts)
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Ask a running daemon to shut down
    Stop,
}

/// Configuration actions
#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Get a configuration value
    Get {
        /// Configuration key (e.g., download.max_concurrent)
        key: String,
    },

    /// Set a configuration value
    Set {
        /// Configuration key (e.g., download.max_concurrent)
        key: String,

        /// Configuration value
        value: String,
    },

    /// Show all configuration
    Show {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

/// Debug and diagnostic actions
#[derive(Subcommand, Debug)]
pub enum DebugAction {
    /// Show download manager internal state
    ManagerState {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show active folder and slot states
    FolderSlots {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show detailed task information
    Task {
        /// Download ID (UUID)
        id: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Validate configuration
    ValidateConfig,

    /// Check queue integrity
    CheckQueue {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

/// Script management actions
#[derive(Subcommand, Debug)]
pub enum ScriptAction {
    /// List all scripts
    List {
        /// Show only enabled scripts
        #[arg(long)]
        enabled_only: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Enable a script
    Enable {
        /// Script filename (e.g., twitter_referer.js)
        name: String,
    },

    /// Disable a script
    Disable {
        /// Script filename (e.g., twitter_referer.js)
        name: String,
    },

    /// Test a script (dry run)
    Test {
        /// Script filename to test
        name: String,

        /// Event to trigger (beforeRequest, headersReceived, completed, error, progress)
        #[arg(long)]
        event: String,

        /// URL for test context
        #[arg(long)]
        url: String,
    },

    /// Reload all scripts (for daemon mode)
    Reload,
}

/// Folder management actions
#[derive(Subcommand, Debug)]
pub enum FolderAction {
    /// List all folders
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Create a new folder
    Create {
        /// Folder ID
        id: String,

        /// Save path for downloads
        #[arg(long)]
        path: String,

        /// Auto-start downloads
        #[arg(long)]
        auto_start: bool,
    },

    /// Show folder settings
    Show {
        /// Folder ID
        id: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Update folder configuration
    Config {
        /// Folder ID
        id: String,

        /// Configuration key=value (e.g., max_concurrent=5)
        #[arg(long)]
        set: String,
    },

    /// Delete a folder
    Delete {
        /// Folder ID
        id: String,
    },
}

/// Export actions
#[derive(Subcommand, Debug)]
pub enum ExportAction {
    /// Export queue to file
    Queue {
        /// Output file path
        #[arg(long)]
        output: String,
    },

    /// Export configuration to file
    Config {
        /// Output file path
        #[arg(long)]
        output: String,
    },
}

/// Import actions
#[derive(Subcommand, Debug)]
pub enum ImportAction {
    /// Import queue from file
    Queue {
        /// Input file path
        #[arg(long)]
        input: String,
    },

    /// Import configuration from file
    Config {
        /// Input file path
        #[arg(long)]
        input: String,
    },
}

/// Bridge management actions (Chrome Native Messaging host)
#[derive(Subcommand, Debug)]
pub enum BridgeAction {
    /// Install the Native Messaging host manifest and registry entry
    Install {
        /// Allowed Chrome extension IDs (repeatable). Each becomes
        /// `chrome-extension://<ID>/` in the manifest's `allowed_origins`.
        #[arg(long = "extension-id", value_name = "ID")]
        extension_ids: Vec<String>,

        /// Override the path to ggg-bridge.exe.
        /// Defaults to `<ggg.exe directory>/ggg-bridge.exe`.
        #[arg(long, value_name = "PATH")]
        bridge_path: Option<std::path::PathBuf>,
    },

    /// Remove the Native Messaging host manifest and registry entry
    Uninstall,

    /// Show the currently installed bridge configuration
    Status,
}

#[cfg(test)]
mod tests {
    use super::*;

    // `-v` logs the parsed `Cli` with `{:?}` (see `main.rs`); this proves a
    // secret passed via `--header`/`--referrer` never reaches that line.
    #[test]
    fn debug_format_redacts_header_and_referrer_values() {
        let cli = Cli::parse_from([
            "ggg",
            "add",
            "https://example.com/f",
            "--header",
            "Cookie: sid=SECRET",
            "--referrer",
            "https://example.org/SECRET-PATH",
        ]);
        let formatted = format!("{cli:?}");
        assert!(!formatted.contains("SECRET"), "formatted: {formatted}");
        // The header name is still visible; only the value is hidden.
        assert!(formatted.contains("Cookie"), "formatted: {formatted}");
    }
}

/// Test utility actions
#[derive(Subcommand, Debug)]
pub enum TestAction {
    /// Generate test download tasks
    GenerateTasks {
        /// Number of tasks to generate
        #[arg(long)]
        count: usize,

        /// Folder ID for generated tasks
        #[arg(long)]
        folder: Option<String>,
    },

    /// Reset queue (delete all downloads)
    ResetQueue,

    /// Reset configuration to defaults
    ResetConfig,
}
