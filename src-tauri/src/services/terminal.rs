use std::process::Command;

use anyhow::Context;

pub(crate) fn launch_terminal(
    target: &str,
    command: &str,
    cwd: Option<&str>,
) -> anyhow::Result<()> {
    if command.trim().is_empty() {
        anyhow::bail!("Command is empty");
    }

    #[cfg(target_os = "macos")]
    {
        match target {
            "terminal" => launch_macos_terminal(command, cwd),
            "iterm2" => launch_iterm(command, cwd),
            "ghostty" => launch_ghostty(command, cwd),
            "kitty" => launch_kitty(command, cwd),
            "warp" => launch_warp(command, cwd),
            "wezterm" => launch_wezterm(command, cwd),
            "alacritty" => launch_alacritty(command, cwd),
            _ => launch_macos_terminal(command, cwd),
        }
    }

    #[cfg(target_os = "windows")]
    {
        match target {
            "powershell" => launch_windows_powershell(command, cwd),
            "cmd" => launch_windows_cmd(command, cwd),
            _ => launch_windows_terminal(command, cwd),
        }
    }

    #[cfg(target_os = "linux")]
    {
        match target {
            "gnome-terminal" => launch_linux_gnome_terminal(command, cwd),
            "konsole" => launch_linux_konsole(command, cwd),
            "xterm" => launch_linux_xterm(command, cwd),
            "alacritty" => launch_linux_alacritty(command, cwd),
            "kitty" => launch_linux_kitty(command, cwd),
            "wezterm" => launch_linux_wezterm(command, cwd),
            _ => launch_linux_default(command, cwd),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        anyhow::bail!("Terminal launch is not supported on this platform")
    }
}

#[cfg(target_os = "macos")]
fn launch_macos_terminal(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let full_command = build_shell_command(command, cwd);
    let escaped = escape_osascript(&full_command);
    // Use "do script" without "in window" to always create a new tab
    // in the frontmost window (or a new window if none exists).
    // Activate first to avoid racing with Terminal's own startup window.
    let script = format!(
        r#"tell application "Terminal"
    activate
    do script "{escaped}"
end tell"#
    );

    let status = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .context("Failed to launch Terminal")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Terminal command execution failed")
    }
}

#[cfg(target_os = "macos")]
fn launch_iterm(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let full_command = build_shell_command(command, cwd);
    let escaped = escape_osascript(&full_command);
    let script = format!(
        r#"tell application "iTerm"
    activate
    if (count of windows) > 0 then
        tell current window
            create tab with default profile
            tell current session
                write text "{escaped}"
            end tell
        end tell
    else
        create window with default profile
        tell current session of current window
            write text "{escaped}"
        end tell
    end if
end tell"#
    );

    let status = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .context("Failed to launch iTerm")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("iTerm command execution failed")
    }
}

#[cfg(target_os = "macos")]
fn launch_ghostty(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let args = build_ghostty_args(command, cwd);

    let status = Command::new("open")
        .args(args.iter().map(String::as_str))
        .status()
        .context("Failed to launch Ghostty")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Failed to launch Ghostty. Make sure it is installed.")
    }
}

#[cfg(target_os = "macos")]
fn build_ghostty_args(command: &str, cwd: Option<&str>) -> Vec<String> {
    let input = ghostty_raw_input(command);

    let mut args = vec![
        "-na".to_string(),
        "Ghostty".to_string(),
        "--args".to_string(),
        "--quit-after-last-window-closed=true".to_string(),
    ];

    if let Some(dir) = cwd {
        if !dir.trim().is_empty() {
            args.push(format!("--working-directory={dir}"));
        }
    }

    args.push(format!("--input={input}"));
    args
}

#[cfg(target_os = "macos")]
fn ghostty_raw_input(command: &str) -> String {
    let mut escaped = String::from("raw:");
    for ch in command.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            _ => escaped.push(ch),
        }
    }
    escaped.push_str("\\n");
    escaped
}

#[cfg(target_os = "macos")]
fn launch_warp(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let full_command = build_shell_command(command, cwd);
    let escaped = escape_osascript(&full_command);
    let script = format!(
        r#"tell application "Warp"
    activate
    delay 0.5
    tell application "System Events"
        keystroke "{escaped}"
        key code 36
    end tell
end tell"#
    );

    let status = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .context("Failed to launch Warp")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Failed to launch Warp. Make sure it is installed.")
    }
}

#[cfg(target_os = "macos")]
fn launch_kitty(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

    let mut cmd = Command::new("open");
    cmd.arg("-na").arg("kitty").arg("--args");

    if let Some(dir) = cwd {
        if !dir.trim().is_empty() {
            cmd.arg("--directory").arg(dir);
        }
    }

    cmd.arg("-e").arg(&shell).arg("-c").arg(command);

    let status = cmd.status().context("Failed to launch Kitty")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Failed to launch Kitty. Make sure it is installed.")
    }
}

#[cfg(target_os = "macos")]
fn launch_wezterm(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let full_command = build_shell_command(command, None);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

    let mut args = vec!["-na", "WezTerm", "--args", "start"];

    if let Some(dir) = cwd {
        args.push("--cwd");
        args.push(dir);
    }

    args.push("--");
    args.push(&shell);
    args.push("-c");
    args.push(&full_command);

    let status = Command::new("open")
        .args(&args)
        .status()
        .context("Failed to launch WezTerm")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Failed to launch WezTerm.")
    }
}

#[cfg(target_os = "macos")]
fn launch_alacritty(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let full_command = build_shell_command(command, None);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

    let mut args = vec!["-na", "Alacritty", "--args"];

    if let Some(dir) = cwd {
        args.push("--working-directory");
        args.push(dir);
    }

    args.push("-e");
    args.push(&shell);
    args.push("-c");
    args.push(&full_command);

    let status = Command::new("open")
        .args(&args)
        .status()
        .context("Failed to launch Alacritty")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("Failed to launch Alacritty.")
    }
}

#[cfg(target_os = "macos")]
fn build_shell_command(command: &str, cwd: Option<&str>) -> String {
    match cwd {
        Some(dir) if !dir.trim().is_empty() => {
            format!("cd {} && {}", shell_escape(dir), command)
        }
        _ => command.to_string(),
    }
}

#[cfg(target_os = "macos")]
fn shell_escape(value: &str) -> String {
    // Single-quote wrapping is the POSIX-safe approach: only ' needs escaping
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(target_os = "macos")]
fn escape_osascript(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "")
}

// --- Windows terminal launchers ---
// Use `cmd /C start` with CREATE_NO_WINDOW to launch terminals cleanly, and
// generate platform-correct command syntax for each shell (PowerShell vs CMD).

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
fn launch_windows_terminal(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    // Windows Terminal: `wt cmd /K <bat_command>`
    // Uses cmd as inner shell so it works regardless of WT default profile.
    let bat_cmd = build_cmd_command(command, cwd);
    run_windows_start(&["wt", "cmd", "/K", &bat_cmd], "Windows Terminal")
}

#[cfg(target_os = "windows")]
fn launch_windows_powershell(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let ps_cmd = build_powershell_command(command, cwd);
    run_windows_start(
        &["powershell", "-NoExit", "-Command", &ps_cmd],
        "PowerShell",
    )
}

#[cfg(target_os = "windows")]
fn launch_windows_cmd(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    // Write a temp .bat file to avoid `&&` being parsed by the outer cmd in
    // `cmd /C start "" cmd /K "cd /d ... && ..."`.
    let bat_path = std::env::temp_dir().join(format!("cc_session_{}.bat", std::process::id()));
    let mut bat = String::from("@echo off\r\n");
    if let Some(dir) = cwd {
        if !dir.is_empty() {
            bat.push_str(&format!("cd /d \"{}\"\r\n", dir));
        }
    }
    bat.push_str(command);
    bat.push_str("\r\n");
    std::fs::write(&bat_path, &bat).context("failed to write temp script")?;

    let bat_str = bat_path.to_string_lossy().to_string();
    run_windows_start(&["cmd", "/K", &bat_str], "Command Prompt")
}

/// Launch a Windows terminal via `cmd /C start` with CREATE_NO_WINDOW to avoid
/// a flash of a console window from the parent process.
#[cfg(target_os = "windows")]
fn run_windows_start(args: &[&str], terminal_name: &str) -> anyhow::Result<()> {
    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg("start").arg("").args(args);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.spawn()
        .with_context(|| format!("failed to launch {terminal_name}"))?;
    Ok(())
}

/// Build a command string for PowerShell: `cd 'dir'; command`
#[cfg(target_os = "windows")]
fn build_powershell_command(command: &str, cwd: Option<&str>) -> String {
    match cwd {
        Some(dir) if !dir.is_empty() => {
            format!("cd '{}'; {}", dir.replace('\'', "''"), command)
        }
        _ => command.to_string(),
    }
}

/// Build a command string for CMD: `cd /d "dir" && command`
#[cfg(target_os = "windows")]
fn build_cmd_command(command: &str, cwd: Option<&str>) -> String {
    match cwd {
        Some(dir) if !dir.is_empty() => {
            format!("cd /d \"{}\" && {}", dir.replace('"', "\"\""), command)
        }
        _ => command.to_string(),
    }
}

// --- Linux terminal launchers ---

#[cfg(target_os = "linux")]
fn linux_shell_command(command: &str, cwd: Option<&str>) -> String {
    match cwd {
        Some(dir) if !dir.trim().is_empty() => {
            let escaped = dir.replace('\'', "'\\''");
            format!("cd '{}' && {}", escaped, command)
        }
        _ => command.to_string(),
    }
}

#[cfg(target_os = "linux")]
fn launch_linux_generic(
    terminal: &str,
    args_builder: impl FnOnce(&str, &str) -> Vec<String>,
    command: &str,
    cwd: Option<&str>,
) -> anyhow::Result<()> {
    let full = linux_shell_command(command, cwd);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let args = args_builder(&shell, &full);
    Command::new(terminal)
        .args(&args)
        .spawn()
        .with_context(|| format!("failed to launch {terminal}"))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn launch_linux_gnome_terminal(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    launch_linux_generic(
        "gnome-terminal",
        |shell, full| vec!["--".into(), shell.into(), "-c".into(), full.into()],
        command,
        cwd,
    )
}

#[cfg(target_os = "linux")]
fn launch_linux_konsole(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    launch_linux_generic(
        "konsole",
        |shell, full| vec!["-e".into(), shell.into(), "-c".into(), full.into()],
        command,
        cwd,
    )
}

#[cfg(target_os = "linux")]
fn launch_linux_xterm(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    launch_linux_generic(
        "xterm",
        |shell, full| vec!["-e".into(), shell.into(), "-c".into(), full.into()],
        command,
        cwd,
    )
}

#[cfg(target_os = "linux")]
fn launch_linux_alacritty(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let full = linux_shell_command(command, None);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let mut cmd = Command::new("alacritty");
    if let Some(dir) = cwd {
        if !dir.trim().is_empty() {
            cmd.arg("--working-directory").arg(dir);
        }
    }
    cmd.args(["-e", &shell, "-c", &full]);
    cmd.spawn().context("failed to launch alacritty")?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn launch_linux_kitty(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let mut cmd = Command::new("kitty");
    if let Some(dir) = cwd {
        if !dir.trim().is_empty() {
            cmd.arg("--directory").arg(dir);
        }
    }
    cmd.args(["-e", &shell, "-c", command]);
    cmd.spawn().context("failed to launch kitty")?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn launch_linux_wezterm(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    let full = linux_shell_command(command, None);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let mut cmd = Command::new("wezterm");
    cmd.arg("start");
    if let Some(dir) = cwd {
        if !dir.trim().is_empty() {
            cmd.arg("--cwd").arg(dir);
        }
    }
    cmd.args(["--", &shell, "-c", &full]);
    cmd.spawn().context("failed to launch wezterm")?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn launch_linux_default(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    // Try common terminals in order of popularity
    let terminals = ["gnome-terminal", "konsole", "xfce4-terminal", "xterm"];

    for term in &terminals {
        let result = launch_linux_generic(
            term,
            |shell, full| vec!["--".into(), shell.into(), "-c".into(), full.into()],
            command,
            cwd,
        );
        if result.is_ok() {
            return Ok(());
        }
    }

    anyhow::bail!("no supported terminal emulator found; install gnome-terminal, konsole, or xterm")
}
