//! Routine evidence collector (wave 8 parity): gathers a redaction-candidate
//! "routine" section from the routine manager (Go `evidenceCollector` in
//! `daemon/internal/app/app.go`). The routine manager is store-backed
//! (`load_from_store`), so the collector reads durable routine state.

use std::collections::HashMap;
use std::sync::Arc;

use crate::Collector;
use crate::Scope;
use crate::ScopeKind;
use crate::Section;

/// Collects a redacted routine section for `ScopeKind::Routine` scopes:
/// resource refs (routine id + current schedule id), a summary (name, state,
/// current version), and the routines API link. Unknown routines and
/// non-routine scopes collect nothing (Go returns nil, nil).
pub struct RoutineCollector {
    routines: Option<Arc<kura_routine::Manager>>,
}

impl RoutineCollector {
    #[must_use]
    pub fn new(routines: Option<Arc<kura_routine::Manager>>) -> Self {
        Self { routines }
    }
}

impl Collector for RoutineCollector {
    fn collect(&self, _tenant_id: &str, scope: &Scope) -> Result<Vec<Section>, String> {
        if scope.kind != ScopeKind::Routine {
            return Ok(Vec::new());
        }
        let Some(routines) = &self.routines else {
            return Ok(Vec::new());
        };
        let Some(routine) = routines.get(&scope.r#ref) else {
            return Ok(Vec::new());
        };
        Ok(vec![Section {
            kind: "routine".to_string(),
            resource_refs: vec![
                routine.routine_id.clone(),
                routine.current_schedule_id.clone(),
            ],
            summary: HashMap::from([
                ("name".to_string(), routine.name.clone()),
                ("state".to_string(), routine.state.to_string()),
                (
                    "currentVersion".to_string(),
                    routine.current_version.to_string(),
                ),
            ]),
            links: vec![format!("/v1/routines/{}", routine.routine_id)],
        }])
    }
}
