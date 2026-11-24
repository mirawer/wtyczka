use anyhow::{Context, Result};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use tokio::net::{UnixListener, UnixStream};

pub type ServerStream = UnixStream;
pub type ClientStream = UnixStream;

pub struct Listener {
    listener: UnixListener,
    path: PathBuf,
}

impl Listener {
    pub fn bind(runtime: &Path) -> Result<Self> {
        let path = runtime.join("control.sock");
        if path.exists() {
            fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(Self { listener, path })
    }
    pub async fn accept(&mut self) -> Result<ServerStream> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            if stream.peer_cred()?.uid() == unsafe { libc::geteuid() } {
                return Ok(stream);
            }
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub async fn connect(runtime: &Path) -> Result<ClientStream> {
    UnixStream::connect(runtime.join("control.sock"))
        .await
        .context("Program nie działa.")
}
