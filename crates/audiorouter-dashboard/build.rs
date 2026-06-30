// Runs `pnpm build` in the frontend dashboard dir before the crate is compiled,
// outputting to `$OUT_DIR/dist` so `include_dir!` in lib.rs can embed it.
// Using OUT_DIR (cargo-managed) means the output survives build.rs cache skips.
fn main() {
    // ponytail: escape hatch for pure-Rust dev iterations (Rust edits don't need
    // a frontend rebuild). Set SKIP_DASHBOARD_BUILD=1 and reuse the last dist.
    if std::env::var("SKIP_DASHBOARD_BUILD").as_deref() == Ok("1") {
        return;
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let frontend_dir = std::path::Path::new(&manifest_dir).join("dashboard");
    let dist_dir = format!("{out_dir}/dist");

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

    let status = cmd
        .status()
        .unwrap_or_else(|e| {
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

    // Rerun this script only when frontend inputs change (cargo watches a
    // directory path recursively), so editing Rust does not trigger pnpm.
    println!("cargo:rerun-if-changed={}/src", frontend_dir.display());
    println!(
        "cargo:rerun-if-changed={}/index.html",
        frontend_dir.display()
    );
    println!(
        "cargo:rerun-if-changed={}/package.json",
        frontend_dir.display()
    );
    println!(
        "cargo:rerun-if-changed={}/vite.config.ts",
        frontend_dir.display()
    );
}
