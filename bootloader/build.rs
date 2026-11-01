fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rerun-if-changed=boot.ld");
    println!("cargo:rustc-link-arg-bins=-T{dir}/boot.ld");
}
