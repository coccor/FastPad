pub mod client;
pub mod protocol;
mod security;
pub(crate) mod server;

pub use protocol::{IpcRequest, decode_frame, encode_frame};
pub use security::CurrentUserAcl;
pub use server::IpcServer;

use crate::Result;
use crate::platform::{last_error, wide_null};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceNames {
    pub mutex: Vec<u16>,
    pub pipe: Vec<u16>,
}

impl InstanceNames {
    pub fn for_current_session() -> Result<Self> {
        Ok(Self::for_session(session_of(unsafe {
            GetCurrentProcessId()
        })?))
    }

    pub fn for_session(session: u32) -> Self {
        Self {
            mutex: wide_null(&format!(r"Local\FastPad-{session}")),
            pipe: wide_null(&format!(r"\\.\pipe\FastPad-{session}")),
        }
    }
}

fn session_of(process_id: u32) -> Result<u32> {
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(process_id, &mut session) } == 0 {
        return Err(last_error());
    }
    Ok(session)
}

/// Binds the listening pipe for this session; used only by deferred window work.
pub fn bind_session_server() -> Result<IpcServer> {
    IpcServer::bind(
        &InstanceNames::for_current_session()?,
        &CurrentUserAcl::current()?,
    )
}

#[cfg(test)]
mod tests {
    use super::InstanceNames;
    use crate::platform::wide_null;

    #[test]
    fn names_are_scoped_to_the_session_id() {
        // Break caught: a global or per-user name lets two interactive sessions steal launches.
        let names = InstanceNames::for_session(3);
        assert_eq!(names.mutex, wide_null(r"Local\FastPad-3"));
        assert_eq!(names.pipe, wide_null(r"\\.\pipe\FastPad-3"));
    }

    #[test]
    fn current_session_names_use_this_process_session() {
        let names = InstanceNames::for_current_session().unwrap();
        let mutex = String::from_utf16(&names.mutex[..names.mutex.len() - 1]).unwrap();
        let session = mutex.strip_prefix(r"Local\FastPad-").unwrap();
        assert_eq!(names, InstanceNames::for_session(session.parse().unwrap()));
    }
}
