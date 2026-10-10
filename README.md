<div align="center">

# Breeze

**Stop clicking "OK" after Windows Hello recognizes you.**

[![CI](https://github.com/evan-choi/breeze-wh/actions/workflows/ci.yml/badge.svg)](https://github.com/evan-choi/breeze-wh/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/breeze-wh)](https://crates.io/crates/breeze-wh)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE-MIT)

[English](#how-it-works) | [한국어](README.ko.md)

</div>

---

Windows Hello face recognition works great — except it still asks you to click "OK" every single time. That extra click is just noise.

**Breeze** removes it. It monitors Windows Hello credential dialogs, detects successful face recognition, and auto-confirms — silently, as a Windows Service.

## How It Works

```
Windows Hello recognizes your face
        ↓
Credential dialog appears with "OK" button
        ↓
Breeze detects it via UI Automation API
        ↓
Confirms automatically (PIN entry is opt-in; see below)
        ↓
You're in. Zero clicks.
```

## Install

```powershell
cargo install breeze-wh
breeze-wh install
```

That's it. Breeze runs silently in the background.

## Automatic PIN entry

To enable PIN entry, run this as the Windows user who will use Windows Hello:

```powershell
breeze-wh pin set
```

Enter your existing Windows Hello PIN twice at the hidden prompts. This saves the PIN in your Windows Credential Manager and enables automatic entry for supported PIN dialogs. It does not change your Windows Hello PIN. No administrator rights or service restart are needed; changes apply to the next dialog.

```powershell
breeze-wh pin status  # Show whether a PIN is saved (never prints it)
breeze-wh pin clear   # Delete the saved PIN and disable automatic entry
```

PIN entry is disabled until you save a PIN. The PIN is stored for the current user on this computer, never in `config.toml` or logs. Run `pin set` again after changing your Windows Hello PIN. Use `pin clear` before uninstalling if you want to remove the saved credential; uninstalling the service leaves it intact.

When enabled, supported prompts can authenticate without you typing your PIN. Breeze fills the PIN and invokes the enabled confirmation button at most once per dialog. A failed attempt is left for manual handling; close and reopen the dialog to try again after correcting the saved PIN.

This supports a single `PasswordField_4` password control hosted by the system `CredentialUIBroker.exe`, using UI Automation's Value pattern. Its accessible label must exactly match the current Windows Hello PIN or work PIN label loaded from Windows' NGC credential provider. Physical security-key PINs, other passwords, PIN setup/change forms, and unrecognized fields are never filled or auto-confirmed. Missing or ambiguous provider labels disable automatic entry for that dialog. Controls that reject programmatic input are left for manual entry. It does not switch authentication methods, unlock the Windows sign-in screen, or handle UAC / Secure Desktop prompts.

## Upgrade

```powershell
breeze-wh upgrade
```

Fetches the latest `breeze-wh.exe` from GitHub Releases and swaps it in place. The old exe is kept as `breeze-wh.exe.old` and removed on the next reboot (Windows won't let a running exe delete itself).

Service state is preserved across the upgrade:

- **Running** → stopped, exe replaced, started again
- **Stopped** → exe replaced only
- **Not installed** → exe replaced only

> **Note:** `upgrade` pulls from GitHub Releases, not crates.io. After an upgrade, `cargo install --list` will still show the older version — `breeze-wh --version` reports what's actually on disk.

## Commands

- `breeze-wh install` — Install and start the service
- `breeze-wh uninstall` — Stop and unregister the service
- `breeze-wh start` — Start the service
- `breeze-wh stop` — Stop the service
- `breeze-wh status` — Show current service state
- `breeze-wh upgrade` — Upgrade to the latest release (see above)
- `breeze-wh pin set|clear|status` — Manage automatic PIN entry for the current user
- `breeze-wh --version` — Print the installed version

`install` / `uninstall` / `start` / `stop` / `upgrade` all need admin rights — they auto-request elevation via UAC, you don't need to launch an elevated shell yourself.

## How It's Built

Breeze is a single binary that runs in two modes:

- **Service mode** — Runs in Session 0 as a Windows Service. Detects user logon/logoff and spawns the helper in the user's desktop session. Automatically restarts it on crash with exponential backoff.

- **Helper mode** — Runs in the user session with elevated privileges. Subscribes to UI Automation focus events to detect `Credential Dialog Xaml Host` windows. On detection, scans the UI tree in a single pass:
  - One verified Windows Hello PIN field present → **fill and confirm once** if a PIN is saved
  - Any other password/PIN field present → **skip both filling and confirmation**
  - `OkButton` present without `PasswordField` → face recognition → **click**
  - Controls not ready → watch structure and enabled/visibility property changes until they are ready

Detection uses `AutomationId` and `ClassName`, plus exact PIN field labels loaded from the installed Windows credential providers in the current UI language. It does not rely on translated window titles or button text.

## Configuration

Config file: `C:\ProgramData\Breeze-WH\config.toml` (created on install)

```toml
enabled = true
debounce_ms = 2000
log_level = "info"
log_max_files = 7
```

Logs: `C:\ProgramData\Breeze-WH\logs\`

## Requirements

- Windows 10 / 11
- Windows Hello face recognition or PIN configured
- [Rust](https://rustup.rs/) toolchain (to install via cargo)

## License

MIT OR Apache-2.0
