use anyhow::{Context, Result, bail};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub fn default_runtime() -> PathBuf {
    let project = std::env::current_exe().ok().and_then(|exe| {
        exe.ancestors()
            .skip(1)
            .find(|dir| dir.join("Cargo.toml").is_file() && dir.join("src/main.rs").is_file())
            .map(Path::to_path_buf)
    });
    project
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        .join(".runtime")
}

pub fn initialize() {
    unsafe {
        libc::setrlimit(
            libc::RLIMIT_CORE,
            &libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            },
        );
    }
}

pub fn prepare_runtime(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink()
            || !meta.is_dir()
            || meta.uid() != unsafe { libc::geteuid() }
        {
            bail!("Katalog stanu musi być zwykłym katalogiem należącym do użytkownika.");
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    } else {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .context("Nie można utworzyć katalogu stanu (katalog nadrzędny musi istnieć).")?;
    }
    fs::canonicalize(path).context("Nie można odczytać katalogu stanu.")
}

pub fn open_private_file(path: &Path, truncate: bool, read: bool) -> Result<File> {
    Ok(OpenOptions::new()
        .create(true)
        .truncate(truncate)
        .read(read)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}

pub fn detach(command: &mut tokio::process::Command) {
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

pub async fn terminate(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
}

pub struct Shutdown {
    term: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
}

pub fn shutdown_signal() -> Result<Shutdown> {
    use tokio::signal::unix::{SignalKind, signal};
    Ok(Shutdown {
        term: signal(SignalKind::terminate())?,
        interrupt: signal(SignalKind::interrupt())?,
    })
}

impl Shutdown {
    pub async fn wait(&mut self) -> Result<()> {
        tokio::select! {_=self.term.recv()=>{},_=self.interrupt.recv()=>{}}
        Ok(())
    }
}
