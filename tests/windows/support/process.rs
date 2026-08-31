#[cfg(windows)]
use fastpad::platform::OwnedHandle;
#[cfg(windows)]
use std::error::Error;
#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::process::Command;
#[cfg(windows)]
use std::time::{Duration, Instant};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    TerminateProcess, WaitForInputIdle,
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
const STILL_ACTIVE_STATUS: u32 = 259;

#[cfg(windows)]
pub struct FastPadProcess {
    process: std::process::Child,
    handle: OwnedHandle,
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
        let handle = unsafe {
            OwnedHandle::from_raw_owned(OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                process.id(),
            ))
        }?;

        unsafe {
            WaitForInputIdle(process_raw_handle(&process), 2_000);
        }

        Ok(Self { process, handle })
    }

    pub fn wait_for_main_window(&self, timeout: Duration) -> TestResult<HWND> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(hwnd) = find_main_window(self.process.id())? {
                return Ok(hwnd);
            }
            let exit_code = self.exit_code()?;
            if exit_code != STILL_ACTIVE_STATUS {
                return Err(format!(
                    "fastpad exited before creating a main window (exit code {})",
                    exit_code
                )
                .into());
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for FastPad main window".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn close(mut self) -> TestResult<()> {
        if let Some(hwnd) = find_main_window(self.process.id())? {
            unsafe {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(_status) = self.process.try_wait()? {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        unsafe {
            TerminateProcess(self.handle.as_raw(), 1);
        }
        let _ = self.process.wait();
        Err("timed out waiting for FastPad to exit after WM_CLOSE".into())
    }

    fn exit_code(&self) -> TestResult<u32> {
        let mut code = 0;
        let ok = unsafe { GetExitCodeProcess(self.handle.as_raw(), &mut code) };
        if ok == 0 {
            Err(Box::new(fastpad::platform::last_error()))
        } else {
            Ok(code)
        }
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
