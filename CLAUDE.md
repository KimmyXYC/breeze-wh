# Breeze-WH — Claude / Contributor Guide

Auto-confirm Windows Hello dialogs. Rust + Windows Service.

## Branch Workflow

- `dev` — default branch, all work lands here
- `main` — release branch, only merged from `dev`
- Merging `dev` → `main` triggers **release-plz**, which opens a "chore: release vX.Y.Z" PR
- Merging the Release PR publishes to crates.io + creates GitHub Release with pre-built `breeze-wh.exe`

## Commit Convention (Conventional Commits — critical)

release-plz reads commit messages to decide version bumps. **Get this wrong and the version won't bump.**

| Prefix | Example | Version bump |
|--------|---------|--------------|
| `feat:` | `feat: add upgrade command` | patch (0.x) / minor (≥1.0) |
| `fix:` | `fix: correct elevation path` | patch |
| `perf:` | `perf: shrink binary size` | patch |
| `docs:` | `docs: update README` | none (but shows in changelog) |
| `chore:` | `chore: bump deps` | none |
| `refactor:` | `refactor: extract dialog module` | none |
| `test:` | `test: add scan_dialog cases` | none |
| `ci:` | `ci: enable release workflow` | none |
| `feat!:` or `BREAKING CHANGE:` in body | breaking change | major |

**Rules of thumb:**
- User-visible new behavior → `feat:`
- User-visible bug fix → `fix:`
- Internal only (refactor, cleanup, tests, ci) → appropriate non-releasing prefix
- Breaking change → `!` suffix or `BREAKING CHANGE:` footer

On `0.x` versions, `feat:` bumps patch (not minor). After 1.0 that changes.

## Build & Test

```powershell
cargo build --release   # release binary
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

`lefthook` runs `fmt --check` + `clippy` on pre-commit. If hooks fail, fix and retry; don't bypass with `--no-verify`.

## Architecture

Single binary `breeze-wh.exe` with three modes (dispatched from `src/main.rs`):

- `breeze-wh service` — registered Windows service, runs in Session 0
- `breeze-wh helper` — spawned by service in user session via `CreateProcessAsUser` + linked elevated token
- `breeze-wh <install|uninstall|start|stop|status|upgrade|--version>` — CLI

CLI commands that modify service state auto-elevate via UAC (`src/common/elevation.rs`).

## Data Locations

- Binary: `~/.cargo/bin/breeze-wh.exe`
- Data dir: `C:\ProgramData\Breeze-WH\` (ACL granted Users modify on install)
- Config: `C:\ProgramData\Breeze-WH\config.toml`
- Logs: `C:\ProgramData\Breeze-WH\logs\` (rolling daily, separate files for service + helper)

## Detection Logic (helper mode)

Language-independent. Do NOT match on window title / button name strings.

- Window class: `"Credential Dialog Xaml Host"`
- Face-recognition mode: **no** `PasswordField_4` element present
- Click target: AutomationId `OkButton` with `InvokePattern`

If `PasswordField_4` is present → PIN mode → fill and confirm only when the user has saved a PIN with `breeze-wh pin set`. PIN entry must target a single password control in the system `CredentialUIBroker.exe`, with an exact current-PIN field label loaded from `NgcCredProv.dll` (resource IDs 105/106). Exclude `FidoCredProv.dll` PIN labels (205/208/209); missing or ambiguous labels must fail closed. Recheck the field before both filling and submitting. Any other password field blocks automatic confirmation too. Never classify `PasswordField_4` alone as Windows Hello; security-key providers reuse field IDs. Store PINs only in the current user's Windows Credential Manager; never put them in shared config, command-line arguments, or logs. Consume each attempt before writing/invoking and never retry automatically within the same dialog. Watch enabled/visibility property changes as well as structure changes for delayed controls.

## Release Flow (manual invocation)

1. Work on `dev`, commit with conventional commit prefixes
2. Open PR `dev` → `main`, merge
3. release-plz opens "chore: release" PR on `main` — review, merge
4. Merging Release PR runs `.github/workflows/release-plz.yml`:
   - `cargo publish` → crates.io
   - `cargo build --release` → `target/release/breeze-wh.exe`
   - `gh release upload` → attaches exe to GitHub Release

## Known Constraints / Non-Goals

- Cannot auto-confirm UAC / Secure Desktop prompts (Windows design wall — UI Automation is blocked there)
- `breeze-wh upgrade` diverges from cargo's registry metadata (user-visible note printed on completion)
- Face recognition is auto-confirmed; Windows Hello PIN input is opt-in via `pin set|clear|status`. Physical security-key PINs and other password fields are excluded. Fingerprint / smart card remain untested.
- Windows-only; `cargo build` fails on Linux/macOS (all deps are Win32)

## When Fixing Things

- **Never bypass `--allow-dirty`, `--no-verify`, pre-commit hooks, or lefthook** unless the user explicitly asks
- Build after edits: `cargo build --release` — don't mark work complete without verification
- If the service is running and a build fails to overwrite `breeze-wh.exe`: user must `breeze-wh stop` first
