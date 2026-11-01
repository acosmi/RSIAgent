use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false"])
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().into())
}

fn main() {
    let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
    else {
        println!("cargo:rustc-env=RSIA_EXPORTER_COMMIT=unknown");
        return;
    };
    for path in ["Cargo.toml", "Cargo.lock", "crates", "apps"] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    for name in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git(root, &["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={}", root.join(path).display());
        }
    }
    if let Some(reference) = git(root, &["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(root, &["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    let commit = git(root, &["rev-parse", "HEAD"]);
    let status = git(root, &["status", "--porcelain", "--untracked-files=normal"]);
    let identity = match (commit.as_deref(), status.as_deref()) {
        (Some(commit), Some(""))
            if commit.len() == 40
                && commit
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            commit
        }
        (Some(_), Some(_)) => "dirty",
        _ => "unknown",
    };
    println!("cargo:rustc-env=RSIA_EXPORTER_COMMIT={identity}");
}
