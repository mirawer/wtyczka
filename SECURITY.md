# Security policy

## Reporting a vulnerability

**Do not disclose vulnerabilities in public issues, discussions or pull requests.** Private vulnerability reporting is enabled for this repository. Use [Security → Advisories → Report a vulnerability](https://github.com/mirawer/wtyczka/security/advisories/new) to send a private report to the maintainer.

Include the affected version, OS and architecture, a minimal reproduction using synthetic data, expected/actual behavior and potential impact. No response-time or fix-time guarantee is offered. Fixes target the current development version and latest release; there is no promised backport window for older versions.

## Never include sensitive data

- Never post API keys, authorization headers, tokens, credential-store exports or runtime/configuration files containing sensitive values.
- Never post private screenshots, private questions, personal data or unredacted logs. Screenshots and questions can reveal sensitive information even without a key.
- Use synthetic questions/images and placeholder credentials in reproductions. Review diagnostic output before sharing it; local paths and custom API URLs may also be sensitive.
- If a key leaks, revoke or rotate it **at the API provider** immediately and review usage/billing. Deleting a local credential does not revoke the remote key.

## Data handling and security boundaries

The analysis shortcut sends the **whole active monitor**, including visible private windows and notifications, to the configured API destination. Provider retention and billing rules apply. The app does not establish the correctness of AI answers.

From v0.1.5, keys can persist in Windows Credential Manager or Linux Secret Service; older versions keep them only in process memory. Configuration contains nonsecret settings and credential/cleanup references, never the keys. Credentials are scoped to the OS user, canonical runtime directory, provider and API base URL, with a random ID. On ordinary startup, an existing reference with a missing native entry, copied runtime configuration or edited destination fails closed and requires `--change` rather than reusing another scope's key.

Linux uses encrypted DH-session transport to Secret Service. **At-rest protection depends on the provider and its password policy**; not every provider guarantees encrypted storage. The default collection must already exist and be unlocked. The app does not unlock stores or create collections, and never falls back to plaintext. Windows persistence accepts up to 2,560 ASCII bytes per key; session-only input accepts up to 4,096 ASCII bytes.

`--session-only` keeps the session's key in RAM and never accesses the credential store, including cleanup. It works with locked stores and temporary model/shortcut choices, preserving an existing persistent key/configuration. If no saved key is configured, nonsecret choices may be remembered. It does not delete saved credentials.

`forget-key` stops the worker, disables automatic reuse and deletes the current and retired app credentials referenced for this runtime, preserving nonsecret settings. Locked or failed deletions retain nonsecret cleanup references and return failure: unlock and retry. Failed rotation cleanup is also retained and retried on normal startup, never by session-only runs. **Complete `forget-key` successfully before moving or deleting the runtime directory.** None of these commands revokes a key at the provider.

Screenshots/JPEG are handled in memory without a screenshot history; keys are not written to configuration or logs. Native cleanup after abrupt crashes is best-effort, and the app cannot guarantee erasure of every OS/library memory copy. This is **not isolation from programs running as the same OS user**, malware, administrators or a compromised provider. An OS credential store does not remove the need to trust the chosen API destination.

Do not disable Windows security or run the app as administrator. The distributed Windows executable is unsigned. Download from the project's [releases](https://github.com/mirawer/wtyczka/releases/latest) and check the published checksums when available.

Ordinary bugs can use the [bug-report form](https://github.com/mirawer/wtyczka/issues/new?template=bug_report.yml), with all sensitive data removed.
