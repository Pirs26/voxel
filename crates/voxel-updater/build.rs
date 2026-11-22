use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=APP_VERSION");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");

    let resolved = std::env::var("APP_VERSION")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(resolve_git_tag)
        .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")));

    let normalized = if let Some(rest) = resolved.strip_prefix('V') {
        format!("v{rest}")
    } else if resolved.starts_with('v') {
        resolved
    } else {
        format!("v{resolved}")
    };

    println!("cargo:rustc-env=RESOLVED_APP_VERSION={normalized}");
}

fn resolve_git_tag() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let tag = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if tag.is_empty() { None } else { Some(tag) }
}
