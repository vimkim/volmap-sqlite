//! Content identity contains no Git metadata, timestamps, or local directory names.
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("build input directory") {
        let path = entry.expect("build input entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else {
            files.push(path);
        }
    }
}

fn main() {
    let mut files: Vec<PathBuf> = [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "rust-toolchain.toml",
        "frontend/package.json",
        "frontend/package-lock.json",
        "frontend/.npmrc",
        "frontend/.node-version",
        "frontend/vite.config.ts",
        "frontend/tsconfig.json",
        "frontend/tsconfig.app.json",
        "release/toolchain.json",
        "release/build.py",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    for directory in ["src", "frontend/dist"] {
        println!("cargo:rerun-if-changed={directory}");
        collect(Path::new(directory), &mut files);
    }
    files.sort();
    let mut hash = Sha256::new();
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.to_str().expect("build input path").as_bytes();
        let bytes = fs::read(&path).expect("read build input");
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name);
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    let rustc = Command::new(std::env::var_os("RUSTC").expect("Cargo rustc"))
        .arg("--version")
        .output()
        .expect("compiler version");
    assert!(rustc.status.success(), "compiler version unavailable");
    let rustc = String::from_utf8(rustc.stdout).expect("compiler version text");
    let target = std::env::var("TARGET").expect("Cargo target");
    hash.update(rustc.trim().as_bytes());
    hash.update(target.as_bytes());
    hash.update(std::env::var("PROFILE").expect("Cargo profile").as_bytes());
    let identity = format!("{:x}", hash.finalize());
    println!("cargo:rustc-env=VOLMAP_BUILD_ID={identity}");
    println!("cargo:rustc-env=VOLMAP_RUSTC={}", rustc.trim());
    println!("cargo:rustc-env=VOLMAP_TARGET={target}");
}
