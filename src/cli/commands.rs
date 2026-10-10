use crate::common::{
    config::BreezeConfig,
    constants::{SERVICE_DISPLAY_NAME, SERVICE_NAME, config_path, data_dir},
};
use anyhow::Context;
use windows_service::{
    service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
        ServiceType,
    },
    service_manager::{ServiceManager, ServiceManagerAccess},
};

/// Quick check if the Breeze-WH service is registered (no elevation needed).
pub fn check_service_exists() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open service manager")?;

    manager
        .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
        .context("Breeze-WH service is not installed. Run 'breeze-wh install' first.")?;

    Ok(())
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(|s| s.as_str()) {
        Some("--version" | "-V" | "version") => {
            println!("breeze-wh {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("install") => cmd_install(),
        Some("uninstall") => cmd_uninstall(),
        Some("start") => cmd_start(),
        Some("stop") => cmd_stop(),
        Some("status") => cmd_status(),
        Some("upgrade") => cmd_upgrade(),
        Some("pin") => cmd_pin(&args[1..]),
        _ => {
            eprintln!("Breeze-WH - Auto Windows Hello");
            eprintln!();
            eprintln!("Usage: breeze-wh <command>");
            eprintln!();
            eprintln!("Commands:");
            eprintln!("  install    Install the Breeze-WH service");
            eprintln!("  uninstall  Uninstall the Breeze-WH service");
            eprintln!("  start      Start the Breeze-WH service");
            eprintln!("  stop       Stop the Breeze-WH service");
            eprintln!("  status     Show the Breeze-WH service status");
            eprintln!("  upgrade    Upgrade to the latest release from GitHub");
            eprintln!("  pin set    Save your PIN and enable automatic PIN entry");
            eprintln!("  pin clear  Delete your saved PIN and disable automatic PIN entry");
            eprintln!("  pin status Show whether automatic PIN entry is enabled");
            eprintln!();
            eprintln!("Internal (used by the service):");
            eprintln!("  service    Run as Windows Service");
            eprintln!("  helper     Run the UI Automation helper");
            std::process::exit(1);
        }
    }
}

fn cmd_pin(args: &[String]) -> anyhow::Result<()> {
    use crate::common::pin;
    use zeroize::Zeroizing;

    if args.len() != 1 {
        anyhow::bail!("Usage: breeze-wh pin <set|clear|status>");
    }
    match args[0].as_str() {
        "set" => {
            let value = Zeroizing::new(rpassword::prompt_password("Windows Hello PIN: ")?);
            let confirmation = Zeroizing::new(rpassword::prompt_password("Confirm PIN: ")?);
            if *value != *confirmation {
                anyhow::bail!("PINs do not match; the saved PIN was not changed.");
            }
            pin::save(&value).context("Failed to save PIN in Windows Credential Manager")?;
            println!("PIN saved for the current Windows user. Automatic PIN entry is enabled.");
        }
        "clear" => {
            pin::clear().context("Failed to delete saved PIN")?;
            println!("Saved PIN deleted. Automatic PIN entry is disabled.");
        }
        "status" => {
            let enabled = pin::load().context("Failed to read saved PIN")?.is_some();
            println!(
                "Automatic PIN entry: {}",
                if enabled { "enabled" } else { "disabled" }
            );
        }
        _ => anyhow::bail!("Usage: breeze-wh pin <set|clear|status>"),
    }
    Ok(())
}

fn cmd_install() -> anyhow::Result<()> {
    let service_exe =
        std::env::current_exe().context("Failed to resolve current executable path")?;

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("Failed to open service manager (try running as Administrator)")?;

    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: SERVICE_DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: service_exe,
        launch_arguments: vec!["service".into()],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };

    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .context("Failed to create service")?;

    service
        .set_description("Automatically confirms Windows Hello face recognition dialogs")
        .context("Failed to set service description")?;

    let dir = data_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create data directory: {}", dir.display()))?;

    // Allow non-admin users to read/write/delete files in the data directory
    // (otherwise logs written by the elevated helper/service aren't accessible).
    if let Err(e) = grant_users_modify(&dir) {
        eprintln!(
            "Warning: failed to set permissions on {}: {e}",
            dir.display()
        );
    }

    let cfg_path = config_path();
    if !cfg_path.exists() {
        let default_cfg = toml::to_string_pretty(&BreezeConfig::default())
            .context("Failed to serialize default config")?;
        std::fs::write(&cfg_path, default_cfg)
            .with_context(|| format!("Failed to write config: {}", cfg_path.display()))?;
        println!("Written default config to {}", cfg_path.display());
    }

    println!("Service '{}' installed successfully.", SERVICE_NAME);

    // Auto-start the service
    service
        .start(&["service"])
        .context("Service installed but failed to start")?;

    println!("Service '{}' started.", SERVICE_NAME);
    Ok(())
}

fn cmd_uninstall() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open service manager (try running as Administrator)")?;

    let service = manager
        .open_service(
            SERVICE_NAME,
            ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
        )
        .context("Failed to open service (is it installed?)")?;

    let status = service
        .query_status()
        .context("Failed to query service status")?;

    if status.current_state != ServiceState::Stopped
        && let Err(e) = service.stop()
    {
        eprintln!("Warning: could not stop service before deletion: {e}");
    }

    service.delete().context("Failed to delete service")?;

    println!("Service '{}' uninstalled successfully.", SERVICE_NAME);
    Ok(())
}

fn cmd_start() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open service manager (try running as Administrator)")?;

    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::START)
        .context("Failed to open service (is it installed?)")?;

    service
        .start(&["service"])
        .context("Failed to start service")?;

    println!("Service '{}' started.", SERVICE_NAME);
    Ok(())
}

fn cmd_stop() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open service manager (try running as Administrator)")?;

    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::STOP)
        .context("Failed to open service (is it installed?)")?;

    service.stop().context("Failed to stop service")?;

    println!("Service '{}' stopped.", SERVICE_NAME);
    Ok(())
}

fn cmd_status() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open service manager")?;

    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
        .context("Failed to open service (is it installed?)")?;

    let status = service
        .query_status()
        .context("Failed to query service status")?;

    let state_label = match status.current_state {
        ServiceState::Stopped => "Stopped",
        ServiceState::StartPending => "Start Pending",
        ServiceState::StopPending => "Stop Pending",
        ServiceState::Running => "Running",
        ServiceState::ContinuePending => "Continue Pending",
        ServiceState::PausePending => "Pause Pending",
        ServiceState::Paused => "Paused",
    };

    println!("Service '{}': {}", SERVICE_NAME, state_label);
    Ok(())
}

const GITHUB_API_LATEST: &str = "https://api.github.com/repos/evan-choi/breeze-wh/releases/latest";
const ASSET_NAME: &str = "breeze-wh.exe";

fn cmd_upgrade() -> anyhow::Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");

    // 1. Fetch latest release metadata from GitHub API
    println!("Checking latest release...");
    let resp = ureq::get(GITHUB_API_LATEST)
        .set("User-Agent", "breeze-wh-updater")
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("Failed to reach GitHub API")?;

    let json: serde_json::Value = resp
        .into_json()
        .context("Failed to parse GitHub API response")?;

    let tag = json["tag_name"]
        .as_str()
        .context("latest release has no tag_name")?;
    let latest_version = tag.trim_start_matches('v');

    if latest_version == current_version {
        println!("Already up to date (v{current_version}).");
        return Ok(());
    }
    println!("Upgrading breeze-wh v{current_version} -> v{latest_version}");

    // 2. Find the exe asset download URL
    let assets = json["assets"].as_array().context("release has no assets")?;
    let download_url = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(ASSET_NAME))
        .and_then(|a| a["browser_download_url"].as_str())
        .with_context(|| format!("{ASSET_NAME} not found in release assets"))?;

    // 3. Inspect service state
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open service manager")?;

    let svc = manager
        .open_service(
            SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START,
        )
        .ok();

    let was_running = match &svc {
        Some(s) => {
            let state = s.query_status()?.current_state;
            matches!(state, ServiceState::Running | ServiceState::StartPending)
        }
        None => false,
    };
    let service_installed = svc.is_some();

    // 4. Stop service if running
    if was_running {
        println!("Stopping service...");
        if let Some(s) = &svc {
            s.stop().context("Failed to stop service")?;
            wait_for_stopped(s)?;
        }
    }

    // 5. Download new exe
    println!("Downloading {ASSET_NAME}...");
    let mut reader = ureq::get(download_url)
        .set("User-Agent", "breeze-wh-updater")
        .call()
        .context("Failed to download exe")?
        .into_reader();
    let mut buf = Vec::new();
    std::io::copy(&mut reader, &mut buf).context("Failed to read download stream")?;
    println!("Downloaded {} bytes", buf.len());

    // 6. Replace current exe using the Windows rename trick
    replace_current_exe(&buf).context("Failed to replace exe")?;

    println!("Upgrade complete: v{current_version} -> v{latest_version}.");

    // 7. Restart service if it was running
    if was_running {
        // Re-open with START access since old handle may be stale after exe swap
        let restart_svc = manager
            .open_service(SERVICE_NAME, ServiceAccess::START)
            .context("Failed to re-open service for restart")?;
        restart_svc
            .start(&["service"])
            .context("Failed to start service after upgrade")?;
        println!("Service restarted.");
    } else if service_installed {
        println!("Service was not running before upgrade; left stopped.");
    } else {
        println!("Service is not installed; exe replaced.");
    }

    eprintln!();
    eprintln!(
        "Note: cargo's registry metadata still reflects the old version. \
         Run `breeze-wh --version` or check the binary directly to confirm."
    );

    Ok(())
}

fn wait_for_stopped(svc: &windows_service::service::Service) -> anyhow::Result<()> {
    for _ in 0..60 {
        let state = svc.query_status()?.current_state;
        if state == ServiceState::Stopped {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    anyhow::bail!("Timed out waiting for service to stop")
}

/// Replace the running exe on Windows.
/// Windows blocks overwriting a running exe, but does allow renaming it.
/// We rename the current exe to `.old`, then write the new bytes to the original path,
/// and schedule the `.old` file for deletion on next reboot.
fn replace_current_exe(new_bytes: &[u8]) -> anyhow::Result<()> {
    let current = std::env::current_exe()?;
    let old_path = current.with_extension("exe.old");

    // Clean up any previous .old left behind
    let _ = std::fs::remove_file(&old_path);

    // Rename current running exe (Windows allows this even while running)
    std::fs::rename(&current, &old_path).context("Failed to rename current exe")?;

    // Write new exe to original path
    if let Err(e) = std::fs::write(&current, new_bytes) {
        // Roll back the rename on failure
        let _ = std::fs::rename(&old_path, &current);
        return Err(e).context("Failed to write new exe");
    }

    // Schedule .old for deletion on next reboot (we can't delete it now — we're running it)
    schedule_delete_on_reboot(&old_path);
    Ok(())
}

/// Grant the built-in Users group Modify access to the directory, with
/// inheritance so future log/config files are accessible without admin.
///
/// SDDL breakdown:
/// - `D:PAI` — Protected DACL (no inherit from parent), Auto-inherited
/// - `(A;OICI;FA;;;BA)` — Allow Full Access to Built-in Admins, Object+Container Inherit
/// - `(A;OICI;FA;;;SY)` — Same for NT AUTHORITY\SYSTEM
/// - `(A;OICI;0x1301bf;;;BU)` — Modify rights to Built-in Users
fn grant_users_modify(path: &std::path::Path) -> anyhow::Result<()> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows::Win32::Security::{
        DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SetFileSecurityW,
    };
    use windows::core::HSTRING;

    let sddl = HSTRING::from("D:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x1301bf;;;BU)");
    let path_h = HSTRING::from(path.as_os_str());

    unsafe {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &sddl, 1, // SDDL_REVISION_1
            &mut sd, None,
        )
        .context("Failed to parse SDDL")?;

        let result = SetFileSecurityW(&path_h, DACL_SECURITY_INFORMATION, sd);
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        result.ok().context("SetFileSecurityW failed")?;
    }

    Ok(())
}

fn schedule_delete_on_reboot(path: &std::path::Path) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW};
    use windows::core::PCWSTR;

    let wide: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
    unsafe {
        let _ = MoveFileExW(
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            MOVEFILE_DELAY_UNTIL_REBOOT,
        );
    }
}
