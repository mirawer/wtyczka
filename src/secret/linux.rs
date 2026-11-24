use super::Console;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    mem::MaybeUninit,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};
use zeroize::Zeroizing;

pub(super) struct Terminal {
    file: File,
    original: libc::termios,
    needs_restore: bool,
}

fn set_mode(fd: libc::c_int, action: libc::c_int, mode: &libc::termios) -> io::Result<()> {
    loop {
        if unsafe { libc::tcsetattr(fd, action, mode) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

impl Terminal {
    pub(super) fn open_hidden() -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOCTTY)
            .open("/dev/tty")?;
        let mut original = MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(file.as_raw_fd(), original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { original.assume_init() };
        // Own restoration before attempting the mode change, so even a
        // failed/partially applied change has a best-effort rollback.
        let terminal = Self {
            file,
            original,
            needs_restore: true,
        };
        let mut hidden = original;
        unsafe { libc::cfmakeraw(&mut hidden) };
        // cfmakeraw disables ECHO, ICANON, ISIG, input translation, and OPOST.
        hidden.c_cc[libc::VMIN] = 1;
        hidden.c_cc[libc::VTIME] = 0;
        set_mode(terminal.file.as_raw_fd(), libc::TCSANOW, &hidden)?;
        Ok(terminal)
    }

    pub(super) fn restore(&mut self) -> io::Result<()> {
        if self.needs_restore {
            // Discard unread paste data before restoring echo/signals. Do
            // not let a failed entry's tail become visible shell input.
            set_mode(self.file.as_raw_fd(), libc::TCSAFLUSH, &self.original)?;
            self.needs_restore = false;
        }
        Ok(())
    }
}

impl Console for Terminal {
    fn read_unit(&mut self) -> io::Result<Option<u32>> {
        let mut byte = Zeroizing::new([0u8; 1]);
        loop {
            match self.file.read(&mut *byte) {
                Ok(0) => return Ok(None),
                Ok(_) => return Ok(Some(u32::from(byte[0]))),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn write_public(&mut self, text: &str) -> io::Result<()> {
        self.file.write_all(text.as_bytes())?;
        self.file.flush()
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
