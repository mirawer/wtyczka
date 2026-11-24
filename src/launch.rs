//! Interactive first setup and prompt-free starts. No API request is made here.
use crate::{
    Cli, Settings, Startup, api, client, config, credentials, keybind, lock, platform, prompt,
    secret, setup,
};
use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::timeout,
};
use zeroize::Zeroizing;

fn launcher_lock(runtime: &Path) -> Result<File> {
    let file = platform::open_private_file(&runtime.join("launcher.lock"), false, true)?;
    file.try_lock()
        .context("Uruchamianie lub zmiana konfiguracji już trwa.")?;
    Ok(file)
}

fn overrides(cli: &Cli) -> bool {
    cli.provider.is_some()
        || cli.model.is_some()
        || cli.base_url.is_some()
        || cli.keybind.is_some()
        || cli.pause_keybind.is_some()
        || cli.mode.is_some()
        || cli.selection.is_some()
}

async fn is_running(runtime: &Path) -> Result<bool> {
    match lock(runtime) {
        Ok(file) => {
            drop(file);
            Ok(false)
        }
        Err(error) => {
            if client(runtime, "status").await.is_ok() {
                Ok(true)
            } else {
                Err(error)
            }
        }
    }
}

pub(crate) async fn start(cli: Cli, runtime: PathBuf) -> Result<()> {
    let _launch = launcher_lock(&runtime)?;
    let running = is_running(&runtime).await?;
    if running && !cli.change && !overrides(&cli) && !cli.session_only && cli.timeout.is_none() {
        println!("Program już działa w tle. Zmiana konfiguracji: --change; zakończenie: stop.");
        return Ok(());
    }
    let saved = match config::load(&runtime) {
        Ok(saved) => saved,
        Err(_) if cli.change => {
            eprintln!(
                "Poprzednia konfiguracja jest nieprawidłowa. Zostanie zastąpiona dopiero po udanym starcie."
            );
            None
        }
        Err(error) => return Err(error),
    };
    let configure = cli.change
        || cli.session_only
        || overrides(&cli)
        || saved.is_none()
        || saved.as_ref().is_some_and(|s| s.credential_id.is_none());
    let mut settings = if configure {
        setup::configure(
            &cli,
            saved.as_ref().map(|s| &s.settings),
            &mut prompt,
            &mut io::stdout(),
        )?
    } else {
        saved
            .as_ref()
            .context("Brak konfiguracji.")?
            .settings
            .clone()
    };
    setup::validate_saved(&settings)
        .map_err(|_| anyhow::anyhow!("Nieprawidłowe zapisane ustawienia. Użyj --change."))?;
    let owned = if running {
        let mut current = saved
            .as_ref()
            .map(|s| s.settings.clone())
            .unwrap_or_else(|| settings.clone());
        if let Some((trigger, pause)) = running_bindings(&runtime).await {
            current.keybind = trigger;
            current.pause_keybind = pause;
        }
        Some(current)
    } else {
        None
    };
    check_hotkeys(
        &cli,
        &mut settings,
        configure || cli.session_only,
        owned.as_ref(),
    )?;
    let os_startup = platform::prepare_start(&cli, &runtime)?;
    api::endpoint_url(settings.provider, &settings.model, &settings.base_url)?;
    let seconds = cli
        .timeout
        .or_else(|| saved.as_ref().map(|s| s.timeout))
        .unwrap_or(config::DEFAULT_TIMEOUT);
    let reuse = if configure || cli.session_only {
        None
    } else {
        saved.as_ref().and_then(|s| s.credential_id.clone())
    };
    let key = if let Some(id) = &reuse {
        credentials::load(&runtime, &settings, id)
            .await?
            .context("Zapisany klucz nie istnieje. Uruchom --change, aby podać go ponownie.")?
    } else {
        println!(
            "Dostawca: {:?}; model: {}; adres: {}",
            settings.provider, settings.model, settings.base_url
        );
        println!(
            "Po skrócie {} cały aktywny monitor trafi do tego API.",
            settings.keybind
        );
        if cli.session_only {
            println!(
                "Klucz tylko na tę sesję. Istniejący zapisany klucz i jego konfiguracja nie będą zmieniane."
            );
        } else {
            println!(
                "Klucz zostanie zapisany w systemowym magazynie poświadczeń, nie w pliku ustawień."
            );
        }
        if settings.provider == api::Provider::Anthropic {
            println!(
                "Limit wyjścia Claude: {} tokenów łącznie na myślenie i odpowiedź. Tokeny myślenia również są płatne.",
                api::ANTHROPIC_MAX_TOKENS
            );
        }
        println!(
            "Tryb odpowiedzi: {}. Liczba poprawnych odpowiedzi: {}.",
            settings.mode.description(),
            settings.selection.description()
        );
        if let Some(pause) = &settings.pause_keybind {
            println!("Pauza/wznowienie: {pause}. Pauza czyści wynik do myślnika (-).");
        }
        println!("\n7/7 — Teraz wklej klucz API. Wpisywane znaki nie będą widoczne.");
        loop {
            match secret::prompt_key("Klucz API (ukryty): ") {
                Ok(key) => break key,
                Err(error) if error.is::<secret::InvalidKey>() => {
                    eprintln!("{error}\nSpróbuj ponownie. Wpisywane znaki pozostają ukryte.")
                }
                Err(error) => return Err(error),
            }
        }
    };
    // A cancelled wizard leaves the old worker and saved configuration alone.
    if running {
        stop(&runtime).await?;
    }
    let startup = Startup {
        settings,
        timeout: seconds,
        platform: os_startup,
        save_key: !cli.session_only && reuse.is_none(),
        preserve_saved: cli.session_only
            && saved
                .as_ref()
                .is_some_and(|s| s.credential_id.is_some() || !s.retired_credentials.is_empty()),
        credential_id: reuse,
    };
    if cli.foreground {
        crate::run(runtime, startup, key, false, Some(_launch)).await
    } else {
        detach(&runtime, startup, key).await
    }
}

async fn running_bindings(runtime: &Path) -> Option<(String, Option<String>)> {
    // Metadata only, never a key or answer. Older workers return ERROR; fall back
    // to saved bindings. New workers can have temporary --session-only settings.
    timeout(Duration::from_secs(2), async {
        let mut stream = platform::ipc::connect(runtime).await.ok()?;
        stream.write_all(b"bindings\n").await.ok()?;
        let mut line = Vec::new();
        BufReader::new(stream)
            .take(1024)
            .read_until(b'\n', &mut line)
            .await
            .ok()?;
        serde_json::from_slice(&line).ok()
    })
    .await
    .ok()
    .flatten()
}

fn owned_key(key: &str, owned: Option<&Settings>) -> bool {
    owned.is_some_and(|s| {
        std::iter::once(s.keybind.as_str())
            .chain(s.pause_keybind.as_deref())
            .any(|old| platform::same_keybind(key, old).unwrap_or(false))
    })
}

fn check_available(key: &str, owned: Option<&Settings>) -> Result<()> {
    if owned_key(key, owned) {
        Ok(())
    } else {
        platform::check_keybind_available(key)
    }
}

fn check_hotkeys(
    cli: &Cli,
    settings: &mut Settings,
    interactive: bool,
    owned: Option<&Settings>,
) -> Result<()> {
    loop {
        match check_available(&settings.keybind, owned).and_then(|()| {
            platform::validate_keybind_pair(&settings.keybind, settings.pause_keybind.as_deref())
        }) {
            Ok(()) => break,
            Err(error) => {
                if !interactive || cli.keybind.is_some() {
                    return Err(error)
                        .context("Skrót jest niedostępny. Użyj --change, aby wybrać inny.");
                }
                eprintln!("Nie można użyć wybranego skrótu: {error}");
                loop {
                    let input = prompt("Inny skrót [Ctrl+Shift+F8]: ")?;
                    let input = if input.is_empty() {
                        "Ctrl+Shift+F8"
                    } else {
                        &input
                    };
                    match keybind::normalize_keybind(input).and_then(|key| {
                        platform::validate_keybind_pair(&key, settings.pause_keybind.as_deref())?;
                        Ok(key)
                    }) {
                        Ok(key) => {
                            settings.keybind = key;
                            break;
                        }
                        Err(_) => eprintln!("Nieprawidłowy skrót. Wpisz np. Ctrl+Shift+F8."),
                    }
                }
            }
        }
    }
    while let Some(pause) = &settings.pause_keybind {
        match check_available(pause, owned) {
            Ok(()) => break,
            Err(error) => {
                if !interactive || cli.pause_keybind.is_some() {
                    return Err(error).context("Skrót pauzy jest niedostępny. Użyj --change.");
                }
                eprintln!("Nie można użyć skrótu pauzy: {error}");
                loop {
                    let input = prompt("Inny skrót pauzy [brak]: ")?;
                    match setup::configured_pause_keybind(&input, &settings.keybind) {
                        Ok(key) => {
                            settings.pause_keybind = key;
                            break;
                        }
                        Err(_) => eprintln!(
                            "Nieprawidłowy lub powtórzony skrót. Wpisz np. Ctrl+Shift+F9 albo brak."
                        ),
                    }
                }
            }
        }
    }
    Ok(())
}

async fn detach(runtime: &Path, startup: Startup, key: Zeroizing<String>) -> Result<()> {
    let log = platform::open_private_file(&runtime.join("daemon.log"), true, false)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--runtime-dir")
        .arg(runtime)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log));
    platform::detach(&mut command);
    let mut child = command
        .spawn()
        .context("Nie można uruchomić procesu w tle.")?;
    let mut input = child.stdin.take().context("Brak stdin procesu.")?;
    input.write_all(key.as_bytes()).await?;
    input.write_all(b"\n").await?;
    input
        .write_all(serde_json::to_string(&startup)?.as_bytes())
        .await?;
    input.write_all(b"\n").await?;
    drop(input);
    drop(key);
    let mut output = BufReader::new(child.stdout.take().context("Brak stdout procesu.")?);
    let mut response = String::new();
    // First setup may need an OS keyring unlock; keys never travel in argv/env.
    let ready = timeout(Duration::from_secs(60), output.read_line(&mut response)).await;
    if !matches!(ready, Ok(Ok(_))) || response.trim() != "READY" {
        platform::terminate(&mut child).await;
        let _ = timeout(Duration::from_secs(5), child.wait()).await;
        bail!(
            "Start nie powiódł się. Sprawdź {} (bez kluczy i treści odpowiedzi). Poprzednia konfiguracja powinna pozostać dostępna.",
            runtime.join("daemon.log").display()
        );
    }
    println!(
        "Działa w tle. Odpowiedź tylko na pasku. Zmiana: --change; zakończenie: screen-answer stop"
    );
    Ok(())
}

/// STOP acknowledges UI cleanup; an older worker can still be dropping IPC/locks.
pub(crate) async fn stop(runtime: &Path) -> Result<()> {
    client(runtime, "stop").await?;
    timeout(Duration::from_secs(5), async {
        loop {
            match lock(runtime) {
                Ok(file) => {
                    drop(file);
                    return;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
    })
    .await
    .context("Proces nie zwolnił jeszcze blokady; ponów polecenie.")?;
    Ok(())
}

pub(crate) async fn forget_key(runtime: &Path) -> Result<()> {
    let _launch = launcher_lock(runtime)?;
    if is_running(runtime).await? {
        stop(runtime).await?;
    }
    let _worker = lock(runtime)?;
    let Some(mut saved) = config::load(runtime)? else {
        println!("Nie ma zapisanej konfiguracji ani przypisanego klucza.");
        return Ok(());
    };
    saved.retire_active();
    let pending = std::mem::take(&mut saved.retired_credentials);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    for old in pending {
        let context = old.settings(&saved.settings);
        if !matches!(
            tokio::time::timeout_at(deadline, credentials::delete(runtime, &context, &old.id))
                .await,
            Ok(Ok(()))
        ) {
            saved.retired_credentials.push(old);
        }
    }
    let mut transaction = config::Transaction::install(runtime, &saved)?;
    transaction.commit();
    if !saved.retired_credentials.is_empty() {
        bail!(
            "Program zatrzymany i automatyczne użycie klucza wyłączone, ale nie wszystkie wpisy usunięto z magazynu. Odblokuj magazyn i ponów forget-key."
        );
    }
    println!(
        "Zapisane klucze tej konfiguracji usunięte; program zatrzymany. Niesekretne ustawienia pozostają."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn running_worker_shortcuts_are_exempt_from_its_own_availability_probe() {
        let owned = Settings {
            provider: api::Provider::Anthropic,
            model: "synthetic-model".into(),
            base_url: api::Provider::Anthropic.default_base().into(),
            keybind: "Ctrl+Shift+F8".into(),
            pause_keybind: Some("Ctrl+Shift+F9".into()),
            mode: crate::mode::AnswerMode::Digits,
            selection: crate::mode::SelectionMode::Single,
        };
        assert!(owned_key("Control+Shift+F8", Some(&owned)));
        assert!(owned_key("Shift+Ctrl+F9", Some(&owned)));
        assert!(!owned_key("Ctrl+F8", Some(&owned)));
        assert!(!owned_key("Ctrl+Shift+F8", None));
        check_available("Ctrl+Shift+F8", Some(&owned)).unwrap();
        check_available("Ctrl+Shift+F9", Some(&owned)).unwrap();
    }

    #[test]
    fn only_explicit_setting_flags_request_a_wizard() {
        assert!(!overrides(&Cli::try_parse_from(["screen-answer"]).unwrap()));
        assert!(!overrides(
            &Cli::try_parse_from(["screen-answer", "--timeout", "90"]).unwrap()
        ));
        assert!(overrides(
            &Cli::try_parse_from(["screen-answer", "--model", "vision-model"]).unwrap()
        ));
        assert!(
            Cli::try_parse_from(["screen-answer", "--change"])
                .unwrap()
                .change
        );
        assert!(
            Cli::try_parse_from(["screen-answer", "--session-only"])
                .unwrap()
                .session_only
        );
    }
}
