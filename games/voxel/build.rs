use std::path::PathBuf;

// Links KOS's C library, build/libc.a (`make libc`). The linker doesn't run
// in this folder, so the path has to be absolute.
//
// The program is a fixed-address executable linked at USER_BASE, which KOS's
// loader runs where it was linked (as it does tcc's programs). Rust's
// prebuilt core and alloc aren't position-independent, so it can't be a PIE
// like the C programs.
fn main() {
    let lib = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build");
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=c");
    println!("cargo:rustc-link-arg=--image-base=0x50000000");
    println!("cargo:rustc-link-arg=-e_start");
    println!("cargo:rerun-if-changed={}", lib.join("libc.a").display());
}
