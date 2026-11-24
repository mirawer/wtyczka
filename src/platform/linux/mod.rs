pub mod capture;
pub mod ipc;
pub mod keybind;
mod sni;
mod system;
mod tray;
pub use system::*;

use crate::{
    Cli, Control, Settings,
    display::State,
    mode::{AnswerMode, SelectionMode},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Serialize, Deserialize)]
pub struct Startup {
    pub niri_config: PathBuf,
}

pub fn prepare_start(cli: &Cli, _runtime: &Path) -> Result<Startup> {
    let niri_config = cli
        .niri_config
        .clone()
        .map(Ok)
        .unwrap_or_else(keybind::discover_config)?;
    keybind::validate(&niri_config)?;
    Ok(Startup { niri_config })
}

pub fn recover(runtime: &Path) -> Result<()> {
    keybind::recover(runtime)
}

pub fn check_keybind_available(_key: &str) -> Result<()> {
    Ok(())
}

pub fn validate_keybind(key: &str) -> Result<()> {
    crate::keybind::normalize_keybind(key)?;
    Ok(())
}

pub fn same_keybind(left: &str, right: &str) -> Result<bool> {
    Ok(crate::keybind::normalize_keybind(left)?
        .eq_ignore_ascii_case(&crate::keybind::normalize_keybind(right)?))
}

pub fn validate_keybind_pair(key: &str, pause: Option<&str>) -> Result<()> {
    let key = crate::keybind::normalize_keybind(key)?;
    if let Some(pause) = pause {
        let pause = crate::keybind::normalize_keybind(pause)?;
        if key.eq_ignore_ascii_case(&pause) {
            bail!("Skrót pauzy musi być inny niż skrót analizy.");
        }
    }
    Ok(())
}

pub fn doctor(cli: &Cli, runtime: &Path) -> Result<()> {
    let startup = prepare_start(cli, runtime)?;
    let key = crate::keybind::normalize_keybind(cli.keybind.as_deref().unwrap_or("Print"))?;
    let pause = crate::setup::configured_pause_keybind(
        cli.pause_keybind.as_deref().unwrap_or("off"),
        &key,
    )?;
    let candidate = runtime.join("doctor-overlay.kdl");
    fs::write(
        &candidate,
        keybind::overlay(
            &startup.niri_config,
            &std::env::current_exe()?,
            runtime,
            &key,
            pause.as_deref(),
        )?,
    )?;
    keybind::validate(&candidate)?;
    let grim = std::process::Command::new("grim")
        .arg("-h")
        .output()
        .context("Brak grim.")?;
    if !grim.status.success() {
        bail!("grim -h zakończył się błędem.");
    }
    println!(
        "Niri: {}\nSkrót: {key}\nSkrót pauzy: {}\nWalidacja nakładki: OK\ngrim: dostępny\nStan: {}\nSprawdź widoczność zasobnika poleceniem demo.",
        startup.niri_config.display(),
        pause.as_deref().unwrap_or("brak"),
        runtime.display()
    );
    Ok(())
}

pub struct Ui {
    bar: tray::Bar,
    binding: Option<keybind::Binding>,
}
impl Ui {
    pub async fn spawn(
        runtime: &Path,
        settings: &Settings,
        startup: Startup,
        _controls: UnboundedSender<Control>,
    ) -> Result<Self> {
        let bar = tray::Bar::spawn(settings.mode, settings.selection).await?;
        let binding = keybind::Binding::install(
            runtime,
            startup.niri_config,
            &settings.keybind,
            settings.pause_keybind.as_deref(),
        )?;
        Ok(Self {
            bar,
            binding: Some(binding),
        })
    }
    pub async fn demo(mode: AnswerMode, selection: SelectionMode) -> Result<Self> {
        Ok(Self {
            bar: tray::Bar::spawn(mode, selection).await?,
            binding: None,
        })
    }
    pub async fn update(&self, state: State) -> Result<()> {
        self.bar.update(state).await
    }
    pub async fn shutdown(&mut self) -> Result<()> {
        let restored = self
            .binding
            .as_mut()
            .map(|binding| binding.restore())
            .unwrap_or(Ok(()));
        self.bar.shutdown().await;
        restored
    }
}
