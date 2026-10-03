use std::process::Command;

fn main() {
    // Embed the short git commit hash so the app can display its exact build revision.
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_HASH={git_hash}");

    // Rebuild when HEAD moves so the embedded hash stays current. A commit on
    // the current branch only rewrites the branch ref, not .git/HEAD itself.
    println!("cargo:rerun-if-changed=.git/HEAD");
    for path in ["HEAD", "packed-refs"].into_iter().chain(head_ref().as_deref()) {
        if let Some(resolved) = git_path(path) {
            println!("cargo:rerun-if-changed={resolved}");
        }
    }
}

/// The ref HEAD points at (e.g. `refs/heads/main`), or None when detached.
fn head_ref() -> Option<String> {
    let out = Command::new("git").args(["symbolic-ref", "-q", "HEAD"]).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).ok())
        .flatten()
        .map(|s| s.trim().to_string())
}

fn git_path(path: &str) -> Option<String> {
    let out = Command::new("git").args(["rev-parse", "--git-path", path]).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).ok())
        .flatten()
        .map(|s| s.trim().to_string())
}
