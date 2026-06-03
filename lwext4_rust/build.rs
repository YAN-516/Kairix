use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, time::SystemTime};

fn main() {
    let c_path = PathBuf::from("c/lwext4")
        .canonicalize()
        .expect("cannot canonicalize path");

    let lwext4_make = Path::new("c/lwext4/toolchain/musl-generic.cmake");
    let lwext4_patch = Path::new("c/lwext4-make.patch").canonicalize().unwrap();

    if !Path::new(lwext4_make).exists() {
        println!("Retrieve lwext4 source code");
        let git_status = Command::new("git")
            .args(&["submodule", "update", "--init", "--recursive"])
            .status()
            .expect("failed to execute process: git submodule");
        assert!(git_status.success());

        println!("To patch lwext4 src");
        Command::new("git")
            .args(&["apply", lwext4_patch.to_str().unwrap()])
            .current_dir(c_path.clone())
            .spawn()
            .expect("failed to execute process: git apply patch");

        fs::copy(
            "c/musl-generic.cmake",
            "c/lwext4/toolchain/musl-generic.cmake",
        )
        .unwrap();
    }

    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let lwext4_lib = &format!("lwext4-{}", arch);
    let lwext4_lib_path = &format!("c/lwext4/lib{}.a", lwext4_lib);
    let rebuild_lwext4 = env::var("LWEXT4_FORCE_REBUILD").is_ok()
        || lwext4_sources_newer_than(
            lwext4_lib_path,
            &[
                "c/lwext4/include/ext4_config.h",
                "c/lwext4/src/ext4.c",
                "c/lwext4/src/ext4_xattr.c",
                "src/blockdev.rs",
            ],
        );
    if rebuild_lwext4 && Path::new(lwext4_lib_path).exists() {
        fs::remove_file(lwext4_lib_path).expect("failed to remove stale lwext4 static library");
    }

    if !Path::new(lwext4_lib_path).exists() {
        let status = Command::new("make")
            .args(&[
                "musl-generic",
                "-C",
                c_path.to_str().expect("invalid path of lwext4"),
            ])
            .arg(&format!("ARCH={}", arch))
            .status()
            .expect("failed to execute process: make lwext4");
        assert!(status.success());

        if !Path::new("src/bindings.rs").exists() {
            let cc = &format!("{}-linux-musl-gcc", arch);
            let output = Command::new(cc)
                .args(["-print-sysroot"])
                .output()
                .expect("failed to execute process: gcc -print-sysroot");

            let sysroot = core::str::from_utf8(&output.stdout).unwrap();
            let sysroot = sysroot.trim_end();
            let sysroot_inc = &format!("-I{}/include/", sysroot);

            generates_bindings_to_rust(sysroot_inc);
        }
    }

    /* No longer need to implement the libc.a
    let libc_name = &format!("c-{}", arch);
    let libc_dir = env::var("LIBC_BUILD_TARGET_DIR").unwrap_or(String::from("./"));
    let libc_dir = PathBuf::from(libc_dir)
        .canonicalize()
        .expect("cannot canonicalize LIBC_BUILD_TARGET_DIR");

    println!("cargo:rustc-link-lib=static={libc_name}");
    println!(
        "cargo:rustc-link-search=native={}",
        libc_dir.to_str().unwrap()
    );
    */

    println!("cargo:rustc-link-lib=static={lwext4_lib}");
    println!(
        "cargo:rustc-link-search=native={}",
        c_path.to_str().unwrap()
    );
    println!("cargo:rerun-if-changed=c/wrapper.h");
    println!("cargo:rerun-if-changed=src/blockdev.rs");
    println!("cargo:rerun-if-changed=c/lwext4/include/ext4_config.h");
    println!("cargo:rerun-if-changed=c/lwext4/src/ext4.c");
    println!("cargo:rerun-if-changed=c/lwext4/src/ext4_xattr.c");
}

fn lwext4_sources_newer_than(lib_path: &str, source_paths: &[&str]) -> bool {
    let Ok(lib_meta) = fs::metadata(lib_path) else {
        return true;
    };
    let lib_modified = lib_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

    source_paths.iter().any(|path| {
        fs::metadata(path)
            .and_then(|meta| meta.modified())
            .map(|modified| modified > lib_modified)
            .unwrap_or(false)
    })
}

fn generates_bindings_to_rust(mpath: &str) {
    let bindings = bindgen::Builder::default()
        .use_core()
        // The input header we would like to generate bindings for.
        .header("c/wrapper.h")
        //.clang_arg("--sysroot=/path/to/sysroot")
        .clang_arg(mpath)
        //.clang_arg("-I../../ulib/axlibc/include")
        .clang_arg("-I./c/lwext4/include")
        .clang_arg("-I./c/lwext4/build_musl-generic/include/")
        .layout_tests(false)
        // Tell cargo to invalidate the built crate whenever any of the included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        // Finish the builder and generate the bindings.
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from("src");
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}
