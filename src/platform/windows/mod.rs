pub mod capture;
mod system;
mod ui;

pub use system::{
    default_runtime, detach, initialize, open_private_file, prepare_runtime, terminate,
};
pub mod ipc {
    pub use super::system::{Listener, connect};
}

use crate::{
    Cli, Control, Settings,
    display::State,
    mode::{AnswerMode, SelectionMode},
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Serialize, Deserialize)]
pub struct Startup {}

pub fn prepare_start(_cli: &Cli, _runtime: &Path) -> Result<Startup> {
    Ok(Startup {})
}
pub fn recover(_runtime: &Path) -> Result<()> {
    Ok(())
}
pub fn validate_keybind(key: &str) -> Result<()> {
    ui::validate_keybind(key)
}
pub fn check_keybind_available(key: &str) -> Result<()> {
    ui::check_keybind_available(key)
}
pub fn validate_keybind_pair(key: &str, pause: Option<&str>) -> Result<()> {
    ui::validate_keybind_pair(key, pause)
}

pub fn doctor(cli: &Cli, runtime: &Path) -> Result<()> {
    let key = crate::keybind::normalize_keybind(cli.keybind.as_deref().unwrap_or("Print"))?;
    let pause = crate::setup::configured_pause_keybind(
        cli.pause_keybind.as_deref().unwrap_or("off"),
        &key,
    )?;
    capture::check_environment()?;
    println!(
        "Windows: natywne Win32/GDI, bez dodatkowych programów\nSkrót: {key}\nSkrót pauzy: {}\nSkładnia poprawna; dostępność skrótów sprawdzana przy starcie\nStan użytkownika: {}\nSprawdź zasobnik poleceniem demo. Jeśli Print Screen jest zajęty przez Narzędzie Wycinanie, wybierz inny skrót.",
        pause.as_deref().unwrap_or("brak"),
        runtime.display()
    );
    Ok(())
}

pub struct Shutdown {
    console: bool,
}
pub fn shutdown_signal() -> Result<Shutdown> {
    Ok(Shutdown {
        console: unsafe { windows_sys::Win32::System::Console::GetConsoleCP() != 0 },
    })
}
impl Shutdown {
    pub async fn wait(&mut self) -> Result<()> {
        if self.console {
            system::shutdown_signal().await
        } else {
            std::future::pending().await
        } // CREATE_NO_WINDOW children stop through private IPC.
    }
}

pub struct Ui {
    native: ui::Ui,
}
impl Ui {
    pub async fn spawn(
        _runtime: &Path,
        settings: &Settings,
        _startup: Startup,
        controls: UnboundedSender<Control>,
    ) -> Result<Self> {
        Ok(Self {
            native: ui::Ui::spawn(
                settings.mode,
                settings.selection,
                &settings.keybind,
                settings.pause_keybind.as_deref(),
                controls,
            )
            .await?,
        })
    }
    pub async fn demo(mode: AnswerMode, selection: SelectionMode) -> Result<Self> {
        Ok(Self {
            native: ui::Ui::demo(mode, selection).await?,
        })
    }
    pub async fn update(&self, state: State) -> Result<()> {
        self.native.update(state).await
    }
    pub async fn shutdown(&mut self) -> Result<()> {
        self.native.shutdown().await
    }
}
