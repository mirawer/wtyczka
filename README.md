# Screen Answer

[![Build](https://github.com/mirawer/wtyczka/actions/workflows/build.yml/badge.svg)](https://github.com/mirawer/wtyczka/actions/workflows/build.yml)
[![Latest release](https://img.shields.io/github/v/release/mirawer/wtyczka)](https://github.com/mirawer/wtyczka/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[Polski](README.pl.md) · [Changelog](CHANGELOG.md) · [Security](SECURITY.md)

A keyboard shortcut sends an image of the **entire active monitor** to your chosen AI API. The answer appears only in the system tray, as digits or dots on a transparent background. For your own practice and comparison with your own answer key.

Author: [mirawer](https://github.com/mirawer).

> **Version note:** these instructions target **v0.1.5 and later**. Older downloads run the wizard and ask for the API key on every start, keeping it only in memory; they do not support `--change`, `forget-key` or `--session-only`.

## Download and supported systems

- **[Windows 10/11 x64 — screen-answer.exe](https://github.com/mirawer/wtyczka/releases/latest/download/screen-answer.exe)**
- **[Linux x64 — screen-answer-linux-x86_64](https://github.com/mirawer/wtyczka/releases/latest/download/screen-answer-linux-x86_64)**
- [Latest release and all assets](https://github.com/mirawer/wtyczka/releases/latest) — choose the executable, not “Source code”, for a ready-to-run app. `SHA256SUMS.txt` and `THIRD_PARTY_NOTICES.txt` provide checksums and dependency notices.

| System | Supported environment |
| --- | --- |
| Windows | Native Windows 10/11 x64; no Rust, Python, local model or VC++ Redistributable installation required |
| Linux | x64, **Niri**, **grim with JPEG support**, session **D-Bus**, and any **StatusNotifierItem tray**; Noctalia is optional |
| Other Linux desktops / macOS | Not supported yet |

For credential persistence, Linux also needs a **Secret Service provider** with an existing, unlocked default collection (for example a compatible desktop keyring). The app does not unlock it or create a collection. Secret Service uses encrypted DH-session transport; protection at rest depends on the provider and its password policy. If the store is unavailable or locked, use explicit `--session-only`; there is no plaintext fallback.

The Windows executable is unsigned; Windows may show a warning. Do not disable system security or run the app as administrator. **Stop the old worker before replacing the Windows EXE:**

```powershell
.\screen-answer.exe stop
```

## Quick start

### Windows

1. Download and run `screen-answer.exe`.
2. On first setup, choose a provider, an image-capable model, analysis shortcut, optional pause shortcut, single/multiple answers and digits/dots. Paste the API key into the **last, hidden field**, then press Enter. On Windows, Ctrl+V explicitly reads the clipboard; the app never changes it. Invalid characters prompt for the key again instead of closing the app.
3. After the “Działa w tle” message, you may close the terminal. Show the entire question and options, then press your analysis shortcut.
4. Windows may initially put the icons in its tray overflow drawer. Windows controls their visibility and order.

The default analysis shortcut is **Print Screen**. If Snipping Tool or another app owns it, choose **Ctrl+Shift+F8**, for example. The app does not change Windows settings or install keyboard hooks. Windows checks both shortcuts before asking for the key.

### Linux

Enable a StatusNotifierItem tray in your bar first. For Noctalia, its `tray` widget must already be enabled; the app does not configure the bar.

The Linux release executable is statically linked with musl; the desktop requirements above still apply. Outside a checkout it automatically uses `${XDG_STATE_HOME:-$HOME/.local/state}/screen-answer`, so ordinary launches need no flags:

```bash
chmod +x screen-answer-linux-x86_64
./screen-answer-linux-x86_64
```

For a custom runtime directory, reuse **the same directory for every command**:

```bash
chmod +x screen-answer-linux-x86_64
mkdir -p "${XDG_STATE_HOME:-$HOME/.local/state}"
./screen-answer-linux-x86_64 --runtime-dir "${XDG_STATE_HOME:-$HOME/.local/state}/screen-answer"
# Stop:
./screen-answer-linux-x86_64 --runtime-dir "${XDG_STATE_HOME:-$HOME/.local/state}/screen-answer" stop
```

Or, from a source checkout:

```bash
./screen-answer
./screen-answer stop
```

The launcher builds release on first use (requires Rust/Cargo 1.89+ and a C compiler). The checkout uses `.runtime/` for its state.

Linux shortcuts use one temporary Niri overlay, without editing the base configuration. Normal stopping restores the base and releases both shortcuts. After `kill -9`, use `restore-keybind` with the same runtime directory, or start the app again. **Do not delete the state directory before restoring the shortcut.** `--niri-config` remains a runtime flag (or is discovered from Niri); it is not a saved setting.

## Saved setup and key management

First run, legacy settings, or settings without a credential reference open the full wizard, with the hidden key field last. On ordinary startup, if a reference exists but its native entry is missing, the config was copied to another runtime, or its API destination was edited, the app **fails closed** and directs you to `--change` rather than silently opening the wizard or reusing another credential. After successful UI startup, nonsecret settings and a credential reference are saved atomically. Failed setup preserves the old saved settings. Saved settings include provider, model, API base URL, both shortcuts, answer/display modes and timeout.

- **Plain start:** with valid settings and a saved credential, starts without the wizard or key prompt. If already running, only reports that fact; no prompt.
- **`--change`:** opens the full wizard using saved values as defaults. If running, the wizard completes before the old worker is stopped and a new one starts; this is not hot reload. Configuration overrides also use this reconfiguration path.
- **`forget-key`:** stops the current worker, disables automatic credential reuse and deletes the current **and retired** app credentials referenced for this runtime. Nonsecret settings stay. If deletion fails (for example because the store is locked), nonsecret cleanup references are retained and the command returns failure: unlock the store and retry. It does not revoke the key at the provider or delete unrelated credentials.
- **`--session-only`:** explicitly keeps the API key in RAM and **never accesses the credential store**, including pending cleanup. It works with a locked keyring and temporary model/shortcut choices while preserving the existing persistent key and configuration unchanged. If no saved key is configured, it may remember nonsecret choices as in older versions. An existing saved setup is used again on a later normal start. It does **not** delete a saved key; use `forget-key` for that.

```powershell
# Windows, from the executable's directory (v0.1.5):
.\screen-answer.exe
.\screen-answer.exe --change
.\screen-answer.exe forget-key
.\screen-answer.exe --session-only
```

```bash
# Linux source checkout (v0.1.5):
./screen-answer
./screen-answer --change
./screen-answer forget-key
./screen-answer --session-only
# Standalone: use the downloaded filename plus the same --runtime-dir as above.
```

Persistent keys use **Windows Credential Manager** or **Linux Secret Service**, not `settings.json`. There is **no automatic plaintext fallback**. Windows persistence accepts up to **2,560 ASCII bytes** per key; session-only input accepts up to **4,096 ASCII bytes**. Credentials are scoped to the OS user, canonical runtime directory, provider and API base URL, with a random credential ID. Moving a config or editing its destination must not reuse a credential bound to another scope. This is not isolation from other programs running as the same user.

**Run `forget-key` successfully before moving or deleting a runtime directory.** Failed deletion of retired keys after rotation is tracked with nonsecret cleanup references and retried on normal startup; session-only runs do not touch those references or the store. Native cleanup after an abrupt crash is best-effort, and the app cannot guarantee erasure of every OS/library memory copy.

## Controls and tray display

```powershell
.\screen-answer.exe status
.\screen-answer.exe toggle-pause
.\screen-answer.exe stop
.\screen-answer.exe doctor
# Synthetic tray demo only; no screenshot, API key or API request:
.\screen-answer.exe --selection multiple demo "1'2'3"
```

On Linux, use `./screen-answer` in a checkout, or the standalone executable with its runtime directory. `demo` shows a synthetic result for 15 seconds; it is not an AI or capture test. `doctor` checks platform/configuration without a screenshot or API request. `status` reports process state, never the model's answer.

Set an optional pause shortcut such as **Ctrl+Shift+F9** in the wizard, or use `--pause-keybind Ctrl+Shift+F9`. Default: none. Enter keeps the saved choice; `brak` or `off` in the wizard (or `--pause-keybind off`) removes it. It must differ from the analysis shortcut.

- Pausing cancels local waiting and clears every result cell to the ordinary **`-`**, identical to startup. There is no special pause icon, hover tooltip, balloon or notification.
- While paused, the analysis shortcut neither captures nor sends anything. Resuming still shows `-`; only a new analysis shortcut press starts another question. Old or late results do not return.
- The key remains available to the running worker. `stop` ends it completely; a new start is active, not paused.
- A request already received by a provider cannot be recalled and may still be billed. Its late response is discarded.

### Answers and providers

- Anthropic/Claude, OpenAI, OpenRouter, Gemini, and custom APIs compatible with OpenAI Chat Completions **and images**.
- Digits: **1–9** (A–I). Dots: **1–6** (A–F). Multiple answers such as `1'2'3` use full-size cells, not tiny text inside one icon.
- `-`: ready or paused (same appearance); `…`: analyzing; `?`: no recognized answer; `!`: error.
- One request at a time; repeated presses are not queued, and there are no automatic paid retries.

### 28 built-in model suggestions

| Provider | Count | Image-capable suggestions |
| --- | ---: | --- |
| Anthropic | 5 | Haiku, Sonnet and Opus 5.5; Fable 5.2; older Sonnet 4.6 |
| OpenAI | 6 | GPT-5.4 mini, GPT-5.4, GPT-5.2, GPT-6 Luna, GPT-6.2 Sol and Astra |
| OpenRouter | 9 | Claude Haiku/Sonnet/Opus 5.5; GPT-5.4 mini, GPT-6 Luna, GPT-6.2 Sol; Gemini 3.8 Flash, 4 Hydrogen and 4 Carbon |
| Gemini | 8 | 3.5 Flash-Lite, 3.8 Flash, 3.6 Flash, 3.1 Flash-Lite; 4 Hydrogen, Argon, Neon and Carbon |

Select a number or enter a full custom model ID. Existing numbers and saved choices stay unchanged. Custom servers require an ID from their own catalog, not another provider's list. The model must exist and be available to your API key.

The lists were checked on 2026-11-24 against [Anthropic](https://platform.claude.com/docs/en/about-claude/models/overview), [OpenAI](https://developers.openai.com/api/docs/models), [Gemini](https://aistudio.google.com/docs/models) and the [public OpenRouter catalog](https://openrouter.ai/api/v1/models). This does not verify access for your key or constitute a paid API test.

**Fable 5.2 requires 30-day data retention at Anthropic**, except for individually authorized organizations ([model policy](https://platform.claude.com/docs/en/models/fable-5-2/overview)). The wizard marks this beside the model name.

## Privacy and costs

**The entire active monitor** goes to the selected API destination, including visible private windows and notifications. Check the screen before pressing the shortcut. Image and prompt retention depend on the provider; custom endpoints receive the same sensitive content.

The app handles screenshots/JPEG in memory, without saving a screenshot history. It does not write API keys to configuration, logs or command history. From v0.1.5, keys can persist in the OS credential store as described above; `--session-only` keeps the session's key in RAM without touching an existing saved setup. Do not put a key in command arguments, GitHub, chat or bug reports. Never publish private screenshots or questions; see [SECURITY.md](SECURITY.md).

Internet and your own API access are required. Providers may charge, including for reasoning tokens. Set a spending limit with your provider. A chat-app subscription does not replace API access. Models can be wrong: the app validates answer format, not correctness.

## Development and verification limits

```bash
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

GitHub Actions builds Linux x86_64/musl and Windows x86_64/MSVC with static CRT, runs unit/local HTTP tests and native Windows IPC tests, and checks for extra Windows DLL dependencies. These checks use no user keys or paid API.

Linux integration tests: `python3 tests/smoke.py`. They need an active session D-Bus with a tray and use fake `niri`/`grim` plus local HTTP, not real screenshots. Set `SCREEN_ANSWER_BIN` to test a particular binary.

Linux persistence tests: run `cargo test --locked` first to compile the test helpers, then `python3 tests/persistence.py`. They require `dbus-daemon` and `gnome-keyring-daemon`, use a private bus and disposable keyring, and never access the user's credential entries. They use synthetic keys/images and local HTTP, not paid APIs. `SCREEN_ANSWER_BIN` can select the app binary.

Optional Windows tray demonstration, after downloading the executable into the checkout:

```powershell
.\scripts\test-windows.ps1 -Executable .\screen-answer.exe -ShowTray
```

The app itself needs no script. Unit/IPC tests live in source/CI (`cargo test`), not a separate app in current release assets.

**Verification limits:** CI does not verify tray appearance, physical shortcuts or monitor capture on an interactive Windows 10/11 desktop. It does not establish real-provider availability or answer accuracy. See [CHANGELOG.md](CHANGELOG.md) for version history; no platform-specific test totals are claimed here.

## License

[MIT](LICENSE). Copyright © 2026 mirawer. Run `--license` to print the full license text.
