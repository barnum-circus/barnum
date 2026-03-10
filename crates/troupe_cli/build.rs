#![allow(missing_docs)]

fn main() {
    // version.txt is at the lib root, one level up from artifacts/
    // Binary is at libs/troupe/artifacts/<platform>/troupe
    // So relative to the crate during CI build: ../../libs/troupe/version.txt
    let version_path = "../../libs/troupe/version.txt";

    if let Ok(version) = std::fs::read_to_string(version_path) {
        println!("cargo:rustc-env=TROUPE_VERSION={}", version.trim());
    } else {
        // No version.txt = development build
        println!("cargo:rustc-env=TROUPE_VERSION=unknown");
    }
}
