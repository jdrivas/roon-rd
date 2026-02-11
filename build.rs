use std::process::Command;
use chrono::Local;

fn main() {
    // Get git commit hash (short form)
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output();
    
    let git_hash = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => "unknown".to_string(),
    };
    
    // Check if tracked files have been modified (ignores untracked files)
    let dirty = Command::new("git")
        .args(["diff", "--quiet", "HEAD"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);
    
    let build_hash = if dirty {
        // For dirty builds, include timestamp for identification
        let timestamp = Local::now().format("%Y%m%d-%H%M%S");
        format!("{}-dirty-{}", git_hash, timestamp)
    } else {
        git_hash
    };
    
    println!("cargo:rustc-env=BUILD_HASH={}", build_hash);
    
    // Rerun if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
