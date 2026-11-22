# Security Policy

## Supported Versions

| Version | Supported |
| :--- | :--- |
| `0.24.x` (`main`) | Yes |
| `< 0.24.0` | No |

## Reporting a Vulnerability

Please **do not** report security vulnerabilities through public GitHub issues.

Instead, report them privately via:
- GitHub Security Advisories (`Security` tab -> `Report a vulnerability`) for `totatake/termway`.

Please include:
1. Affected `termway` version / commit SHA, OS, architecture (`x86_64` / `aarch64`), and terminal emulator.
2. Reproduction steps or command invocation.
3. Security boundary affected and potential impact.

## Security Architecture & Threat Boundaries

- **D-Bus Secret Service Bridge**: Host `org.freedesktop.secrets` relay enforces an allow-list, blocks destructive collection deletion, and limits message size/pending counts (opt out with `--no-host-keyring` / `TERMWAY_NO_HOST_KEYRING=1`).
- **OSC 52 & Bracketed Paste**: Clipboard sync enforces a `1 MiB` payload limit and never logs plaintext clipboard data to stderr (opt out with `--no-osc52` / `TERMWAY_NO_OSC52=1`).
- **Polkit Agent Integration**: Elevated GUI prompts can be disabled with `--no-polkit` / `TERMWAY_NO_POLKIT=1`.
- **Runtime & Display Bounds**: `XDG_RUNTIME_DIR` validation is enforced for PATH wrapper shims, and virtual display resolution is clamped to `7680x4320`.

We aim to acknowledge reports within 72 hours and coordinate a fix before public disclosure.
