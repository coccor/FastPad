#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    let exit_code = match run() {
        Ok(code) => code,
        Err(error) => {
            show_error(&error.to_string());
            error.startup_stage().map_or(1, |stage| stage.exit_code())
        }
    };
    std::process::exit(exit_code);
}

#[cfg(windows)]
fn run() -> fastpad::Result<i32> {
    let options = fastpad::launch::parse(std::env::args_os().skip(1))?;
    fastpad::bootstrap::run(options)
}

#[cfg(windows)]
fn show_error(message: &str) {
    let title = fastpad::platform::wide_null("FastPad");
    let message = fastpad::platform::wide_null(message);
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_OK,
        );
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("FastPad requires Windows.");
}
