use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn init(base: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cella"))
        .env_clear()
        .env("PATH", "")
        .current_dir(base)
        .args(args)
        .output()
        .unwrap()
}

fn assert_scaffold(root: &Path, name: &str) {
    let package: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("package.json")).unwrap()).unwrap();
    assert_eq!(package["name"], name);
    assert_eq!(package["private"], true);
    assert_eq!(package["type"], "module");
    assert_eq!(package["scripts"]["deploy"], "cella deploy");
    assert_eq!(package["dependencies"]["esbuild"], "0.25.10");
    let (_, config) = cella::config::read_project(root).unwrap();
    assert_eq!(cella::config::slug(root, None).unwrap(), name);
    assert_eq!(config["main"], "src/index.js");
    assert_eq!(
        config["durable_objects"]["bindings"][0]["class_name"],
        "AppletState"
    );
    assert_eq!(
        config["migrations"][0]["new_sqlite_classes"][0],
        "AppletState"
    );
    let worker = fs::read_to_string(root.join("src/index.js")).unwrap();
    assert!(worker.contains("export class AppletState"));
    assert!(worker.contains("storage.sql.exec"));
    assert!(worker.contains("return Response.json({ count: row.value })"));
    assert_eq!(
        fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap(),
        "packages: []\n"
    );
    assert!(fs::read_to_string(root.join(".gitignore"))
        .unwrap()
        .contains("node_modules"));
    assert!(fs::read_to_string(root.join("README.md"))
        .unwrap()
        .contains("cella deploy"));
    // Applet hosting settings aren't supported by Cella.
    assert!(!root.join("applet.jsonc").exists());
    assert!(!root.join("node_modules").exists());
}

#[test]
fn creates_named_project_without_external_tools_or_ssh_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let output = init(temp.path(), &["init", "my-app"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_scaffold(&temp.path().join("my-app"), "my-app");
    assert!(String::from_utf8_lossy(&output.stdout).contains("Created my-app"));
}

#[test]
fn current_directory_preserves_unrelated_files_and_source() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("my-app");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("notes.txt"), "keep me").unwrap();
    fs::write(root.join("src/other.js"), "keep me too").unwrap();
    let output = init(&root, &["init", "."]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_scaffold(&root, "my-app");
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).unwrap(),
        "keep me"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/other.js")).unwrap(),
        "keep me too"
    );
}

#[test]
fn refuses_all_scaffold_and_alternative_config_conflicts_without_writing() {
    for conflict in [
        ".gitignore",
        "pnpm-workspace.yaml",
        "package.json",
        "wrangler.jsonc",
        "src/index.js",
        "README.md",
        "wrangler.json",
        "wrangler.toml",
        "src",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("my-app");
        let path = root.join(conflict);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "original").unwrap();
        let output = init(&root, &["init", "."]);
        assert!(!output.status.success(), "{conflict}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        for generated in [
            ".gitignore",
            "pnpm-workspace.yaml",
            "package.json",
            "wrangler.jsonc",
            "README.md",
        ] {
            if generated != conflict {
                assert!(
                    !root.join(generated).exists(),
                    "{conflict}: wrote {generated}"
                );
            }
        }
    }
}

#[test]
fn refuses_existing_named_directory_even_if_empty() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("my-app");
    fs::create_dir(&root).unwrap();
    let output = init(temp.path(), &["init", "my-app"]);
    assert!(!output.status.success());
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
}

#[test]
fn rejects_invalid_names_before_creating_directories() {
    let temp = tempfile::tempdir().unwrap();
    for name in ["UPPER", "bad_name", "1app", "app-", &"a".repeat(64)] {
        let output = init(temp.path(), &["init", name]);
        assert!(!output.status.success(), "{name}");
        assert!(!temp.path().join(name).exists(), "{name}");
    }
}

#[test]
fn requires_exactly_one_directory_argument() {
    let temp = tempfile::tempdir().unwrap();
    for args in [vec!["init"], vec!["init", "my-app", "extra"]] {
        assert!(!init(temp.path(), &args).status.success());
    }
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn project_option_sets_the_scaffold_base_directory() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("base")).unwrap();
    let output = init(temp.path(), &["--project", "base", "init", "my-app"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_scaffold(&temp.path().join("base/my-app"), "my-app");
}

#[cfg(unix)]
#[test]
fn refuses_dangling_symlink_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("my-app");
    fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink("missing", root.join("package.json")).unwrap();
    assert!(!init(&root, &["init", "."]).status.success());
    assert!(fs::symlink_metadata(root.join("package.json"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!root.join("wrangler.jsonc").exists());
}
