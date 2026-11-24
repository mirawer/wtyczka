//! Windows runtime directories, hidden child processes and current-user local IPC.
//!
//! Required windows-sys features: Win32_Foundation, Win32_Security,
//! Win32_Security_Authorization, Win32_Security_Cryptography,
//! Win32_Storage_FileSystem, Win32_System_Pipes, Win32_System_Threading.
//! The security boundary is the Windows user, not other processes running as that
//! user or an administrator. In particular, these helpers do not protect keys
//! from process-memory inspection and do not change Windows Error Reporting.

#[path = "system/naming.rs"]
mod naming;
#[path = "system/security.rs"]
mod security;

use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Write},
    mem,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf},
    ptr,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE,
        INVALID_HANDLE_VALUE,
    },
    Security::{
        Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash},
        SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, DELETE,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK,
        GetFileInformationByHandle, GetFileType, GetFinalPathNameByHandleW, OPEN_ALWAYS,
        OPEN_EXISTING, READ_CONTROL, SECURITY_IDENTIFICATION, WRITE_DAC,
    },
    System::{
        Pipes::GetNamedPipeServerProcessId,
        Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, OpenProcess,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

use security::{Descriptor, User, bool_result};

pub type ServerStream = tokio::net::windows::named_pipe::NamedPipeServer;
pub type ClientStream = tokio::net::windows::named_pipe::NamedPipeClient;

pub fn default_runtime() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|p| !p.is_empty()) {
        PathBuf::from(local).join("ScreenAnswer")
    } else if let Some(home) = std::env::var_os("USERPROFILE").filter(|p| !p.is_empty()) {
        PathBuf::from(home)
            .join("AppData")
            .join("Local")
            .join("ScreenAnswer")
    } else {
        // prepare_runtime still checks ownership and atomically applies a
        // private ACL; an existing directory belonging to someone else fails.
        std::env::temp_dir().join("ScreenAnswer")
    }
}

/// Deliberately no registry, console-policy or system-setting changes.
pub fn initialize() {}

fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut value: Vec<u16> = value.encode_wide().collect();
    ensure!(!value.contains(&0), "Windows path contains NUL");
    value.push(0);
    Ok(value)
}

/// Create/open without following the final component, and without allowing it
/// to be renamed/deleted while this handle is held. SECURITY_ATTRIBUTES never
/// makes the handle inheritable.
fn open_handle(
    path: &Path,
    access: u32,
    disposition: u32,
    flags: u32,
    descriptor: Option<&Descriptor>,
) -> Result<File> {
    let path = wide(path.as_os_str())?;
    let attributes = descriptor.map(Descriptor::attributes).transpose()?;
    let attributes = attributes
        .as_ref()
        .map_or(ptr::null(), |a| a as *const SECURITY_ATTRIBUTES);
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            attributes,
            disposition,
            flags | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn checked_info(file: &File, directory: bool) -> Result<BY_HANDLE_FILE_INFORMATION> {
    ensure!(
        unsafe { GetFileType(file.as_raw_handle()) } == FILE_TYPE_DISK,
        "Runtime object is not a disk file/directory"
    );
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
    unsafe { bool_result(GetFileInformationByHandle(file.as_raw_handle(), &mut info)) }
        .context("Inspect runtime object")?;
    ensure!(
        info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "Refusing a symlink/reparse-point runtime object"
    );
    ensure!(
        (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) == directory,
        "Unexpected runtime object type"
    );
    if !directory {
        ensure!(
            info.nNumberOfLinks == 1,
            "Refusing a hard-linked private file"
        );
    }
    Ok(info)
}

/// Obtain the normalized name of the *checked handle*, not a second path lookup.
fn canonical_name(file: &File) -> Result<PathBuf> {
    let mut buffer = vec![0u16; 256];
    loop {
        let capacity = u32::try_from(buffer.len()).context("Runtime path too long")?;
        let length = unsafe {
            GetFinalPathNameByHandleW(file.as_raw_handle(), buffer.as_mut_ptr(), capacity, 0)
        };
        ensure!(
            length != 0,
            "Cannot canonicalize runtime: {}",
            io::Error::last_os_error()
        );
        if length < capacity {
            buffer.truncate(length as usize);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        buffer.resize(
            usize::try_from(length)?
                .checked_add(1)
                .context("Runtime path too long")?,
            0,
        );
    }
}

struct Directory {
    _handle: File,
    path: PathBuf,
}

fn absolute_object_path(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path).context("Resolve runtime path")?;
    let mut clean = PathBuf::new();
    // Strip trailing separators and `.` before opening with OPEN_REPARSE_POINT:
    // otherwise a destination spelled `junction\\.` could select its target.
    // Ordinary Windows paths have `..` resolved by absolute(); reject remaining
    // verbatim `..` rather than give it ambiguous object-manager semantics.
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => anyhow::bail!("Unresolved parent component in runtime path"),
            _ => clean.push(component.as_os_str()),
        }
    }
    Ok(clean)
}

fn secure_directory(path: &Path, user: &User) -> Result<Directory> {
    let path = absolute_object_path(path)?;
    let file = open_handle(
        &path,
        READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS,
        None,
    )
    .context("Open runtime directory without following reparse points")?;
    checked_info(&file, true)?;
    // Never take ownership or "repair" an object owned by a different user.
    user.require_owner(file.as_raw_handle())?;
    Descriptor::private(&user.sid_text, true)?.protect(file.as_raw_handle())?;
    let path = canonical_name(&file)?;
    Ok(Directory {
        _handle: file,
        path,
    })
}

pub fn prepare_runtime(path: &Path) -> Result<PathBuf> {
    let user = User::current()?;
    let absolute = absolute_object_path(path)?;
    // Only the final directory is the private runtime. Existing ancestors may
    // have the normal Windows user-profile ACLs and are not modified.
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).context("Create runtime parent directories")?;
    }
    let descriptor = Descriptor::private(&user.sid_text, true)?;
    let attributes = descriptor.attributes()?;
    let name = wide(absolute.as_os_str())?;
    if unsafe { CreateDirectoryW(name.as_ptr(), &attributes) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
            return Err(error).context("Create private runtime directory");
        }
    }
    let directory = secure_directory(&absolute, &user)?;
    probe_writable(&directory, &user)?;
    Ok(directory.path)
}

fn probe_writable(directory: &Directory, user: &User) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let descriptor = Descriptor::private(&user.sid_text, false)?;
    for _ in 0..16 {
        let path = directory.path.join(format!(
            ".write-probe-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match open_handle(
            &path,
            GENERIC_WRITE | DELETE,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_DELETE_ON_CLOSE,
            Some(&descriptor),
        ) {
            Ok(mut file) => {
                // DELETE_ON_CLOSE cleans up even on a write error/panic.
                file.write_all(&[0])
                    .context("Runtime directory is not writable")?;
                return Ok(());
            }
            Err(error)
                if error.downcast_ref::<io::Error>().is_some_and(|e| {
                    matches!(e.raw_os_error(), Some(code) if code == ERROR_FILE_EXISTS as i32
                    || code == ERROR_ALREADY_EXISTS as i32)
                }) =>
            {
                continue;
            }
            Err(error) => return Err(error).context("Probe private runtime directory"),
        }
    }
    anyhow::bail!("Cannot create a unique runtime write probe")
}

pub fn open_private_file(path: &Path, truncate: bool, read: bool) -> Result<File> {
    let leaf = path.file_name().context("Private file needs a filename")?;
    ensure!(
        naming::valid_file_name(&leaf.encode_wide().collect::<Vec<_>>()),
        "Invalid private filename (alternate streams are not permitted)"
    );
    let user = User::current()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let directory = secure_directory(parent, &user)?;
    let descriptor = Descriptor::private(&user.sid_text, false)?;
    // OPEN_ALWAYS, not CREATE_ALWAYS: inspect owner/reparse/hard-link status
    // before any truncation, even for an existing malicious file.
    let file = open_handle(
        &directory.path.join(leaf),
        GENERIC_WRITE | READ_CONTROL | WRITE_DAC | if read { GENERIC_READ } else { 0 },
        OPEN_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        Some(&descriptor),
    )
    .context("Open private lock/log file")?;
    checked_info(&file, false)?;
    user.require_owner(file.as_raw_handle())?;
    descriptor.protect(file.as_raw_handle())?;
    if truncate {
        file.set_len(0).context("Truncate private file")?;
    }
    Ok(file)
}

pub fn detach(command: &mut tokio::process::Command) {
    // Do not set DETACHED_PROCESS or replace stdio: the anonymous stdin pipe
    // must survive so the hidden child receives its key without argv/env/files.
    command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
}

pub async fn terminate(child: &mut tokio::process::Child) {
    let _ = child.start_kill(); // TerminateProcess, not a Unix signal.
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
}

pub async fn shutdown_signal() -> Result<()> {
    tokio::signal::ctrl_c().await.context("Wait for Ctrl+C")
}

fn pipe_address(directory: &Directory, user: &User) -> Result<OsString> {
    let path: Vec<u16> = directory.path.as_os_str().encode_wide().collect();
    let input = naming::hash_input(&user.sid_text, &path);
    let mut digest = [0u8; 32];
    // Win10+ CNG pseudo-handle: no algorithm/hash handles need closing.
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            ptr::null(),
            0,
            input.as_ptr(),
            u32::try_from(input.len())?,
            digest.as_mut_ptr(),
            u32::try_from(digest.len())?,
        )
    };
    ensure!(
        status >= 0,
        "Cannot hash local IPC name (NTSTATUS {status:#x})"
    );
    Ok(naming::pipe_name(&digest).into())
}

fn create_server(name: &OsStr, sid: &str, first: bool) -> Result<ServerStream> {
    let descriptor = Descriptor::private(sid, false)?;
    let mut attributes = descriptor.attributes()?;
    // The native API copies this descriptor during CreateNamedPipeW; its
    // allocation and the attributes may be dropped after this call.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                name,
                (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
            )
    }
    .context("Create private local control pipe (name may already be in use)")
}

pub struct Listener {
    name: OsString,
    sid: String,
    pending: ServerStream,
    // Keep the checked directory pinned for the lifetime of the listener.
    _directory: Directory,
}

impl Listener {
    pub fn bind(runtime: &Path) -> Result<Self> {
        let user = User::current()?;
        let directory = secure_directory(runtime, &user)?;
        let name = pipe_address(&directory, &user)?;
        // Fail closed if *any* instance already has this name: never attach a
        // legitimate listener to an attacker's existing pipe namespace.
        let pending = create_server(&name, &user.sid_text, true)?;
        Ok(Self {
            name,
            sid: user.sid_text,
            pending,
            _directory: directory,
        })
    }

    pub async fn accept(&mut self) -> Result<ServerStream> {
        self.pending
            .connect()
            .await
            .context("Accept local control connection")?;
        // No gap: the connected server remains owned by us until its replacement
        // exists. Also retain it on replacement failure. connect() is cancellation
        // safe, so select! cancellation never removes the listening instance.
        let replacement = create_server(&self.name, &self.sid, false)?;
        Ok(mem::replace(&mut self.pending, replacement))
    }
}

fn verify_server(stream: &ClientStream, user: &User) -> Result<()> {
    let pipe = stream.as_raw_handle();
    let mut pid = 0;
    unsafe { bool_result(GetNamedPipeServerProcessId(pipe, &mut pid)) }
        .context("Identify control pipe server")?;
    ensure!(pid != 0, "Control pipe server has no process ID");
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    ensure!(
        !process.is_null(),
        "Cannot inspect control pipe server: {}",
        io::Error::last_os_error()
    );
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    let server_user = User::for_process(process.as_raw_handle())?;
    ensure!(
        user.is_same(&server_user),
        "Control pipe server belongs to another Windows user"
    );
    // Fail closed if the server disappeared/changed while obtaining its token.
    let mut checked_pid = 0;
    unsafe { bool_result(GetNamedPipeServerProcessId(pipe, &mut checked_pid)) }?;
    ensure!(
        pid == checked_pid,
        "Control pipe server changed during verification"
    );
    Ok(())
}

pub async fn connect(runtime: &Path) -> Result<ClientStream> {
    let user = User::current()?;
    let directory = secure_directory(runtime, &user)?;
    let name = pipe_address(&directory, &user)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        // Identification permits identity checks but not impersonation with the
        // caller's privileges, even if a fake same-user server owns this name.
        match ClientOptions::new()
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(&name)
        {
            Ok(stream) => {
                verify_server(&stream, &user)?;
                return Ok(stream);
            }
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => return Err(error).context("Connect to private local control pipe"),
        }
    }
}

#[cfg(test)]
#[path = "system/tests.rs"]
mod tests;
