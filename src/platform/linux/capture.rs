use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

const MAX_IMAGE: usize = 4 * 1024 * 1024;

#[derive(Deserialize)]
struct Output {
    name: String,
    logical: Option<Logical>,
}

#[derive(Deserialize)]
struct Logical {
    scale: f64,
}

/// Native scale preserves small text; JPEG bounds upload costs. No file or clipboard access.
pub async fn capture() -> Result<Vec<u8>> {
    timeout(Duration::from_secs(15), capture_inner())
        .await
        .context("Upłynął limit czasu zrzutu ekranu.")?
}

async fn capture_inner() -> Result<Vec<u8>> {
    let result = Command::new("niri")
        .args(["msg", "-j", "focused-output"])
        .kill_on_drop(true)
        .output()
        .await
        .context("Nie można uruchomić niri.")?;
    if !result.status.success() {
        bail!("Nie można ustalić aktywnego monitora Niri.");
    }
    let output: Output =
        serde_json::from_slice(&result.stdout).context("Brak aktywnego monitora.")?;
    let scale = output.logical.map(|l| l.scale).unwrap_or(1.0);
    if !scale.is_finite() || scale <= 0.0 {
        bail!("Nieprawidłowa skala monitora.");
    }
    let scale = scale.to_string();
    let mut child = Command::new("grim")
        .args([
            "-o",
            &output.name,
            "-s",
            &scale,
            "-t",
            "jpeg",
            "-q",
            "85",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Nie można uruchomić grim. Zainstaluj grim z obsługą JPEG.")?;
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .context("Brak wyjścia grim.")?
        .take(MAX_IMAGE as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > MAX_IMAGE {
        bail!("Zrzut przekracza limit 4 MiB; zmniejsz rozdzielczość monitora.");
    }
    if !child.wait().await?.success() {
        bail!("grim nie wykonał zrzutu (sprawdź obsługę screencopy w compositorze).");
    }
    if !bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        bail!("grim nie zwrócił obrazu JPEG.");
    }
    Ok(bytes)
}
