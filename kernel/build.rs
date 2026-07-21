fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg=-T{manifest_dir}/linker.ld");
    println!("cargo:rerun-if-changed={manifest_dir}/linker.ld");
    println!("cargo:rustc-link-arg=-nostdlib");
    println!("cargo:rustc-link-arg=-static");
    println!("cargo:rustc-link-arg=-no-pie");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=max-page-size=0x1000");

    // The userland test binaries (`userland/init`, `userland/counter`) are
    // separate, standalone cargo projects (their own linker scripts/target
    // base addresses), built first by tools/build.sh, which exports these
    // env vars. Each falls back to the same path tools/build.sh uses, so a
    // plain `cargo build` still works once those binaries have been built
    // at least once.
    for (env_var, crate_name) in [
        ("USERLAND_INIT_ELF", "init"),
        ("USERLAND_COUNTER_ELF", "counter"),
    ] {
        let path = std::env::var(env_var).unwrap_or_else(|_| {
            format!(
                "{manifest_dir}/../userland/{crate_name}/target/x86_64-unknown-none/release/{crate_name}"
            )
        });
        println!("cargo:rustc-env={env_var}={path}");
        println!("cargo:rerun-if-env-changed={env_var}");
        println!("cargo:rerun-if-changed={path}");
    }
}
