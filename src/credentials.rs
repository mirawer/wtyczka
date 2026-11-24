//! Native credentials, addressed only by a hash of the complete configuration scope.
//! No secret is part of an identifier, error, environment variable, or command line.
use anyhow::{Result, bail};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use zeroize::Zeroizing;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as native;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as native;

const MAX_KEY_LEN: usize = 4096;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(20);
const TARGET_PREFIX: &str = "screen-answer:v1:";

/// Validate without including the rejected input in an error. Whitespace is not
/// trimmed: the bytes saved must be exactly the bytes used by the API client.
pub fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > MAX_KEY_LEN {
        bail!("API key must contain between 1 and 4096 printable ASCII characters.");
    }
    if !key.bytes().all(|b| (b' '..=b'~').contains(&b)) || key.bytes().all(|b| b == b' ') {
        bail!("API key must contain nonblank printable ASCII characters only.");
    }
    Ok(())
}

/// 256 bits from the OS CSPRNG; independent of the key and public configuration.
pub fn new_id() -> Result<String> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random)
        .map_err(|_| anyhow::anyhow!("Cannot generate credential identifier."))?;
    Ok(hex(&random))
}

fn validate_id(id: &str) -> Result<()> {
    // Accept the configuration format, but emit exactly 64 lowercase hex digits.
    if !(32..=128).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Invalid credential identifier.");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}

fn normalized_base(base: &str) -> Result<String> {
    // URL handles hostname case, default ports and dot-segments. Match the API's
    // pop_if_empty(): ONE terminal slash is equivalent, not repeated slashes.
    let mut url = crate::api::validate_base_url(base)
        .map_err(|_| anyhow::anyhow!("Invalid credential API endpoint."))?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("Invalid credential API endpoint."))?
        .pop_if_empty();
    Ok(url.to_string())
}

fn scoped_target(
    canonical_runtime: &Path,
    provider: crate::api::Provider,
    base: &str,
    id: &str,
) -> Result<String> {
    validate_id(id)?;
    let provider: &[u8] = match provider {
        crate::api::Provider::Anthropic => b"anthropic",
        crate::api::Provider::Openai => b"openai",
        crate::api::Provider::Openrouter => b"openrouter",
        crate::api::Provider::Gemini => b"gemini",
    };
    let normalized = normalized_base(base)?;
    let mut hash = Sha256::new();
    hash.update(b"screen-answer credential scope v1\0");
    // Length-framing prevents concatenation ambiguities. Lossless path bytes
    // avoid collisions between non-UTF8 paths; scopes are deliberately OS-local.
    for field in [
        canonical_runtime.as_os_str().as_encoded_bytes(),
        provider,
        normalized.as_bytes(),
        id.as_bytes(),
    ] {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    Ok(format!("{TARGET_PREFIX}{}", hex(&hash.finalize())))
}

async fn target(runtime: &Path, settings: &crate::Settings, id: &str) -> Result<String> {
    validate_id(id)?;
    let canonical = tokio::fs::canonicalize(runtime)
        .await
        .map_err(|_| anyhow::anyhow!("Cannot resolve credential runtime directory."))?;
    scoped_target(&canonical, settings.provider, &settings.base_url, id)
}

async fn bounded<T>(operation: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(OPERATION_TIMEOUT, operation)
        .await
        .map_err(|_| anyhow::anyhow!("Native credential operation timed out."))?
}

/// Store a fresh identifier. Existing targets are rejected, never replaced.
/// The caller holds the runtime lock and commits the ID only after success.
/// On failure/timeout, delete the pending ID: a native write may have completed.
pub async fn save(runtime: &Path, settings: &crate::Settings, id: &str, key: &str) -> Result<()> {
    validate_key(key)?;
    bounded(async {
        let target = target(runtime, settings, id).await?;
        native::save(&target, key).await
    })
    .await
}

pub async fn load(
    runtime: &Path,
    settings: &crate::Settings,
    id: &str,
) -> Result<Option<Zeroizing<String>>> {
    bounded(async {
        let target = target(runtime, settings, id).await?;
        native::load(&target).await
    })
    .await
}

/// Missing entries are success. Only entries matching the entire scope are touched.
pub async fn delete(runtime: &Path, settings: &crate::Settings, id: &str) -> Result<()> {
    bounded(async {
        let target = target(runtime, settings, id).await?;
        native::delete(&target).await
    })
    .await
}

fn decode_key(bytes: &[u8]) -> Result<Zeroizing<String>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("Stored native credential is invalid."))?;
    validate_key(text).map_err(|_| anyhow::anyhow!("Stored native credential is invalid."))?;
    let mut key = Zeroizing::new(String::new());
    key.try_reserve_exact(text.len())
        .map_err(|_| anyhow::anyhow!("Cannot allocate credential buffer."))?;
    key.push_str(text);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Provider;

    const ID: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn scope(path: &str, provider: Provider, base: &str, id: &str) -> String {
        scoped_target(Path::new(path), provider, base, id).unwrap()
    }

    #[test]
    fn scope_binds_runtime_provider_endpoint_and_identifier_not_model() {
        let original = scope("/runtime/a", Provider::Openai, "https://example.com/v1", ID);
        assert_eq!(original.len(), TARGET_PREFIX.len() + 64);
        assert_eq!(
            original,
            "screen-answer:v1:6e20726a550cd451d81fbb0872f77ea5e1c8b2974dae356bb2404bd42c61554e"
        );
        assert_eq!(
            original,
            scope(
                "/runtime/a",
                Provider::Openai,
                "https://EXAMPLE.COM:443/v1/",
                ID
            )
        );
        for different in [
            scope("/runtime/b", Provider::Openai, "https://example.com/v1", ID),
            scope(
                "/runtime/a",
                Provider::Openrouter,
                "https://example.com/v1",
                ID,
            ),
            scope("/runtime/a", Provider::Openai, "https://example.com/v2", ID),
            scope(
                "/runtime/a",
                Provider::Openai,
                "https://other.example/v1",
                ID,
            ),
            scope(
                "/runtime/a",
                Provider::Openai,
                "https://example.com:444/v1",
                ID,
            ),
            scope(
                "/runtime/a",
                Provider::Openai,
                "https://example.com/v1//",
                ID,
            ),
            scope(
                "/runtime/a",
                Provider::Openai,
                "https://example.com/v1",
                &"ab".repeat(32),
            ),
        ] {
            assert_ne!(original, different);
        }
        assert_eq!(
            original,
            scope("/runtime/a", Provider::Openai, "https://example.com/v1", ID)
        );
        assert!(!original.contains("example.com"));
    }

    #[test]
    fn scope_rejects_invalid_inputs_without_quoting_them() {
        for id in ["", "g123", "../../secret", &"a".repeat(129)] {
            assert!(
                scoped_target(Path::new("/a"), Provider::Openai, "https://example.com", id)
                    .is_err()
            );
        }
        for base in [
            "not a URL",
            "https://user:private@example.com",
            "https://example.com/?secret=private",
            "http://example.com",
        ] {
            let error = scoped_target(Path::new("/a"), Provider::Openai, base, ID).unwrap_err();
            assert_eq!(error.to_string(), "Invalid credential API endpoint.");
        }
        assert!(validate_id(&"A".repeat(32)).is_ok());
        assert!(validate_id(&"a".repeat(128)).is_ok());
    }

    #[test]
    fn validates_bounded_printable_ascii_without_echoing() {
        assert!(validate_key("synthetic-key_123").is_ok());
        assert!(validate_key(&"x".repeat(4096)).is_ok());
        for value in [
            "",
            "   ",
            "sk\nprivate",
            "\0",
            "é",
            "\u{7f}",
            &"x".repeat(4097),
        ] {
            assert!(validate_key(value).is_err());
        }
        assert_eq!(decode_key(b"synthetic").unwrap().as_str(), "synthetic");
        assert_eq!(
            decode_key(b"private\n").unwrap_err().to_string(),
            "Stored native credential is invalid."
        );
    }

    #[tokio::test]
    async fn runtime_scope_is_canonical_and_resolution_errors_are_sanitized() {
        struct TempRuntime(std::path::PathBuf);
        impl Drop for TempRuntime {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root = TempRuntime(std::env::temp_dir().join(format!(
            "screen-answer-credential-scope-test-{}",
            new_id().unwrap()
        )));
        std::fs::create_dir(&root.0).unwrap();
        let settings = crate::Settings {
            provider: Provider::Openai,
            model: "synthetic-model".into(),
            base_url: "https://example.com/v1".into(),
            keybind: "Print".into(),
            pause_keybind: None,
            mode: Default::default(),
            selection: Default::default(),
        };
        let original = target(&root.0, &settings, ID).await.unwrap();
        assert_eq!(
            original,
            target(&root.0.join("."), &settings, ID).await.unwrap()
        );
        #[cfg(target_os = "linux")]
        {
            let alias = root.0.join("alias");
            std::os::unix::fs::symlink(&root.0, &alias).unwrap();
            assert_eq!(original, target(&alias, &settings, ID).await.unwrap());
        }
        let error = target(&root.0.join("missing-private-path"), &settings, ID)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Cannot resolve credential runtime directory."
        );
    }

    #[test]
    fn identifiers_are_256_bit_lowercase_hex() {
        let first = new_id().unwrap();
        let second = new_id().unwrap();
        assert_eq!(first.len(), 64);
        assert!(
            first
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_ne!(first, second);
    }
}
