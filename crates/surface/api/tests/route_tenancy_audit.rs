//! Route tenancy audit gate (Stage 8.4 of the agent deepening program).
//!
//! The defect this prevents: five route families
//! (`policy`, `improvement`, `routine`, `triage`, `workflows`) shipped with no
//! tenant, owner, or actor concept at all, so any authenticated principal in a
//! multi-user deployment reached every other principal's data. The families
//! were fixed; this gate stops the next one from repeating it.
//!
//! The check is deliberately crude and source-based rather than clever: every
//! route-family module must either carry a tenancy mechanism, or be named in
//! [`GLOBAL_BY_DESIGN`] with a recorded reason. A new family that does
//! neither fails this test, which is the point — the author must make a
//! decision instead of inheriting the default of "no scoping".

use std::collections::BTreeSet;
use std::path::Path;

/// Families that are daemon-global by design rather than per-principal. Each
/// entry records *why*, and each is expected to carry the operator-role guard
/// (`require_daemon_global_operator`) on its mutating routes.
const GLOBAL_BY_DESIGN: &[(&str, &str)] = &[
    (
        "capabilities",
        "capability registration is daemon-wide code execution; guarded by require_daemon_global_operator",
    ),
    (
        "config",
        "read-only projection of the daemon's own configuration, with redactions",
    ),
    (
        "improvement",
        "proposals rewrite plugins.json for the whole daemon; guarded by require_daemon_global_operator",
    ),
    (
        "plugins",
        "the plugin profile is the daemon's assembly; guarded by require_daemon_global_operator",
    ),
    (
        "release",
        "release/launch-gate state is a property of the deployment, not of a tenant",
    ),
    ("mod", "router assembly, not a route family"),
    // Health/version/system-info live in mod.rs and are intentionally open.
];

/// Source markers that count as "this family reasons about tenancy". Any one
/// is enough; the gate checks that the question was asked, not how.
const TENANCY_MARKERS: &[&str] = &[
    "TenantContext",
    "tenant_id",
    "for_tenant",
    "guard_document_tenant",
    "bind_document_tenant",
    "tenant_visible_document_ids",
    "ByIDTenantGuardLayer",
    "require_daemon_global_operator",
];

#[test]
fn every_route_family_declares_a_tenancy_posture() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/routes");
    let exempt: BTreeSet<&str> = GLOBAL_BY_DESIGN.iter().map(|(name, _)| *name).collect();

    let mut offenders = Vec::new();
    let mut seen = BTreeSet::new();

    for entry in std::fs::read_dir(&dir).expect("read routes dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("file stem")
            .to_string();
        seen.insert(stem.clone());
        if exempt.contains(stem.as_str()) {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read route family");
        // Only the non-test portion counts: a tenancy marker that appears
        // solely inside `mod tests` proves nothing about the handlers.
        let production = source.split("\n#[cfg(test)]").next().unwrap_or(&source);
        if !TENANCY_MARKERS.iter().any(|m| production.contains(m)) {
            offenders.push(stem);
        }
    }

    assert!(
        !offenders.is_empty() || !seen.is_empty(),
        "no route families were scanned — the gate would pass vacuously"
    );
    assert!(
        offenders.is_empty(),
        "route families with no tenancy posture: {offenders:?}\n\
         Every family handling per-principal data must scope it to the acting \
         tenant. If the family is genuinely daemon-global, add it to \
         GLOBAL_BY_DESIGN in this file with the reason, and guard its mutating \
         routes with require_daemon_global_operator."
    );
}

/// The exemption list must stay honest: an entry naming a module that no
/// longer exists silently widens the gate.
#[test]
fn global_by_design_entries_all_exist() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/routes");
    for (name, reason) in GLOBAL_BY_DESIGN {
        assert!(
            dir.join(format!("{name}.rs")).exists(),
            "GLOBAL_BY_DESIGN names {name}, which no longer exists; drop the entry"
        );
        assert!(
            !reason.trim().is_empty(),
            "GLOBAL_BY_DESIGN entry {name} has no recorded reason"
        );
    }
}
