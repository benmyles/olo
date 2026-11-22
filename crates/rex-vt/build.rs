use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir.parent().unwrap().parent().unwrap();
    let vendor_dir = repo_root.join("vendor/ghostty-vt");
    let include_dir = vendor_dir.join("include");

    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let target_subdir = match arch.as_str() {
        "x86_64" => "x86_64-linux-gnu",
        "aarch64" => "aarch64-linux-gnu",
        other => panic!("unsupported architecture for libghostty-vt: {other}"),
    };

    let lib_dir = env::var("GHOSTTY_VT_LIB_DIR").map(PathBuf::from).unwrap_or_else(|_| vendor_dir.join("lib").join(target_subdir));
    let lib_path = lib_dir.join("libghostty-vt.a");

    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_LIB_DIR");
    println!("cargo:rerun-if-changed={}", lib_path.display());
    println!("cargo:rerun-if-changed={}", include_dir.join("ghostty/vt.h").display());

    if !lib_path.exists() {
        panic!("libghostty-vt.a not found at {}. Run `git lfs pull` or `tools/build-ghostty-vt.sh`.", lib_path.display());
    }

    // Detect an unpulled Git LFS pointer file before the linker fails cryptically.
    let mut head = [0; 42];
    if fs::File::open(&lib_path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head.starts_with(b"version https://git-lfs.github.com/spec/v1") {
        panic!("{} is a Git LFS pointer, not the static archive. Run `git lfs pull` or `tools/build-ghostty-vt.sh`.", lib_path.display());
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:rustc-link-lib=m");

    let header = include_dir.join("ghostty/vt.h");
    generate_bindings(&header, &include_dir);
}

fn generate_bindings(header: &Path, include_dir: &Path) {
    let bindings = bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .clang_arg(format!("-I{}", include_dir.display()))
        .clang_arg("-DGHOSTTY_STATIC")
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .prepend_enum_name(false)
        .layout_tests(false)
        .derive_default(true)
        .derive_debug(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("unable to generate libghostty-vt bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("ghostty_vt_bindings.rs");
    bindings.write_to_file(&out_path).expect("couldn't write bindings");
}
