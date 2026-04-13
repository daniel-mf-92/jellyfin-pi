use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::Duration;
use log::{info, warn, debug};

use super::DaemonShared;

/// Wayland environment vars needed for pactl/wpctl/systemctl --user.
const WAYLAND_DISPLAY: &str = "wayland-0";
const XDG_RUNTIME_DIR: &str = "/run/user/1000";

pub struct AudioHealer {
    shared: Arc<DaemonShared>,
    interval_sec: u64,
    hdmi_miss_streak: Mutex<u8>,
}

impl AudioHealer {
    pub fn new(shared: Arc<DaemonShared>, interval_sec: u64) -> Self {
        Self {
            shared,
            interval_sec,
            hdmi_miss_streak: Mutex::new(0),
        }
    }

    pub async fn run(self) {
        // Wait for GUI to be ready
        tokio::time::sleep(Duration::from_secs(30)).await;

        let mut interval = tokio::time::interval(Duration::from_secs(self.interval_sec));
        loop {
            interval.tick().await;

            // Only heal if labwc compositor is running
            if !super::system::is_process_running("labwc").await {
                continue;
            }

            self.heal_pipewire().await;
            self.heal_hdmi_sink().await;
        }
    }

    /// Check PipeWire health via `pactl list sinks short` with 3s timeout.
    /// Restart the audio stack if unresponsive.
    async fn heal_pipewire(&self) {
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            self.run_wayland_cmd("pactl", &["list", "sinks", "short"]),
        )
        .await;

        let sink_output = match result {
            Ok(Ok(output)) if !output.is_empty() => output,
            _ => {
                // PipeWire not responding — attempt restart
                warn!("PipeWire/pactl not responding, restarting audio stack...");

                let mut cb = self.shared.circuit_breaker.write().await;
                if !cb.try_restart("pipewire", None) {
                    return;
                }
                drop(cb);

                let _ = Command::new("systemctl")
                    .args(["--user", "restart", "pipewire", "pipewire-pulse", "wireplumber"])
                    .env("XDG_RUNTIME_DIR", XDG_RUNTIME_DIR)
                    .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/bus", XDG_RUNTIME_DIR))
                    .output()
                    .await;

                tokio::time::sleep(Duration::from_secs(3)).await;

                // Verify recovery
                if let Ok(output) = self
                    .run_wayland_cmd("pactl", &["list", "sinks", "short"])
                    .await
                {
                    if !output.is_empty() {
                        info!("Audio stack restored");
                    } else {
                        warn!("Audio stack still broken after restart");
                    }
                }
                return;
            }
        };

        // Ensure HDMI is default sink (not null/network)
        if let Some(hdmi_sink) = sink_output.lines().find(|l| l.contains("hdmi")) {
            let sink_name = hdmi_sink.split_whitespace().nth(1).unwrap_or("");
            if !sink_name.is_empty() {
                // Check current default
                let current = self
                    .run_wayland_cmd("pactl", &["get-default-sink"])
                    .await
                    .unwrap_or_default();
                let current = current.trim();

                if current != sink_name {
                    let _ = self
                        .run_wayland_cmd("pactl", &["set-default-sink", sink_name])
                        .await;
                    info!("Set default sink to HDMI: {} (was: {})", sink_name, current);
                }

                // Ensure volume at 100%
                let _ = self
                    .run_wayland_cmd("pactl", &["set-sink-volume", sink_name, "100%"])
                    .await;
            }
        } else {
            debug!("No HDMI sink found in pactl output");
        }
    }

    /// Check WirePlumber HDMI sink via `wpctl status`.
    async fn heal_hdmi_sink(&self) {
        let status = match self.run_wayland_cmd("wpctl", &["status"]).await {
            Ok(s) => s,
            Err(_) => return,
        };

        // Find HDMI sink ID
        let hdmi_id = status
            .lines()
            .find(|l| l.contains("Built-in Audio Digital Stereo (HDMI)"))
            .and_then(|l| {
                l.trim()
                    .trim_start_matches('*')
                    .trim()
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.trim_end_matches('.').parse::<u32>().ok())
            });

        if let Some(id) = hdmi_id {
            {
                let mut miss_streak = self.hdmi_miss_streak.lock().await;
                *miss_streak = 0;
            }

            // Check if it's the default (has * prefix)
            let is_default = status
                .lines()
                .any(|l| l.contains("Built-in Audio Digital Stereo (HDMI)") && l.trim().starts_with('*'));

            if !is_default {
                let _ = self
                    .run_wayland_cmd("wpctl", &["set-default", &id.to_string()])
                    .await;
                info!("Reset WirePlumber default sink to HDMI (id: {})", id);
            }
        } else {
            let misses = {
                let mut miss_streak = self.hdmi_miss_streak.lock().await;
                *miss_streak = miss_streak.saturating_add(1);
                *miss_streak
            };

            // Debounce transient PipeWire race conditions before restart.
            if misses < 3 {
                debug!("HDMI sink not visible yet (miss {}/3), deferring WirePlumber restart", misses);
                return;
            }

            // HDMI sink missing repeatedly — restart WirePlumber
            let mut cb = self.shared.circuit_breaker.write().await;
            if !cb.try_restart("wireplumber", None) {
                return;
            }
            drop(cb);

            {
                let mut miss_streak = self.hdmi_miss_streak.lock().await;
                *miss_streak = 0;
            }

            warn!("HDMI sink missing, restarting WirePlumber...");
            let _ = Command::new("systemctl")
                .args(["--user", "restart", "wireplumber"])
                .env("XDG_RUNTIME_DIR", XDG_RUNTIME_DIR)
                .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/bus", XDG_RUNTIME_DIR))
                .output()
                .await;

            tokio::time::sleep(Duration::from_secs(3)).await;
            info!("WirePlumber restarted");
        }
    }

    /// Run a command with Wayland + XDG environment set.
    async fn run_wayland_cmd(&self, program: &str, args: &[&str]) -> Result<String, std::io::Error> {
        let output = Command::new(program)
            .args(args)
            .env("WAYLAND_DISPLAY", WAYLAND_DISPLAY)
            .env("XDG_RUNTIME_DIR", XDG_RUNTIME_DIR)
            .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}/bus", XDG_RUNTIME_DIR))
            .output()
            .await?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}
