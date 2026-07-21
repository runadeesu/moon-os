fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg=-T{manifest_dir}/linker.ld");
    println!("cargo:rerun-if-changed={manifest_dir}/linker.ld");
    println!("cargo:rustc-link-arg=-nostdlib");
    println!("cargo:rustc-link-arg=-static");
    println!("cargo:rustc-link-arg=-no-pie");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=max-page-size=0x1000");

    // The userland test binary (`userland/init`) is a separate, standalone
    // cargo project (its own linker script/target base address), built
    // first by tools/build.sh, which exports this env var. Falls back to
    // the same path tools/build.sh uses, so a plain `cargo build` still
    // works once that binary's been built at least once.
    let init_elf = std::env::var("USERLAND_INIT_ELF").unwrap_or_else(|_| {
        format!("{manifest_dir}/../userland/init/target/x86_64-unknown-none/release/init")
    });
    println!("cargo:rustc-env=USERLAND_INIT_ELF={init_elf}");
    println!("cargo:rerun-if-env-changed=USERLAND_INIT_ELF");
    println!("cargo:rerun-if-changed={init_elf}");
}
