//! Port of daemon/internal/reminders/live_validation.go: the live-validation support
//! matrix rows contributed by the reminders domain.

use kura_livevalidation::{MatrixRow, ToolClass, default_matrix_row};

/// Go LiveValidationMatrixRows: the reminder lifecycle mutation row from the default
/// support matrix, or an empty list when the row is not defined.
#[must_use]
pub fn live_validation_matrix_rows() -> Vec<MatrixRow> {
    let tool_class = ToolClass::from(ToolClass::REMINDER_LIFECYCLE_MUTATION);
    default_matrix_row(&tool_class).into_iter().collect()
}
