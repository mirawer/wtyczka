# Changelog

Release history for Screen Answer. The v0.1.5 date is the release-preparation date; earlier dates are GitHub publication dates (UTC). Earlier summaries come from the linked release notes; they are not new verification claims.

## [0.1.5] — 2026-11-24

### Added / changed

- Save nonsecret setup and a credential reference atomically after successful UI startup; preserve previous settings on failed setup.
- Use Windows Credential Manager or Linux Secret Service for persisted API keys, scoped to the OS user, canonical runtime directory, provider and API base URL, with a random credential ID. Never fall back to plaintext storage. Linux uses encrypted DH-session transport; at-rest protection depends on the provider/password policy. Require an existing, unlocked default collection without creating or unlocking it.
- Skip the wizard and key prompt on ordinary startup when saved settings and the bound credential are available. First/legacy setup and settings without a credential reference use the full wizard, with the hidden key field last. Existing references with a missing native entry, copied configuration or edited destination fail closed and require `--change`.
- Add `--change` for the full setup wizard with saved defaults. Reconfiguration completes the wizard before replacing a running worker; no hot reload.
- Add `forget-key` to stop the worker, disable automatic reuse and delete current/retired app credentials for this runtime while preserving nonsecret settings. Retain failed deletions as nonsecret cleanup references, return failure and request unlocking/retry. Retain failed rotation cleanup for retry on normal startup.
- Add explicit `--session-only` for RAM-only keys without ever accessing the credential store or pending cleanup. Preserve existing persistent keys/configuration, including when temporarily changing model/shortcuts with a locked keyring. If no saved key is configured, nonsecret choices may be remembered; deletion remains explicit through `forget-key`.
- Limit Windows persisted keys to 2,560 ASCII bytes; allow up to 4,096 ASCII bytes for session-only input.
- Persist timeout alongside provider, model, base URL, shortcuts and answer/display modes. Keep `--niri-config` runtime-only/discovered.
- Add `--license` to print the full MIT license, an English main README, Polish translation, security policy and structured bug-report form.
- Add Linux persistence tests using a private D-Bus and disposable keyring without touching user entries. Document best-effort native crash cleanup and limits on clearing OS/library memory.

macOS, broader Linux desktop support, menus and profiles remain deferred. No new Windows test totals are claimed here.

## [0.1.4] — 2026-11-24

- Optional pause/resume shortcut and `toggle-pause`, with the same plain idle dash after pausing or resuming.
- No capture or API request while paused; cancel local waiting and discard late results without paid automatic retries. Already-received requests may still be billed.
- Clear all cells for single/multiple answers and digits/dots. Check both Windows shortcuts; use one temporary Niri overlay on Linux.
- API keys remain memory-only in this published version.

## [0.1.3] — 2026-11-24

- Expand image-capable model suggestions to 28: Anthropic 5, OpenAI 6, OpenRouter 9 and Gemini 8.
- Preserve existing model numbers and saved selections; retain custom full model IDs.
- Mark Anthropic Fable 5.2's 30-day retention requirement and explain that catalog checks do not establish account access.

## [0.1.2] — 2026-11-24

- Correct multiple-answer cell mapping for the usual Windows tray order, including dots and apostrophe separators.
- Encode Windows JPEG directly from GDI's BGRA buffer, avoiding a second full RGB allocation while preserving capture resolution, quality and size limit.
- Simplify HTTP client lifetime, analysis-result handling and key validation; separate console/API tests and reduce duplicated tests/dependencies.
- Use the standard library for single-instance locking; add build caching and Clippy on both platforms.
- Remove the separate Windows test executable from end-user downloads; keep tests in source/CI.

## [0.1.1] — 2026-11-24

- Fix hidden Windows Ctrl+V API-key entry: treat paste as an explicit clipboard read, not an invalid key character.
- Do not modify the clipboard; strip pasted trailing CR/LF without submitting the field.
- Retry invalid key input instead of exiting, preserving hidden input and console-mode restoration.

## [0.1.0] — 2026-11-24

- Initial Windows 10/11 x64 and Linux x64/Niri release, with native Windows tray/capture/shortcuts and Linux grim/D-Bus/StatusNotifierItem integration.
- Setup wizard for provider, image-capable model, shortcut, single/multiple answers, digits/dots and a hidden API key.
- Windows static CRT and Linux static musl executables, checksums and third-party notices.
- Local/unit/IPC tests without real API keys or paid requests; interactive Windows tray, physical shortcut and capture verification remain outside CI guarantees.

[0.1.5]: https://github.com/mirawer/wtyczka/releases/tag/v0.1.5
[0.1.4]: https://github.com/mirawer/wtyczka/releases/tag/v0.1.4
[0.1.3]: https://github.com/mirawer/wtyczka/releases/tag/v0.1.3
[0.1.2]: https://github.com/mirawer/wtyczka/releases/tag/v0.1.2
[0.1.1]: https://github.com/mirawer/wtyczka/releases/tag/v0.1.1
[0.1.0]: https://github.com/mirawer/wtyczka/releases/tag/v0.1.0
