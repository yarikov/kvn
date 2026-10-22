use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Wayland,
    XclipX11,
    XselX11,
}

pub fn read_clipboard_text() -> Result<String> {
    let backend = backend()?;
    let (cmd, args): (&str, &[&str]) = match backend {
        Backend::Wayland => ("wl-paste", &[]),
        Backend::XclipX11 => ("xclip", &["-selection", "clipboard", "-o"]),
        Backend::XselX11 => ("xsel", &["--clipboard", "--output"]),
    };
    let text = read_clipboard_command(cmd, args)?;
    if !text.is_empty() {
        Ok(text)
    } else {
        anyhow::bail!("Clipboard is empty or unavailable")
    }
}

pub fn write_clipboard_text(text: &str) -> Result<()> {
    match backend()? {
        Backend::Wayland => write_via_daemon("wl-copy", &[], text),
        Backend::XclipX11 => write_via_pipe("xclip", &["-selection", "clipboard", "-i"], text),
        Backend::XselX11 => write_via_pipe("xsel", &["--clipboard", "--input"], text),
    }
}

fn write_via_daemon(cmd: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to spawn {}", cmd))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .with_context(|| format!("{} stdin not available", cmd))?;
        stdin
            .write_all(text.as_bytes())
            .with_context(|| format!("Failed to write to {} stdin", cmd))?;
    }
    let status = child
        .wait()
        .with_context(|| format!("Failed to wait for {}", cmd))?;
    if !status.success() {
        anyhow::bail!("{} failed: {}", cmd, status);
    }
    Ok(())
}

fn write_via_pipe(cmd: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn {}", cmd))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .with_context(|| format!("{} stdin not available", cmd))?;
        stdin
            .write_all(text.as_bytes())
            .with_context(|| format!("Failed to write to {} stdin", cmd))?;
    }
    let output = child
        .wait_with_output()
        .with_context(|| format!("Failed to wait for {}", cmd))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "{} failed: {} (stderr: {})",
            cmd,
            output.status,
            stderr.trim()
        );
    }
    Ok(())
}

fn read_clipboard_command(cmd: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute {}", cmd))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "{} failed: {} (stderr: {})",
            cmd,
            output.status,
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn backend() -> Result<Backend> {
    static CACHED: OnceLock<Option<Backend>> = OnceLock::new();
    CACHED
        .get_or_init(|| detect(|k| std::env::var(k).ok(), is_on_path))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No clipboard tool found: install wl-clipboard (Wayland), xclip, or xsel"
            )
        })
}

fn detect<E, P>(env: E, has_bin: P) -> Option<Backend>
where
    E: Fn(&str) -> Option<String>,
    P: Fn(&str) -> bool,
{
    let wayland_session = env("WAYLAND_DISPLAY").is_some();
    let x11_session = env("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("x11"))
        .unwrap_or(false);

    if wayland_session && has_bin("wl-paste") {
        return Some(Backend::Wayland);
    }
    if x11_session {
        if has_bin("xclip") {
            return Some(Backend::XclipX11);
        }
        if has_bin("xsel") {
            return Some(Backend::XselX11);
        }
    }
    if has_bin("wl-paste") {
        return Some(Backend::Wayland);
    }
    if has_bin("xclip") {
        return Some(Backend::XclipX11);
    }
    if has_bin("xsel") {
        return Some(Backend::XselX11);
    }
    None
}

fn is_on_path(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file() || candidate.symlink_metadata().is_ok_and(|m| !m.is_dir())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_prefers_wayland_when_session_and_binary_present() {
        let env = |k: &str| (k == "WAYLAND_DISPLAY").then(|| "wayland-0".to_string());
        let bins = |n: &str| matches!(n, "wl-paste" | "xclip");
        assert_eq!(detect(env, bins), Some(Backend::Wayland));
    }

    #[test]
    fn detect_falls_back_to_x11_when_wayland_session_but_no_binary() {
        let env = |k: &str| (k == "WAYLAND_DISPLAY").then(|| "wayland-0".to_string());
        let bins = |n: &str| n == "xclip";
        assert_eq!(detect(env, bins), Some(Backend::XclipX11));
    }

    #[test]
    fn detect_uses_xclip_for_x11_session() {
        let env = |k: &str| (k == "XDG_SESSION_TYPE").then(|| "x11".to_string());
        let bins = |n: &str| matches!(n, "xclip" | "xsel");
        assert_eq!(detect(env, bins), Some(Backend::XclipX11));
    }

    #[test]
    fn detect_uses_xsel_when_xclip_missing_on_x11() {
        let env = |k: &str| (k == "XDG_SESSION_TYPE").then(|| "x11".to_string());
        let bins = |n: &str| n == "xsel";
        assert_eq!(detect(env, bins), Some(Backend::XselX11));
    }

    #[test]
    fn detect_falls_back_through_chain_with_no_session_hints() {
        let env = |_: &str| None;
        let bins = |n: &str| n == "xsel";
        assert_eq!(detect(env, bins), Some(Backend::XselX11));
    }

    #[test]
    fn detect_prefers_wayland_in_fallback_chain() {
        let env = |_: &str| None;
        let bins = |n: &str| matches!(n, "wl-paste" | "xclip" | "xsel");
        assert_eq!(detect(env, bins), Some(Backend::Wayland));
    }

    #[test]
    fn detect_returns_none_when_nothing_available() {
        let env = |k: &str| (k == "WAYLAND_DISPLAY").then(|| "wayland-0".to_string());
        let bins = |_: &str| false;
        assert_eq!(detect(env, bins), None);
    }

    #[test]
    fn detect_ignores_x11_session_var_casing() {
        let env = |k: &str| (k == "XDG_SESSION_TYPE").then(|| "X11".to_string());
        let bins = |n: &str| n == "xclip";
        assert_eq!(detect(env, bins), Some(Backend::XclipX11));
    }
}
