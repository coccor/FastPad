//! Secondary-launch side of single-instance IPC: the only IPC code bootstrap may call.

use super::InstanceNames;
use super::protocol::{IpcRequest, encode_frame};
use crate::launch::LaunchRequest;
use crate::platform::{OwnedHandle, last_error};
use crate::{FastPadError, Result};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_WRITE, GetLastError,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_READ_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::Pipes::{GetNamedPipeServerProcessId, WaitNamedPipeW};
use windows_sys::Win32::System::Threading::{CreateMutexW, Sleep};
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

pub const FORWARD_BUDGET: Duration = Duration::from_millis(500);
const RETRY_SLICE: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub enum InstanceClaim {
    Primary(OwnedHandle),
    Forwarded,
    Independent,
}

/// Creates the session mutex, or forwards `request` to the instance that already owns it.
pub fn claim_or_forward(request: &LaunchRequest) -> InstanceClaim {
    let Ok(names) = InstanceNames::for_current_session() else {
        return InstanceClaim::Independent;
    };
    match create_instance_mutex(&names) {
        Ok(Some(mutex)) => return InstanceClaim::Primary(mutex),
        Ok(None) => {}
        Err(_) => return InstanceClaim::Independent,
    }
    let forwarded = ipc_request_for(request)
        .and_then(|request| encode_frame(&request))
        .and_then(|frame| send_frame(&names, &frame, FORWARD_BUDGET));
    if forwarded.is_ok() {
        InstanceClaim::Forwarded
    } else {
        InstanceClaim::Independent
    }
}

/// Returns the handle only when this call created the mutex; an existing one is closed at once.
fn create_instance_mutex(names: &InstanceNames) -> Result<Option<OwnedHandle>> {
    let raw = unsafe { CreateMutexW(std::ptr::null(), 0, names.mutex.as_ptr()) };
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let mutex = unsafe { OwnedHandle::from_raw_owned(raw) }?;
    Ok((!existed).then_some(mutex))
}

/// Resolves a relative path against this process's current directory without touching the disk.
pub fn ipc_request_for(request: &LaunchRequest) -> Result<IpcRequest> {
    match request {
        LaunchRequest::New => Ok(IpcRequest::New),
        LaunchRequest::Open(path) => Ok(IpcRequest::Open(std::path::absolute(path)?)),
    }
}

/// Connects within `budget`, lets the server take the foreground, and writes the whole frame.
pub fn send_frame(names: &InstanceNames, frame: &[u8], budget: Duration) -> Result<()> {
    let pipe = connect(&names.pipe, Instant::now() + budget)?;
    let mut server_process = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw(), &mut server_process) } != 0 {
        unsafe {
            AllowSetForegroundWindow(server_process);
        }
    }
    let mut remaining = frame;
    while !remaining.is_empty() {
        let mut written = 0;
        let ok = unsafe {
            WriteFile(
                pipe.as_raw(),
                remaining.as_ptr(),
                remaining.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(last_error());
        }
        if written == 0 {
            return Err(FastPadError::Ipc("pipe accepted no bytes"));
        }
        remaining = &remaining[written as usize..];
    }
    Ok(())
}

fn connect(pipe: &[u16], deadline: Instant) -> Result<OwnedHandle> {
    loop {
        let raw = unsafe {
            CreateFileW(
                pipe.as_ptr(),
                GENERIC_WRITE | FILE_READ_ATTRIBUTES,
                FILE_SHARE_NONE,
                std::ptr::null(),
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                std::ptr::null_mut(),
            )
        };
        if raw != INVALID_HANDLE_VALUE {
            return unsafe { OwnedHandle::from_raw_owned(raw) };
        }
        let error = unsafe { GetLastError() };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !matches!(error, ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY) || remaining.is_zero() {
            return Err(FastPadError::Win32(error));
        }
        let slice = remaining.min(RETRY_SLICE).as_millis().max(1) as u32;
        // A missing pipe makes WaitNamedPipeW fail immediately, so sleep out the slice instead.
        if unsafe { WaitNamedPipeW(pipe.as_ptr(), slice) } == 0
            && unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND
        {
            unsafe { Sleep(slice) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InstanceNames, ipc_request_for, send_frame};
    use crate::ipc::IpcRequest;
    use crate::launch::LaunchRequest;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    fn launch_new_maps_to_ipc_new() {
        assert_eq!(
            ipc_request_for(&LaunchRequest::New).unwrap(),
            IpcRequest::New
        );
    }

    #[test]
    fn relative_launch_path_is_resolved_against_the_secondary_directory_without_disk_access() {
        // Break caught: forwarding a relative path makes the primary resolve it against its own,
        // unrelated current directory.
        let request = LaunchRequest::Open(OsString::from(r"missing-dir\..\notes.md"));
        let expected = std::env::current_dir().unwrap().join("notes.md");
        assert_eq!(
            ipc_request_for(&request).unwrap(),
            IpcRequest::Open(expected)
        );
        let absolute = PathBuf::from(r"C:\definitely\absent\file.txt");
        assert_eq!(
            ipc_request_for(&LaunchRequest::Open(absolute.clone().into_os_string())).unwrap(),
            IpcRequest::Open(absolute)
        );
    }

    #[test]
    fn unreachable_pipe_fails_within_the_forward_budget() {
        // Break caught: an unbounded wait hangs every secondary launch while no server listens.
        let names = InstanceNames {
            mutex: Vec::new(),
            pipe: crate::platform::wide_null(&format!(
                r"\\.\pipe\FastPad-test-absent-{}",
                std::process::id()
            )),
        };
        let started = Instant::now();
        assert!(send_frame(&names, b"FPI1\x02\0\0\0\0", Duration::from_millis(200)).is_err());
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(150), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(450), "{elapsed:?}");
    }
}
