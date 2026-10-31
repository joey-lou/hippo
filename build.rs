use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Release LTO keeps ONNX Runtime's CoreML objects, which call
    // `___isPlatformVersionAtLeast`. rustc does not link clang's compiler-rt,
    // so provide the one object that defines it.
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("macos") {
        return;
    }
    let obj = os_version_check_object();
    println!("cargo:rustc-link-arg={}", obj.display());
}

fn os_version_check_object() -> PathBuf {
    let runtime_dir = clang_runtime_dir();
    let archive = runtime_dir.join("libclang_rt.osx.a");
    if !archive.is_file() {
        panic!(
            "missing {} (needed to link ONNX Runtime on macOS)",
            archive.display()
        );
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let thin = out.join("libclang_rt.osx.thin.a");
    let lipo_arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        other => panic!("unsupported macOS arch {other:?}"),
    };
    let status = Command::new("lipo")
        .args(["-thin", lipo_arch, "-output"])
        .arg(&thin)
        .arg(&archive)
        .status()
        .unwrap_or_else(|err| panic!("failed to run lipo: {err}"));
    let archive_for_ar = if status.success() { &thin } else { &archive };
    let obj_dir = out.join("os_version_check");
    std::fs::create_dir_all(&obj_dir).unwrap_or_else(|err| panic!("failed to create {err}"));
    let status = Command::new("ar")
        .current_dir(&obj_dir)
        .args([
            "-x",
            archive_for_ar
                .to_str()
                .expect("runtime archive path is utf-8"),
            "os_version_check.c.o",
        ])
        .status()
        .unwrap_or_else(|err| panic!("failed to run ar: {err}"));
    if !status.success() {
        panic!(
            "ar could not extract os_version_check.c.o from {}",
            archive.display()
        );
    }
    let obj = obj_dir.join("os_version_check.c.o");
    if !obj.is_file() {
        panic!("extracted object missing at {}", obj.display());
    }
    obj
}

fn clang_runtime_dir() -> PathBuf {
    let output = Command::new("clang")
        .arg("-print-runtime-dir")
        .output()
        .unwrap_or_else(|err| panic!("clang is required to link ONNX Runtime on macOS: {err}"));
    if !output.status.success() {
        panic!(
            "clang -print-runtime-dir failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
}
