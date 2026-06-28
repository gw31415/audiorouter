fn main() {
    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();

    let git_hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    let is_tagged = std::process::Command::new("git")
        .args(["describe", "--exact-match", "--tags", "HEAD"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let is_dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let version = if is_tagged && !is_dirty {
        pkg_version
    } else {
        let mut suffix: Vec<String> = Vec::new();
        if let Some(hash) = git_hash {
            suffix.push(hash);
        }
        if is_dirty {
            suffix.push("dirty".to_string());
        }
        if suffix.is_empty() {
            pkg_version
        } else {
            format!("{}+{}", pkg_version, suffix.join("."))
        }
    };

    println!("cargo:rustc-env=APP_VERSION={version}");
    // Re-run when git state changes.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
}
