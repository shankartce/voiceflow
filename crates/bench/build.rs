//! Embeds the git commit in `vt-bench --version` and in every results report.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=VT_GIT_SHA");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let sha = std::env::var("VT_GIT_SHA").ok().filter(|s| !s.is_empty()).or_else(|| {
        Command::new("git")
            .args(["rev-parse", "--short=10", "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    });
    println!("cargo:rustc-env=VT_GIT_SHA={}", sha.unwrap_or_else(|| "dev".into()));
}
