use base64::{engine::general_purpose::STANDARD, Engine};
use celld_ctl_core::{
    valid_upload_path, PreparedBundle, UploadFile, MAX_CONFIG_BYTES, MAX_DECODED_BYTES,
    MAX_UPLOAD_FILES,
};
use serde_json::json;
fn file(path: &str, body: &[u8]) -> UploadFile {
    UploadFile {
        path: path.into(),
        content: STANDARD.encode(body),
    }
}
fn bundle() -> PreparedBundle {
    PreparedBundle {
        config: json!({"name":"worker","main":"src/index.ts","define":{"X":"1"},"rules":[],"assets":{"directory":"/must/not/read/root","binding":"ASSETS"}}),
        modules: vec![
            file("index.js", b"export default {}"),
            file("nested/lib.wasm?module", b"wasm"),
        ],
        assets: vec![
            file("index.html", b"asset"),
            file("asset.wasm", b"not a Worker module"),
        ],
    }
}
#[test]
fn normalization_is_safe_idempotent_and_preserves_native_runtime_options() {
    let input = bundle();
    let output = input.normalize().unwrap();
    assert_eq!(output.config["main"], "modules/index.js");
    assert_eq!(output.config["no_bundle"], true);
    assert_eq!(output.config["assets"]["directory"], "assets");
    assert_eq!(output.config["assets"]["binding"], "ASSETS");
    assert!(output.config.get("define").is_none());
    assert!(output.config.get("rules").is_none());
    assert_eq!(
        serde_json::to_value(&output).unwrap(),
        serde_json::to_value(output.normalize().unwrap()).unwrap()
    );
    assert_eq!(input.config["main"], "src/index.ts");
}
#[test]
fn rejects_unsafe_paths_duplicates_prefix_collisions_and_nonmodule_files() {
    for path in [
        "",
        "/etc/shadow",
        "../x",
        "a/../x",
        "a//b",
        "a/./b",
        "a/",
        "a\\b",
        "C:/x",
        "nul\0x",
        "newline\nx",
    ] {
        assert!(!valid_upload_path(path), "{path:?}");
        let mut b = bundle();
        b.assets[0].path = path.into();
        assert!(b.normalize().is_err());
    }
    assert!(!valid_upload_path(&"a".repeat(513)));
    assert!(!valid_upload_path(&format!("{}/x", "a".repeat(256))));
    for paths in [["a", "a/b"], ["a/b", "a"], ["same", "same"]] {
        let mut b = bundle();
        b.assets = paths.iter().map(|p| file(p, b"x")).collect();
        assert!(b.normalize().is_err());
    }
    let mut b = bundle();
    b.modules.push(file("other.js", b"x"));
    assert!(b.normalize().is_err());
    let mut b = bundle();
    b.modules.retain(|f| f.path != "index.js");
    assert!(b.normalize().is_err());
}
#[test]
fn denies_container_python_nonjs_and_host_path_execution() {
    for config in [
        json!({"name":"a","main":"index.js","containers":[]}),
        json!({"name":"a","main":"index.py"}),
        json!({"name":"a","main":"index.rs"}),
        json!({"name":"a","main":"index.js","compatibility_flags":["python_workers"]}),
        json!({"name":"a","main":"index.js","python_runtime":{}}),
    ] {
        let mut b = bundle();
        b.config = config;
        assert!(b.normalize().is_err());
    }
    let mut b = bundle();
    b.config["main"] = json!("/etc/malicious.js");
    b.config["no_bundle"] = json!(false);
    let b = b.normalize().unwrap();
    assert_eq!(b.config["main"], "modules/index.js");
    assert_eq!(b.config["no_bundle"], true);
}
#[test]
fn asset_only_config_does_not_enable_bundling() {
    let mut b = bundle();
    b.config.as_object_mut().unwrap().remove("main");
    b.modules.clear();
    let b = b.normalize().unwrap();
    assert!(b.config.get("main").is_none());
    assert!(b.config.get("no_bundle").is_none());
    let mut bad = b;
    bad.config["no_bundle"] = json!(true);
    assert!(bad.normalize().is_err());
}
#[test]
fn config_allocation_bound_is_checked_before_value_materialization() {
    let raw = format!(
        "{{\"config\":{{\"vars\":[{}]}},\"modules\":[],\"assets\":[]}}",
        "0,".repeat(MAX_CONFIG_BYTES) + "0"
    );
    let error = serde_json::from_str::<PreparedBundle>(&raw).unwrap_err();
    assert!(error.to_string().contains("64 KiB"));
    let mut b = bundle();
    b.config["vars"] = json!({"huge":"x".repeat(MAX_CONFIG_BYTES)});
    assert!(b.normalize().is_err());
}
#[test]
fn file_count_is_bounded_during_deserialization_and_in_total() {
    let entries = std::iter::repeat_n(r#"{"path":"x","content":""}"#, MAX_UPLOAD_FILES + 1)
        .collect::<Vec<_>>()
        .join(",");
    let raw = format!("{{\"config\":{{}},\"modules\":[{entries}],\"assets\":[]}}");
    assert!(serde_json::from_str::<PreparedBundle>(&raw)
        .unwrap_err()
        .to_string()
        .contains("4096"));
    let mut b = bundle();
    b.assets = vec![file("x", b""); MAX_UPLOAD_FILES];
    assert!(b.normalize().is_err());
}
#[test]
fn base64_and_aggregate_decoded_size_are_bounded() {
    let mut b = bundle();
    b.assets[0].content = "%%%not-base64".into();
    assert!(b.normalize().is_err());
    let mut b = bundle();
    b.modules = vec![file("index.js", &vec![0; MAX_DECODED_BYTES])];
    assert!(b.normalize().is_err());
}
#[test]
fn strict_json_fields_and_string_limits() {
    let mut v = serde_json::to_value(bundle()).unwrap();
    v["unknown"] = json!(true);
    assert!(serde_json::from_slice::<PreparedBundle>(&serde_json::to_vec(&v).unwrap()).is_err());
    let mut v = serde_json::to_value(bundle()).unwrap();
    v["modules"][0]["path"] = json!("x".repeat(513));
    assert!(serde_json::from_slice::<PreparedBundle>(&serde_json::to_vec(&v).unwrap()).is_err());
    let mut v = serde_json::to_value(bundle()).unwrap();
    v["modules"][0]["mode"] = json!(493);
    assert!(serde_json::from_slice::<PreparedBundle>(&serde_json::to_vec(&v).unwrap()).is_err());
}

#[test]
fn distinct_directory_budget_is_enforced_before_materialization_without_a_depth_cap() {
    let mut b = PreparedBundle {
        config: json!({"name":"worker","main":"index.js","assets":{"directory":"assets"}}),
        modules: vec![file("index.js", b"")],
        assets: (0..MAX_UPLOAD_FILES - 1)
            .map(|i| file(&format!("d{i}/inner/file"), b""))
            .collect(),
    };
    // Two roots + 4095 disjoint directory pairs is exactly the 8192 budget.
    assert_eq!(
        celld_ctl_core::MAX_STAGING_DIRECTORIES,
        2 + 2 * (MAX_UPLOAD_FILES - 1)
    );
    b.normalize().unwrap();
    b.assets[0].path = "d0/inner/extra/file".into();
    assert!(b
        .normalize()
        .unwrap_err()
        .contains("8192 staging directories"));
    // Preserve the existing 512-byte relative-path contract: >16 components
    // is fine when the global distinct-directory budget is respected.
    b.assets = vec![file(&format!("{}file", "dir/".repeat(30)), b"")];
    b.normalize().unwrap();
}
