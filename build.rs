use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let init_bin_path = out_dir.join("bootart-init");

    println!("cargo:rerun-if-changed=src/bin/bootart_init.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let target = env::var("TARGET").unwrap();
    let target_dir = env::var("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("target"));

    let candidates = [
        target_dir.join(&target).join("release").join("bootart-init"),
        target_dir.join("release").join("bootart-init"),
        PathBuf::from("target").join(&target).join("release").join("bootart-init"),
        PathBuf::from("target").join("release").join("bootart-init"),
    ];

    let mut copied = false;
    for cand in &candidates {
        if cand.exists() {
            if fs::copy(cand, &init_bin_path).is_ok() {
                copied = true;
                break;
            }
        }
    }

    if !copied {
        let _ = fs::write(&init_bin_path, b"BOOTART_INIT_STUB");
    }
}
