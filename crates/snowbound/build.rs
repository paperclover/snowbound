//! On Windows, links in the executable's icon and manifest, compiled by llvm-mingw's windres,
//! which `platform/windows/cargo.sh` names. On Linux, enters through `loader`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
        && ["x86_64", "aarch64"].contains(&arch.as_str())
    {
        println!("cargo:rustc-link-arg-bins=-Wl,-e,sb_entry");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let folder = std::path::Path::new("windows");
    for input in ["snowbound.rc", "snowbound.ico", "snowbound.manifest"] {
        println!("cargo:rerun-if-changed={}", folder.join(input).display());
    }
    println!("cargo:rerun-if-env-changed=LLVM_MINGW");
    let tools = std::env::var("LLVM_MINGW")
        .expect("Windows builds need llvm-mingw: build through platform/windows/cargo.sh");
    let output = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("resources.o");
    let status = std::process::Command::new(format!("{tools}/bin/{arch}-w64-mingw32-windres"))
        .current_dir(folder)
        .arg("snowbound.rc")
        .arg(&output)
        .status()
        .expect("windres runs");
    assert!(status.success(), "windres compiled the resources");
    println!("cargo:rustc-link-arg-bins={}", output.display());
}
