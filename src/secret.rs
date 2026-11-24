use anyhow::{Context, Result, bail, ensure};
use std::io;
use zeroize::{Zeroize, Zeroizing};

const MAX_KEY_LEN: usize = 4096;

#[derive(Debug)]
pub struct EmptyKey;
impl std::fmt::Display for EmptyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("API key cannot be empty.")
    }
}
impl std::error::Error for EmptyKey {}

/// Read an ASCII API key from the controlling terminal, never from redirected
/// stdin. Terminal echo and signal processing are disabled before any prompt.
#[cfg(any(target_os = "linux", windows))]
pub fn prompt_key(label: &str) -> Result<Zeroizing<String>> {
    let mut terminal =
        platform::Terminal::open_hidden().context("Cannot open a terminal with hidden input.")?;
    let result = edit_key(&mut terminal, label);
    let restored = terminal.restore();
    match result {
        Ok(key) => {
            // On restoration failure the key is dropped/zeroized, not returned.
            restored.context("Cannot restore terminal input mode.")?;
            Ok(key)
        }
        Err(error) => Err(error), // Drop retries restoration if necessary.
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn prompt_key(_label: &str) -> Result<Zeroizing<String>> {
    bail!("Hidden API key input is unsupported on this platform.")
}

trait Console {
    /// A byte on Unix, or a UTF-16 code unit on Windows. Neither is decoded or
    /// included in errors; only printable ASCII is ever accepted.
    fn read_unit(&mut self) -> io::Result<Option<u32>>;
    /// Write and flush only public text (the label and the final newline).
    fn write_public(&mut self, text: &str) -> io::Result<()>;
}

fn edit_key(console: &mut impl Console, label: &str) -> Result<Zeroizing<String>> {
    // The native adapter must already own a live hidden-mode RAII guard here.
    console
        .write_public(label)
        .context("Cannot display API key prompt.")?;
    let result = read_edited_key(console);
    let newline = console.write_public("\r\n");
    match result {
        Ok(key) => {
            newline.context("Cannot finish API key prompt.")?;
            Ok(key)
        }
        Err(error) => Err(error),
    }
}

fn read_edited_key(console: &mut impl Console) -> Result<Zeroizing<String>> {
    // Fixed capacity avoids reallocating secret-bearing buffers. Deleted bytes
    // are wiped immediately; every exit (including errors) wipes the full array.
    let mut bytes = Zeroizing::new([0u8; MAX_KEY_LEN]);
    let mut len = 0usize;
    loop {
        // Even a single input unit is secret-bearing, so give it a drop guard.
        let unit = Zeroizing::new(
            console
                .read_unit()
                .context("Cannot read hidden API key input.")?
                .context("API key input ended before Enter.")?,
        );
        match *unit {
            3 | 4 => bail!("API key entry cancelled."), // Ctrl+C / Ctrl+D, no signal.
            10 | 13 => {
                if len == 0 {
                    return Err(EmptyKey.into());
                }
                let mut key = Zeroizing::new(String::new());
                key.try_reserve_exact(len)
                    .context("Cannot allocate API key buffer.")?;
                // The editing loop only stores printable ASCII.
                key.push_str(std::str::from_utf8(&bytes[..len]).context("Invalid API key input.")?);
                return Ok(key);
            }
            8 | 127 => {
                if len != 0 {
                    len -= 1;
                    bytes[len].zeroize();
                }
            }
            21 => {
                bytes.zeroize(); // Ctrl+U clears the whole entry without echo.
                len = 0;
            }
            0x20..=0x7e => {
                ensure!(
                    len < MAX_KEY_LEN,
                    "API key exceeds the 4096 character limit."
                );
                bytes[len] = *unit as u8;
                len += 1;
            }
            _ => bail!("API key must contain printable ASCII characters only."),
        }
    }
}

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(windows)]
use windows as platform;

#[cfg(target_os = "linux")]
mod linux {
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
}

#[cfg(windows)]
mod windows {
    use super::Console;
    use std::{io, ptr::null_mut};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::Console::{
            ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            ENABLE_VIRTUAL_TERMINAL_INPUT, FlushConsoleInputBuffer, GetConsoleMode, ReadConsoleW,
            SetConsoleMode, WriteConsoleW,
        },
    };
    use zeroize::Zeroizing;

    struct Handle(HANDLE);
    impl Handle {
        fn open(name: &[u16], access: u32) -> io::Result<Self> {
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE || handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(handle))
        }
    }
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    pub(super) struct Terminal {
        input: Handle,
        output: Handle,
        original: u32,
        needs_restore: bool,
    }
    impl Terminal {
        pub(super) fn open_hidden() -> io::Result<Self> {
            let input = Handle::open(
                &[67, 79, 78, 73, 78, 36, 0], // CONIN$
                GENERIC_READ | GENERIC_WRITE,
            )?;
            let output = Handle::open(&[67, 79, 78, 79, 85, 84, 36, 0], GENERIC_WRITE)?; // CONOUT$
            let mut original = 0;
            if unsafe { GetConsoleMode(input.0, &mut original) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let terminal = Self {
                input,
                output,
                original,
                needs_restore: true,
            };
            let hidden = original
                & !(ENABLE_ECHO_INPUT
                    | ENABLE_LINE_INPUT
                    | ENABLE_PROCESSED_INPUT
                    | ENABLE_VIRTUAL_TERMINAL_INPUT);
            if unsafe { SetConsoleMode(terminal.input.0, hidden) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(terminal)
        }

        pub(super) fn restore(&mut self) -> io::Result<()> {
            if self.needs_restore {
                // Flush first while echo/processed input remain disabled.
                let flushed = unsafe { FlushConsoleInputBuffer(self.input.0) };
                let flush_error = if flushed == 0 {
                    Some(io::Error::last_os_error())
                } else {
                    None
                };
                if unsafe { SetConsoleMode(self.input.0, self.original) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                self.needs_restore = false;
                if let Some(error) = flush_error {
                    return Err(error);
                }
            }
            Ok(())
        }
    }

    impl Console for Terminal {
        fn read_unit(&mut self) -> io::Result<Option<u32>> {
            let mut unit = Zeroizing::new([0u16; 1]);
            let mut read = 0;
            if unsafe {
                ReadConsoleW(
                    self.input.0,
                    unit.as_mut_ptr().cast(),
                    1,
                    &mut read,
                    std::ptr::null(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(if read == 0 {
                None
            } else {
                Some(u32::from(unit[0]))
            })
        }

        fn write_public(&mut self, text: &str) -> io::Result<()> {
            // This buffer contains only the public prompt/newline, never input.
            let text: Vec<u16> = text.encode_utf16().collect();
            let mut remaining = text.as_slice();
            while !remaining.is_empty() {
                let count = remaining.len().min(u32::MAX as usize) as u32;
                let mut written = 0;
                if unsafe {
                    WriteConsoleW(
                        self.output.0,
                        remaining.as_ptr(),
                        count,
                        &mut written,
                        std::ptr::null(),
                    )
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                if written == 0 || written > count {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Cannot write console prompt.",
                    ));
                }
                remaining = &remaining[written as usize..];
            }
            // WriteConsoleW is unbuffered; there is no Rust stdout buffer.
            Ok(())
        }
    }
    impl Drop for Terminal {
        fn drop(&mut self) {
            let _ = self.restore();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const LABEL: &str = "Klucz API (ukryty): ";

    struct FakeConsole {
        input: VecDeque<u32>,
        output: String,
        fail_read: bool,
        fail_write: bool,
    }
    impl FakeConsole {
        fn new(input: impl IntoIterator<Item = u32>) -> Self {
            Self {
                input: input.into_iter().collect(),
                output: String::new(),
                fail_read: false,
                fail_write: false,
            }
        }
        fn ascii(input: &[u8]) -> Self {
            Self::new(input.iter().copied().map(u32::from))
        }
    }
    impl Console for FakeConsole {
        fn read_unit(&mut self) -> io::Result<Option<u32>> {
            assert_eq!(
                self.output, LABEL,
                "prompt must be flushed before the first read"
            );
            if self.fail_read {
                return Err(io::Error::other("injected read failure"));
            }
            Ok(self.input.pop_front())
        }
        fn write_public(&mut self, text: &str) -> io::Result<()> {
            if self.fail_write {
                return Err(io::Error::other("injected write failure"));
            }
            self.output.push_str(text);
            Ok(())
        }
    }

    #[test]
    fn instant_ascii_paste_is_never_echoed() {
        let mut console = FakeConsole::ascii(b"sk-instant-paste_123\r");
        assert_eq!(
            edit_key(&mut console, LABEL).unwrap().as_str(),
            "sk-instant-paste_123"
        );
        assert_eq!(console.output, format!("{LABEL}\r\n"));
    }

    #[test]
    fn backspace_delete_and_ctrl_u_edit_without_output() {
        let mut console = FakeConsole::ascii(b"\x08\x7fwrong\x15sk-az\x08b!\x7f\n");
        assert_eq!(edit_key(&mut console, LABEL).unwrap().as_str(), "sk-ab");
        assert_eq!(console.output, format!("{LABEL}\r\n"));
    }

    #[test]
    fn ctrl_c_ctrl_d_and_eof_cancel_without_disclosing_partial_entry() {
        for ending in [Some(3), Some(4), None] {
            let mut input = b"private"
                .iter()
                .copied()
                .map(u32::from)
                .collect::<Vec<_>>();
            input.extend(ending);
            let mut console = FakeConsole::new(input);
            let error = edit_key(&mut console, LABEL).unwrap_err().to_string();
            assert!(!error.contains("private"));
            assert_eq!(console.output, format!("{LABEL}\r\n"));
        }
    }

    #[test]
    fn rejects_non_ascii_and_unhandled_controls_without_disclosing_input() {
        for invalid in [0, 9, 11, 12, 14, 26, 27, 31, 128, 233, 0xd800, 0xffff] {
            let mut console = FakeConsole::new([u32::from(b's'), invalid, 13]);
            assert!(edit_key(&mut console, LABEL).is_err());
            assert_eq!(console.output, format!("{LABEL}\r\n"));
        }
    }

    #[test]
    fn exact_length_limit_and_empty_entry() {
        let mut console = FakeConsole::new(std::iter::repeat_n(65, MAX_KEY_LEN).chain([13]));
        assert_eq!(edit_key(&mut console, LABEL).unwrap().len(), MAX_KEY_LEN);
        let mut console = FakeConsole::new(std::iter::repeat_n(65, MAX_KEY_LEN + 1).chain([13]));
        assert!(edit_key(&mut console, LABEL).is_err());
        let mut console = FakeConsole::ascii(b"\r");
        assert!(edit_key(&mut console, LABEL).is_err());
    }

    #[test]
    fn propagates_read_and_prompt_errors_without_reading_or_echoing_secrets() {
        let mut console = FakeConsole::ascii(b"private\r");
        console.fail_write = true;
        assert!(edit_key(&mut console, LABEL).is_err());
        assert_eq!(console.input.len(), 8);
        assert!(console.output.is_empty());
        console.fail_write = false;
        console.fail_read = true;
        assert!(edit_key(&mut console, LABEL).is_err());
        assert_eq!(console.output, format!("{LABEL}\r\n"));
    }
}
