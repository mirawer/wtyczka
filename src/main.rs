mod answer;
mod api;
mod display;
mod keybind;
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
    fs::{self, File},
    io::{self, Write},
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

#[derive(Parser)]
#[command(
    version,
    about = "Jedna lub wiele odpowiedzi: cyfry 1–9 albo kropki 1–6 na pasku. Linux/Niri i Windows."
)]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Katalog stanu (Linux: .runtime projektu; Windows: LocalAppData/ScreenAnswer)"
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
    #[arg(long,default_value_t=60,value_parser=clap::value_parser!(u64).range(5..=300))]
    timeout: u64,
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

#[derive(Clone, Serialize, Deserialize)]
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

fn read_settings(cli: &Cli, runtime: &Path) -> Result<Settings> {
    let path = runtime.join("settings.json");
    let saved: Option<Settings> = if path.exists() {
        Some(serde_json::from_slice(&fs::read(path)?).context("Uszkodzony settings.json.")?)
    } else {
        None
    };
    setup::configure(cli, saved.as_ref(), &mut prompt, &mut io::stdout())
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
        Some(Action::Stop) => {
            client(&runtime, "stop").await?;
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
            let mut reader = BufReader::new(tokio::io::stdin());
            let mut key = Zeroizing::new(String::new());
            reader.read_line(&mut key).await?;
            while key.ends_with(['\r', '\n']) {
                key.pop();
            }
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            let startup: Startup = serde_json::from_str(&line)?;
            run(runtime, startup, key, true).await?;
        }
        None => start(cli, runtime).await?,
    }
    Ok(())
}

async fn start(cli: Cli, runtime: PathBuf) -> Result<()> {
    drop(lock(&runtime)?);
    let mut settings = read_settings(&cli, &runtime)?;
    loop {
        match platform::check_keybind_available(&settings.keybind).and_then(|()| {
            platform::validate_keybind_pair(&settings.keybind, settings.pause_keybind.as_deref())
        }) {
            Ok(()) => break,
            Err(error) => {
                if cli.keybind.is_some() {
                    return Err(error);
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
        match platform::check_keybind_available(pause) {
            Ok(()) => break,
            Err(error) => {
                if cli.pause_keybind.is_some() {
                    return Err(error);
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
    let os_startup = platform::prepare_start(&cli, &runtime)?;
    api::endpoint_url(settings.provider, &settings.model, &settings.base_url)?;
    println!(
        "Dostawca: {:?}; model: {}; adres: {}",
        settings.provider, settings.model, settings.base_url
    );
    println!(
        "Po skrócie {} cały aktywny monitor trafi do tego API. Klucz nie zostanie zapisany.",
        settings.keybind
    );
    if settings.provider == Provider::Anthropic {
        println!(
            "Limit wyjścia Claude: {} tokenów łącznie na myślenie i odpowiedź. Tokeny myślenia również są płatne.",
            api::ANTHROPIC_MAX_TOKENS
        );
    }
    println!("Tryb odpowiedzi: {}.", settings.mode.description());
    println!(
        "Liczba poprawnych odpowiedzi: {}.",
        settings.selection.description()
    );
    if let Some(pause) = &settings.pause_keybind {
        println!("Pauza/wznowienie: {pause}. Pauza czyści wynik do myślnika (-).");
    }
    println!("\n7/7 — Teraz wklej klucz API. Wpisywane znaki nie będą widoczne.");
    let key = loop {
        match secret::prompt_key("Klucz API (ukryty): ") {
            Ok(key) => break key,
            Err(err) if err.is::<secret::InvalidKey>() => {
                eprintln!("{err}\nSpróbuj ponownie. Wpisywane znaki pozostają ukryte.");
            }
            Err(err) => return Err(err),
        }
    };
    let startup = Startup {
        settings,
        timeout: cli.timeout,
        platform: os_startup,
    };
    if cli.foreground {
        run(runtime, startup, key, false).await
    } else {
        let log = platform::open_private_file(&runtime.join("daemon.log"), true, false)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--runtime-dir")
            .arg(&runtime)
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
        let mut output = BufReader::new(child.stdout.take().context("Brak stdout procesu.")?);
        let mut response = String::new();
        let ready = timeout(Duration::from_secs(20), output.read_line(&mut response)).await;
        if !matches!(ready, Ok(Ok(_))) || response.trim() != "READY" {
            platform::terminate(&mut child).await;
            let _ = timeout(Duration::from_secs(5), child.wait()).await;
            bail!(
                "Start nie powiódł się. Sprawdź {} (bez kluczy i treści odpowiedzi).",
                runtime.join("daemon.log").display()
            );
        }
        println!("Działa w tle. Odpowiedź tylko na pasku. Zatrzymanie: screen-answer stop");
        Ok(())
    }
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

async fn run(
    runtime: PathBuf,
    startup: Startup,
    key: Zeroizing<String>,
    detached: bool,
) -> Result<()> {
    let _lock = lock(&runtime)?;
    platform::recover(&runtime)?;
    let mut listener = platform::ipc::Listener::bind(&runtime)?;
    let api = std::sync::Arc::new(
        Api::new(
            startup.settings.provider,
            startup.settings.model.clone(),
            &startup.settings.base_url,
            key,
            startup.timeout,
        )?
        .with_mode(startup.settings.mode)
        .with_selection(startup.settings.selection),
    );
    // Install Unix signal handlers before changing any compositor configuration.
    let mut shutdown = platform::shutdown_signal()?;
    let (controls_tx, mut controls_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut ui = platform::Ui::spawn(&runtime, &startup.settings, startup.platform, controls_tx)
        .await
        .context("Nie można uruchomić wskaźnika lub zarejestrować skrótu.")?;
    let mut analysis = Analysis::default();
    let mut stop_client = None;
    // Every error after native UI creation must await cleanup, not merely enqueue
    // it in Drop immediately before the process exits.
    let result: Result<()> = async {
        fs::write(
            runtime.join("settings.json"),
            serde_json::to_vec_pretty(&startup.settings)?,
        )?;
        if detached {
            println!("READY");
            io::stdout().flush()?;
        } else {
            println!("Działa. Ctrl+C kończy program; odpowiedź jest wyłącznie na pasku.");
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
    let restored = ui.shutdown().await;
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
