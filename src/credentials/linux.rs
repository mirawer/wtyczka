//! Secret Service with an encrypted DH session, never a plaintext fallback.
//! We do not create collections or request Unlock. The user's existing default
//! collection must already be unlocked. A provider may request consent for a
//! user-driven create/delete; the outer operation deadline still applies.
use anyhow::{Result, bail};
use secret_service::{EncryptionType, SecretService};
use std::collections::HashMap;
use zeroize::Zeroizing;

const APPLICATION: &str = "screen-answer";

fn attributes(target: &str) -> HashMap<&str, &str> {
    HashMap::from([
        ("application", APPLICATION),
        ("screen-answer-scope-v1", target),
    ])
}

async fn connect() -> Result<SecretService<'static>> {
    SecretService::connect(EncryptionType::Dh)
        .await
        .map_err(|_| anyhow::anyhow!("Cannot open encrypted native keyring session. Unlock the OS keyring or use session-only mode."))
}

pub(super) async fn save(target: &str, key: &str) -> Result<()> {
    super::validate_key(key)?;
    let service = connect().await?;
    let existing = service
        .search_items(attributes(target))
        .await
        .map_err(|_| anyhow::anyhow!("Cannot search native credentials."))?;
    if !existing.unlocked.is_empty() || !existing.locked.is_empty() {
        bail!("Native credential identifier already exists.");
    }
    let collection = service.get_default_collection().await.map_err(|_| {
        anyhow::anyhow!("No default native keyring is available. Unlock the OS keyring or use session-only mode.")
    })?;
    collection.ensure_unlocked().await.map_err(|_| {
        anyhow::anyhow!("Native keyring is locked or unavailable. Unlock the OS keyring or use session-only mode.")
    })?;
    // Never replace: fresh random IDs let the caller commit configuration only
    // after creation succeeds, without overwriting a previously working entry.
    collection
        .create_item(
            "Screen Answer API credential",
            attributes(target),
            key.as_bytes(),
            false,
            "text/plain; charset=utf-8",
        )
        .await
        .map_err(|_| anyhow::anyhow!("Cannot save native credential."))?;
    Ok(())
}

pub(super) async fn load(target: &str) -> Result<Option<Zeroizing<String>>> {
    let service = connect().await?;
    let items = service
        .search_items(attributes(target))
        .await
        .map_err(|_| anyhow::anyhow!("Cannot search native credentials."))?;
    if !items.locked.is_empty() {
        bail!("Native credential is locked. Unlock the OS keyring or use session-only mode.");
    }
    if items.unlocked.len() > 1 {
        bail!("Native credential identifier is ambiguous.");
    }
    let Some(item) = items.unlocked.first() else {
        return Ok(None);
    };
    item.ensure_unlocked()
        .await
        .map_err(|_| anyhow::anyhow!("Native credential is locked or unavailable."))?;
    // Immediately guard the library-owned plaintext after ownership transfer.
    // The mature library performs DH/AES; no application crypto or plain mode.
    let bytes = Zeroizing::new(
        item.get_secret()
            .await
            .map_err(|_| anyhow::anyhow!("Cannot read native credential."))?,
    );
    super::decode_key(&bytes).map(Some)
}

pub(super) async fn delete(target: &str) -> Result<()> {
    let service = connect().await?;
    let items = service
        .search_items(attributes(target))
        .await
        .map_err(|_| anyhow::anyhow!("Cannot search native credentials."))?;
    if !items.locked.is_empty() {
        bail!("Native credential is locked. Unlock the OS keyring before forgetting it.");
    }
    // Only the exact app + scope pair, including the random ID, is ever removed.
    // Deleting all exact matches also repairs duplicates left by an interrupted
    // external writer; no other application's items are enumerated or touched.
    for item in items.unlocked {
        item.ensure_unlocked()
            .await
            .map_err(|_| anyhow::anyhow!("Native credential is locked or unavailable."))?;
        item.delete()
            .await
            .map_err(|_| anyhow::anyhow!("Cannot delete native credential."))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run ONLY with a disposable service on a fresh dbus-run-session bus, an
    /// unlocked disposable default collection, and this explicit opt-in flag.
    /// Never point this test at the desktop/user's bus. Ignored by normal CI.
    #[tokio::test]
    #[ignore = "requires isolated disposable Secret Service; never the desktop keyring"]
    async fn isolated_encrypted_roundtrip_and_cleanup() {
        assert_eq!(
            std::env::var("SCREEN_ANSWER_ISOLATED_SECRET_SERVICE_TEST").as_deref(),
            Ok("1"),
            "Start an isolated Secret Service first; do not use the desktop bus"
        );
        let target = format!(
            "screen-answer:v1:test:{}",
            crate::credentials::new_id().unwrap()
        );
        let result = super::super::bounded(async {
            if load(&target).await?.is_some() {
                bail!("Fresh synthetic target unexpectedly exists.");
            }
            let key = Zeroizing::new("x".repeat(4096));
            save(&target, &key).await?;
            if save(&target, "other-synthetic-key").await.is_ok() {
                bail!("Duplicate synthetic target was not rejected.");
            }
            let loaded = load(&target)
                .await?
                .ok_or_else(|| anyhow::anyhow!("New synthetic credential missing."))?;
            if loaded.len() != 4096 || !loaded.bytes().all(|b| b == b'x') {
                bail!("Synthetic credential did not roundtrip.");
            }
            Ok(())
        })
        .await;
        // Cleanup also runs if the roundtrip returned an error.
        let cleanup = super::super::bounded(delete(&target)).await;
        cleanup.unwrap();
        result.unwrap();
        assert!(
            super::super::bounded(load(&target))
                .await
                .unwrap()
                .is_none()
        );
        super::super::bounded(delete(&target)).await.unwrap();
    }
}
