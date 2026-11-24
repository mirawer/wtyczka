use super::*;
use crate::secret::{InvalidKey, edit_key};
use std::{
    process::Stdio,
    sync::atomic::{AtomicUsize, Ordering},
};
use windows_sys::Win32::{
    System::{
        Console::{
            COORD, GetConsoleWindow, INPUT_RECORD, INPUT_RECORD_0, KEY_EVENT, KEY_EVENT_RECORD,
            KEY_EVENT_RECORD_0, ReadConsoleOutputCharacterW, WriteConsoleInputW,
        },
        Threading::CREATE_NEW_CONSOLE,
    },
    UI::WindowsAndMessaging::{SW_HIDE, ShowWindow},
};

const FIXTURE_ENV: &str = "SCREEN_ANSWER_NATIVE_PASTE_FIXTURE";
const LABEL: &str = "Klucz API (ukryty): ";
const DUMMY: &str = "test-native-paste";
static CLIPBOARD_READS: AtomicUsize = AtomicUsize::new(0);

fn dummy_clipboard() -> io::Result<Zeroizing<Vec<u16>>> {
    CLIPBOARD_READS.fetch_add(1, Ordering::Relaxed);
    Ok(Zeroizing::new(
        format!("{DUMMY}\r\n").encode_utf16().collect(),
    ))
}

fn inject(terminal: &Terminal, units: impl IntoIterator<Item = u16>) -> io::Result<()> {
    // Inject raw Unicode units, not a system Ctrl+V gesture: the test must not
    // read, replace or clear the user's actual clipboard, even on a desktop PC.
    let records: Vec<_> = units
        .into_iter()
        .map(|unit| INPUT_RECORD {
            EventType: KEY_EVENT as u16,
            Event: INPUT_RECORD_0 {
                KeyEvent: KEY_EVENT_RECORD {
                    bKeyDown: 1,
                    wRepeatCount: 1,
                    uChar: KEY_EVENT_RECORD_0 { UnicodeChar: unit },
                    ..Default::default()
                },
            },
        })
        .collect();
    let mut written = 0;
    if unsafe {
        WriteConsoleInputW(
            terminal.input.0,
            records.as_ptr(),
            records.len() as u32,
            &mut written,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    assert_eq!(written as usize, records.len());
    Ok(())
}

fn console_mode(terminal: &Terminal) -> io::Result<u32> {
    let mut mode = 0;
    if unsafe { GetConsoleMode(terminal.input.0, &mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(mode)
}

#[tokio::test]
async fn native_ctrl_v_input_is_hidden_and_console_modes_are_restored() -> anyhow::Result<()> {
    // Never change the test runner's/user's console. A separate, bounded child
    // owns a fresh console; its clipboard provider is synthetic.
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "secret::windows::tests::console_fixture",
            "--nocapture",
        ])
        .env(FIXTURE_ENV, "1")
        .creation_flags(CREATE_NEW_CONSOLE)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(20), command.output()).await??;
    assert!(
        output.status.success(),
        "Console regression failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    Ok(())
}

#[test]
#[ignore = "isolated child fixture, invoked by the native console regression test"]
fn console_fixture() -> anyhow::Result<()> {
    if std::env::var(FIXTURE_ENV).as_deref() != Ok("1") {
        return Ok(());
    }
    let window = unsafe { GetConsoleWindow() };
    if !window.is_null() {
        unsafe { ShowWindow(window, SW_HIDE) };
    }

    let mut terminal = Terminal::open_hidden()?;
    let original = terminal.original;
    assert_eq!(console_mode(&terminal)? & ENABLE_ECHO_INPUT, 0);
    terminal.clipboard_reader = dummy_clipboard;
    inject(&terminal, [22, u16::from(b'Z'), 13])?;
    let key = edit_key(&mut terminal, LABEL)?;
    assert_eq!(key.as_str(), format!("{DUMMY}Z"));
    assert_eq!(CLIPBOARD_READS.load(Ordering::Relaxed), 1);
    terminal.restore()?;
    assert_eq!(console_mode(&terminal)?, original);
    drop(key);
    drop(terminal);

    // Normal paste delivery/typing is still accepted without clipboard reads.
    let mut terminal = Terminal::open_hidden()?;
    terminal.clipboard_reader = dummy_clipboard;
    inject(&terminal, DUMMY.encode_utf16().chain([13]))?;
    assert_eq!(edit_key(&mut terminal, LABEL)?.as_str(), DUMMY);
    assert_eq!(CLIPBOARD_READS.load(Ordering::Relaxed), 1);
    terminal.restore()?;
    assert_eq!(console_mode(&terminal)?, original);
    drop(terminal);

    // Validation errors and cancellation must restore echo too, without input
    // ever being displayed in the private console's screen buffer.
    for unit in [0x200b, 3] {
        let mut terminal = Terminal::open_hidden()?;
        inject(&terminal, [unit, 13])?;
        let error = edit_key(&mut terminal, LABEL).unwrap_err();
        if unit == 0x200b {
            assert!(error.is::<InvalidKey>());
        }
        terminal.restore()?;
        assert_eq!(console_mode(&terminal)?, original);
    }
    let output = Handle::open(&[67, 79, 78, 79, 85, 84, 36, 0], GENERIC_READ)?;
    let mut text = vec![0u16; 4096];
    let mut read = 0;
    if unsafe {
        ReadConsoleOutputCharacterW(
            output.0,
            text.as_mut_ptr(),
            text.len() as u32,
            COORD { X: 0, Y: 0 },
            &mut read,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    let text = String::from_utf16_lossy(&text[..read as usize]);
    assert!(text.contains(LABEL));
    assert!(
        !text.contains(DUMMY),
        "Hidden input was echoed in the fresh test console"
    );
    Ok(())
}
