use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=DUCKDB_LIB_DIR");
    let dir = env::var_os("DUCKDB_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".local/duckdb"));
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=dylib=duckdb");
    let origin = if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        "@loader_path"
    } else {
        "$ORIGIN"
    };
    // Cargo puts test binaries one directory deeper than the executable.
    for prefix in ["../..", "../../.."] {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{origin}/{prefix}/.local/duckdb");
    }
}
