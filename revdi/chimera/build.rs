use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=EVDI_LIB_DIR");

    if env::var_os("CARGO_FEATURE_LIVE").is_none() {
        return;
    }

    let lib_dir = env::var_os("EVDI_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(&env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"))
                .join("../../evdi/library")
        });
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=evdi");
}
