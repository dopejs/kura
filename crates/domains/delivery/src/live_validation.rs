//! Port of `daemon/internal/delivery/live_validation.go`: the support-matrix rows for the
//! delivery domain.

use kura_livevalidation::{ToolClass, default_matrix_row};

/// Port of `LiveValidationMatrixRows`: delivery dispatch and connector message send are
/// non-idempotent mutations requiring per-action approval (see the livevalidation default
/// matrix).
#[must_use]
pub fn live_validation_matrix_rows() -> Vec<kura_livevalidation::MatrixRow> {
    let classes = [
        ToolClass::from(ToolClass::DELIVERY_DISPATCH),
        ToolClass::from(ToolClass::CONNECTOR_MESSAGE_SEND),
    ];
    classes.iter().filter_map(default_matrix_row).collect()
}
