#![cfg(windows)]

use super::*;
use std::{ffi::c_void, io::Read, process::Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE,
    Authorization::{ConvertStringSidToSidW, GetSecurityInfo, SE_FILE_OBJECT, SetSecurityInfo},
    CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
    GetSecurityDescriptorControl, GetTokenInformation, OBJECT_INHERIT_ACE,
    OWNER_SECURITY_INFORMATION, RevertToSelf, SE_DACL_PROTECTED, SecurityIdentification,
    TOKEN_QUERY, TokenImpersonationLevel,
};
use windows_sys::Win32::{
    Storage::FileSystem::{FILE_ALL_ACCESS, WRITE_OWNER},
    System::{
        Pipes::ImpersonateNamedPipeClient,
        Threading::{GetCurrentThread, OpenThreadToken},
    },
};

fn assert_private(handle: windows_sys::Win32::Foundation::HANDLE, directory: bool) -> Result<()> {
    let user = User::current()?;
    user.require_owner(handle)?;
    let mut acl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let code = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut acl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    let _descriptor = security::Local(descriptor);
    security::win32_result(code)?;
    assert!(!acl.is_null());
    let mut control = 0;
    let mut revision = 0;
    unsafe {
        bool_result(GetSecurityDescriptorControl(
            descriptor,
            &mut control,
            &mut revision,
        ))
    }?;
    assert_ne!(control & SE_DACL_PROTECTED, 0);
    assert_eq!(unsafe { (*acl).AceCount }, 1);
    let mut ace = ptr::null_mut();
    unsafe { bool_result(GetAce(acl, 0, &mut ace)) }?;
    let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
    assert_eq!(ace.Header.AceType, 0); // ACCESS_ALLOWED_ACE_TYPE
    assert_eq!(ace.Mask, FILE_ALL_ACCESS);
    if directory {
        assert_eq!(
            u32::from(ace.Header.AceFlags) & (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE),
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        );
    }
    let sid = (&ace.SidStart as *const u32).cast::<c_void>().cast_mut();
    assert_ne!(unsafe { EqualSid(sid, user.sid()) }, 0);
    Ok(())
}

#[test]
fn runtime_and_private_files_have_only_current_user_acl() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = prepare_runtime(&temp.path().join("runtime"))?;
    assert!(runtime.is_absolute());
    assert_eq!(prepare_runtime(&runtime)?, runtime);
    let user = User::current()?;
    let directory = secure_directory(&runtime, &user)?;
    assert_private(directory._handle.as_raw_handle(), true)?;
    let log = runtime.join("daemon.log");
    {
        let mut file = open_private_file(&log, false, false)?;
        assert_private(file.as_raw_handle(), false)?;
        file.write_all(b"not-a-real-key")?;
    }
    let mut content = String::new();
    open_private_file(&log, false, true)?.read_to_string(&mut content)?;
    assert_eq!(content, "not-a-real-key");
    assert_eq!(open_private_file(&log, true, true)?.metadata()?.len(), 0);
    assert!(open_private_file(&runtime.join("daemon.log:stream"), false, true).is_err());
    assert!(open_private_file(&runtime, false, false).is_err());
    Ok(())
}

#[test]
fn hard_links_are_rejected_before_truncation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = prepare_runtime(&temp.path().join("runtime"))?;
    let original = runtime.join("original");
    std::fs::write(&original, b"unchanged")?;
    let alias = runtime.join("daemon.log");
    std::fs::hard_link(&original, &alias)?;
    assert!(open_private_file(&alias, true, true).is_err());
    assert_eq!(std::fs::read(&original)?, b"unchanged");
    Ok(())
}

#[test]
fn file_and_directory_reparse_points_are_rejected() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = prepare_runtime(&temp.path().join("runtime"))?;
    let target = runtime.join("target");
    std::fs::write(&target, b"unchanged")?;
    let link = runtime.join("daemon.log");
    match std::os::windows::fs::symlink_file(&target, &link) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(1314) => {
            eprintln!("Symlink test requires Developer Mode or SeCreateSymbolicLinkPrivilege");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    }
    assert!(open_private_file(&link, true, true).is_err());
    assert_eq!(std::fs::read(&target)?, b"unchanged");
    let alias = temp.path().join("runtime-link");
    std::os::windows::fs::symlink_dir(&runtime, &alias)?;
    assert!(prepare_runtime(&alias).is_err());
    // Cover a reparse destination with a trailing separator as well.
    let trailing = PathBuf::from(format!("{}\\", alias.display()));
    assert!(prepare_runtime(&trailing).is_err());
    assert!(prepare_runtime(&alias.join(".")).is_err());
    assert!(secure_directory(&alias.join("."), &User::current()?).is_err());
    Ok(())
}

#[test]
fn foreign_owner_is_not_repaired_when_owner_change_is_available() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = prepare_runtime(&temp.path().join("runtime"))?;
    let file = open_handle(
        &runtime,
        READ_CONTROL | WRITE_DAC | WRITE_OWNER,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS,
        None,
    )?;
    let mut administrators = ptr::null_mut();
    let sid = wide(OsStr::new("S-1-5-32-544"))?;
    unsafe { bool_result(ConvertStringSidToSidW(sid.as_ptr(), &mut administrators)) }?;
    let _sid_owner = security::Local(administrators);
    let code = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            administrators,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
        )
    };
    if matches!(code, 5 | 1307 | 1314) {
        eprintln!(
            "Foreign-owner test requires an elevated token allowed to assign Administrators ownership"
        );
        return Ok(());
    }
    security::win32_result(code)?;
    assert!(
        User::current()?
            .require_owner(file.as_raw_handle())
            .is_err()
    );
    assert!(prepare_runtime(&runtime).is_err());
    Ok(())
}

#[tokio::test]
async fn ipc_round_trip_keeps_a_private_instance_alive() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = prepare_runtime(&temp.path().join("runtime"))?;
    let mut listener = Listener::bind(&runtime)?;
    assert_private(listener.pending.as_raw_handle(), false)?;
    assert!(Listener::bind(&runtime).is_err());
    assert!(Listener::bind(&runtime.join(".")).is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    for _ in 0..3 {
        let (server, client) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(listener.accept(), connect(&runtime))
        })
        .await?;
        let (mut server, mut client) = (server?, client?);
        assert!(Listener::bind(&runtime).is_err());
        // Even with an accepted connection alive, a new FIRST instance cannot
        // squat, and a pending replacement is ready before this stream is used.
        assert!(create_server(&listener.name, &listener.sid, true).is_err());
        client.write_all(b"status\n").await?;
        let mut input = [0; 7];
        server.read_exact(&mut input).await?;
        assert_eq!(&input, b"status\n");
        server.write_all(b"ok\n").await?;
        let mut output = [0; 3];
        client.read_exact(&mut output).await?;
        assert_eq!(&output, b"ok\n");
    }
    drop(listener);
    let listener = Listener::bind(&runtime)?;
    drop(listener);
    Ok(())
}

#[tokio::test]
async fn a_preexisting_pipe_name_is_never_reused() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = prepare_runtime(&temp.path().join("runtime"))?;
    let user = User::current()?;
    let directory = secure_directory(&runtime, &user)?;
    let name = pipe_address(&directory, &user)?;
    let _squatter = create_server(&name, &user.sid_text, true)?;
    assert!(Listener::bind(&runtime).is_err());
    Ok(())
}

#[tokio::test]
async fn clients_offer_identification_not_impersonation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = prepare_runtime(&temp.path().join("runtime"))?;
    let mut listener = Listener::bind(&runtime)?;
    let (server, client) = tokio::join!(listener.accept(), connect(&runtime));
    let (mut server, mut client) = (server?, client?);
    client.write_all(b"x").await?;
    server.read_exact(&mut [0]).await?;
    unsafe { bool_result(ImpersonateNamedPipeClient(server.as_raw_handle())) }?;
    struct Revert;
    impl Drop for Revert {
        fn drop(&mut self) {
            unsafe {
                RevertToSelf();
            }
        }
    }
    let _revert = Revert;
    // No await until the thread's impersonation token has been reverted.
    let mut token = ptr::null_mut();
    unsafe {
        bool_result(OpenThreadToken(
            GetCurrentThread(),
            TOKEN_QUERY,
            1,
            &mut token,
        ))
    }?;
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut level = 0i32;
    let mut length = 0;
    unsafe {
        bool_result(GetTokenInformation(
            token.as_raw_handle(),
            TokenImpersonationLevel,
            (&mut level as *mut i32).cast(),
            u32::try_from(mem::size_of::<i32>())?,
            &mut length,
        ))
    }?;
    assert_eq!(level, SecurityIdentification);
    Ok(())
}

#[tokio::test]
async fn hidden_children_preserve_anonymous_piped_stdin() -> Result<()> {
    let windows = std::env::var_os("WINDIR").context("WINDIR is unset")?;
    let mut command =
        tokio::process::Command::new(PathBuf::from(windows).join("System32").join("findstr.exe"));
    command
        .arg("^")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    detach(&mut command);
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().context("Child lost its stdin pipe")?;
    stdin.write_all(b"anonymous-pipe-test\r\n").await?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await??;
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)?.contains("anonymous-pipe-test"));
    Ok(())
}

#[tokio::test]
async fn termination_reaps_a_waiting_hidden_child() -> Result<()> {
    let windows = std::env::var_os("WINDIR").context("WINDIR is unset")?;
    let mut command =
        tokio::process::Command::new(PathBuf::from(windows).join("System32").join("findstr.exe"));
    command
        .arg("^")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    detach(&mut command);
    let mut child = command.spawn()?;
    // Keep stdin open: findstr has no EOF and cannot normally finish by itself.
    terminate(&mut child).await;
    assert!(child.try_wait()?.is_some());
    Ok(())
}

#[test]
fn async_ipc_is_send() {
    fn assert_send<T: Send>(_: T) {}
    fn assert_send_type<T: Send>() {}
    assert_send_type::<Listener>();
    assert_send_type::<ServerStream>();
    assert_send_type::<ClientStream>();
    assert_send(connect(Path::new("unused")));
}
