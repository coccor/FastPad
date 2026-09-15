#[cfg(windows)]
use super::win32::Deadline;
#[cfg(windows)]
use fastpad::platform::OwnedHandle;
#[cfg(windows)]
use std::error::Error;
#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::mem::size_of;
#[cfg(windows)]
use std::process::Command;
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{HANDLE, HMODULE, HWND, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT};
#[cfg(windows)]
use windows_sys::Win32::System::ProcessStatus::{EnumProcessModules, GetModuleBaseNameW};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_VM_READ, WaitForInputIdle, WaitForSingleObject,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, EnumChildWindows, EnumWindows, GetClassNameW, GetWindowTextW,
    GetWindowThreadProcessId, PostMessageW, SendMessageW, WM_CLOSE,
};
#[cfg(windows)]
use windows_sys::core::BOOL;

#[cfg(windows)]
type TestResult<T> = Result<T, Box<dyn Error>>;

#[cfg(windows)]
pub struct FastPadProcess {
    process: std::process::Child,
}

#[cfg(windows)]
impl FastPadProcess {
    pub fn spawn<I, S>(args: I) -> TestResult<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fastpad"));
        command.args(args);
        Self::spawn_command(command)
    }

    /// Spawns FastPad with `LOCALAPPDATA` redirected so settings never touch the real profile.
    pub fn spawn_with_local_app_data<I, S>(
        args: I,
        local_app_data: &std::path::Path,
    ) -> TestResult<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fastpad"));
        command.args(args).env("LOCALAPPDATA", local_app_data);
        Self::spawn_command(command)
    }

    /// Like `spawn_with_local_app_data`, plus extra environment variables for the child. Returns
    /// immediately, without `WaitForInputIdle`, so callers can reach the window before its deferred
    /// startup chain runs.
    pub fn spawn_with_environment<I, S>(
        args: I,
        local_app_data: &std::path::Path,
        environment: &[(&str, String)],
    ) -> TestResult<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fastpad"));
        command.args(args).env("LOCALAPPDATA", local_app_data);
        for (name, value) in environment {
            command.env(name, value);
        }
        Ok(Self {
            process: command.spawn()?,
        })
    }

    pub fn has_dialog(&self) -> TestResult<bool> {
        Ok(find_unsaved_changes_dialog(self.process.id())?.is_some())
    }

    fn spawn_command(mut command: Command) -> TestResult<Self> {
        let process = command.spawn()?;

        unsafe {
            WaitForInputIdle(process_raw_handle(&process), 2_000);
        }

        Ok(Self { process })
    }

    pub fn id(&self) -> u32 {
        self.process.id()
    }

    pub fn wait_for_main_window(&mut self, timeout: Duration) -> TestResult<HWND> {
        let deadline = Deadline::after(timeout);
        loop {
            if let Some(hwnd) = find_main_window(self.process.id())? {
                return Ok(hwnd);
            }
            if let Some(status) = self.process.try_wait()? {
                return Err(format!(
                    "fastpad exited before creating a main window (exit code {})",
                    status.code().unwrap_or(-1)
                )
                .into());
            }
            if deadline.expired() {
                return Err("timed out waiting for FastPad main window".into());
            }
            deadline.sleep_step();
        }
    }

    /// Requests a normal shutdown and waits for the process to exit. Does not dismiss any dialog
    /// that might be blocking that shutdown (e.g. a "Save changes?" prompt): an unexpected dialog
    /// at close time is meant to surface as a loud timeout failure for every caller of this shared
    /// helper. A caller that deliberately provokes a real, blocking `MessageBoxW` (e.g.
    /// `tests/windows/highlighting.rs`'s missing-Lexilla scenario) must dismiss it itself first,
    /// via `wait_and_dismiss_dialog`, before calling `close`.
    pub fn close(mut self) -> TestResult<()> {
        if let Some(status) = self.process.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("fastpad exited with nonzero exit code {:?}", status.code()).into())
            };
        }
        if let Some(hwnd) = find_main_window(self.process.id())? {
            unsafe {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
        }

        wait_for_exit(
            &mut self.process,
            &Deadline::after(Duration::from_secs(2)),
            true,
        )
    }
}

#[cfg(windows)]
impl Drop for FastPadProcess {
    fn drop(&mut self) {
        let _ = cleanup_process(&mut self.process, &Deadline::after(Duration::from_secs(2)));
    }
}

/// Checks whether a module named `module_file_name` (e.g. `"Lexilla.dll"`) is currently loaded in
/// another process, via `EnumProcessModules`/`GetModuleBaseNameW` (Psapi). Used to observe deferred
/// DLL loading from outside the target process without reaching into its internals.
#[cfg(windows)]
pub fn process_has_module_loaded(process_id: u32, module_file_name: &str) -> TestResult<bool> {
    let raw = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, process_id) };
    if raw.is_null() {
        return Err(Box::new(fastpad::platform::last_error()));
    }
    let handle = unsafe { OwnedHandle::from_raw_owned(raw) }?;

    let mut needed: u32 = 0;
    unsafe {
        EnumProcessModules(handle.as_raw(), std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        return Ok(false);
    }
    let count = needed as usize / size_of::<HMODULE>();
    let mut modules: Vec<HMODULE> = vec![std::ptr::null_mut(); count];
    let mut needed_after: u32 = 0;
    let ok = unsafe {
        EnumProcessModules(
            handle.as_raw(),
            modules.as_mut_ptr(),
            needed,
            &mut needed_after,
        )
    };
    if ok == 0 {
        return Err(Box::new(fastpad::platform::last_error()));
    }
    let actual_count = (needed_after as usize / size_of::<HMODULE>()).min(modules.len());

    for &module in &modules[..actual_count] {
        let mut name = [0_u16; 260];
        let length = unsafe {
            GetModuleBaseNameW(
                handle.as_raw(),
                module,
                name.as_mut_ptr(),
                name.len() as u32,
            )
        };
        if length == 0 {
            continue;
        }
        if String::from_utf16_lossy(&name[..length as usize]).eq_ignore_ascii_case(module_file_name)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Waits for a standard `MessageBoxW` dialog to appear over `process_id`'s windows and dismisses
/// it (see `dismiss_dialog`). Useful for deliberately clearing a *predictable, isolated* dialog
/// (e.g. a failed-language-activation warning) before doing anything else, so it cannot later
/// overlap with a second dialog `close()` may need to show/dismiss reentrantly (nested modal
/// `MessageBoxW` calls on the same thread are surprising to reason about; avoiding the overlap in
/// the first place is simpler than making `close()` robust to it).
#[cfg(windows)]
pub fn wait_and_dismiss_dialog(process_id: u32, timeout: Duration) -> TestResult<()> {
    let deadline = Deadline::after(timeout);
    loop {
        if let Some(dialog) = find_unsaved_changes_dialog(process_id)? {
            dismiss_dialog(dialog);
            return Ok(());
        }
        if deadline.expired() {
            return Err("timed out waiting for a dialog to appear".into());
        }
        deadline.sleep_step();
    }
}

#[cfg(windows)]
pub fn wait_for_process_exit(process_id: u32, timeout: Duration) -> TestResult<()> {
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            process_id,
        )
    };
    if raw.is_null() {
        return Ok(());
    }
    let handle = unsafe { OwnedHandle::from_raw_owned(raw) }?;
    let deadline = Deadline::after(timeout);
    loop {
        let wait = unsafe { WaitForSingleObject(handle.as_raw(), deadline.remaining_millis()) };
        if wait == WAIT_OBJECT_0 {
            return Ok(());
        }
        if wait == WAIT_TIMEOUT && !deadline.expired() {
            continue;
        }
        return Err("timed out waiting for process exit".into());
    }
}

#[cfg(windows)]
fn find_main_window(process_id: u32) -> TestResult<Option<HWND>> {
    let mut search = WindowSearch {
        process_id,
        hwnd: None,
    };
    let ok = unsafe {
        EnumWindows(
            Some(enum_main_window),
            &mut search as *mut WindowSearch as isize,
        )
    };
    if ok == 0 && search.hwnd.is_none() {
        return Err(Box::new(fastpad::platform::last_error()));
    }
    Ok(search.hwnd)
}

#[cfg(windows)]
struct WindowSearch {
    process_id: u32,
    hwnd: Option<HWND>,
}

#[cfg(windows)]
unsafe extern "system" fn enum_main_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = unsafe { &mut *(lparam as *mut WindowSearch) };
    let mut process_id = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut process_id);
    }
    if process_id == search.process_id {
        search.hwnd = Some(hwnd);
        return 0;
    }
    1
}

/// Clicks whichever of the "No" (discard, for the "Save changes?" prompt) or "OK" (for a plain
/// warning, e.g. a failed language activation) buttons a standard `MessageBoxW` actually has.
#[cfg(windows)]
fn dismiss_dialog(dialog: HWND) {
    let mut search = ButtonSearch { hwnd: None };
    unsafe {
        EnumChildWindows(
            dialog,
            Some(enum_no_or_ok_button),
            &mut search as *mut ButtonSearch as isize,
        );
    }
    if let Some(button) = search.hwnd {
        unsafe {
            SendMessageW(button, BM_CLICK, 0, 0);
        }
    }
}

#[cfg(windows)]
struct ButtonSearch {
    hwnd: Option<HWND>,
}

/// Matches a standard `MessageBoxW` button by its (English, matching this test suite's existing
/// hardcoded-English assumption in the prompt text it is dismissing) caption rather than by
/// control id: `GetDlgItem` did not reliably find these buttons by id cross-process in practice.
#[cfg(windows)]
unsafe extern "system" fn enum_no_or_ok_button(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = unsafe { &mut *(lparam as *mut ButtonSearch) };
    let mut text = [0_u16; 32];
    let length = unsafe { GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32) };
    if length > 0 {
        let text = String::from_utf16_lossy(&text[..length as usize]);
        let normalized = text.trim_start_matches('&');
        if normalized.eq_ignore_ascii_case("No") || normalized.eq_ignore_ascii_case("OK") {
            search.hwnd = Some(hwnd);
            return 0;
        }
    }
    1
}

/// Finds a top-level standard `MessageBoxW` dialog (window class `"#32770"`) owned by `process_id`,
/// such as FastPad's "Save changes?" close prompt.
#[cfg(windows)]
fn find_unsaved_changes_dialog(process_id: u32) -> TestResult<Option<HWND>> {
    let mut search = DialogSearch {
        process_id,
        hwnd: None,
    };
    let ok = unsafe {
        EnumWindows(
            Some(enum_dialog_for_process),
            &mut search as *mut DialogSearch as isize,
        )
    };
    if ok == 0 && search.hwnd.is_none() {
        return Err(Box::new(fastpad::platform::last_error()));
    }
    Ok(search.hwnd)
}

#[cfg(windows)]
struct DialogSearch {
    process_id: u32,
    hwnd: Option<HWND>,
}

#[cfg(windows)]
unsafe extern "system" fn enum_dialog_for_process(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = unsafe { &mut *(lparam as *mut DialogSearch) };
    let mut process_id = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut process_id);
    }
    if process_id != search.process_id {
        return 1;
    }
    let mut class_name = [0_u16; 32];
    let length = unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
    if length > 0 && String::from_utf16_lossy(&class_name[..length as usize]) == "#32770" {
        search.hwnd = Some(hwnd);
        return 0;
    }
    1
}

#[cfg(windows)]
fn process_raw_handle(process: &std::process::Child) -> HANDLE {
    use std::os::windows::io::AsRawHandle;

    process.as_raw_handle() as HANDLE
}

#[cfg(windows)]
fn wait_for_exit(
    process: &mut std::process::Child,
    deadline: &Deadline,
    require_zero_exit: bool,
) -> TestResult<()> {
    loop {
        if let Some(status) = process.try_wait()? {
            if require_zero_exit && !status.success() {
                return Err(
                    format!("fastpad exited with nonzero exit code {:?}", status.code()).into(),
                );
            }
            return Ok(());
        }
        if deadline.expired() {
            return Err("timed out waiting for FastPad to exit after WM_CLOSE".into());
        }
        deadline.sleep_step();
    }
}

#[cfg(windows)]
fn cleanup_process(process: &mut std::process::Child, deadline: &Deadline) -> TestResult<()> {
    if process.try_wait()?.is_none() {
        process.kill()?;
        wait_for_exit(process, deadline, false)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Deadline, cleanup_process, wait_for_exit};
    use std::process::Command;
    use std::time::Duration;

    #[test]
    fn wait_for_exit_rejects_nonzero_process_status() {
        // Break caught: treating any exited child as a clean shutdown hides failures from helper
        // callers that rely on WM_CLOSE producing a zero exit code.
        let mut child = Command::new("cmd").args(["/C", "exit 5"]).spawn().unwrap();

        let error = wait_for_exit(&mut child, &Deadline::after(Duration::from_secs(2)), true)
            .unwrap_err()
            .to_string();

        assert!(error.contains("nonzero exit code"));
    }

    #[test]
    fn cleanup_process_kills_and_reaps_a_running_child() {
        // Break caught: helper cleanup can leave an orphaned child alive if teardown does not kill
        // and wait on timeout or assertion failure paths.
        let mut child = Command::new("cmd")
            .args(["/C", "ping", "127.0.0.1", "-n", "30"])
            .spawn()
            .unwrap();

        cleanup_process(&mut child, &Deadline::after(Duration::from_secs(2))).unwrap();

        assert!(child.try_wait().unwrap().is_some());
    }
}
