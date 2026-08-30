use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if env::var_os("STITCH_BUILDING_STUB").is_some() {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let stub_target_dir = out_dir.join("stub-target");

    let status = Command::new(env::var("CARGO").unwrap())
        .current_dir(&manifest_dir)
        .env("STITCH_BUILDING_STUB", "1")
        .args(["build", "--release", "--lib", "--target-dir"])
        .arg(&stub_target_dir)
        .status()
        .expect("failed to invoke cargo to build stitch_stub.dll");

    if !status.success() {
        panic!("nested cargo build of stitch_stub.dll failed");
    }

    let dll_path = stub_target_dir.join("release").join("stitch_stub.dll");
    if !dll_path.exists() {
        panic!("expected stub dll at {}", dll_path.display());
    }

    println!("cargo:rustc-env=STITCH_STUB_DLL_PATH={}", dll_path.display());
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/pipe.rs");
    println!("cargo:rerun-if-changed=src/python.rs");
}
