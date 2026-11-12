//! Checks the renderer's shaders as the crate builds, so a mistake in
//! them is a build error and not a window that never draws.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(gles)");
    for path in ["./src/platform/blade/shaders.wgsl"] {
        println!("cargo:rerun-if-changed={path}");
        let source = std::fs::read_to_string(path).unwrap();
        if let Err(e) = naga::front::wgsl::parse_str(&source) {
            println!("cargo::error=WGSL shader compilation failed:\n{e}");
            std::process::exit(1);
        }
    }
}
