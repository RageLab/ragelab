use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=RAGELAB_CORE_REVISION");
    println!("cargo:rerun-if-env-changed=RAGELAB_CORE_DIRTY");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("ragelab-engine must remain under crates/")
        .to_path_buf();

    if let Some(git_dir) = git_output(&workspace_root, &["rev-parse", "--git-dir"]) {
        let git_dir = PathBuf::from(git_dir);
        let git_dir = if git_dir.is_absolute() {
            git_dir
        } else {
            workspace_root.join(git_dir)
        };
        let head = git_dir.join("HEAD");
        println!("cargo:rerun-if-changed={}", head.display());
        println!(
            "cargo:rerun-if-changed={}",
            git_dir.join("packed-refs").display()
        );
        println!("cargo:rerun-if-changed={}", git_dir.join("index").display());
        if let Ok(contents) = std::fs::read_to_string(&head) {
            if let Some(reference) = contents.trim().strip_prefix("ref: ") {
                println!(
                    "cargo:rerun-if-changed={}",
                    git_dir.join(reference).display()
                );
            }
        }
    }

    // Watch source/documentation roots as well as their current files, so newly
    // added workspace files also refresh dirty metadata. The Cargo target directory
    // is a sibling of these roots and is intentionally excluded.
    for directory in ["crates", "apps", "docs"] {
        println!(
            "cargo:rerun-if-changed={}",
            workspace_root.join(directory).display()
        );
    }

    // Track every current non-ignored workspace file so changes outside this crate
    // also refresh the build-time dirty/revision metadata.
    if let Some(files) = git_output(
        &workspace_root,
        &["ls-files", "--cached", "--others", "--exclude-standard"],
    ) {
        for relative in files.lines().filter(|value| !value.is_empty()) {
            println!(
                "cargo:rerun-if-changed={}",
                workspace_root.join(relative).display()
            );
        }
    }

    let revision = env::var("RAGELAB_CORE_REVISION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git_output(&workspace_root, &["rev-parse", "HEAD"]))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_owned());

    let dirty = env::var("RAGELAB_CORE_DIRTY")
        .ok()
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or_else(|| {
            git_output(
                &workspace_root,
                &["status", "--porcelain", "--untracked-files=normal"],
            )
            .is_some_and(|status| !status.is_empty())
        });

    println!("cargo:rustc-check-cfg=cfg(ragelab_core_dirty)");
    if dirty {
        println!("cargo:rustc-cfg=ragelab_core_dirty");
    }
    println!("cargo:rustc-env=RAGELAB_CORE_REVISION={revision}");
    println!("cargo:rustc-env=RAGELAB_CORE_DIRTY={dirty}");
}
