#![allow(missing_docs)]

fn main() {
    // version.txt is at the lib root, one level up from artifacts/
    // Binary is at libs/barnum/artifacts/<platform>/barnum
    // So relative to the crate during CI build: ../../libs/barnum/version.txt
    let version_path = "../../libs/barnum/version.txt";

    if let Ok(version) = std::fs::read_to_string(version_path) {
        println!("cargo:rustc-env=BARNUM_VERSION={}", version.trim());
    } else {
        // No version.txt = development build
        println!("cargo:rustc-env=BARNUM_VERSION=unknown");
    }
}
