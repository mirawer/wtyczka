//! Small ownership wrappers for Win32 security APIs. No token impersonation.
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::{ffi::c_void, io, mem, ptr};

use anyhow::{Context, Result, bail, ensure};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, LocalFree},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
        },
        DACL_SECURITY_INFORMATION, EqualSid, GetSecurityDescriptorDacl, GetTokenInformation,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

/// Memory returned by the LocalAlloc family (including converted SIDs/SDs).
pub(super) struct Local(pub(super) *mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

pub(super) fn bool_result(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn win32_result(code: u32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

pub(super) struct User {
    // usize alignment suffices for TOKEN_USER. Its SID points inside this buffer;
    // moving the Vec itself does not move that allocation.
    token_user: Vec<usize>,
    pub(super) sid_text: String,
}

impl User {
    pub(super) fn current() -> Result<Self> {
        Self::for_process(unsafe { GetCurrentProcess() })
    }

    pub(super) fn for_process(process: HANDLE) -> Result<Self> {
        let mut token = ptr::null_mut();
        unsafe { bool_result(OpenProcessToken(process, TOKEN_QUERY, &mut token)) }
            .context("Open process token")?;
        // OpenProcessToken succeeded and transferred ownership.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut bytes = 0;
        let ok = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &mut bytes,
            )
        };
        let error = io::Error::last_os_error();
        ensure!(
            ok == 0 && error.raw_os_error() == Some(ERROR_INSUFFICIENT_BUFFER as i32),
            "Cannot size token user: {error}"
        );
        ensure!(
            bytes as usize >= mem::size_of::<TOKEN_USER>(),
            "Invalid token user size"
        );
        let words = (bytes as usize).div_ceil(mem::size_of::<usize>());
        let mut token_user = vec![0usize; words];
        unsafe {
            bool_result(GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                token_user.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            ))
        }
        .context("Read process user SID")?;
        let sid = unsafe { (*token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text = ptr::null_mut();
        unsafe { bool_result(ConvertSidToStringSidW(sid, &mut text)) }
            .context("Format user SID")?;
        let text_owner = Local(text.cast());
        let mut length = 0;
        // ConvertSidToStringSidW guarantees a NUL-terminated UTF-16 SID.
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        let sid_text = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })?;
        drop(text_owner);
        Ok(Self {
            token_user,
            sid_text,
        })
    }

    pub(super) fn sid(&self) -> PSID {
        unsafe { (*self.token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    pub(super) fn is_same(&self, other: &Self) -> bool {
        unsafe { EqualSid(self.sid(), other.sid()) != 0 }
    }

    pub(super) fn require_owner(&self, handle: HANDLE) -> Result<()> {
        let mut owner = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let code = unsafe {
            GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        let _descriptor = Local(descriptor);
        win32_result(code).context("Read runtime object owner")?;
        ensure!(
            !owner.is_null() && unsafe { EqualSid(owner, self.sid()) } != 0,
            "Runtime object is not owned by the current user"
        );
        Ok(())
    }
}

pub(super) struct Descriptor(Local);
impl Descriptor {
    pub(super) fn private(sid: &str, directory: bool) -> Result<Self> {
        // O: explicitly uses TokenUser, including for elevated processes whose
        // default object owner might otherwise be the Administrators group.
        let inheritance = if directory { "OICI" } else { "" };
        let sddl = format!("O:{sid}D:P(A;{inheritance};FA;;;{sid})");
        let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        let owner = Local(descriptor);
        bool_result(ok).context("Build current-user security descriptor")?;
        Ok(Self(owner))
    }

    pub(super) fn attributes(&self) -> Result<SECURITY_ATTRIBUTES> {
        Ok(SECURITY_ATTRIBUTES {
            nLength: u32::try_from(mem::size_of::<SECURITY_ATTRIBUTES>())?,
            lpSecurityDescriptor: self.0.0,
            bInheritHandle: 0,
        })
    }

    pub(super) fn protect(&self, handle: HANDLE) -> Result<()> {
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = ptr::null_mut();
        unsafe {
            bool_result(GetSecurityDescriptorDacl(
                self.0.0,
                &mut present,
                &mut acl,
                &mut defaulted,
            ))
        }
        .context("Read private DACL")?;
        if present == 0 || acl.is_null() {
            bail!("Refusing an absent/null private DACL");
        }
        unsafe {
            win32_result(SetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null_mut(),
            ))
        }
        .context("Protect current-user DACL")
    }
}
