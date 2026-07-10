use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=../user/src/bin/");
    println!("cargo:rerun-if-env-changed=KAIRIX_SSH_TEST_KEY");
    println!("cargo:rustc-check-cfg=cfg(embed_ssh_test_key)");

    let Some(key_path) = env::var_os("KAIRIX_SSH_TEST_KEY") else {
        return;
    };

    let key_path = PathBuf::from(key_path);
    println!("cargo:rerun-if-changed={}", key_path.display());

    let key = fs::read(&key_path).unwrap_or_else(|err| {
        panic!(
            "failed to read KAIRIX_SSH_TEST_KEY {}: {err}",
            key_path.display()
        )
    });
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo did not set OUT_DIR"));
    fs::write(out_dir.join("id_ed25519"), key)
        .expect("failed to copy KAIRIX_SSH_TEST_KEY into OUT_DIR");
    println!("cargo:rustc-cfg=embed_ssh_test_key");
}
