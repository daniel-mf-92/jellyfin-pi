use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::process::Command;
use tokio::time::{Duration, Instant};
use log::{info, warn, debug, error};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;

use crate::player::vlc::PlayerEvent;
use super::{DaemonEvent, DaemonShared};
use super::system::{get_pids, get_pids_pattern};

pub struct QosController {
    shared: Arc<DaemonShared>,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    player_event_rx: mpsc::UnboundedReceiver<PlayerEvent>,
    grace_period: Duration,
}

impl QosController {
    pub fn new(
        shared: Arc<DaemonShared>,
        event_tx: mpsc::UnboundedSender<DaemonEvent>,
        player_event_rx: mpsc::UnboundedReceiver<PlayerEvent>,
        grace_period_sec: u64,
    ) -> Self {
        Self {
            shared,
            event_tx,
            player_event_rx,
            grace_period: Duration::from_secs(grace_period_sec),
        }
    }

    pub async fn run(mut self) {
        let mut last_playing_at: Option<Instant> = None;
        let mut qos_active = false;

        loop {
            tokio::select! {
                event = self.player_event_rx.recv() => {
                    match event {
                        Some(PlayerEvent::Playing) => {
                            last_playing_at = Some(Instant::now());
                            if !qos_active {
                                self.enable().await;
                                qos_active = true;
                            }
                        }
                        Some(PlayerEvent::Stopped) | Some(PlayerEvent::EndOfFile) => {
                            // Don't disable immediately — start grace period
                            last_playing_at = Some(Instant::now());
                        }
                        Some(PlayerEvent::Paused) => {
                            // Keep QoS active during pause
                            last_playing_at = Some(Instant::now());
                        }
                        None => break, // sender dropped
                        _ => {}
                    }
                }
                // Check grace period every 30s
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    if qos_active {
                        if let Some(last) = last_playing_at {
                            if last.elapsed() > self.grace_period {
                                self.disable().await;
                                qos_active = false;
                                last_playing_at = None;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Enable streaming QoS mode.
    async fn enable(&self) {
        info!("QoS: ENABLING streaming mode");

        // 1. SIGSTOP go2rtc (most effective — immediate bandwidth relief)
        self.signal_go2rtc(Signal::SIGSTOP).await;

        // 2. tc rate limiting on wg0
        self.apply_tc_shaping().await;

        // 3. Renice media players high, go2rtc/chromium low
        self.set_priorities_streaming().await;

        // 4. Kill bandwidth hogs
        self.kill_bandwidth_hogs().await;

        // 5. Notify Azure VM QoS (background, non-blocking)
        tokio::spawn(async {
            let _ = Command::new("ssh")
                .args([
                    "-o", "ConnectTimeout=3",
                    "-o", "BatchMode=yes",
                    "-o", "StrictHostKeyChecking=no",
                    "relay-host.local",
                    "$HOME/bin/jellyfin-qos.sh enable",
                ])
                .output()
                .await;
        });

        *self.shared.qos_active.write().await = true;
        let _ = self.event_tx.send(DaemonEvent::QosEnabled);
        info!("QoS: Streaming mode ACTIVE");
    }

    /// Disable streaming QoS mode.
    async fn disable(&self) {
        info!("QoS: DISABLING streaming mode");

        // 1. SIGCONT go2rtc
        self.signal_go2rtc(Signal::SIGCONT).await;

        // 2. Remove tc shaping
        self.remove_tc_shaping().await;

        // 3. Restore all priorities to 0
        self.restore_priorities().await;

        // 4. Notify Azure VM QoS disable
        tokio::spawn(async {
            let _ = Command::new("ssh")
                .args([
                    "-o", "ConnectTimeout=3",
                    "-o", "BatchMode=yes",
                    "-o", "StrictHostKeyChecking=no",
                    "relay-host.local",
                    "$HOME/bin/jellyfin-qos.sh disable",
                ])
                .output()
                .await;
        });

        *self.shared.qos_active.write().await = false;
        let _ = self.event_tx.send(DaemonEvent::QosDisabled);
        info!("QoS: Normal operation resumed");
    }

    /// Send SIGSTOP or SIGCONT to go2rtc.
    /// go2rtc runs as root (systemd), so we need sudo kill.
    async fn signal_go2rtc(&self, sig: Signal) {
        let sig_name = match sig {
            Signal::SIGSTOP => "STOP",
            Signal::SIGCONT => "CONT",
            _ => return,
        };
        let pids = get_pids("go2rtc").await;
        for pid in pids {
            match Command::new("sudo")
                .args(["kill", &format!("-{}", sig_name), &pid.to_string()])
                .output()
                .await
            {
                Ok(out) if out.status.success() => {
                    info!("QoS: go2rtc (pid {}) sent SIG{}", pid, sig_name);
                }
                Ok(out) => {
                    warn!("QoS: Failed to send SIG{} to go2rtc pid {}: {}",
                        sig_name, pid, String::from_utf8_lossy(&out.stderr));
                }
                Err(e) => warn!("QoS: sudo kill failed for pid {}: {}", pid, e),
            }
        }
    }

    /// Apply tc qdisc rules: cap camera at 400kbit, prioritize media at 3200kbit.
    async fn apply_tc_shaping(&self) {
        // Remove existing rules first
        let _ = Command::new("sudo")
            .args(["tc", "qdisc", "del", "dev", "wg0", "root"])
            .output()
            .await;

        let commands: Vec<Vec<&str>> = vec![
            vec!["tc", "qdisc", "add", "dev", "wg0", "root", "handle", "1:", "htb", "default", "10"],
            vec!["tc", "class", "add", "dev", "wg0", "parent", "1:", "classid", "1:1", "htb", "rate", "3600kbit", "ceil", "3600kbit"],
            vec!["tc", "class", "add", "dev", "wg0", "parent", "1:1", "classid", "1:10", "htb", "rate", "3200kbit", "ceil", "3600kbit", "prio", "0"],
            vec!["tc", "class", "add", "dev", "wg0", "parent", "1:1", "classid", "1:20", "htb", "rate", "400kbit", "ceil", "400kbit", "prio", "1"],
            vec!["tc", "filter", "add", "dev", "wg0", "parent", "1:", "protocol", "ip", "prio", "1", "u32", "match", "ip", "sport", "8554", "0xffff", "flowid", "1:20"],
            vec!["tc", "filter", "add", "dev", "wg0", "parent", "1:", "protocol", "ip", "prio", "2", "u32", "match", "ip", "src", "0.0.0.0/0", "flowid", "1:10"],
        ];

        for args in commands {
            let result = Command::new("sudo")
                .args(&args)
                .output()
                .await;
            if let Err(e) = result {
                warn!("QoS: tc command failed: {}", e);
            }
        }

        info!("QoS: tc shaping applied — camera 400kbit, media 3200kbit priority");
    }

    /// Remove tc qdisc rules from wg0.
    async fn remove_tc_shaping(&self) {
        let _ = Command::new("sudo")
            .args(["tc", "qdisc", "del", "dev", "wg0", "root"])
            .output()
            .await;
        info!("QoS: tc shaping removed from wg0");
    }

    /// Boost media players, deprioritize go2rtc/chromium/kodi.
    async fn set_priorities_streaming(&self) {
        // Boost VLC, mpv
        for name in &["vlc", "mpv", "ffplay"] {
            for pid in get_pids(name).await {
                let _ = Command::new("sudo")
                    .args(["renice", "-15", "-p", &pid.to_string()])
                    .output()
                    .await;
            }
        }

        // Deprioritize go2rtc
        for pid in get_pids_pattern("go2rtc").await {
            let _ = Command::new("sudo")
                .args(["renice", "19", "-p", &pid.to_string()])
                .output()
                .await;
        }

        // Deprioritize chromium (limit to first 10)
        let chrome_pids = get_pids_pattern("chromium").await;
        for pid in chrome_pids.iter().take(10) {
            let _ = Command::new("sudo")
                .args(["renice", "15", "-p", &pid.to_string()])
                .output()
                .await;
        }

        // Deprioritize kodi
        for pid in get_pids_pattern("kodi").await {
            let _ = Command::new("sudo")
                .args(["renice", "15", "-p", &pid.to_string()])
                .output()
                .await;
        }
    }

    /// Restore all process priorities to 0.
    async fn restore_priorities(&self) {
        for name in &["vlc", "mpv", "ffplay"] {
            for pid in get_pids(name).await {
                let _ = Command::new("sudo")
                    .args(["renice", "0", "-p", &pid.to_string()])
                    .output()
                    .await;
            }
        }
        for pid in get_pids_pattern("go2rtc").await {
            let _ = Command::new("sudo")
                .args(["renice", "0", "-p", &pid.to_string()])
                .output()
                .await;
        }
        for pid in get_pids_pattern("chromium").await.iter().take(10) {
            let _ = Command::new("sudo")
                .args(["renice", "0", "-p", &pid.to_string()])
                .output()
                .await;
        }
        for pid in get_pids_pattern("kodi").await {
            let _ = Command::new("sudo")
                .args(["renice", "0", "-p", &pid.to_string()])
                .output()
                .await;
        }
    }

    /// Kill bandwidth hogs: wget, aria2c, and large curl transfers (not Jellyfin).
    async fn kill_bandwidth_hogs(&self) {
        let mut killed = 0u32;

        for name in &["wget", "aria2c"] {
            for pid in get_pids(name).await {
                let _ = Command::new("kill")
                    .arg(pid.to_string())
                    .output()
                    .await;
                killed += 1;
            }
        }

        // Kill large curl transfers that aren't Jellyfin-related
        let curl_pids = get_pids("curl").await;
        for pid in curl_pids {
            let cmdline = tokio::fs::read_to_string(format!("/proc/{}/cmdline", pid))
                .await
                .unwrap_or_default();

            // Skip Jellyfin buffer/API calls
            if cmdline.contains("localhost")
                || cmdline.contains("jellyfin-buffer")
                || cmdline.contains("localhost:8096")
            {
                continue;
            }

            let _ = Command::new("kill")
                .arg(pid.to_string())
                .output()
                .await;
            killed += 1;
        }

        if killed > 0 {
            info!("QoS: Killed {} bandwidth hogs", killed);
        }
    }
}
