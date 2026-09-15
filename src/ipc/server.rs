use super::protocol::{IpcRequest, MAX_FRAME_BYTES, decode_frame};
use super::{CurrentUserAcl, InstanceNames};
use crate::platform::{OwnedHandle, last_error};
use crate::{FastPadError, Result};
use std::fmt;
use std::pin::Pin;
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED,
    GetLastError, HANDLE,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_INBOUND, ReadFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

const MAX_STEPS_PER_POLL: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Connect,
    Read,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Completion {
    Pending,
    Done(u32),
    Failed(u32),
}

/// One overlapped, inbound pipe instance serviced from the UI thread. Each connection carries one
/// frame terminated by the client closing its end.
pub struct IpcServer {
    pipe: OwnedHandle,
    event: OwnedHandle,
    read: Pin<Box<OVERLAPPED>>,
    // One byte past the limit makes an oversized frame observable before end of stream.
    buffer: Box<[u8; MAX_FRAME_BYTES + 1]>,
    filled: usize,
    operation: Operation,
    in_flight: bool,
    immediate: Option<Completion>,
}

impl IpcServer {
    pub fn bind(names: &InstanceNames, security: &CurrentUserAcl) -> Result<Self> {
        let event = unsafe {
            OwnedHandle::from_raw_owned(CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()))
        }?;
        let attributes = security.attributes();
        let raw = unsafe {
            CreateNamedPipeW(
                names.pipe.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                MAX_FRAME_BYTES as u32,
                0,
                &attributes,
            )
        };
        let pipe = unsafe { OwnedHandle::from_raw_owned(raw) }?;
        let buffer = vec![0_u8; MAX_FRAME_BYTES + 1]
            .into_boxed_slice()
            .try_into()
            .map_err(|_| FastPadError::Invariant("pipe buffer has the wrong length"))?;
        let mut server = Self {
            pipe,
            event,
            read: Box::pin(OVERLAPPED::default()),
            buffer,
            filled: 0,
            operation: Operation::Connect,
            in_flight: false,
            immediate: None,
        };
        server.begin_connect()?;
        Ok(server)
    }

    /// Manual-reset event signaled whenever `poll` has work; owned by `self`.
    pub fn event(&self) -> HANDLE {
        self.event.as_raw()
    }

    /// Advances connect/read without blocking and returns every complete, valid request.
    pub fn poll(&mut self) -> Result<Vec<IpcRequest>> {
        let mut requests = Vec::new();
        for _ in 0..MAX_STEPS_PER_POLL {
            match (self.take_completion(), self.operation) {
                (Completion::Pending, _) => return Ok(requests),
                (Completion::Done(_), Operation::Connect) => {
                    self.filled = 0;
                    self.begin_read()?;
                }
                (Completion::Done(bytes), Operation::Read) => {
                    self.filled += bytes as usize;
                    if self.filled > MAX_FRAME_BYTES {
                        self.reconnect()?;
                    } else {
                        self.begin_read()?;
                    }
                }
                (Completion::Failed(ERROR_BROKEN_PIPE), Operation::Read) => {
                    if let Ok(request) = decode_frame(&self.buffer[..self.filled]) {
                        requests.push(request);
                    }
                    self.reconnect()?;
                }
                (Completion::Failed(_), _) => self.reconnect()?,
            }
        }
        // Kernel completions signal the event themselves; only a synthesized one needs a nudge.
        if self.immediate.is_some() {
            unsafe {
                SetEvent(self.event.as_raw());
            }
        }
        Ok(requests)
    }

    fn take_completion(&mut self) -> Completion {
        if let Some(completion) = self.immediate.take() {
            return completion;
        }
        if !self.in_flight {
            return Completion::Pending;
        }
        let mut bytes = 0;
        if unsafe { GetOverlappedResult(self.pipe.as_raw(), &*self.read, &mut bytes, 0) } != 0 {
            self.in_flight = false;
            return Completion::Done(bytes);
        }
        match unsafe { GetLastError() } {
            ERROR_IO_INCOMPLETE => Completion::Pending,
            error => {
                self.in_flight = false;
                Completion::Failed(error)
            }
        }
    }

    fn arm(&mut self, operation: Operation) -> Result<*mut OVERLAPPED> {
        if unsafe { ResetEvent(self.event.as_raw()) } == 0 {
            return Err(last_error());
        }
        self.operation = operation;
        let overlapped = self.read.as_mut().get_mut();
        *overlapped = OVERLAPPED {
            hEvent: self.event.as_raw(),
            ..OVERLAPPED::default()
        };
        Ok(overlapped)
    }

    fn begin_connect(&mut self) -> Result<()> {
        let overlapped = self.arm(Operation::Connect)?;
        if unsafe { ConnectNamedPipe(self.pipe.as_raw(), overlapped) } != 0 {
            self.in_flight = true;
            return Ok(());
        }
        match unsafe { GetLastError() } {
            ERROR_IO_PENDING => self.in_flight = true,
            // The client connected (and possibly already closed) before this call.
            ERROR_PIPE_CONNECTED | ERROR_NO_DATA => self.immediate = Some(Completion::Done(0)),
            error => return Err(FastPadError::Win32(error)),
        }
        Ok(())
    }

    fn begin_read(&mut self) -> Result<()> {
        let overlapped = self.arm(Operation::Read)?;
        let spare = &mut self.buffer[self.filled..];
        let ok = unsafe {
            ReadFile(
                self.pipe.as_raw(),
                spare.as_mut_ptr(),
                spare.len() as u32,
                std::ptr::null_mut(),
                overlapped,
            )
        };
        if ok != 0 {
            self.in_flight = true;
            return Ok(());
        }
        match unsafe { GetLastError() } {
            ERROR_IO_PENDING => self.in_flight = true,
            error => self.immediate = Some(Completion::Failed(error)),
        }
        Ok(())
    }

    fn reconnect(&mut self) -> Result<()> {
        self.filled = 0;
        if unsafe { DisconnectNamedPipe(self.pipe.as_raw()) } == 0 {
            return Err(last_error());
        }
        self.begin_connect()
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if self.in_flight {
            // The kernel still references `read` and `buffer`; wait out cancellation before freeing.
            let mut bytes = 0;
            unsafe {
                CancelIoEx(self.pipe.as_raw(), &*self.read);
                GetOverlappedResult(self.pipe.as_raw(), &*self.read, &mut bytes, 1);
            }
        }
    }
}

impl fmt::Debug for IpcServer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IpcServer")
            .field("pipe", &self.pipe)
            .field("event", &self.event)
            .field("filled", &self.filled)
            .field("operation", &self.operation)
            .field("in_flight", &self.in_flight)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::IpcServer;
    use crate::ipc::client::send_frame;
    use crate::ipc::protocol::{IpcRequest, MAX_FRAME_BYTES, encode_frame};
    use crate::ipc::{CurrentUserAcl, InstanceNames};
    use crate::platform::wide_null;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    pub(crate) fn unique_names() -> InstanceNames {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let suffix = format!(
            "test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        InstanceNames {
            mutex: wide_null(&format!(r"Local\FastPad-{suffix}")),
            pipe: wide_null(&format!(r"\\.\pipe\FastPad-{suffix}")),
        }
    }

    fn bind(names: &InstanceNames) -> IpcServer {
        IpcServer::bind(names, &CurrentUserAcl::current().unwrap()).unwrap()
    }

    /// Sends from a helper thread (the client blocks) while this thread services the server.
    pub(crate) fn exchange(
        server: &mut IpcServer,
        names: &InstanceNames,
        frame: Vec<u8>,
    ) -> Vec<IpcRequest> {
        let pipe = names.clone();
        let client = std::thread::spawn(move || send_frame(&pipe, &frame, Duration::from_secs(2)));
        let requests = service_until_quiet(server, || client.is_finished());
        client.join().unwrap().unwrap();
        requests
    }

    /// Polls on every signal until `finished` holds and the event stays quiet for 100 ms.
    pub(crate) fn service_until_quiet(
        server: &mut IpcServer,
        finished: impl Fn() -> bool,
    ) -> Vec<IpcRequest> {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut quiet_since = None;
        while Instant::now() < deadline {
            if unsafe { WaitForSingleObject(server.event(), 20) } == WAIT_OBJECT_0 {
                requests.extend(server.poll().unwrap());
                quiet_since = None;
            } else if finished() {
                let since = *quiet_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(100) {
                    break;
                }
            }
        }
        requests
    }

    #[test]
    fn valid_frames_are_decoded_across_successive_connections() {
        // Break caught: forgetting to disconnect and re-listen leaves every later secondary busy.
        let names = unique_names();
        let mut server = bind(&names);
        let open = IpcRequest::Open(PathBuf::from(r"C:\notes\zăpadă.md"));
        assert_eq!(
            exchange(&mut server, &names, encode_frame(&open).unwrap()),
            vec![open]
        );
        assert_eq!(
            exchange(
                &mut server,
                &names,
                encode_frame(&IpcRequest::Activate).unwrap()
            ),
            vec![IpcRequest::Activate]
        );
    }

    #[test]
    fn malformed_and_oversized_frames_yield_nothing_and_the_server_keeps_listening() {
        // Break caught: a bad client either reaches app state or wedges the single pipe instance.
        let names = unique_names();
        let mut server = bind(&names);
        assert!(exchange(&mut server, &names, b"FPI1\xFF\0\0\0\0".to_vec()).is_empty());
        let mut trailing = encode_frame(&IpcRequest::New).unwrap();
        trailing.push(0);
        assert!(exchange(&mut server, &names, trailing).is_empty());
        let pipe = names.clone();
        let oversized = std::thread::spawn(move || {
            let _ = send_frame(
                &pipe,
                &vec![0x41; MAX_FRAME_BYTES * 2],
                Duration::from_secs(2),
            );
        });
        assert!(service_until_quiet(&mut server, || oversized.is_finished()).is_empty());
        oversized.join().unwrap();
        assert_eq!(
            exchange(&mut server, &names, encode_frame(&IpcRequest::New).unwrap()),
            vec![IpcRequest::New]
        );
    }

    #[test]
    fn second_server_for_the_same_name_is_refused() {
        // Break caught: dropping FILE_FLAG_FIRST_PIPE_INSTANCE lets a squatter share the name.
        let names = unique_names();
        let _first = bind(&names);
        assert!(IpcServer::bind(&names, &CurrentUserAcl::current().unwrap()).is_err());
    }

    #[test]
    fn dropping_a_listening_server_cancels_its_pending_connect() {
        let names = unique_names();
        drop(bind(&names));
        let _again = bind(&names);
    }
}
