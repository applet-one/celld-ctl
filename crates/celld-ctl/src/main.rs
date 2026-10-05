use anyhow::{bail, ensure, Context, Result};
use celld_ctl::{config::Paths, manager::Manager, runtime::RealRuntime, storage, transport};
use celld_ctl_core::{Request, Response};
use serde_json::{json, Value};
use std::path::Path;

const HELP:&str="celld-ctl (root operator)\n  create|enable|disable|start|stop|restart|remove|target|reload|status SLUG\n  logs SLUG [--lines 1..1000]\n  deployments SLUG\n  list\n  backup\n  storage prepare-local|init-local  (host installer only)\n  import-counter --celld-version VERSION [--version-id ID] [--enabled]\n  transport  (fixed forced SSH command; one JSON request on stdin)\nOperator-only test/staging path injection: --root ABSOLUTE_PATH before command.\n";
fn require_root() -> Result<()> {
    // SAFETY: geteuid has no preconditions or side effects.
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "celld-ctl requires root; use the installed restricted helper"
    );
    Ok(())
}
fn operator(mut args: Vec<String>) -> Result<Value> {
    require_root()?;
    let paths = if args.first().map(String::as_str) == Some("--root") {
        ensure!(
            args.len() >= 3,
            "--root needs an absolute path and operator command"
        );
        let root = args.remove(1);
        args.remove(0);
        ensure!(Path::new(&root).is_absolute(), "--root must be absolute");
        ensure!(
            args.first().map(String::as_str) != Some("transport"),
            "transport forbids path overrides"
        );
        Paths::under(Path::new(&root))
    } else {
        Paths::production()
    };
    let operation = args.first().context("missing command")?.as_str();
    if operation == "storage" {
        ensure!(
            args.len() == 2,
            "storage requires prepare-local or init-local"
        );
        match args[1].as_str() {
            "prepare-local" => storage::prepare_local(&paths)?,
            "init-local" => storage::init_local(&paths)?,
            _ => bail!("unknown storage operation"),
        }
        return Ok(json!({"ok":true}));
    }
    let runtime = RealRuntime {
        paths: paths.clone(),
    };
    let mut manager = Manager::open(paths, runtime)?;
    match operation {
        "list" => {
            ensure!(args.len() == 1, "list takes no arguments");
            Ok(serde_json::to_value(manager.registry.list()?)?)
        }
        "backup" => {
            ensure!(args.len() == 1, "backup takes no arguments");
            manager.backup()
        }
        "import-counter" => {
            let mut version = None;
            let mut id = None;
            let mut enabled = false;
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--celld-version" => {
                        ensure!(version.is_none(), "duplicate version");
                        i += 1;
                        version = Some(args.get(i).context("missing version")?.as_str());
                    }
                    "--version-id" => {
                        ensure!(id.is_none(), "duplicate version ID");
                        i += 1;
                        id = Some(args.get(i).context("missing version ID")?.as_str());
                    }
                    "--enabled" => {
                        ensure!(!enabled, "duplicate enabled flag");
                        enabled = true;
                    }
                    _ => bail!("invalid import argument"),
                }
                i += 1;
            }
            if let Some(id) = id {
                ensure!(
                    celld_ctl_core::valid_deployment_id(id),
                    "invalid deployment ID"
                );
            }
            manager.import_counter(version.context("--celld-version is required")?, id, enabled)
        }
        "logs" => {
            ensure!(
                args.len() == 2 || (args.len() == 4 && args[2] == "--lines"),
                "logs SLUG [--lines N]"
            );
            let lines = if args.len() == 4 {
                args[3].parse().context("invalid line count")?
            } else {
                100
            };
            manager.request(Request::Logs {
                slug: args[1].clone(),
                lines,
            })
        }
        "create" | "target" | "status" | "deployments" => {
            ensure!(args.len() == 2, "command needs one slug");
            let slug = args[1].clone();
            manager.request(match operation {
                "create" => Request::Provision { slug },
                "target" => Request::Target { slug },
                "status" => Request::Status { slug },
                _ => Request::Deployments { slug },
            })
        }
        "enable" | "disable" | "start" | "stop" | "restart" | "reload" | "remove" => {
            ensure!(args.len() == 2, "command needs one slug");
            manager.lifecycle(operation, &args[1])
        }
        _ => bail!("unknown command; use --help"),
    }
}
fn run_transport(args: &[String]) -> Result<Value> {
    ensure!(
        args == ["transport"],
        "transport accepts no command-line arguments or overrides"
    );
    let original = std::env::var_os("SSH_ORIGINAL_COMMAND")
        .map(|s| {
            s.into_string()
                .map_err(|_| anyhow::anyhow!("SSH command is not UTF-8"))
        })
        .transpose()?;
    transport::validate_original_command(original.as_deref())?;
    require_root()?;
    let incoming = transport::read_stdin()?;
    let paths = Paths::production();
    let runtime = RealRuntime {
        paths: paths.clone(),
    };
    Manager::open(paths, runtime)?.transport_request(incoming.request, incoming.bundle)
}
fn main() {
    // Private files and child-created SQLite journals are private from their first byte.
    // SAFETY: called once before threads or file creation.
    unsafe {
        libc::umask(0o077);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let is_transport = args.first().map(String::as_str) == Some("transport");
    // A remotely inherited original command can never select an operator path.
    if is_transport || std::env::var_os("SSH_ORIGINAL_COMMAND").is_some() {
        // Outer failsafe leaves room for the bounded input, lock, native publish,
        // configured readiness and Caddy/systemd rollback phases to complete.
        unsafe {
            libc::alarm(600);
        }
        let result = run_transport(&args);
        let ok = result.is_ok();
        let response = match result {
            Ok(v) => Response::success(v).unwrap(),
            Err(e) => Response::failure(e.to_string()),
        };
        if transport::write_response(response).is_err() {
            std::process::exit(1);
        }
        if !ok {
            std::process::exit(1);
        }
        return;
    }
    if args.is_empty() || args == ["--help"] || args == ["help"] {
        print!("{HELP}");
        return;
    }
    if args == ["--version"] {
        println!("celld-ctl {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    match operator(args) {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        Err(e) => {
            eprintln!("{}", json!({"ok":false,"error":e.to_string()}));
            std::process::exit(1);
        }
    }
}
