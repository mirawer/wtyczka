use anyhow::{Context, Result, bail};
use std::io;
use zeroize::{Zeroize, Zeroizing};

const MAX_KEY_LEN: usize = 4096;

const ASCII_ONLY: &str =
    "Klucz API może zawierać tylko widoczne znaki ASCII. Wklej sam klucz, bez dodatkowego tekstu.";

#[derive(Debug)]
pub struct InvalidKey(&'static str);
impl std::fmt::Display for InvalidKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for InvalidKey {}

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
    /// Only requested after an explicit Ctrl+V, never on normal typing.
    fn read_paste(&mut self) -> io::Result<Option<Zeroizing<Vec<u16>>>> {
        Ok(None)
    }
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
                if bytes[..len].iter().all(|b| *b == b' ') {
                    return Err(InvalidKey(
                        "Klucz nie może być pusty. Wklej go tutaj i naciśnij Enter.",
                    )
                    .into());
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
            22 => {
                let paste = console
                    .read_paste()
                    .map_err(|_| {
                        InvalidKey("Nie udało się odczytać tekstu do wklejenia. Spróbuj ponownie.")
                    })?
                    .ok_or(InvalidKey(
                        "Brak tekstu do wklejenia. Użyj funkcji Wklej swojego terminala.",
                    ))?;
                append_paste(&mut bytes, &mut len, &paste)?;
            }
            0x20..=0x7e => {
                if len == MAX_KEY_LEN {
                    return Err(InvalidKey("Klucz API przekracza limit 4096 znaków.").into());
                }
                bytes[len] = *unit as u8;
                len += 1;
            }
            _ => return Err(InvalidKey(ASCII_ONLY).into()),
        }
    }
}

fn append_paste(bytes: &mut [u8; MAX_KEY_LEN], len: &mut usize, paste: &[u16]) -> Result<()> {
    // Copy buttons sometimes append CR/LF. They are not part of the key and
    // must not silently submit the prompt; the user still presses Enter.
    let end = paste
        .iter()
        .rposition(|c| !matches!(c, 10 | 13))
        .map_or(0, |i| i + 1);
    let paste = &paste[..end];
    if paste.is_empty() {
        return Err(InvalidKey("Schowek nie zawiera klucza API.").into());
    }
    if paste.len() > MAX_KEY_LEN - *len {
        return Err(InvalidKey("Klucz API przekracza limit 4096 znaków.").into());
    }
    if !paste.iter().all(|c| (0x20..=0x7e).contains(c)) {
        return Err(InvalidKey(ASCII_ONLY).into());
    }
    for unit in paste {
        bytes[*len] = *unit as u8;
        *len += 1;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(windows)]
use windows as platform;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

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
        paste: Option<Zeroizing<Vec<u16>>>,
        paste_reads: usize,
    }
    impl FakeConsole {
        fn new(input: impl IntoIterator<Item = u32>) -> Self {
            Self {
                input: input.into_iter().collect(),
                output: String::new(),
                fail_read: false,
                fail_write: false,
                paste: None,
                paste_reads: 0,
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
        fn read_paste(&mut self) -> io::Result<Option<Zeroizing<Vec<u16>>>> {
            assert_eq!(self.output, LABEL);
            self.paste_reads += 1;
            Ok(self.paste.take())
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
    fn ascii_entry_never_echoes_or_reads_clipboard() {
        let mut console = FakeConsole::ascii(b"sk-instant-paste_123\r");
        assert_eq!(
            edit_key(&mut console, LABEL).unwrap().as_str(),
            "sk-instant-paste_123"
        );
        assert_eq!(console.paste_reads, 0);
        assert_eq!(console.output, format!("{LABEL}\r\n"));
    }

    #[test]
    fn explicit_ctrl_v_pastes_without_echo_or_automatic_enter() {
        let mut console = FakeConsole::ascii(b"\x16Z\r");
        console.paste = Some(Zeroizing::new("test-key\r\n".encode_utf16().collect()));
        assert_eq!(edit_key(&mut console, LABEL).unwrap().as_str(), "test-keyZ");
        assert_eq!(console.paste_reads, 1);
        assert_eq!(console.output, format!("{LABEL}\r\n"));
    }

    #[test]
    fn clipboard_text_is_validated_without_disclosing_it() {
        for paste in [
            "private\nsecond",
            "private\u{200b}",
            "private\t",
            "private\0",
            "\r\n",
            "",
        ]
        .into_iter()
        .map(Some)
        .chain([None])
        {
            let mut console = FakeConsole::ascii(b"\x16\r");
            console.paste = paste.map(|text| Zeroizing::new(text.encode_utf16().collect()));
            let err = edit_key(&mut console, LABEL).unwrap_err();
            assert!(err.is::<InvalidKey>());
            assert!(!err.to_string().contains("private"));
            assert_eq!(console.output, format!("{LABEL}\r\n"));
        }
    }

    #[test]
    fn clipboard_length_limit_includes_already_typed_characters() {
        let mut console = FakeConsole::ascii(b"\x16\r");
        console.paste = Some(Zeroizing::new(vec![65; MAX_KEY_LEN]));
        assert_eq!(edit_key(&mut console, LABEL).unwrap().len(), MAX_KEY_LEN);
        let mut console = FakeConsole::ascii(b"X\x16\r");
        console.paste = Some(Zeroizing::new(vec![65; MAX_KEY_LEN]));
        assert!(
            edit_key(&mut console, LABEL)
                .unwrap_err()
                .is::<InvalidKey>()
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
        for empty in [b"\r".as_slice(), b"   \r"] {
            let mut console = FakeConsole::ascii(empty);
            assert!(
                edit_key(&mut console, LABEL)
                    .unwrap_err()
                    .is::<InvalidKey>()
            );
        }
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
