//! Build stamp for the tester log. An issue arrives from a stranger on
//! some release; without this line there is no way to tell which build
//! they ran. Baked at compile time so no one has to remember a flag.
//!
//! A source copy with no `.git` (a release tarball) stamps `unknown` —
//! the version from Cargo still identifies the release.

fn main() {
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=MN_BUILD_SHA={sha}");
    // Re-stamp when the checked-out commit moves.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}
