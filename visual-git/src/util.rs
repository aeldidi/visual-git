use std::{
    env,
    error::Error,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_wsl() -> bool {
    env::var_os("WSL_INTEROP").is_some()
        || env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/version")
            .map(|content| content.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

pub fn try_open_browser(url: &str) -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "windows")]
    let status = Command::new("cmd")
        .args(["/C", "start", "", url])
        .status()?;

    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(url).status()?;

    #[cfg(all(unix, not(target_os = "macos")))]
    let status = {
        if is_wsl() {
            match Command::new("cmd.exe")
                .args(["/C", "start", "", url])
                .status()
            {
                Ok(status) if status.success() => return Ok(()),
                Ok(status) => {
                    eprintln!(
                        "WSL Windows-side browser launch failed (status: {}), falling back to xdg-open",
                        status
                    );
                }
                Err(err) => {
                    eprintln!(
                        "WSL Windows-side browser launch failed ({}), falling back to xdg-open",
                        err
                    );
                }
            }
        }
        Command::new("xdg-open").arg(url).status()?
    };

    if status.success() {
        Ok(())
    } else {
        Err(
            format!("browser open command exited with status: {}", status)
                .into(),
        )
    }
}
