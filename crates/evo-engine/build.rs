use std::path::Path;
use std::process::Command;

// The repository inputs that actually enter the build. The rerun triggers and
// the dirty check are scoped to exactly these, so an edit elsewhere (docs,
// scripts) neither reruns this script nor marks the build dirty.
const BUILD_INPUTS: [&str; 5] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "crates",
    "apps",
];

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

fn rerun_if_exists(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// The clean commit of this repository, `dirty`, or `unknown`. Only a work
/// tree whose top level is this repository root identifies the build: a
/// missing `.git`, or an enclosing foreign repository, is `unknown`.
fn identity(root: &Path) -> String {
    let Some(toplevel) = git(root, &["rev-parse", "--show-toplevel"]) else {
        return "unknown".into();
    };
    if Path::new(&toplevel).canonicalize().ok() != root.canonicalize().ok() {
        return "unknown".into();
    }
    for name in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git(root, &["rev-parse", "--git-path", name]) {
            rerun_if_exists(&root.join(path));
        }
    }
    if let Some(reference) = git(root, &["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(root, &["rev-parse", "--git-path", &reference])
    {
        rerun_if_exists(&root.join(path));
    }
    let Some(commit) = git(root, &["rev-parse", "HEAD"]) else {
        return "unknown".into();
    };
    let mut status = vec!["status", "--porcelain", "--untracked-files=normal", "--"];
    status.extend(BUILD_INPUTS);
    match git(root, &status) {
        Some(changes) if changes.is_empty() => commit,
        Some(_) => "dirty".into(),
        None => "unknown".into(),
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("evo-engine lives two levels below the repository root");
    for input in BUILD_INPUTS {
        rerun_if_exists(&root.join(input));
    }
    println!("cargo:rustc-env=RSIA_EXPORTER_COMMIT={}", identity(root));
}
