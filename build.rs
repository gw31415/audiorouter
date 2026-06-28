fn main() {
    // If APP_VERSION is explicitly set (e.g. by the release workflow), use it
    // directly.  This avoids a false "dirty" flag caused by the Cargo.toml
    // version-bump that the release workflow performs via set-version.sh.
    if let Ok(v) = std::env::var("APP_VERSION") {
        if !v.is_empty() {
            println!("cargo:rustc-env=APP_VERSION={v}");
            return;
        }
    }

    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();

    // Bail out early when there is no .git directory — the crate is being
    // built from a source tarball (e.g. crates.io) where git metadata is
    // unavailable.  In that case Cargo.toml is the sole source of truth.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let has_git = std::path::Path::new(&manifest_dir).join(".git").exists();
    if !has_git {
        println!("cargo:rustc-env=APP_VERSION={pkg_version}");
        return;
    }

    // Verify consistency: if a git tag exists, it must match Cargo.toml.
    let git_tag = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    if let Some(ref tag) = git_tag {
        let tag_version = tag.trim_start_matches('v');
        if tag_version != pkg_version {
            panic!(
                "Version mismatch: git tag '{}' ({}) != Cargo.toml version '{}'.\n\
                 Run `scripts/set-version.sh {}` to sync Cargo.toml with the tag.",
                tag, tag_version, pkg_version, tag_version,
            );
        }
    }

    let base_version = pkg_version;

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
        base_version
    } else {
        let mut suffix: Vec<String> = Vec::new();
        if let Some(hash) = git_hash {
            suffix.push(hash);
        }
        if is_dirty {
            suffix.push("dirty".to_string());
        }
        if suffix.is_empty() {
            base_version
        } else {
            format!("{}+{}", base_version, suffix.join("."))
        }
    };

    println!("cargo:rustc-env=APP_VERSION={version}");
    // Re-run when git state changes.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
}
