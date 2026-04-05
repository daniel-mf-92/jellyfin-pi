use std::sync::Arc;
use std::path::Path;
use tokio::sync::{mpsc, watch, RwLock};
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use log::{info, warn, debug, error};

use crate::player::vlc::PlayerEvent;
use super::DaemonShared;

pub struct SystemTasks {
    shared: Arc<DaemonShared>,
    screen_watch_rx: watch::Receiver<String>,
    player_event_rx: mpsc::UnboundedReceiver<PlayerEvent>,
    flex_heal_enabled: bool,
}

impl SystemTasks {
    pub fn new(
        shared: Arc<DaemonShared>,
        screen_watch_rx: watch::Receiver<String>,
        player_event_rx: mpsc::UnboundedReceiver<PlayerEvent>,
        flex_heal_enabled: bool,
    ) -> Self {
        Self {
            shared,
            screen_watch_rx,
            player_event_rx,
            flex_heal_enabled,
        }
    }

    /// Spawn all system tasks. Returns join handles.
    pub fn spawn(self) -> Vec<JoinHandle<()>> {
        let mut handles = Vec::new();

        // One-shot: block IMU device at startup
        handles.push(tokio::spawn(async { Self::block_imu().await }));

        // Periodic: write /tmp/foreground-app on screen changes
        handles.push(tokio::spawn(Self::foreground_app_writer(self.screen_watch_rx)));

        // Periodic: keep screen alive during playback
        handles.push(tokio::spawn(Self::screen_alive(self.player_event_rx)));

        // Periodic: flex-launcher health check
        if self.flex_heal_enabled {
            // DISABLED: flex-launcher managed by labwc autostart, not by jellyfin-pi daemon
            // handles.push(tokio::spawn(Self::flex_launcher_heal(self.shared)));
        }

        handles
    }

    /// One-shot: chmod 000 on Pro Controller IMU device to prevent phantom mouse movement.
    /// The IMU spams ~500 events/sec that cause cursor drift.
    async fn block_imu() {
        // Scan /sys/class/input/*/device/name for "Pro Controller (IMU)"
        let output = Command::new("grep")
            .args(["-rl", "Pro Controller (IMU)", "/sys/class/input/"])
            .output()
            .await;

        let path = match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let line = stdout.lines().next().unwrap_or("");
                if line.is_empty() {
                    debug!("No Pro Controller IMU device found");
                    return;
                }
                // Extract eventN from path like /sys/class/input/event5/device/name
                if let Some(event) = line.split('/').find(|s| s.starts_with("event")) {
                    format!("/dev/input/{}", event)
                } else {
                    return;
                }
            }
            Err(_) => return,
        };

        if !Path::new(&path).exists() {
            return;
        }

        // Check current permissions
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(_) => return,
        };

        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0 {
            match Command::new("sudo")
                .args(["chmod", "000", &path])
                .output()
                .await
            {
                Ok(out) if out.status.success() => {
                    info!("Blocked IMU device {} (was {:o})", path, mode);
                }
                Ok(out) => {
                    warn!("Failed to block IMU: {}", String::from_utf8_lossy(&out.stderr));
                }
                Err(e) => warn!("Failed to run chmod for IMU: {}", e),
            }
        }
    }

    /// Write current screen name to /tmp/foreground-app on every navigation change.
    /// Uses watch channel — only wakes on changes, not polling.
    async fn foreground_app_writer(mut rx: watch::Receiver<String>) {
        loop {
            if rx.changed().await.is_err() {
                break; // sender dropped
            }
            let screen_name = rx.borrow().clone();

            // Map screen names to the app identifiers the master script expects
            let app_name = match screen_name.as_str() {
                "player" => "jellyfinmediaplayer",
                "home" | "detail" | "library" | "search" | "settings" | "login" => "jellyfin",
                _ => "jellyfin",
            };

            if let Err(e) = tokio::fs::write("/tmp/foreground-app", app_name).await {
                warn!("Failed to write /tmp/foreground-app: {}", e);
            } else {
                debug!("Foreground app: {}", app_name);
            }
        }
    }

    /// Keep screen alive during playback by running wlopm --on every 60s.
    /// Also simulates input to prevent flex-launcher screensaver.
    async fn screen_alive(mut rx: mpsc::UnboundedReceiver<PlayerEvent>) {
        let mut playing = false;

        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Some(PlayerEvent::Playing) => playing = true,
                        Some(PlayerEvent::Stopped) | Some(PlayerEvent::EndOfFile) => playing = false,
                        None => break,
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    if playing {
                        // Keep screen on
                        let _ = Command::new("bash")
                            .args(["-c", "wlopm --on $(wlopm 2>/dev/null | awk '{print $1}' | head -1) 2>/dev/null"])
                            .env("WAYLAND_DISPLAY", "wayland-0")
                            .env("XDG_RUNTIME_DIR", "/run/user/1000")
                            .output()
                            .await;

                        // Simulate minimal input to prevent flex-launcher screensaver
                        let _ = Command::new("wlrctl")
                            .args(["pointer", "move", "0", "0"])
                            .env("WAYLAND_DISPLAY", "wayland-0")
                            .env("XDG_RUNTIME_DIR", "/run/user/1000")
                            .output()
                            .await;
                    }
                }
            }
        }
    }

    /// Restart flex-launcher if dead (skip if moonlight-qt or no labwc).
    /// Runs every 2 minutes. Circuit-breaker gated.
    async fn flex_launcher_heal(shared: Arc<DaemonShared>) {
        let mut interval = tokio::time::interval(Duration::from_secs(120));

        loop {
            interval.tick().await;

            // Skip if flex-launcher is running
            if is_process_running("flex-launcher").await {
                continue;
            }

            // Skip if moonlight-qt is active (game session)
            if is_process_running("moonlight-qt").await {
                debug!("flex-launcher dead but moonlight active, skipping");
                continue;
            }

            // Skip if labwc not active (no GUI)
            if !is_process_running("labwc").await {
                debug!("flex-launcher dead but no labwc, skipping");
                continue;
            }

            // Circuit breaker check
            {
                let mut cb = shared.circuit_breaker.write().await;
                if !cb.try_restart("flex-launcher", None) {
                    continue;
                }
            }

            info!("flex-launcher not running, restarting...");
            match Command::new("nohup")
                .arg("/usr/local/bin/flex-launcher")
                .env("WAYLAND_DISPLAY", "wayland-0")
                .env("XDG_RUNTIME_DIR", "/run/user/1000")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(_) => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if is_process_running("flex-launcher").await {
                        info!("flex-launcher restarted successfully");
                    } else {
                        warn!("flex-launcher failed to restart");
                    }
                }
                Err(e) => error!("Failed to spawn flex-launcher: {}", e),
            }
        }
    }
}

/// Check if a process is running via pgrep -x.
pub async fn is_process_running(name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", name])
        .output()
        .await
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Get PIDs of a process by name.
pub async fn get_pids(name: &str) -> Vec<i32> {
    let output = Command::new("pgrep")
        .args(["-x", name])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|line| line.trim().parse::<i32>().ok())
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Get PIDs of processes matching a pattern.
pub async fn get_pids_pattern(pattern: &str) -> Vec<i32> {
    let output = Command::new("pgrep")
        .args(["-f", pattern])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|line| line.trim().parse::<i32>().ok())
                .collect()
        }
        _ => Vec::new(),
    }
}
