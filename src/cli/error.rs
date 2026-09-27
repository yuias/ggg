// Exit codes for CLI automation
pub const SUCCESS: i32 = 0;
pub const ERROR: i32 = 1;
pub const NOT_FOUND: i32 = 2;
pub const INVALID_INPUT: i32 = 3;
pub const DAEMON_RUNNING: i32 = 4; // second --headless, or refused while daemon runs
pub const DAEMON_UNREACHABLE: i32 = 5; // daemon required but absent, or probe/protocol failure
pub const TASK_FAILED: i32 = 6; // wait: final status Error
pub const TASK_PAUSED: i32 = 7; // wait: paused by a user/command
pub const TIMEOUT: i32 = 8; // wait --timeout elapsed, or daemon stop did not finish
