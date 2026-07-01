// Runs `pnpm build` in the frontend dashboard dir before the crate is compiled,
// outputting to `$OUT_DIR/dist` so `include_dir!` in lib.rs can embed it.
// Using OUT_DIR (cargo-managed) means the output survives build.rs cache skips.
fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let frontend_dir = std::path::Path::new(&manifest_dir).join("dashboard");
    let dist_dir = format!("{out_dir}/dist");

    println!("cargo:rerun-if-env-changed=SKIP_DASHBOARD_BUILD");
    emit_frontend_rerun_if_changed(&frontend_dir);

    // ponytail: escape hatch for pure-Rust dev iterations (Rust edits don't need
    // a frontend rebuild). Set SKIP_DASHBOARD_BUILD=1 and reuse the last dist.
    if std::env::var("SKIP_DASHBOARD_BUILD").as_deref() == Ok("1") {
        return;
    }

    let mut cmd = if cfg!(windows) {
        // ponytail: Rust's Command doesn't search PATHEXT, so pnpm.cmd isn't
        // found directly; shell out via cmd. Upgrade to which::glob resolution
        // if we ever need arg quoting beyond what cmd /c handles.
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "pnpm", "build"]);
        c
    } else {
        let mut c = std::process::Command::new("pnpm");
        c.arg("build");
        c
    };
    cmd.env("AUDIOROUTER_DIST_DIR", &dist_dir)
        .current_dir(&frontend_dir);

    let status = cmd.status().unwrap_or_else(|e| {
        panic!(
            "failed to invoke `pnpm build` in {}: {e}\n\
                 set SKIP_DASHBOARD_BUILD=1 to skip the frontend build.",
            frontend_dir.display()
        )
    });
    if !status.success() {
        panic!(
            "`pnpm build` failed in {}\n\
             set SKIP_DASHBOARD_BUILD=1 to skip the frontend build.",
            frontend_dir.display()
        );
    }
}

fn emit_frontend_rerun_if_changed(frontend_dir: &std::path::Path) {
    if let Some(repo_root) = frontend_dir
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
    {
        let logo = repo_root.join("assets/audiorouter.svg");
        if logo.exists() {
            println!("cargo:rerun-if-changed={}", logo.display());
        }
    }

    for file in [
        "index.html",
        "package.json",
        "pnpm-lock.yaml",
        "tsconfig.json",
        "vite.config.ts",
    ] {
        let path = frontend_dir.join(file);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    for dir in ["src", "plugins", "public"] {
        let path = frontend_dir.join(dir);
        if path.exists() {
            emit_rerun_for_path_recursive(&path);
        }
    }
}

fn emit_rerun_for_path_recursive(path: &std::path::Path) {
    println!("cargo:rerun-if-changed={}", path.display());

    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            emit_rerun_for_path_recursive(&path);
        } else if file_type.is_file() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
