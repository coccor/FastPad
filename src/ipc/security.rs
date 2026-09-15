use crate::platform::{OwnedHandle, last_error};
use crate::{FastPadError, Result};
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, GENERIC_ALL, GetLastError, HANDLE,
};
use windows_sys::Win32::Security::{
    ACL_REVISION, GetLengthSid, GetTokenInformation, IsValidSid, SE_DACL_PRESENT, SE_SELF_RELATIVE,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

// winnt.h values; windows-sys exposes them only behind Win32_System_SystemServices.
const SECURITY_DESCRIPTOR_REVISION: u8 = 1;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

const DESCRIPTOR_HEADER_BYTES: usize = 20;
const ACL_HEADER_BYTES: usize = 8;
const ACE_FIXED_BYTES: usize = 8;

/// A self-relative security descriptor whose DACL grants access only to the process token's user.
#[derive(Debug)]
pub struct CurrentUserAcl {
    descriptor: Vec<u8>,
}

impl CurrentUserAcl {
    pub fn current() -> Result<Self> {
        Ok(Self {
            descriptor: self_relative_descriptor(&current_user_sid()?, GENERIC_ALL),
        })
    }

    /// The returned attributes point into `self` and are valid only while `self` is borrowed.
    pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.as_ptr() as *mut _,
            bInheritHandle: 0,
        }
    }
}

/// The user and session a pipe server must share with this process before a frame is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProcessIdentity {
    pub(crate) user_sid: Vec<u8>,
    pub(crate) session: u32,
}

impl ProcessIdentity {
    pub(crate) fn current() -> Result<Self> {
        Ok(Self {
            user_sid: current_user_sid()?,
            session: super::session_of(unsafe { GetCurrentProcessId() })?,
        })
    }

    pub(crate) fn of_process(process_id: u32) -> Result<Self> {
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if raw.is_null() {
            return Err(last_error());
        }
        let process = unsafe { OwnedHandle::from_raw_owned(raw) }?;
        Ok(Self {
            user_sid: process_user_sid(process.as_raw())?,
            session: super::session_of(process_id)?,
        })
    }

    /// A squatting server from another user or session must never receive a path.
    pub(crate) fn trusts_server(&self, server: &Self) -> bool {
        !self.user_sid.is_empty()
            && self.user_sid == server.user_sid
            && self.session == server.session
    }
}

fn current_user_sid() -> Result<Vec<u8>> {
    process_user_sid(unsafe { GetCurrentProcess() })
}

fn process_user_sid(process: HANDLE) -> Result<Vec<u8>> {
    let mut raw = std::ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw) } == 0 {
        return Err(last_error());
    }
    let token = unsafe { OwnedHandle::from_raw_owned(raw) }?;
    let mut needed = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
    }
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || needed == 0 {
        return Err(last_error());
    }
    // usize storage keeps TOKEN_USER's pointer field aligned.
    let mut storage = vec![0_usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.as_raw(),
            TokenUser,
            storage.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(last_error());
    }
    // SAFETY: the buffer was filled with a TOKEN_USER whose SID points inside `storage`.
    let sid = unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(FastPadError::Ipc("process token user SID is invalid"));
    }
    let length = unsafe { GetLengthSid(sid) } as usize;
    Ok(unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length) }.to_vec())
}

fn self_relative_descriptor(sid: &[u8], access_mask: u32) -> Vec<u8> {
    let ace_bytes = (ACE_FIXED_BYTES + sid.len()).next_multiple_of(4);
    let acl_bytes = ACL_HEADER_BYTES + ace_bytes;
    let mut descriptor = Vec::with_capacity(DESCRIPTOR_HEADER_BYTES + acl_bytes);
    descriptor.push(SECURITY_DESCRIPTOR_REVISION);
    descriptor.push(0);
    descriptor.extend_from_slice(&(SE_DACL_PRESENT | SE_SELF_RELATIVE).to_le_bytes());
    for offset in [0_u32, 0, 0, DESCRIPTOR_HEADER_BYTES as u32] {
        descriptor.extend_from_slice(&offset.to_le_bytes());
    }
    descriptor.push(ACL_REVISION as u8);
    descriptor.push(0);
    descriptor.extend_from_slice(&(acl_bytes as u16).to_le_bytes());
    descriptor.extend_from_slice(&1_u16.to_le_bytes());
    descriptor.extend_from_slice(&0_u16.to_le_bytes());
    descriptor.push(ACCESS_ALLOWED_ACE_TYPE);
    descriptor.push(0);
    descriptor.extend_from_slice(&(ace_bytes as u16).to_le_bytes());
    descriptor.extend_from_slice(&access_mask.to_le_bytes());
    descriptor.extend_from_slice(sid);
    descriptor.resize(DESCRIPTOR_HEADER_BYTES + acl_bytes, 0);
    descriptor
}

#[cfg(test)]
mod tests {
    use super::{CurrentUserAcl, ProcessIdentity, current_user_sid, self_relative_descriptor};
    use windows_sys::Win32::Security::IsValidSecurityDescriptor;

    fn identity(sid: &[u8], session: u32) -> ProcessIdentity {
        ProcessIdentity {
            user_sid: sid.to_vec(),
            session,
        }
    }

    #[test]
    fn server_is_trusted_only_with_the_same_user_and_session() {
        // Break caught: a squatter owned by another user, or the same user's other session,
        // receiving absolute paths from this session's launches.
        let me = identity(&LOCAL_SYSTEM_SID, 2);
        assert!(me.trusts_server(&identity(&LOCAL_SYSTEM_SID, 2)));
        let mut other_user = LOCAL_SYSTEM_SID;
        other_user[8] = 19;
        assert!(!me.trusts_server(&identity(&other_user, 2)));
        assert!(!me.trusts_server(&identity(&LOCAL_SYSTEM_SID, 3)));
        assert!(!identity(&[], 2).trusts_server(&identity(&[], 2)));
    }

    #[test]
    fn this_process_is_its_own_trusted_server_and_unknown_processes_fail_closed() {
        let me = ProcessIdentity::current().unwrap();
        let server = ProcessIdentity::of_process(std::process::id()).unwrap();
        assert!(me.trusts_server(&server));
        assert!(ProcessIdentity::of_process(0).is_err());
    }

    const LOCAL_SYSTEM_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];

    #[test]
    fn descriptor_has_exactly_one_allow_ace_for_the_given_sid() {
        // Break caught: an absent DACL (NULL = everyone) or a second ACE widens pipe access.
        let bytes = self_relative_descriptor(&LOCAL_SYSTEM_SID, 0x1000_0000);
        assert_eq!(bytes.len(), 20 + 8 + 8 + 12);
        assert_eq!(&bytes[..4], &[1, 0, 0x04, 0x80]);
        assert_eq!(
            &bytes[4..20],
            &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 20, 0, 0, 0]
        );
        assert_eq!(&bytes[20..28], &[2, 0, 28, 0, 1, 0, 0, 0]);
        assert_eq!(&bytes[28..36], &[0, 0, 20, 0, 0, 0, 0, 0x10]);
        assert_eq!(&bytes[36..], &LOCAL_SYSTEM_SID);
    }

    #[test]
    fn current_user_descriptor_is_valid_and_names_the_token_user() {
        let acl = CurrentUserAcl::current().unwrap();
        let sid = current_user_sid().unwrap();
        assert_ne!(
            unsafe { IsValidSecurityDescriptor(acl.descriptor.as_ptr() as *mut _) },
            0
        );
        assert!(acl.descriptor.windows(sid.len()).any(|bytes| bytes == sid));
    }
}
