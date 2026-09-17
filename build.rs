//! Embeds the application icon into the FastPad executables as Win32 icon resource 1.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/fastpad.ico");
    println!("cargo:rerun-if-env-changed=RC");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    // `cfg!` in a build script describes the host. A non-Windows host (CI's Linux Clippy
    // type-check of the MSVC target) has no rc.exe and never links, so the icon is not needed.
    if !cfg!(windows) {
        println!(
            "cargo:warning=skipping the icon resource: rc.exe needs a Windows host (type-check only)"
        );
        return;
    }

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let icon = manifest_dir.join("assets").join("fastpad.ico");
    let script = out_dir.join("fastpad.rc");
    let resource = out_dir.join("fastpad.res");

    // rc.exe treats backslashes in string literals as escapes.
    let icon_literal = icon.display().to_string().replace('\\', "/");
    std::fs::write(&script, format!("1 ICON \"{icon_literal}\"\n")).expect("write fastpad.rc");

    let rc = find_resource_compiler();
    let status = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg(&script)
        .status()
        .unwrap_or_else(|error| panic!("failed to run '{}': {error}", rc.display()));
    assert!(status.success(), "'{}' failed with {status}", rc.display());

    println!("cargo:rustc-link-arg-bins={}", resource.display());
}

fn find_resource_compiler() -> PathBuf {
    if let Some(rc) = env::var_os("RC") {
        return PathBuf::from(rc);
    }
    if let Some(rc) = env::var_os("PATH")
        .iter()
        .flat_map(env::split_paths)
        .map(|dir| dir.join("rc.exe"))
        .find(|candidate| candidate.is_file())
    {
        return rc;
    }
    let kits = env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
        .join(r"Windows Kits\10\bin");
    newest_sdk_rc(&kits).unwrap_or_else(|| {
        panic!(
            "rc.exe was not found on PATH or under '{}'. Install the Windows 10/11 SDK or set RC.",
            kits.display()
        )
    })
}

fn newest_sdk_rc(kits: &Path) -> Option<PathBuf> {
    let host = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    std::fs::read_dir(kits)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let version = entry
                .file_name()
                .to_str()?
                .split('.')
                .map(|part| part.parse::<u32>().ok())
                .collect::<Option<Vec<_>>>()?;
            let rc = entry.path().join(host).join("rc.exe");
            rc.is_file().then_some((version, rc))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, rc)| rc)
}
