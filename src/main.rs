mod answer;
mod api;
mod config;
mod credentials;
mod display;
mod keybind;
mod launch;
mod mode;
mod platform;
mod secret;
mod setup;

use anyhow::{Context, Result, bail};
use api::{Api, Provider};
use clap::{Parser, Subcommand};
use display::State;
use mode::{AnswerMode, SelectionMode};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    time::timeout,
};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(
    version,
    about = "Jedna lub wiele odpowiedzi: cyfry 1–9 albo kropki 1–6 na pasku. Linux/Niri i Windows."
)]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Katalog stanu (Linux: .runtime projektu lub XDG_STATE_HOME/screen-answer; Windows: LocalAppData/ScreenAnswer)"
    )]
    runtime_dir: Option<PathBuf>,
    #[arg(long, value_enum)]
    provider: Option<Provider>,
    #[arg(long, help = "Identyfikator modelu obsługującego obrazy")]
    model: Option<String>,
    #[arg(
        long,
        help = "Bazowy URL API, np. https://host/v1 (bez /chat/completions)"
    )]
    base_url: Option<String>,
    #[arg(long, help = "Skrót: domyślnie Print; np. Ctrl+Shift+F8")]
    keybind: Option<String>,
    #[arg(
        long,
        help = "Opcjonalny skrót pauzy/wznowienia, np. Ctrl+Shift+F9; off = brak"
    )]
    pause_keybind: Option<String>,
    #[arg(
        long,
        value_enum,
        help = "digits = cyfry 1–9, dots = kropki 1–6 (normalnie wybierane w kreatorze)"
    )]
    mode: Option<AnswerMode>,
    #[arg(
        long,
        value_enum,
        help = "Jedna lub wiele poprawnych odpowiedzi (normalnie wybierane w kreatorze)"
    )]
    selection: Option<SelectionMode>,
    #[arg(long, value_parser=clap::value_parser!(u64).range(5..=300), help = "Limit API w sekundach; domyślnie zapisany lub 60")]
    timeout: Option<u64>,
    #[arg(
        long,
        help = "Ponownie przejdź cały kreator i zastąp zapisaną konfigurację"
    )]
    change: bool,
    #[arg(
        long,
        help = "Klucz tylko na tę sesję; nie zmieniaj istniejącej konfiguracji z zapisanym kluczem"
    )]
    session_only: bool,
    #[arg(long, help = "Wyświetl licencję MIT i zakończ")]
    license: bool,
    #[cfg(target_os = "linux")]
    #[arg(
        long,
        help = "Bazowa konfiguracja Niri; domyślnie wykrywana z procesu compositora"
    )]
    niri_config: Option<PathBuf>,
    #[arg(long, help = "Nie odłączaj od terminala; Ctrl+C kończy program")]
    foreground: bool,
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Wyślij pojedyncze żądanie analizy do działającego programu.
    Trigger,
    /// Wstrzymaj/wznów analizę i wyczyść wynik do myślnika.
    TogglePause,
    /// Zatrzymaj program, usuń zapisany klucz; pozostaw niesekretne ustawienia.
    ForgetKey,
    /// Zakończ program i zwolnij skróty.
    Stop,
    /// Pokaż tylko stan procesu, nigdy odpowiedź modelu.
    Status,
    #[cfg(target_os = "linux")]
    /// Przywróć konfigurację Niri po awaryjnym zamknięciu.
    RestoreKeybind,
    /// Sprawdź platformę i konfigurację; bez zrzutu i API.
    Doctor,
    /// Test wskaźnika przez 15 sekund; bez zrzutu, API i zmiany skrótu.
    Demo {
        #[arg(
            default_value = "9",
            help = "Odpowiedź testowa, np. 3 lub 1'2 przy --selection multiple"
        )]
        answers: String,
    },
    #[command(hide = true)]
    Serve,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Settings {
    provider: Provider,
    model: String,
    base_url: String,
    keybind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pause_keybind: Option<String>,
    #[serde(default)]
    mode: AnswerMode,
    #[serde(default)]
    selection: SelectionMode,
}

#[derive(Serialize, Deserialize)]
struct Startup {
    settings: Settings,
    timeout: u64,
    #[serde(default)]
    save_key: bool,
    #[serde(default)]
    preserve_saved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential_id: Option<String>,
    // Preserve the existing Linux startup JSON (niri_config) while keeping it out of Windows.
    #[serde(flatten)]
    platform: platform::Startup,
}

fn lock(runtime: &Path) -> Result<File> {
    let lock = platform::open_private_file(&runtime.join("daemon.lock"), false, true)?;
    lock.try_lock()
        .context("Program już działa. Najpierw użyj stop.")?;
    Ok(lock)
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut value = String::new();
    if io::stdin().read_line(&mut value)? == 0 {
        bail!("Brak wejścia. Uruchom interaktywnie lub podaj ustawienia flagami.");
    }
    Ok(value.trim().to_owned())
}

#[tokio::main]
async fn main() {
    platform::initialize();
    if let Err(err) = entry().await {
        eprintln!("Błąd: {err:#}");
        std::process::exit(1);
    }
}

async fn entry() -> Result<()> {
    let cli = Cli::parse();
    if cli.license {
        print!("{}", include_str!("../LICENSE"));
        return Ok(());
    }
    if cli.change && cli.command.is_some() {
        bail!("Użyj --change bez dodatkowego polecenia.");
    }
    let runtime = platform::prepare_runtime(
        &cli.runtime_dir
            .clone()
            .unwrap_or_else(platform::default_runtime),
    )?;
    match cli.command {
        Some(Action::Trigger) => {
            client(&runtime, "trigger").await?;
        }
        Some(Action::TogglePause) => {
            client(&runtime, "toggle-pause").await?;
        }
        Some(Action::ForgetKey) => {
            launch::forget_key(&runtime).await?;
        }
        Some(Action::Stop) => {
            launch::stop(&runtime).await?;
            println!("Program zatrzymany; skróty zwolnione.");
        }
        Some(Action::Status) => match client(&runtime, "status").await {
            Ok(_) => println!("Program działa."),
            Err(_) => {
                println!("Program nie działa lub nie odpowiada.");
                std::process::exit(1);
            }
        },
        #[cfg(target_os = "linux")]
        Some(Action::RestoreKeybind) => {
            let _lock = lock(&runtime)?;
            platform::recover(&runtime)?;
            println!("Konfiguracja Niri przywrócona (jeśli istniał zapis przywracania).");
        }
        Some(Action::Doctor) => platform::doctor(&cli, &runtime)?,
        Some(Action::Demo { answers }) => {
            let mode = cli.mode.unwrap_or_default();
            let selection = cli.selection.unwrap_or_default();
            let answer = match selection {
                SelectionMode::Single => answer::parse_answer(&answers)?,
                SelectionMode::Multiple => answer::parse_multiple_answer(&answers)?,
            };
            if answer.values().iter().any(|n| *n > mode.max_choices()) {
                bail!(
                    "Ten tryb obsługuje maksymalnie {} opcji.",
                    mode.max_choices()
                );
            }
            let mut ui = platform::Ui::demo(mode, selection).await?;
            ui.update(State::Answer(answer)).await?;
            println!("Test wskaźnika paska przez 15 sekund — bez zrzutu i bez API.");
            tokio::select! {_=tokio::time::sleep(Duration::from_secs(15))=>{},_=tokio::signal::ctrl_c()=>{}}
            ui.shutdown().await?;
        }
        Some(Action::Serve) => {
            let mut input = tokio::io::stdin();
            let key = secret::read_pipe_key(&mut input).await?;
            // Only non-secret startup JSON enters a buffered reader.
            let mut reader = BufReader::new(input).take(64 * 1024 + 1);
            let mut line = String::new();
            let count = reader.read_line(&mut line).await?;
            if count > 64 * 1024 || !line.ends_with('\n') {
                bail!("Nieprawidłowe lub zbyt duże dane startowe.");
            }
            let startup: Startup = serde_json::from_str(&line)
                .map_err(|_| anyhow::anyhow!("Nieprawidłowe dane startowe."))?;
            run(runtime, startup, key, true, None).await?;
        }
        None => launch::start(cli, runtime).await?,
    }
    Ok(())
}

async fn client(runtime: &Path, request: &str) -> Result<()> {
    timeout(Duration::from_secs(15), async {
        let mut stream = platform::ipc::connect(runtime).await?;
        stream.write_all(format!("{request}\n").as_bytes()).await?;
        // Reading a complete line also works with Windows byte-mode pipes, without waiting for EOF.
        let mut response = Vec::new();
        BufReader::new(stream)
            .take(128)
            .read_until(b'\n', &mut response)
            .await?;
        match std::str::from_utf8(&response).unwrap_or("").trim() {
            "OK" | "BUSY" => Ok(()),
            "RESTORE_FAILED" => {
                bail!("Nie udało się poprawnie zamknąć programu lub przywrócić skrótu.")
            }
            _ => bail!("Nieprawidłowa odpowiedź procesu."),
        }
    })
    .await
    .context("Proces nie odpowiedział na polecenie.")?
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Control {
    Trigger,
    TogglePause,
}

type Job = tokio::task::JoinHandle<Result<answer::Answer>>;

#[derive(Default)]
struct Analysis {
    paused: bool,
    job: Option<Job>,
    last_trigger: Option<tokio::time::Instant>,
}

impl Analysis {
    fn can_trigger(&self) -> bool {
        !self.paused
            && self.job.is_none()
            && self
                .last_trigger
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(750))
    }

    async fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
            let _ = job.await;
        }
    }

    async fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        self.last_trigger = None;
        self.cancel().await;
    }
}

async fn finish_job(job: &mut Option<Job>) -> Result<answer::Answer> {
    match job {
        Some(job) => job
            .await
            .map_err(|_| anyhow::anyhow!("Przerwany proces analizy."))?,
        None => std::future::pending().await,
    }
}

async fn apply_control(
    control: Control,
    api: &std::sync::Arc<Api>,
    ui: &platform::Ui,
    seconds: u64,
    analysis: &mut Analysis,
) -> Result<bool> {
    if control == Control::TogglePause {
        analysis.toggle_pause().await;
        ui.update(State::Idle).await?;
        return Ok(true);
    }
    if !analysis.can_trigger() {
        return Ok(false);
    }
    analysis.last_trigger = Some(tokio::time::Instant::now());
    ui.update(State::Busy).await?;
    let api = api.clone();
    analysis.job = Some(tokio::spawn(async move {
        timeout(Duration::from_secs(seconds + 20), async {
            let image = platform::capture::capture().await?;
            api.solve(&image).await
        })
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("Upłynął limit czasu analizy.")))
    }));
    Ok(true)
}

async fn bind_listener(runtime: &Path) -> Result<platform::ipc::Listener> {
    #[cfg(not(windows))]
    {
        platform::ipc::Listener::bind(runtime)
    }
    #[cfg(windows)]
    {
        // Cancelled named-pipe IO can retain its final handle until IOCP cleanup.
        // We hold the worker lock, so bounded retries cannot create two workers.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match platform::ipc::Listener::bind(runtime) {
                Ok(listener) => return Ok(listener),
                Err(error) if tokio::time::Instant::now() >= deadline => return Err(error),
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
    }
}

async fn run(
    runtime: PathBuf,
    startup: Startup,
    key: Zeroizing<String>,
    detached: bool,
    mut launcher_guard: Option<File>,
) -> Result<()> {
    credentials::validate_key(&key)?;
    setup::validate_saved(&startup.settings)?;
    if !(5..=300).contains(&startup.timeout) {
        bail!("Nieprawidłowy limit czasu analizy.");
    }
    let _lock = lock(&runtime)?;
    platform::recover(&runtime)?;
    // Install Unix signal handlers before changing any compositor configuration.
    let mut shutdown = platform::shutdown_signal()?;
    let (controls_tx, mut controls_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ui = platform::Ui::spawn(&runtime, &startup.settings, startup.platform, controls_tx)
        .await
        .context("Nie można uruchomić wskaźnika lub zarejestrować skrótu.")?;
    let mut analysis = Analysis::default();
    let mut stop_client = None;
    let mut pending_credential = None;
    let mut committed = false;
    let mut retired_cleanup = None;
    // Every error after native UI creation must await cleanup, not merely enqueue
    // it in Drop immediately before the process exits.
    let result: Result<()> = async {
        let mut credential_id = startup.credential_id.clone();
        if startup.save_key {
            let id = credentials::new_id()?;
            // Track the ID before writing: even a lost create reply can be cleaned up.
            pending_credential = Some(id.clone());
            credentials::save(&runtime, &startup.settings, &id, &key).await?;
            credential_id = Some(id);
        }
        let api = std::sync::Arc::new(Api::new(
            startup.settings.provider, startup.settings.model.clone(),
            &startup.settings.base_url, key, startup.timeout,
        )?.with_mode(startup.settings.mode).with_selection(startup.settings.selection));
        let mut listener = bind_listener(&runtime).await?;
        let mut retired = if startup.preserve_saved { None } else { config::load(&runtime).ok().flatten() };
        if let Some(previous) = &mut retired
            && previous.credential_id != credential_id { previous.retire_active(); }
        let retired_credentials = retired.map(|s| s.retired_credentials).unwrap_or_default();
        if retired_credentials.len() + usize::from(credential_id.is_some()) > 32 { bail!("Zbyt wiele starych kluczy. Najpierw użyj forget-key."); }
        let saved = config::Saved { settings: startup.settings.clone(), timeout: startup.timeout, credential_id, retired_credentials };
        let mut transaction = if startup.preserve_saved { None } else { Some(config::Transaction::install(&runtime, &saved)?) };
        // Discard presses received while the keyring/setup was still pending.
        while controls_rx.try_recv().is_ok() {}
        if detached {
            let mut output = io::stdout();
            writeln!(output, "READY")?;
            output.flush()?;
        } else {
            let mut output = io::stdout();
            writeln!(output, "Działa. Ctrl+C kończy program; odpowiedź jest wyłącznie na pasku.")?;
            output.flush()?;
        }
        if let Some(transaction) = &mut transaction { transaction.commit(); }
        committed = true;
        // Foreground operation must not keep the launcher lock for its lifetime.
        drop(launcher_guard.take());
        if !startup.preserve_saved && !saved.retired_credentials.is_empty() {
            let runtime = runtime.clone();
            retired_cleanup = Some(tokio::spawn(async move { config::cleanup_retired(&runtime, saved).await; }));
        }
        loop {
            tokio::select! {
                biased;
                _ = shutdown.wait() => break,
                Some(control) = controls_rx.recv() => {
                    apply_control(control, &api, &ui, startup.timeout, &mut analysis).await?;
                },
                result = finish_job(&mut analysis.job) => {
                    analysis.job = None;
                    let state = match result {
                        Ok(answer) => State::Answer(answer),
                        Err(err) => {
                            eprintln!("Analiza nie powiodła się: {err:#}");
                            State::Error
                        }
                    };
                    ui.update(state).await?;
                },
                connection = listener.accept() => {
                    let mut reader = BufReader::new(connection?);
                    let mut line = Vec::new();
                    let received = timeout(Duration::from_millis(300),
                        (&mut reader).take(64).read_until(b'\n', &mut line)).await;
                    if !matches!(received, Ok(Ok(_))) { continue; }
                    let command = std::str::from_utf8(&line).unwrap_or("").trim();
                    let mut stream = reader.into_inner();
                    if command == "bindings" {
                        // Reconfiguration may retain a different persistent setup;
                        // disclose only actual shortcut names, never answers/keys.
                        let mut bindings = serde_json::to_vec(&(&startup.settings.keybind, &startup.settings.pause_keybind))?;
                        bindings.push(b'\n');
                        let _ = stream.write_all(&bindings).await;
                        continue;
                    }
                    let response = match command {
                        "stop" => { stop_client = Some(stream); break; },
                        "status" => "OK\n",
                        "trigger" | "toggle-pause" => {
                            let control = if command == "trigger" { Control::Trigger } else { Control::TogglePause };
                            if apply_control(control, &api, &ui, startup.timeout, &mut analysis).await? {
                                "OK\n"
                            } else { "BUSY\n" }
                        },
                        _ => "ERROR\n",
                    };
                    let _ = stream.write_all(response.as_bytes()).await;
                },
            }
        }
        Ok(())
    }.await;
    analysis.cancel().await;
    if !committed
        && let Some(id) = pending_credential
        && credentials::delete(&runtime, &startup.settings, &id)
            .await
            .is_err()
    {
        eprintln!(
            "Nie udało się usunąć tymczasowego wpisu z magazynu poświadczeń po nieudanym starcie."
        );
    }
    let restored = ui.shutdown().await;
    if let Some(cleanup) = retired_cleanup {
        let _ = cleanup.await;
    }
    if let Some(mut stream) = stop_client {
        let response: &[u8] = if restored.is_ok() {
            b"OK\n"
        } else {
            b"RESTORE_FAILED\n"
        };
        let _ = stream.write_all(response).await;
    }
    result.and(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pause_cancels_and_discards_work_and_resume_is_idle_without_debounce() {
        let mut analysis = Analysis::default();
        assert!(analysis.can_trigger());
        analysis.last_trigger = Some(tokio::time::Instant::now());
        assert!(!analysis.can_trigger());
        let (started, ready) = tokio::sync::oneshot::channel();
        let (dropped, ended) = tokio::sync::oneshot::channel::<()>();
        analysis.job = Some(tokio::spawn(async move {
            let _guard = dropped;
            let _ = started.send(());
            std::future::pending().await
        }));
        ready.await.unwrap();
        analysis.toggle_pause().await;
        assert!(analysis.paused);
        assert!(analysis.job.is_none());
        assert!(!analysis.can_trigger());
        assert!(
            ended.await.is_err(),
            "Paused worker must be dropped before returning"
        );
        analysis.toggle_pause().await;
        assert!(!analysis.paused);
        assert!(analysis.job.is_none());
        assert!(analysis.can_trigger());

        // Even an already completed old answer must never be published after a reset.
        analysis.job = Some(tokio::spawn(async { Ok(answer::Answer::Choice(9)) }));
        tokio::task::yield_now().await;
        analysis.toggle_pause().await;
        assert!(analysis.job.is_none());
        assert!(
            timeout(Duration::from_millis(20), finish_job(&mut analysis.job))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn worker_completion_and_panic_are_both_observed() {
        let mut job: Option<Job> = Some(tokio::spawn(async { Ok(answer::Answer::Choice(2)) }));
        assert_eq!(
            finish_job(&mut job).await.unwrap(),
            answer::Answer::Choice(2)
        );
        job = Some(tokio::spawn(async { panic!("synthetic worker failure") }));
        let error = timeout(Duration::from_secs(2), finish_job(&mut job))
            .await
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(error.contains("Przerwany"));
        assert!(!error.contains("synthetic worker failure"));
    }
}
