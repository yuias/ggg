//! TUI adapter for `ui::command_i18n`, so call sites can pass `TuiState`.

use super::state::TuiState;
use crate::ui::commands::CommandError;

/// Translate a [`CommandError`] using the TUI's current locale.
pub(crate) fn localize_command_error(state: &TuiState, err: &CommandError) -> String {
    crate::ui::command_i18n::localize_command_error(&state.i18n, err)
}
