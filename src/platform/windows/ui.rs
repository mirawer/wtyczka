//! Native notification-area cells and a registered (not hooked) global hotkey.
//! All HWND, HICON and GDI ownership stays on the dedicated message-pump thread.

use std::{
    cell::Cell,
    ffi::c_void,
    mem::size_of,
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result, anyhow, bail};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
        DeleteObject, HBITMAP,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        Threading::GetCurrentThreadId,
    },
    UI::{
        Input::KeyboardAndMouse::*,
        Shell::{
            NIF_ICON, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4,
            NOTIFYICONDATAW, Shell_NotifyIconW,
        },
        WindowsAndMessaging::*,
    },
};

use crate::{
    display::{State, cell_states, render_cell},
    mode::{AnswerMode, SelectionMode},
};

const HOTKEY_ID: i32 = 1;
const WAKE: u32 = WM_APP + 1;
const RESTORE_TIMER: usize = 1;
static NEXT_CLASS: AtomicU64 = AtomicU64::new(1);

pub struct Ui {
    dispatch: Arc<Dispatch>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl Ui {
    pub async fn spawn(
        mode: AnswerMode,
        selection: SelectionMode,
        keybind: &str,
        triggers: UnboundedSender<()>,
    ) -> Result<Self> {
        Self::start(
            mode,
            selection,
            Some((parse_keybind(keybind)?, keybind.to_owned(), triggers)),
        )
        .await
    }

    /// Synthetic-answer demo: no global shortcut is registered or altered.
    pub async fn demo(mode: AnswerMode, selection: SelectionMode) -> Result<Self> {
        Self::start(mode, selection, None).await
    }

    async fn start(
        mode: AnswerMode,
        selection: SelectionMode,
        binding: Option<(Hotkey, String, UnboundedSender<()>)>,
    ) -> Result<Self> {
        let (commands, receiver) = mpsc::channel();
        let dispatch = Arc::new(Dispatch {
            commands,
            endpoint: Mutex::new(None),
        });
        let (ready, startup) = oneshot::channel();
        let thread_dispatch = Arc::clone(&dispatch);
        let thread = thread::Builder::new()
            .name("screen-answer-win32".into())
            .spawn(move || {
                let mut native = match Native::new(mode, selection, binding, thread_dispatch) {
                    Ok(native) => native,
                    Err(error) => {
                        let message = format!("{error:#}");
                        let _ = ready.send(Err(error));
                        return Err(anyhow!(message));
                    }
                };
                native.publish();
                // Cancellation of spawn must not leave a detached native UI running.
                if ready.send(Ok(())).is_err() {
                    return Ok(());
                }
                native.run(receiver)
            })
            .context("Cannot start the Windows UI thread")?;
        // Construct the guard before awaiting: dropping this future also requests shutdown.
        let ui = Self {
            dispatch,
            thread: Some(thread),
        };
        startup
            .await
            .context("Windows UI thread stopped during startup")??;
        Ok(ui)
    }

    pub async fn update(&self, state: State) -> Result<()> {
        let (reply, result) = oneshot::channel();
        self.dispatch.send(Command::Update(state, reply))?;
        result
            .await
            .context("Windows UI thread stopped before applying the update")?
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        // If the endpoint is already gone, joining still reports the actual thread failure.
        let _ = self.dispatch.send(Command::Shutdown);
        tokio::task::spawn_blocking(move || {
            thread
                .join()
                .map_err(|_| anyhow!("Windows UI thread panicked"))?
        })
        .await
        .context("Cannot join the Windows UI thread")?
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        // No blocking join in Drop (which can run on a Tokio executor thread).
        // The native thread owns its cleanup guards, even when this handle is detached.
        let _ = self.dispatch.send(Command::Shutdown);
    }
}

/// Parsing only: registration/conflict checks happen on the UI thread during spawn.
pub fn validate_keybind(keybind: &str) -> Result<()> {
    parse_keybind(keybind).map(|_| ())
}

/// A short-lived thread checks ownership before the user is asked for an API key.
/// No hook or OS setting is installed, and thread exit also releases its hotkey.
pub fn check_keybind_available(keybind: &str) -> Result<()> {
    let hotkey = parse_keybind(keybind)?;
    thread::spawn(move || -> Result<()> {
        const PROBE_ID: i32 = 0x53A9;
        if unsafe { RegisterHotKey(null_mut(), PROBE_ID, hotkey.modifiers | MOD_NOREPEAT, hotkey.key) } == 0 {
            bail!("Skrót jest zajęty przez Windows lub inną aplikację. Print Screen może być przejęty przez Narzędzie Wycinanie. Wybierz np. Ctrl+Shift+F8; ustawień Windows nie zmieniamy.");
        }
        if unsafe { UnregisterHotKey(null_mut(), PROBE_ID) } == 0 {
            return Err(win_error("UnregisterHotKey (preflight)"));
        }
        Ok(())
    }).join().map_err(|_| anyhow!("Nie udało się sprawdzić dostępności skrótu."))?
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hotkey {
    modifiers: u32,
    key: u32,
}

fn parse_keybind(input: &str) -> Result<Hotkey> {
    if input.chars().any(char::is_control) {
        bail!("A Windows keybind cannot contain control characters");
    }
    let mut parts: Vec<_> = input.split('+').map(str::trim).collect();
    let key = parts.pop().filter(|key| !key.is_empty()).context(
        "Empty Windows keybind; use Print, Ctrl+Shift+F8, or another modifier+key combination",
    )?;
    let mut modifiers = 0;
    for part in parts {
        let flag = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => MOD_CONTROL,
            "shift" => MOD_SHIFT,
            "alt" => MOD_ALT,
            "super" | "win" | "windows" | "mod" => MOD_WIN,
            _ => bail!(
                "Unsupported Windows keybind modifier {part:?}; use Ctrl, Shift, Alt, or Win/Super/Mod"
            ),
        };
        if modifiers & flag != 0 {
            bail!("Repeated Windows keybind modifier {part:?}");
        }
        modifiers |= flag;
    }
    let name = key.to_ascii_lowercase();
    let vk = if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() {
        u32::from(name.as_bytes()[0].to_ascii_uppercase())
    } else if let Some(number) = name.strip_prefix('f').and_then(|s| s.parse::<u32>().ok()) {
        if !(1..=24).contains(&number) {
            bail!("Unsupported Windows function key {key:?}; use F1 through F24");
        }
        u32::from(VK_F1) + number - 1
    } else {
        u32::from(match name.as_str() {
            "print" | "printscreen" | "print-screen" | "print_screen" | "prtsc" | "prtscr"
            | "snapshot" => VK_SNAPSHOT,
            "backspace" | "back" => VK_BACK,
            "tab" => VK_TAB,
            "enter" | "return" => VK_RETURN,
            "esc" | "escape" => VK_ESCAPE,
            "space" | "spacebar" => VK_SPACE,
            "home" => VK_HOME,
            "end" => VK_END,
            "pageup" | "page_up" | "pgup" | "prior" => VK_PRIOR,
            "pagedown" | "page_down" | "pgdn" | "next" => VK_NEXT,
            "insert" | "ins" => VK_INSERT,
            "delete" | "del" => VK_DELETE,
            "left" => VK_LEFT,
            "right" => VK_RIGHT,
            "up" => VK_UP,
            "down" => VK_DOWN,
            "pause" | "break" => VK_PAUSE,
            "capslock" | "caps_lock" => VK_CAPITAL,
            "numlock" | "num_lock" => VK_NUMLOCK,
            "scrolllock" | "scroll_lock" => VK_SCROLL,
            "menu" | "apps" => VK_APPS,
            "minus" | "-" => VK_OEM_MINUS,
            "plus" | "equal" | "equals" | "=" => VK_OEM_PLUS,
            "comma" | "," => VK_OEM_COMMA,
            "period" | "dot" | "." => VK_OEM_PERIOD,
            "semicolon" | ";" => VK_OEM_1,
            "slash" | "/" => VK_OEM_2,
            "grave" | "backtick" | "`" => VK_OEM_3,
            "bracketleft" | "[" => VK_OEM_4,
            "backslash" | "\\" => VK_OEM_5,
            "bracketright" | "]" => VK_OEM_6,
            "apostrophe" | "quote" | "'" => VK_OEM_7,
            "multiply" => VK_MULTIPLY,
            "add" => VK_ADD,
            "subtract" => VK_SUBTRACT,
            "decimal" => VK_DECIMAL,
            "divide" => VK_DIVIDE,
            "numpad0" => VK_NUMPAD0,
            "numpad1" => VK_NUMPAD1,
            "numpad2" => VK_NUMPAD2,
            "numpad3" => VK_NUMPAD3,
            "numpad4" => VK_NUMPAD4,
            "numpad5" => VK_NUMPAD5,
            "numpad6" => VK_NUMPAD6,
            "numpad7" => VK_NUMPAD7,
            "numpad8" => VK_NUMPAD8,
            "numpad9" => VK_NUMPAD9,
            _ => bail!(
                "Unsupported Windows key name {key:?}; use Print, A-Z, 0-9, F1-F24, or a named navigation key"
            ),
        })
    };
    Ok(Hotkey { modifiers, key: vk })
}

enum Command {
    Update(State, oneshot::Sender<Result<()>>),
    Shutdown,
}

#[derive(Clone, Copy)]
struct Endpoint {
    // HWND is thread-affine. Only its numeric value is shared, exclusively for PostMessage.
    hwnd: usize,
    thread_id: u32,
}

struct Dispatch {
    commands: mpsc::Sender<Command>,
    endpoint: Mutex<Option<Endpoint>>,
}

impl Dispatch {
    fn send(&self, command: Command) -> Result<()> {
        // Serializes posting against HWND destruction, avoiding handle-reuse races.
        let endpoint = self.endpoint.lock().unwrap_or_else(|e| e.into_inner());
        let endpoint = endpoint.as_ref().context("Windows UI is not running")?;
        self.commands
            .send(command)
            .context("Windows UI command channel is closed")?;
        if unsafe { PostMessageW(endpoint.hwnd as HWND, WAKE, 0, 0) } == 0 {
            let error = std::io::Error::last_os_error();
            // An exhausted/broken window queue must not leave shutdown waiting forever.
            unsafe { PostThreadMessageW(endpoint.thread_id, WM_QUIT, 0, 0) };
            return Err(error).context("Cannot wake the Windows UI thread");
        }
        Ok(())
    }
}

struct Events {
    triggers: Option<UnboundedSender<()>>,
    taskbar_created: u32,
    restore_requested: Cell<bool>,
    theme_changed: Cell<bool>,
}

// The Events allocation remains alive until after DestroyWindow; callbacks never own it.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let events = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Events;
        if !events.is_null() {
            let events = &*events;
            if message == WM_HOTKEY && wparam == HOTKEY_ID as usize {
                if let Some(triggers) = &events.triggers {
                    let _ = triggers.send(());
                }
                return 0;
            }
            if message == events.taskbar_created {
                events.restore_requested.set(true);
                PostMessageW(hwnd, WAKE, 0, 0);
                return 0;
            }
            if matches!(
                message,
                WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE
            ) {
                events.theme_changed.set(true);
                PostMessageW(hwnd, WAKE, 0, 0);
                return 0;
            }
        }
        match message {
            WM_CLOSE => {
                PostQuitMessage(0);
                0
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

#[derive(Default)]
struct Slot {
    // Desired icon, retained even when Explorer cannot currently accept it.
    icon: Option<OwnedIcon>,
    present: bool,
    dirty: bool,
}

struct Native {
    dispatch: Arc<Dispatch>,
    instance: HINSTANCE,
    class: Vec<u16>,
    class_registered: bool,
    hwnd: HWND,
    hotkey_registered: bool,
    events: Box<Events>,
    mode: AnswerMode,
    state: State,
    foreground: Foreground,
    slots: Vec<Slot>,
}

impl Native {
    fn new(
        mode: AnswerMode,
        selection: SelectionMode,
        binding: Option<(Hotkey, String, UnboundedSender<()>)>,
        dispatch: Arc<Dispatch>,
    ) -> Result<Self> {
        let (binding, triggers) = match binding {
            Some((hotkey, keybind, triggers)) => (Some((hotkey, keybind)), Some(triggers)),
            None => (None, None),
        };
        let count = if selection == SelectionMode::Multiple {
            usize::from(mode.max_choices())
        } else {
            1
        };
        let class = wide(&format!(
            "ScreenAnswerHidden-{}-{}",
            std::process::id(),
            NEXT_CLASS.fetch_add(1, Ordering::Relaxed)
        ));
        let mut native = Self {
            dispatch,
            instance: unsafe { GetModuleHandleW(null()) },
            class,
            class_registered: false,
            hwnd: null_mut(),
            hotkey_registered: false,
            events: Box::new(Events {
                triggers,
                taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
                restore_requested: Cell::new(false),
                theme_changed: Cell::new(false),
            }),
            mode,
            state: State::Idle,
            foreground: system_foreground(),
            slots: (0..count).map(|_| Slot::default()).collect(),
        };
        if native.instance.is_null() {
            return Err(win_error("GetModuleHandleW"));
        }
        if native.events.taskbar_created == 0 {
            return Err(win_error("RegisterWindowMessageW(TaskbarCreated)"));
        }
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: native.instance,
            lpszClassName: native.class.as_ptr(),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(win_error("RegisterClassW"));
        }
        native.class_registered = true;
        // A nonvisible top-level tool window, NOT HWND_MESSAGE: Explorer broadcasts
        // TaskbarCreated only to top-level windows. No caption or answer UI is shown.
        native.hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                native.class.as_ptr(),
                wide("").as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                native.instance,
                native.events.as_ref() as *const Events as *const c_void,
            )
        };
        if native.hwnd.is_null() {
            return Err(win_error("CreateWindowExW"));
        }
        if let Some((hotkey, keybind)) = binding {
            if unsafe {
                RegisterHotKey(
                    native.hwnd,
                    HOTKEY_ID,
                    hotkey.modifiers | MOD_NOREPEAT,
                    hotkey.key,
                )
            } == 0
            {
                let error = std::io::Error::last_os_error();
                bail!(
                    "Cannot register Windows hotkey {keybind:?}: {error}. It may already be registered by another application or reserved by Windows. PrintScreen/Print may be owned by Snipping Tool (including the Windows Print Screen screen-snipping setting). Choose a different --keybind, e.g. Ctrl+Shift+F8; screen-answer does not change Windows settings, install hooks, or require administrator access."
                );
            }
            native.hotkey_registered = true;
        }
        native.cache_state(State::Idle, native.foreground)?;
        // Startup remains strict: do not silently succeed without a working tray.
        native
            .reconcile()
            .context("Cannot initialize the Windows notification area")?;
        Ok(native)
    }

    fn publish(&self) {
        *self
            .dispatch
            .endpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Endpoint {
            hwnd: self.hwnd as usize,
            thread_id: unsafe { GetCurrentThreadId() },
        });
    }

    fn run(&mut self, commands: mpsc::Receiver<Command>) -> Result<()> {
        loop {
            let mut message = MSG::default();
            let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
            if result == -1 {
                return Err(win_error("GetMessageW"));
            }
            if result == 0 {
                bail!("Windows UI message loop stopped unexpectedly");
            }
            let restore = self.events.restore_requested.replace(false);
            let theme_changed = self.events.theme_changed.replace(false);
            if restore {
                // Explorer discarded every old ID, including pending trailing deletions.
                for slot in &mut self.slots {
                    slot.present = false;
                    slot.dirty = slot.icon.is_some();
                }
            }
            if restore || theme_changed {
                let foreground = system_foreground();
                if foreground != self.foreground {
                    self.cache_state(self.state, foreground)?;
                }
                self.restore_or_retry()?;
            }
            if message.hwnd == self.hwnd && message.message == WAKE {
                loop {
                    match commands.try_recv() {
                        Ok(Command::Update(state, reply)) => {
                            let _ = reply.send(self.update(state));
                        }
                        Ok(Command::Shutdown) => return Ok(()),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                    }
                }
            } else if message.hwnd == self.hwnd
                && message.message == WM_TIMER
                && message.wParam == RESTORE_TIMER
            {
                self.restore_or_retry()?;
            } else {
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }

    fn update(&mut self, state: State) -> Result<()> {
        self.cache_state(state, self.foreground)?;
        // Temporary Shell_NotifyIcon failures must not stop the capture daemon.
        self.restore_or_retry()
    }

    fn cache_state(&mut self, state: State, foreground: Foreground) -> Result<()> {
        let cells = cell_states(state, self.mode, self.slots.len());
        // Prepare every handle first. A rendering failure leaves the previous cache intact.
        let mut icons: Vec<_> = cells
            .into_iter()
            .enumerate()
            .map(|(index, state)| {
                state
                    .map(|state| {
                        let mut icon = render_cell(state, self.mode, index > 0);
                        tint_foreground(&mut icon, foreground);
                        OwnedIcon::new(&icon)
                    })
                    .transpose()
            })
            .collect::<Result<_>>()?;
        if icons.len() != self.slots.len() {
            bail!("Shared display returned an incorrect Windows tray cell count");
        }
        // Explorer places successive notification icons to the left of the
        // existing one. Reverse rendered answers, not glyph prefix positions.
        reverse_active_cells(&mut icons);
        for (slot, icon) in self.slots.iter_mut().zip(icons) {
            slot.dirty = icon.is_some();
            // Shell keeps its own copy of the last published icon, so superseded
            // HICONs can be released even if publishing the new state must wait.
            slot.icon = icon;
        }
        self.state = state;
        self.foreground = foreground;
        Ok(())
    }

    fn reconcile(&mut self) -> Result<()> {
        let mut deletion_error = None;
        // A failed trailing deletion remains pending. Do not publish a different
        // answer alongside stale trailing digits; retries always clear them first.
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.icon.is_none() && slot.present {
                match notify(self.hwnd, index, NIM_DELETE, None) {
                    Ok(()) => slot.present = false,
                    Err(error) => deletion_error = Some(error),
                }
            }
        }
        if let Some(error) = deletion_error {
            return Err(error);
        }
        let mut publication_error = None;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if (slot.dirty || !slot.present)
                && let Some(icon) = &slot.icon
            {
                let action = if slot.present { NIM_MODIFY } else { NIM_ADD };
                // Explorer may have lost an ID before TaskbarCreated is dispatched.
                // Conversely, a timed-out add may already have reached Explorer.
                let fallback = if slot.present { NIM_ADD } else { NIM_MODIFY };
                match notify(self.hwnd, index, action, Some(icon.0))
                    .or_else(|_| notify(self.hwnd, index, fallback, Some(icon.0)))
                {
                    Ok(()) => {
                        slot.present = true;
                        slot.dirty = false;
                    }
                    Err(error) => publication_error = Some(error),
                }
            }
        }
        if let Some(error) = publication_error {
            return Err(error);
        }
        Ok(())
    }

    fn restore_or_retry(&mut self) -> Result<()> {
        if self.reconcile().is_err() {
            // Retain the latest desired state during Explorer downtime. Later updates
            // replace this cache, never replaying an obsolete answer after recovery.
            if unsafe { SetTimer(self.hwnd, RESTORE_TIMER, 1000, None) } == 0 {
                return Err(win_error("SetTimer (restore notification-area icons)"));
            }
        } else {
            unsafe { KillTimer(self.hwnd, RESTORE_TIMER) };
        }
        Ok(())
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        // Stop other threads posting before destroying/reusing the window handle.
        *self
            .dispatch
            .endpoint
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        if !self.hwnd.is_null() {
            unsafe {
                KillTimer(self.hwnd, RESTORE_TIMER);
                if self.hotkey_registered {
                    UnregisterHotKey(self.hwnd, HOTKEY_ID);
                }
            }
            for (index, slot) in self.slots.iter_mut().enumerate() {
                // Also retry deletion of IDs whose earlier add/version/delete failed.
                let _ = notify(self.hwnd, index, NIM_DELETE, None);
                slot.icon = None;
            }
            unsafe { DestroyWindow(self.hwnd) };
        }
        if self.class_registered {
            unsafe { UnregisterClassW(self.class.as_ptr(), self.instance) };
        }
    }
}

fn notify(hwnd: HWND, slot: usize, action: u32, icon: Option<HICON>) -> Result<()> {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: slot as u32 + 1,
        // Deliberately no NIF_TIP, NIF_SHOWTIP, NIF_INFO, callback, or text.
        uFlags: if icon.is_some() { NIF_ICON } else { 0 },
        hIcon: icon.unwrap_or(null_mut()),
        ..Default::default()
    };
    if unsafe { Shell_NotifyIconW(action, &data) } == 0 {
        // Shell_NotifyIcon does not promise meaningful GetLastError information.
        bail!(
            "Windows notification area rejected operation {action} for cell {}; Explorer may be unavailable or restarting",
            slot + 1
        );
    }
    if action == NIM_ADD {
        // Version 4 suppresses the standard tooltip unless NIF_SHOWTIP is supplied.
        data.uFlags = 0;
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        if unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) } == 0 {
            unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
            bail!(
                "Cannot set Windows notification-area icon version for cell {}",
                slot + 1
            );
        }
    }
    Ok(())
}

struct OwnedBitmap(HBITMAP);
impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.0) };
    }
}

struct OwnedIcon(HICON);
impl OwnedIcon {
    fn new(icon: &crate::display::Icon) -> Result<Self> {
        if icon.width <= 0 || icon.height <= 0 {
            bail!("Invalid Windows tray icon dimensions");
        }
        let width = icon.width as usize;
        let height = icon.height as usize;
        let length = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(4))
            .context("Windows tray icon dimensions overflow")?;
        if icon.data.len() != length {
            bail!("Invalid Windows tray icon pixel count");
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: icon.width,
                biHeight: -icon.height, // top-down, just like the shared renderer
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = null_mut();
        let color = unsafe {
            CreateDIBSection(null_mut(), &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0)
        };
        if color.is_null() {
            return Err(win_error("CreateDIBSection"));
        }
        let color = OwnedBitmap(color);
        if bits.is_null() {
            bail!("CreateDIBSection returned no pixel buffer");
        }
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits as *mut u8, length) };
        // Win32 expects premultiplied BGRA bytes; shared rendering uses A,R,G,B bytes.
        for (src, dst) in icon.data.chunks_exact(4).zip(pixels.chunks_exact_mut(4)) {
            let alpha = u16::from(src[0]);
            dst[0] = ((u16::from(src[3]) * alpha + 127) / 255) as u8;
            dst[1] = ((u16::from(src[2]) * alpha + 127) / 255) as u8;
            dst[2] = ((u16::from(src[1]) * alpha + 127) / 255) as u8;
            dst[3] = src[0];
        }
        // A monochrome CreateBitmap scan line is WORD-aligned (not DWORD-aligned).
        let stride = width.div_ceil(16) * 2;
        let mut mask = vec![0u8; stride * height];
        for y in 0..height {
            for x in 0..width {
                if icon.data[(y * width + x) * 4] == 0 {
                    mask[y * stride + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        let mask = unsafe { CreateBitmap(icon.width, icon.height, 1, 1, mask.as_ptr().cast()) };
        if mask.is_null() {
            return Err(win_error("CreateBitmap (transparency mask)"));
        }
        let mask = OwnedBitmap(mask);
        let info = ICONINFO {
            fIcon: 1,
            hbmMask: mask.0,
            hbmColor: color.0,
            ..Default::default()
        };
        let handle = unsafe { CreateIconIndirect(&info) };
        if handle.is_null() {
            return Err(win_error("CreateIconIndirect"));
        }
        // CreateIconIndirect copies both bitmaps; their guards delete them here.
        Ok(Self(handle))
    }
}
impl Drop for OwnedIcon {
    fn drop(&mut self) {
        unsafe { DestroyIcon(self.0) };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Foreground {
    Black,
    White,
}

fn system_foreground() -> Foreground {
    let mut light = 0u32;
    let mut length = size_of::<u32>() as u32;
    // Read-only, using the predefined HKCU handle: no key to close and no setting
    // is created or modified. Older Windows versions without this value use a
    // dark taskbar by default, so white is the conservative fallback.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
            wide("SystemUsesLightTheme").as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut light as *mut u32).cast(),
            &mut length,
        )
    };
    if result == 0 && light != 0 {
        Foreground::Black
    } else {
        Foreground::White
    }
}

fn tint_foreground(icon: &mut crate::display::Icon, foreground: Foreground) {
    let value = match foreground {
        Foreground::Black => 0,
        Foreground::White => 255,
    };
    for pixel in icon.data.chunks_exact_mut(4) {
        if pixel[0] != 0 {
            pixel[1..].fill(value);
        }
    }
}

fn reverse_active_cells<T>(cells: &mut [Option<T>]) {
    let active = cells.iter().take_while(|cell| cell.is_some()).count();
    cells[..active].reverse();
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn win_error(operation: &str) -> anyhow::Error {
    anyhow!("{operation} failed: {}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_order_keeps_apostrophes_with_their_logical_answers() {
        use crate::answer::{Answer, Choices};
        for mode in [AnswerMode::Digits, AnswerMode::Dots] {
            let state = State::Answer(Answer::Multiple(Choices::new(&[1, 3, 6]).unwrap()));
            let mut icons: Vec<_> = cell_states(state, mode, 5)
                .into_iter()
                .enumerate()
                .map(|(index, state)| state.map(|state| render_cell(state, mode, index > 0)))
                .collect();
            reverse_active_cells(&mut icons);
            let expected: Vec<_> = [6, 3, 1]
                .into_iter()
                .enumerate()
                .map(|(index, n)| {
                    Some(render_cell(
                        State::Answer(Answer::Choice(n)),
                        mode,
                        index < 2,
                    ))
                })
                .chain([None, None])
                .collect();
            assert_eq!(icons, expected);
        }
        let mut idle = vec![Some(State::Idle), None, None];
        reverse_active_cells(&mut idle);
        assert_eq!(idle, [Some(State::Idle), None, None]);
    }

    #[test]
    fn theme_tint_preserves_alpha_and_pixel_positions() {
        let mut icon = crate::display::Icon {
            width: 3,
            height: 1,
            data: vec![255, 255, 255, 255, 0, 0, 0, 0, 128, 255, 255, 255],
        };
        tint_foreground(&mut icon, Foreground::Black);
        assert_eq!(icon.data, [255, 0, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0]);
        tint_foreground(&mut icon, Foreground::White);
        assert_eq!(
            icon.data,
            [255, 255, 255, 255, 0, 0, 0, 0, 128, 255, 255, 255]
        );
        assert_eq!((icon.width, icon.height), (3, 1));
    }

    #[test]
    fn print_aliases() {
        for name in [
            "Print",
            "PrintScreen",
            "print-screen",
            "Print_Screen",
            "PrtSc",
            "snapshot",
        ] {
            assert_eq!(
                parse_keybind(name).unwrap(),
                Hotkey {
                    modifiers: 0,
                    key: u32::from(VK_SNAPSHOT)
                }
            );
        }
    }

    #[test]
    fn letters_digits_and_all_function_keys() {
        for key in b'A'..=b'Z' {
            assert_eq!(
                parse_keybind(&(key as char).to_string()).unwrap().key,
                u32::from(key)
            );
            assert_eq!(
                parse_keybind(&(key as char).to_ascii_lowercase().to_string())
                    .unwrap()
                    .key,
                u32::from(key)
            );
        }
        for key in b'0'..=b'9' {
            assert_eq!(
                parse_keybind(&(key as char).to_string()).unwrap().key,
                u32::from(key)
            );
        }
        for number in 1..=24 {
            assert_eq!(
                parse_keybind(&format!("F{number}")).unwrap().key,
                u32::from(VK_F1) + number - 1
            );
        }
    }

    #[test]
    fn modifier_aliases_and_common_keys() {
        for name in ["Super", "Win", "Windows", "Mod"] {
            assert_eq!(
                parse_keybind(&format!("{name}+a")).unwrap().modifiers,
                MOD_WIN
            );
        }
        let hotkey = parse_keybind(" control + SHIFT + Alt + Win + F24 ").unwrap();
        assert_eq!(
            hotkey.modifiers,
            MOD_CONTROL | MOD_SHIFT | MOD_ALT | MOD_WIN
        );
        assert_eq!(hotkey.key, u32::from(VK_F24));
        for name in [
            "Escape",
            "Return",
            "Space",
            "Backspace",
            "Tab",
            "Home",
            "End",
            "PageUp",
            "Page_Down",
            "Insert",
            "Delete",
            "Left",
            "Right",
            "Up",
            "Down",
            "Pause",
            "Menu",
            "Numpad0",
        ] {
            assert!(validate_keybind(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn rejects_invalid_and_unsupported_keybinds_without_registering() {
        for name in [
            "",
            " ",
            "Ctrl+",
            "+A",
            "Ctrl++A",
            "Ctrl+Ctrl+A",
            "Win+Mod+A",
            "Super+Win+A",
            "Alt+alt+A",
            "F0",
            "F25",
            "NoSuchKey",
            "Hyper+A",
            "Ctrl",
            "A+B",
            "Print\n",
            "Ctrl+\tA",
            "💻",
            "Ctrl+Shift",
        ] {
            assert!(validate_keybind(name).is_err(), "{name:?}");
        }
    }
}
