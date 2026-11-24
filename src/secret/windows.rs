use super::Console;
use std::{
    io,
    ptr::null_mut,
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    },
    System::{
        Console::{
            ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
            ENABLE_VIRTUAL_TERMINAL_INPUT, FlushConsoleInputBuffer, GetConsoleMode, ReadConsoleW,
            SetConsoleMode, WriteConsoleW,
        },
        DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard},
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        Ole::CF_UNICODETEXT,
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
    clipboard_reader: fn() -> io::Result<Zeroizing<Vec<u16>>>,
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
            clipboard_reader: read_clipboard,
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

    fn read_paste(&mut self) -> io::Result<Option<Zeroizing<Vec<u16>>>> {
        (self.clipboard_reader)().map(Some)
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

/// Read-only, on-demand clipboard access for consoles that deliver Ctrl+V
/// as U+0016 in raw input mode. Never clear or replace clipboard contents.
fn read_clipboard() -> io::Result<Zeroizing<Vec<u16>>> {
    let deadline = Instant::now() + Duration::from_millis(200);
    while unsafe { OpenClipboard(null_mut()) } == 0 {
        if Instant::now() >= deadline {
            return Err(io::Error::last_os_error());
        }
        thread::sleep(Duration::from_millis(5));
    }
    struct Clipboard;
    impl Drop for Clipboard {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }
    let _clipboard = Clipboard;
    let handle = unsafe { GetClipboardData(u32::from(CF_UNICODETEXT)) };
    if handle.is_null() {
        return Err(io::Error::other("No Unicode text in clipboard"));
    }
    // Never allocate/read an unbounded clipboard value. The extra units
    // allow a maximum-length key followed by CR/LF and its terminating NUL.
    let count = (unsafe { GlobalSize(handle) } / size_of::<u16>()).min(super::MAX_KEY_LEN + 3);
    if count == 0 {
        return Err(io::Error::other("Empty clipboard text"));
    }
    let data = unsafe { GlobalLock(handle) };
    if data.is_null() {
        return Err(io::Error::last_os_error());
    }
    struct Locked(windows_sys::Win32::Foundation::HGLOBAL);
    impl Drop for Locked {
        fn drop(&mut self) {
            unsafe { GlobalUnlock(self.0) };
        }
    }
    let _locked = Locked(handle);
    // CF_UNICODETEXT is a GlobalAlloc-backed UTF-16 buffer. Its allocation
    // size is checked above and it remains locked while these units are read.
    let units = unsafe { std::slice::from_raw_parts(data.cast::<u16>(), count) };
    let end = units
        .iter()
        .position(|c| *c == 0)
        .ok_or_else(|| io::Error::other("Clipboard text is too long or unterminated"))?;
    let mut copy = Zeroizing::new(Vec::new());
    copy.try_reserve_exact(end)
        .map_err(|_| io::Error::other("Cannot allocate paste buffer"))?;
    copy.extend_from_slice(&units[..end]);
    Ok(copy)
}

#[cfg(test)]
mod tests;
