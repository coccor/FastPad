#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() -> fastpad::Result<()> {
    let options = fastpad::launch::parse(std::env::args_os().skip(1))?;
    std::process::exit(fastpad::bootstrap::run(options)?);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("FastPad requires Windows.");
}
