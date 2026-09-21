use std::{env, fs, path::PathBuf};

fn main() {
    cc::Build::new()
        .file("wrapper.c")
        .include("/usr/local/include") // путь к netmap_user.h
        .flag("-DNETMAP_WITH_LIBS") // иногда нужно для nm_open
        .compile("netmap_wrapper");

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-env-changed=NETMAP_LOCATION");
    println!("cargo:rerun-if-env-changed=DISABLE_NETMAP_KERNEL");

    // Allow disabling Netmap via env or feature flag
    if cfg!(feature = "disable-netmap-kernel") || env::var("DISABLE_NETMAP_KERNEL").is_ok() {
        let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
        fs::write(out_path.join("binding.rs"), "// empty, Netmap disabled\n")
            .expect("Failed to write empty bindings.rs");
        println!("cargo:warning=Netmap disabled; skipping bindgen");
        return;
    }

    let install_dir = env::var("NETMAP_LOCATION").unwrap_or_else(|_| "/usr/local".into());
    println!("cargo:warning=Linking against Netmap in: {}", install_dir);
    println!("cargo:rustc-link-search=native={}/lib", install_dir);
    println!("cargo:rustc-link-lib=dylib=netmap");

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}/include", install_dir))
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate_inline_functions(true)
        .clang_arg("-isystem/usr/include")
        .clang_arg("-isystem/usr/include/x86_64-linux-gnu")
        .clang_arg("-DNETMAP_WITH_LIBS")
        //.allowlist_type("netmap_.*")
        //.allowlist_type("nm.*")
        //.allowlist_function("nm_.*")
        //.allowlist_var("NETMAP_.*")
        //.allowlist_var("IFNAMSIZ")
        .clang_arg("-Dstatic=")
        .clang_arg("-D_GNU_SOURCE")
        .size_t_is_usize(true)
        //.default_enum_style(bindgen::EnumVariation::Rust {
        //    non_exhaustive: false,
        //})
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings with bindgen");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("binding.rs"))
        .expect("Couldn't write bindings to file");
}
