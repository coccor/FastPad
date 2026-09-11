#[cfg(windows)]
use super::win32::Deadline;
#[cfg(windows)]
use fastpad::platform::OwnedHandle;
#[cfg(windows)]
use std::error::Error;
#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::process::Command;
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForInputIdle,
    WaitForSingleObject,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
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
