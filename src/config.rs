//! Non-secret configuration, with atomic replacement and startup rollback.
use crate::{Settings, credentials, platform};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_CONFIG: u64 = 64 * 1024;
pub(crate) const DEFAULT_TIMEOUT: u64 = 60;

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Retired {
    pub provider: crate::api::Provider,
    pub base_url: String,
    pub id: String,
}

impl Retired {
    pub(crate) fn settings(&self, current: &Settings) -> Settings {
        let mut settings = current.clone();
        settings.provider = self.provider;
        settings.base_url = self.base_url.clone();
        settings
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Saved {
    #[serde(flatten)]
    pub settings: Settings,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired_credentials: Vec<Retired>,
}

impl Saved {
    pub(crate) fn retire_active(&mut self) {
        if let Some(id) = self.credential_id.take() {
            let entry = Retired {
                provider: self.settings.provider,
                base_url: self.settings.base_url.clone(),
                id,
            };
            if !self.retired_credentials.contains(&entry) {
                self.retired_credentials.push(entry);
            }
        }
    }
}

pub(crate) async fn cleanup_retired(runtime: &Path, saved: Saved) {
    if saved.retired_credentials.is_empty() {
        return;
    }
    let mut cleaned = saved.clone();
    cleaned.retired_credentials.clear();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    for old in &saved.retired_credentials {
        let settings = old.settings(&saved.settings);
        let deleted =
            tokio::time::timeout_at(deadline, credentials::delete(runtime, &settings, &old.id))
                .await;
        if !matches!(deleted, Ok(Ok(()))) {
            cleaned.retired_credentials.push(old.clone());
        }
    }
    // Do not overwrite changes made outside the worker while cleanup was pending.
    if cleaned != saved && load(runtime).ok().flatten().as_ref() == Some(&saved) {
        match Transaction::install(runtime, &cleaned) {
            Ok(mut transaction) => transaction.commit(),
            Err(_) => {
                eprintln!("Nie udało się zapisać wyniku porządkowania odwołań do starych kluczy.")
            }
        }
    }
    if !cleaned.retired_credentials.is_empty() {
        eprintln!(
            "Stare klucze oczekują na usunięcie z magazynu. Odblokuj magazyn i użyj forget-key, aby usunąć wszystkie zapisane klucze tej konfiguracji."
        );
    }
}

pub(crate) fn load(runtime: &Path) -> Result<Option<Saved>> {
    let Some(bytes) = read_raw(runtime)? else {
        return Ok(None);
    };
    let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| {
        anyhow::anyhow!("Uszkodzona konfiguracja. Użyj --change, aby ustawić program ponownie.")
    })?;
    ensure!(
        (5..=300).contains(&saved.timeout),
        "Nieprawidłowy zapisany limit czasu. Użyj --change."
    );
    ensure!(
        saved.retired_credentials.len() + usize::from(saved.credential_id.is_some()) <= 32,
        "Zbyt wiele odwołań do starych kluczy. Użyj forget-key."
    );
    for id in saved
        .credential_id
        .iter()
        .chain(saved.retired_credentials.iter().map(|old| &old.id))
    {
        ensure!(
            (32..=128).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit()),
            "Nieprawidłowe odwołanie do klucza. Użyj --change."
        );
    }
    Ok(Some(saved))
}

fn read_raw(runtime: &Path) -> Result<Option<Vec<u8>>> {
    let path = runtime.join("settings.json");
    match fs::symlink_metadata(&path) {
        Ok(meta) => ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Konfiguracja musi być zwykłym plikiem."
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("Nie można sprawdzić pliku konfiguracji."),
    }
    let file = platform::open_private_file(&path, false, true)
        .map_err(|_| anyhow::anyhow!("Nie można bezpiecznie otworzyć konfiguracji."))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file() && meta.nlink() == 1 && meta.uid() == unsafe { libc::geteuid() },
            "Nieprawidłowy właściciel lub typ pliku konfiguracji."
        );
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Nie można odczytać konfiguracji."))?;
    ensure!(
        bytes.len() as u64 <= MAX_CONFIG,
        "Plik konfiguracji jest za duży."
    );
    Ok(Some(bytes))
}

fn replace(runtime: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() as u64 <= MAX_CONFIG,
        "Konfiguracja jest za duża."
    );
    let temporary = runtime.join(format!(".settings-{}.tmp", credentials::new_id()?));
    let result: Result<()> = (|| {
        let mut file = platform::open_private_file(&temporary, true, false)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, runtime.join("settings.json"))?;
        #[cfg(unix)]
        if let Ok(directory) = fs::File::open(runtime) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
        .context("Nie można atomowo zapisać konfiguracji; poprzednia nie została celowo usunięta.")
}

/// Keep the old bytes until startup has actually reported readiness. Secrets are
/// never present in either file; a failed startup drops this guard and restores it.
pub(crate) struct Transaction {
    runtime: PathBuf,
    previous: Option<Vec<u8>>,
    active: bool,
}

impl Transaction {
    pub(crate) fn install(runtime: &Path, saved: &Saved) -> Result<Self> {
        let previous = read_raw(runtime)?;
        let bytes = serde_json::to_vec_pretty(saved)?;
        replace(runtime, &bytes)?;
        Ok(Self {
            runtime: runtime.to_owned(),
            previous,
            active: true,
        })
    }
    pub(crate) fn commit(&mut self) {
        self.active = false;
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.active {
            let restored = match &self.previous {
                Some(bytes) => replace(&self.runtime, bytes),
                None => match fs::remove_file(self.runtime.join("settings.json")) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(_) => Err(anyhow::anyhow!(
                        "Nie można usunąć niezatwierdzonej konfiguracji."
                    )),
                },
            };
            if restored.is_err() {
                eprintln!("Nie udało się przywrócić poprzedniej konfiguracji; użyj --change.");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::Provider,
        mode::{AnswerMode, SelectionMode},
    };

    struct Runtime(PathBuf);
    impl Runtime {
        fn new() -> Self {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(".cache/config-tests")
                .join(credentials::new_id().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            Self(platform::prepare_runtime(&path).unwrap())
        }
    }
    impl Drop for Runtime {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn saved(model: &str) -> Saved {
        Saved {
            settings: Settings {
                provider: Provider::Anthropic,
                model: model.into(),
                base_url: Provider::Anthropic.default_base().into(),
                keybind: "Print".into(),
                pause_keybind: None,
                mode: AnswerMode::Digits,
                selection: SelectionMode::Single,
            },
            timeout: 75,
            credential_id: Some("a".repeat(64)),
            retired_credentials: Vec::new(),
        }
    }
    #[test]
    fn legacy_config_and_transaction_commit_and_rollback() {
        let runtime = Runtime::new();
        assert!(load(&runtime.0).unwrap().is_none());
        {
            let _pending = Transaction::install(&runtime.0, &saved("first")).unwrap();
        }
        assert!(load(&runtime.0).unwrap().is_none());
        let mut first = Transaction::install(&runtime.0, &saved("first")).unwrap();
        first.commit();
        drop(first);
        assert_eq!(load(&runtime.0).unwrap().unwrap().settings.model, "first");
        {
            let _pending = Transaction::install(&runtime.0, &saved("second")).unwrap();
        }
        let current = load(&runtime.0).unwrap().unwrap();
        assert_eq!(current.settings.model, "first");
        assert_eq!(current.timeout, 75);
        assert_eq!(
            current.credential_id.as_deref(),
            Some("a".repeat(64).as_str())
        );
        let legacy = serde_json::to_vec(&current.settings).unwrap();
        replace(&runtime.0, &legacy).unwrap();
        let legacy = load(&runtime.0).unwrap().unwrap();
        assert_eq!(legacy.timeout, DEFAULT_TIMEOUT);
        assert!(legacy.credential_id.is_none());
        assert!(
            fs::read_dir(&runtime.0).unwrap().all(|p| !p
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );
    }
    #[test]
    fn invalid_data_is_private_and_never_destroyed_by_a_failed_load() {
        let runtime = Runtime::new();
        let raw = br#"{"private":"test-private-value", broken}"#;
        replace(&runtime.0, raw).unwrap();
        assert!(
            !load(&runtime.0)
                .err()
                .unwrap()
                .to_string()
                .contains("test-private-value")
        );
        assert_eq!(fs::read(runtime.0.join("settings.json")).unwrap(), raw);
        let mut bad = saved("valid");
        bad.timeout = 0;
        let mut tx = Transaction::install(&runtime.0, &bad).unwrap();
        tx.commit();
        assert!(load(&runtime.0).is_err());
    }

    #[cfg(target_os = "linux")]
    struct TestWatcher;
    #[cfg(target_os = "linux")]
    #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
    impl TestWatcher {
        fn register_status_notifier_item(&self, _service: &str) {}
        #[zbus(property)]
        fn is_status_notifier_host_registered(&self) -> bool {
            true
        }
    }

    /// A private-bus test host only, never installed on the desktop session bus.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "helper for tests/persistence.py on a disposable D-Bus session"]
    async fn isolated_test_watcher() {
        let private = std::env::var("SCREEN_ANSWER_PRIVATE_TEST_DIR")
            .expect("private test directory required");
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
        assert!(
            address.contains(&private),
            "Never run the fixture on the desktop bus"
        );
        let _connection = zbus::connection::Builder::session()
            .unwrap()
            .name("org.kde.StatusNotifierWatcher")
            .unwrap()
            .serve_at("/StatusNotifierWatcher", TestWatcher)
            .unwrap()
            .build()
            .await
            .unwrap();
        println!("TEST_WATCHER_READY");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        use tokio::io::AsyncBufReadExt;
        let _ = tokio::io::BufReader::new(tokio::io::stdin())
            .read_line(&mut line)
            .await;
    }
}
