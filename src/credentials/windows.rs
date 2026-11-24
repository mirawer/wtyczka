//! Per-user Windows Credential Manager generic credentials. LOCAL_MACHINE means
//! persistence across this user's logons on this machine, NOT machine-wide access.
use anyhow::{Result, bail};
use std::ptr;
use windows_sys::Win32::{
    Foundation::{ERROR_NOT_FOUND, GetLastError},
    Security::Credentials::{
        CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW,
        CredDeleteW, CredFree, CredReadW, CredWriteW,
    },
};
use zeroize::{Zeroize, Zeroizing};

/// CredRead owns a single native allocation. Always wipe its secret blob before
/// CredFree, including existence checks, malformed entries and early returns.
struct NativeCredential(*mut CREDENTIALW);
impl Drop for NativeCredential {
    fn drop(&mut self) {
        // SAFETY: constructed only from a successful CredReadW allocation. The
        // documented blob pointer/size describe writable memory within it.
        unsafe {
            let credential = &mut *self.0;
            if !credential.CredentialBlob.is_null() && credential.CredentialBlobSize != 0 {
                std::slice::from_raw_parts_mut(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
                .zeroize();
            }
            CredFree(self.0.cast());
        }
    }
}

fn wide(target: &str) -> Vec<u16> {
    // Targets are an application prefix + SHA256 hex; never secret/user input.
    target.encode_utf16().chain(Some(0)).collect()
}

fn read(target: &[u16]) -> Result<Option<NativeCredential>> {
    let mut pointer = ptr::null_mut();
    // SAFETY: target is NUL terminated and output pointer is initialized.
    if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut pointer) } == 0 {
        if unsafe { GetLastError() } == ERROR_NOT_FOUND {
            return Ok(None);
        }
        bail!("Cannot read native credential.");
    }
    if pointer.is_null() {
        bail!("Native credential response is invalid.");
    }
    Ok(Some(NativeCredential(pointer)))
}

fn save_blocking(target: &str, key: &str) -> Result<()> {
    super::validate_key(key)?;
    if key.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        bail!(
            "Windows Credential Manager supports API keys up to 2560 bytes. Use session-only mode for longer keys."
        );
    }
    let target = wide(target);
    // Caller holds the runtime lock. CredWrite itself has no create-only flag,
    // so reject an existing entry before writing; never deliberately overwrite.
    if read(&target)?.is_some() {
        bail!("Native credential identifier already exists.");
    }
    let mut blob = Zeroizing::new(key.as_bytes().to_vec());
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: target.as_ptr().cast_mut(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    // SAFETY: native call copies the blob synchronously; all referenced buffers
    // remain live. The copied application buffer is zeroized on every exit.
    if unsafe { CredWriteW(&credential, 0) } == 0 {
        bail!("Cannot save native credential.");
    }
    Ok(())
}

fn load_blocking(target: &str) -> Result<Option<Zeroizing<String>>> {
    let Some(native) = read(&wide(target))? else {
        return Ok(None);
    };
    // SAFETY: native allocation is held until after decode copies the ASCII
    // into a Zeroizing<String>. The OS blob is wiped before freeing it.
    let credential = unsafe { &*native.0 };
    if credential.CredentialBlobSize == 0
        || credential.CredentialBlobSize > CRED_MAX_CREDENTIAL_BLOB_SIZE
        || credential.CredentialBlob.is_null()
    {
        bail!("Stored native credential is invalid.");
    }
    let bytes = unsafe {
        std::slice::from_raw_parts(
            credential.CredentialBlob,
            credential.CredentialBlobSize as usize,
        )
    };
    super::decode_key(bytes).map(Some)
}

fn delete_blocking(target: &str) -> Result<()> {
    let target = wide(target);
    // SAFETY: target is NUL terminated. Deletion does not read a secret blob.
    if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0
        && unsafe { GetLastError() } != ERROR_NOT_FOUND
    {
        bail!("Cannot delete native credential.");
    }
    Ok(())
}

/// Blocking Windows calls cannot be cancelled. If the async caller times out
/// while CredWrite is running, dropping the undelivered result schedules removal
/// of that fresh target, rather than leaving a late successful write behind.
struct PendingWrite {
    target: String,
    committed: bool,
}
impl Drop for PendingWrite {
    fn drop(&mut self) {
        if !self.committed {
            let target = self.target.clone();
            let _ = std::thread::Builder::new()
                .name("credential-cleanup".into())
                .spawn(move || {
                    let _ = delete_blocking(&target);
                });
        }
    }
}

pub(super) async fn save(target: &str, key: &str) -> Result<()> {
    // Reject before copying secrets into a worker-owned buffer.
    super::validate_key(key)?;
    if key.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        bail!(
            "Windows Credential Manager supports API keys up to 2560 bytes. Use session-only mode for longer keys."
        );
    }
    let target = target.to_owned();
    let key = Zeroizing::new(key.to_owned());
    let mut pending = tokio::task::spawn_blocking(move || {
        save_blocking(&target, &key)?;
        Ok::<_, anyhow::Error>(PendingWrite {
            target,
            committed: false,
        })
    })
    .await
    .map_err(|_| anyhow::anyhow!("Native credential worker failed."))??;
    // No await between receiving and committing: cancellation cannot interleave.
    pending.committed = true;
    Ok(())
}

pub(super) async fn load(target: &str) -> Result<Option<Zeroizing<String>>> {
    let target = target.to_owned();
    tokio::task::spawn_blocking(move || load_blocking(&target))
        .await
        .map_err(|_| anyhow::anyhow!("Native credential worker failed."))?
}

pub(super) async fn delete(target: &str) -> Result<()> {
    let target = target.to_owned();
    tokio::task::spawn_blocking(move || delete_blocking(&target))
        .await
        .map_err(|_| anyhow::anyhow!("Native credential worker failed."))?
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = delete_blocking(&self.0);
        }
    }

    /// Only fresh random synthetic targets; does not enumerate user entries.
    /// Runs on Windows CI. Never run/claimed to run on the Linux development host.
    #[tokio::test]
    async fn native_synthetic_roundtrip_duplicate_rejection_and_cleanup() {
        let target = format!(
            "screen-answer:v1:test:{}",
            crate::credentials::new_id().unwrap()
        );
        let _cleanup = Cleanup(target.clone());
        assert!(
            super::super::bounded(load(&target))
                .await
                .unwrap()
                .is_none()
        );
        let key = Zeroizing::new("x".repeat(CRED_MAX_CREDENTIAL_BLOB_SIZE as usize));
        super::super::bounded(save(&target, &key)).await.unwrap();
        assert!(
            super::super::bounded(save(&target, "other-synthetic-key"))
                .await
                .is_err()
        );
        let loaded = super::super::bounded(load(&target)).await.unwrap().unwrap();
        assert_eq!(loaded.len(), key.len());
        assert!(loaded.bytes().all(|b| b == b'x'));
        super::super::bounded(delete(&target)).await.unwrap();
        super::super::bounded(delete(&target)).await.unwrap();
        assert!(
            super::super::bounded(load(&target))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn oversized_blob_is_rejected_without_truncation_or_native_write() {
        let target = format!(
            "screen-answer:v1:test:{}",
            crate::credentials::new_id().unwrap()
        );
        let _cleanup = Cleanup(target.clone());
        let error = save(&target, &"x".repeat(2561)).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Windows Credential Manager supports API keys up to 2560 bytes. Use session-only mode for longer keys."
        );
        assert!(
            super::super::bounded(load(&target))
                .await
                .unwrap()
                .is_none()
        );
    }
}
