use std::collections::HashMap;

use kura_delivery::{
    DeliveryOutcome, DeliveryPreference, DeliveryTarget, ResultClass, TargetKind, TargetStatus,
};

#[test]
fn delivery_target_roundtrips() {
    let target = DeliveryTarget {
        target_id: "t1".to_string(),
        display_name: "Discord".to_string(),
        environment_scope: "test".to_string(),
        target_kind: TargetKind::ConnectorRoute,
        status: TargetStatus::Active,
        supports_immediate: true,
        ..DeliveryTarget::default()
    };
    let json = serde_json::to_string(&target).unwrap();
    assert!(json.contains("targetId"));
    assert!(json.contains("connector_route"));
    let back: DeliveryTarget = serde_json::from_str(&json).unwrap();
    assert_eq!(back.target_id, "t1");
    assert_eq!(back.target_kind, TargetKind::ConnectorRoute);
}

#[test]
fn preference_enum_key_map_roundtrips() {
    let mut by_class = HashMap::new();
    by_class.insert(ResultClass::Urgent, "t1".to_string());
    by_class.insert(ResultClass::Failure, "t2".to_string());
    let preference = DeliveryPreference {
        preference_id: "p1".to_string(),
        environment_scope: "test".to_string(),
        preferred_targets_by_class: by_class,
        ..DeliveryPreference::default()
    };
    let json = serde_json::to_string(&preference).unwrap();
    assert!(json.contains("urgent"));
    let back: DeliveryPreference = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.preferred_targets_by_class
            .get(&ResultClass::Urgent)
            .map(|s| s.as_str()),
        Some("t1")
    );
}

#[test]
fn delivery_outcome_roundtrips() {
    let outcome = DeliveryOutcome {
        delivery_id: "d1".to_string(),
        environment_scope: "test".to_string(),
        source_kind: "runtime".to_string(),
        source_id: "r1".to_string(),
        result_class: ResultClass::Failure,
        ..DeliveryOutcome::default()
    };
    let json = serde_json::to_string(&outcome).unwrap();
    assert!(json.contains("deliveryId"));
    let back: DeliveryOutcome = serde_json::from_str(&json).unwrap();
    assert_eq!(back.delivery_id, "d1");
    assert_eq!(back.result_class, ResultClass::Failure);
}
