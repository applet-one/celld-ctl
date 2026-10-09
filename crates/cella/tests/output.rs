use cella::output::{deployments, source_label, status};
use serde_json::json;

fn status_value() -> serde_json::Value {
    json!({
        "target": {"slug": "test", "celld_version": "0.6.1", "enabled": true},
        "active": true, "version_id": "new", "observed_version_id": "new",
        "unit": "celld-cell@test.service", "port": 8101,
        "internal_port": 18101, "public_port": 9101
    })
}

#[test]
fn status_distinguishes_adoption_from_activation_and_enabled_from_running() {
    let mut value = status_value();
    assert!(status(&value).unwrap().contains("Runtime has adopted"));
    value["observed_version_id"] = json!("old");
    assert!(status(&value).unwrap().contains("has not yet adopted"));
    value["observed_version_id"] = json!(null);
    assert!(status(&value).unwrap().contains("adoption is unknown"));
    value["active"] = json!(false);
    let text = status(&value).unwrap();
    assert!(text.contains("test · stopped · enabled"));
    assert!(text.contains("adoption cannot be confirmed"));
    value["target"]["enabled"] = json!(false);
    value["version_id"] = json!(null);
    let text = status(&value).unwrap();
    assert!(text.contains("stopped · disabled"));
    assert!(text.contains("Activated  none"));
}

#[test]
fn history_preserves_order_and_does_not_claim_latest_entry_is_active() {
    let text = deployments("test", &json!([
        {"id":2,"slug":"test","version_id":"new","source_revision":"b49f6d7d5a23e9de5d0d4810e0ce19f5ac49dd51-dirty","deployed_at":"2026-10-08 17:00:00"},
        {"id":1,"slug":"test","version_id":"old","source_revision":null,"deployed_at":"2026-10-07 17:00:00"}
    ])).unwrap();
    assert!(text.contains("VERSION"));
    assert!(text.contains("b49f6d7d5a23 (dirty)"));
    assert!(text.contains("unknown"));
    assert!(text.find("new ").unwrap() < text.find("old ").unwrap());
    assert!(!text.contains("active"));
    assert_eq!(
        deployments("test", &json!([])).unwrap(),
        "No deployments recorded for test.\n"
    );
}

#[test]
fn malformed_responses_are_errors_not_misleading_summaries() {
    assert!(status(&json!({})).is_err());
    assert!(deployments("test", &json!({})).is_err());
    assert!(deployments("test", &json!([{"version_id":"id"}])).is_err());
}

#[test]
fn arbitrary_source_labels_are_not_truncated() {
    assert_eq!(
        source_label(Some("release-2026-10-08")),
        "release-2026-10-08"
    );
    assert_eq!(source_label(None), "unknown");
}
