use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use directories::ProjectDirs;
use log::{info, warn};

/// Top-level application configuration, loaded from `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub playback: PlaybackConfig,
    pub controller: ControllerConfig,
    pub ui: UiConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub url: String,
    pub device_id: String,
    pub device_name: String,
    pub client_name: String,
    pub client_version: String,
    /// Cached user ID for auto-login on next launch.
    pub saved_user_id: Option<String>,
    /// Cached access token for auto-login on next launch.
    pub saved_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackConfig {
    /// Extra VLC command-line arguments.
    pub vlc_args: Vec<String>,
    /// ALSA audio device path.
    pub audio_device: String,
    /// Audio delay in milliseconds (negative = earlier). Useful for HDMI lip-sync.
    pub audio_delay_ms: f64,
    /// On-screen subtitle font size.
    pub subtitle_size: i32,
    /// Maximum streaming bitrate in bits/sec.
    pub max_streaming_bitrate: i64,
    /// Whether to prefer direct play over transcoding.
    pub prefer_direct_play: bool,
    /// Enable audio passthrough (AC3, DTS, TrueHD) via SPDIF/HDMI.
    pub audio_passthrough: bool,
    /// Subtitle font size override (0 = default).
    pub subtitle_font_size: i32,
    /// Subtitle color as hex string (e.g. "#FFFFFF").
    pub subtitle_color: String,
    /// Subtitle position (0 = bottom, 100 = top).
    pub subtitle_position: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerConfig {
    /// Analog stick deadzone (0-32767).
    pub deadzone: i32,
    /// Milliseconds before key-repeat begins.
    pub repeat_delay_ms: u64,
    /// Milliseconds between repeated key events.
    pub repeat_rate_ms: u64,
    /// Minutes of inactivity before the controller is considered idle.
    pub idle_disconnect_min: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// Seconds of inactivity before the screensaver activates.
    pub screensaver_timeout_sec: u32,
    /// UI theme name.
    pub theme: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Bandwidth test interval in seconds.
    pub bandwidth_interval_sec: u64,
    /// Buffer check interval in seconds.
    pub buffer_interval_sec: u64,
    /// Audio heal interval in seconds.
    pub audio_heal_interval_sec: u64,
    /// Flex-launcher heal interval in seconds.
    pub flex_heal_interval_sec: u64,
    /// Minimum free RAM in MB before buffer eviction.
    pub buffer_min_free_ram_mb: u64,
    /// QoS grace period in seconds after playback stops.
    pub qos_grace_period_sec: u64,
    /// Max circuit breaker restarts per hour.
    pub circuit_breaker_max_per_hour: usize,
    /// Known Jellyfin item ID for bandwidth speed test.
    pub bandwidth_test_item_id: String,
    /// Enable QoS (tc/renice/SIGSTOP).
    pub qos_enabled: bool,
    /// Enable audio healing.
    pub audio_heal_enabled: bool,
    /// Enable flex-launcher healing.
    pub flex_heal_enabled: bool,
    /// Enable buffer manager.
    pub buffer_enabled: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            bandwidth_interval_sec: 300,
            buffer_interval_sec: 120,
            audio_heal_interval_sec: 120,
            flex_heal_interval_sec: 120,
            buffer_min_free_ram_mb: 2048,
            qos_grace_period_sec: 600,
            circuit_breaker_max_per_hour: 3,
            bandwidth_test_item_id: "e6067924303046d641ce61f9f80e260d".to_string(),
            qos_enabled: true,
            audio_heal_enabled: true,
            flex_heal_enabled: true,
            buffer_enabled: true,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                url: "https://localhost:8096".to_string(),
                device_id: uuid::Uuid::new_v4().to_string(),
                device_name: "Jellyfin TV (Pi)".to_string(),
                client_name: "Jellyfin TV".to_string(),
                client_version: "1.0.0".to_string(),
                saved_user_id: None,
                saved_token: None,
            },
            playback: PlaybackConfig {
                vlc_args: vec![
                    "--avcodec-hw".to_string(),
                    "any".to_string(),
                    "--network-caching".to_string(),
                    "5000".to_string(),
                ],
                audio_device: "alsa/default".to_string(),
                audio_delay_ms: -300.0,
                subtitle_size: 48,
                max_streaming_bitrate: 120_000_000,
                prefer_direct_play: true,
                audio_passthrough: true,
                subtitle_font_size: 48,
                subtitle_color: "#FFFFFF".to_string(),
                subtitle_position: 10,
            },
            controller: ControllerConfig {
                deadzone: 12000,
                repeat_delay_ms: 400,
                repeat_rate_ms: 150,
                idle_disconnect_min: 15,
            },
            ui: UiConfig {
                screensaver_timeout_sec: 300,
                theme: "dark".to_string(),
            },
            daemon: DaemonConfig::default(),
        }
    }
}

impl AppConfig {
    /// Returns the configuration directory for this application.
    ///
    /// Uses `directories::ProjectDirs` when available, otherwise falls back
    /// to `~/.config/jellyfin-tv/`.
    pub fn config_dir() -> PathBuf {
        if let Some(proj) = ProjectDirs::from("org", "jellyfin", "jellyfin-tv") {
            proj.config_dir().to_path_buf()
        } else {
            warn!("Could not determine XDG config directory, falling back to ~/.config/jellyfin-tv/");
            let mut fallback = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
            fallback.push(".config");
            fallback.push("jellyfin-tv");
            fallback
        }
    }

    /// Returns the full path to the configuration file.
    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// Loads the configuration from disk.
    ///
    /// If the file does not exist or cannot be parsed, a default configuration
    /// is created, saved to disk, and returned.
    pub fn load() -> Self {
        let path = Self::config_path();

        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str::<AppConfig>(&contents) {
                    Ok(config) => {
                        info!("Loaded configuration from {}", path.display());
                        return config;
                    }
                    Err(e) => {
                        warn!(
                            "Failed to parse config at {}: {}. Using defaults.",
                            path.display(),
                            e
                        );
                    }
                },
                Err(e) => {
                    warn!(
                        "Failed to read config at {}: {}. Using defaults.",
                        path.display(),
                        e
                    );
                }
            }
        } else {
            info!(
                "No config file found at {}. Creating with defaults.",
                path.display()
            );
        }

        let config = AppConfig::default();
        if let Err(e) = config.save() {
            warn!("Failed to save default config: {}", e);
        }
        config
    }

    /// Serializes and writes the current configuration to disk.
    ///
    /// Creates parent directories if they do not exist.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::config_path();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let toml_str = toml::to_string_pretty(self)?;
        std::fs::write(&path, toml_str)?;
        info!("Configuration saved to {}", path.display());
        Ok(())
    }

    /// Saves authentication credentials for auto-login on next launch.
    pub fn save_auth(&mut self, user_id: &str, token: &str) {
        self.server.saved_user_id = Some(user_id.to_string());
        self.server.saved_token = Some(token.to_string());
        if let Err(e) = self.save() {
            warn!("Failed to save auth credentials: {}", e);
        }
    }

    /// Clears saved authentication credentials.
    pub fn clear_auth(&mut self) {
        self.server.saved_user_id = None;
        self.server.saved_token = None;
        if let Err(e) = self.save() {
            warn!("Failed to save config after clearing auth: {}", e);
        }
    }
}
