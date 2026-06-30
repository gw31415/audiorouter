// Runs `pnpm build` in the frontend dashboard dir before the crate is compiled,
// so `include_dir!` in lib.rs can embed a fresh `dashboard/dist`.
fn main() {
    // ponytail: escape hatch for pure-Rust dev iterations (Rust edits don't need
    // a frontend rebuild). Set SKIP_DASHBOARD_BUILD=1 and reuse the last dist.
    if std::env::var("SKIP_DASHBOARD_BUILD").as_deref() == Ok("1") {
        return;
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let frontend_dir = std::path::Path::new(&manifest_dir).join("dashboard");

    let status = std::process::Command::new("pnpm")
        .arg("build")
        .current_dir(&frontend_dir)
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
    println!("cargo:rerun-if-changed={}/index.html", frontend_dir.display());
    println!("cargo:rerun-if-changed={}/package.json", frontend_dir.display());
    println!("cargo:rerun-if-changed={}/vite.config.ts", frontend_dir.display());
}
