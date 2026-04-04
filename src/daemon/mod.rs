pub mod circuit_breaker;
pub mod bandwidth;
pub mod buffer;
pub mod qos;
pub mod audio;
pub mod streaming;
pub mod system;

use std::sync::Arc;
use tokio::sync::{mpsc, watch, RwLock};
use tokio::task::JoinHandle;
use log::info;

use crate::api::JellyfinClient;
use crate::config::AppConfig;
use crate::player::vlc::PlayerEvent;
use crate::state::StateManager;

use circuit_breaker::CircuitBreaker;
use bandwidth::BandwidthMonitor;
use buffer::BufferManager;
use qos::QosController;
use audio::AudioHealer;
use streaming::StreamingHealth;
use system::SystemTasks;

// ---------------------------------------------------------------------------
// Bandwidth profile — shared between bandwidth, buffer, and main.rs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BandwidthProfile {
    pub timestamp: String,
    pub raw_bytes_per_sec: f64,
    pub total_bps: u64,
    pub video_bitrate: u64,
    pub audio_bitrate: u64,
    pub max_width: u32,
    pub max_height: u32,
}

// ---------------------------------------------------------------------------
// Daemon events — emitted by background tasks, consumed by main.rs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum DaemonEvent {
    /// Buffer file is ready for auto-play
    BufferReady { item_id: String, path: String },
    /// Bandwidth profile updated
    BandwidthUpdated(BandwidthProfile),
    /// Bitrate adaptation triggered (streaming health detected buffering)
    BitrateAdapted { video_bitrate: u64, audio_bitrate: u64 },
    /// QoS streaming mode enabled
    QosEnabled,
    /// QoS streaming mode disabled
    QosDisabled,
}

// ---------------------------------------------------------------------------
// Shared state visible to all daemon sub-tasks
// ---------------------------------------------------------------------------

pub struct DaemonShared {
    pub bandwidth: RwLock<Option<BandwidthProfile>>,
    pub qos_active: RwLock<bool>,
    pub circuit_breaker: RwLock<CircuitBreaker>,
}

impl DaemonShared {
    fn new(max_restarts_per_hour: usize) -> Self {
        Self {
            bandwidth: RwLock::new(None),
            qos_active: RwLock::new(false),
            circuit_breaker: RwLock::new(CircuitBreaker::new(max_restarts_per_hour)),
        }
    }
}

// ---------------------------------------------------------------------------
// DaemonManager — central orchestrator
// ---------------------------------------------------------------------------

pub struct DaemonManager {
    shared: Arc<DaemonShared>,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    event_rx: Option<mpsc::UnboundedReceiver<DaemonEvent>>,
    /// Main.rs sends PlayerEvents here; we fan out internally
    player_event_tx: mpsc::UnboundedSender<PlayerEvent>,
    player_event_rx: Option<mpsc::UnboundedReceiver<PlayerEvent>>,
    /// Screen name changes from navigation
    screen_watch_tx: watch::Sender<String>,
    screen_watch_rx: Option<watch::Receiver<String>>,
    task_handles: Vec<JoinHandle<()>>,
}

impl DaemonManager {
    pub fn new(max_restarts_per_hour: usize) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (player_event_tx, player_event_rx) = mpsc::unbounded_channel();
        let (screen_watch_tx, screen_watch_rx) = watch::channel("login".to_string());

        Self {
            shared: Arc::new(DaemonShared::new(max_restarts_per_hour)),
            event_tx,
            event_rx: Some(event_rx),
            player_event_tx,
            player_event_rx: Some(player_event_rx),
            screen_watch_tx,
            screen_watch_rx: Some(screen_watch_rx),
            task_handles: Vec::new(),
        }
    }

    /// Take the daemon event receiver (called once by main.rs).
    pub fn take_event_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<DaemonEvent>> {
        self.event_rx.take()
    }

    /// Get a clone of the player event sender for main.rs to forward events.
    pub fn player_event_sender(&self) -> mpsc::UnboundedSender<PlayerEvent> {
        self.player_event_tx.clone()
    }

    /// Get a clone of the screen watch sender for main.rs navigation callbacks.
    pub fn screen_watch_sender(&self) -> watch::Sender<String> {
        self.screen_watch_tx.clone()
    }

    /// Get an Arc to the shared state (for bandwidth-aware URL construction).
    pub fn shared(&self) -> Arc<DaemonShared> {
        self.shared.clone()
    }

    /// Spawn all background tasks.
    pub fn start(
        &mut self,
        client: Arc<RwLock<JellyfinClient>>,
        config: Arc<RwLock<AppConfig>>,
        _state: Arc<StateManager>,
    ) {
        info!("Starting daemon manager background tasks");

        // Take receivers (consumed once)
        let player_event_rx = self.player_event_rx.take()
            .expect("player_event_rx already taken");
        let screen_watch_rx = self.screen_watch_rx.take()
            .expect("screen_watch_rx already taken");

        // Fan-out: distribute PlayerEvents to QoS, streaming health
        let (qos_tx, qos_rx) = mpsc::unbounded_channel::<PlayerEvent>();
        let (streaming_tx, streaming_rx) = mpsc::unbounded_channel::<PlayerEvent>();
        let (system_player_tx, system_player_rx) = mpsc::unbounded_channel::<PlayerEvent>();

        // Fan-out task
        self.task_handles.push(tokio::spawn(async move {
            let mut rx = player_event_rx;
            while let Some(event) = rx.recv().await {
                let _ = qos_tx.send(event.clone());
                let _ = streaming_tx.send(event.clone());
                let _ = system_player_tx.send(event);
            }
        }));

        // Read config once for daemon settings
        let shared = self.shared.clone();
        let event_tx = self.event_tx.clone();

        // We need to read config synchronously for task setup.
        // Spawn an async init task that reads config then starts everything.
        let client_clone = client.clone();
        let config_clone = config.clone();
        let mut handles: Vec<JoinHandle<()>> = Vec::new();

        // We'll collect handles from a spawned init task
        let shared2 = shared.clone();
        let event_tx2 = event_tx.clone();

        let init_handle = tokio::spawn(async move {
            let cfg = config_clone.read().await;
            let daemon_cfg = &cfg.daemon;

            // 1. System tasks (IMU block, foreground-app, screen-alive, flex-launcher)
            let system = SystemTasks::new(
                shared2.clone(),
                screen_watch_rx,
                system_player_rx,
                daemon_cfg.flex_heal_enabled,
            );
            let sys_handles = system.spawn();
            // sys_handles are self-managing

            // 2. Bandwidth monitor
            let bw_monitor = BandwidthMonitor::new(
                shared2.clone(),
                event_tx2.clone(),
                client_clone.clone(),
                daemon_cfg.bandwidth_test_item_id.clone(),
                daemon_cfg.bandwidth_interval_sec,
            );
            tokio::spawn(async move { bw_monitor.run().await });

            // 3. QoS controller
            if daemon_cfg.qos_enabled {
                let qos = QosController::new(
                    shared2.clone(),
                    event_tx2.clone(),
                    qos_rx,
                    daemon_cfg.qos_grace_period_sec,
                );
                tokio::spawn(async move { qos.run().await });
            }

            // 4. Audio healer
            if daemon_cfg.audio_heal_enabled {
                let audio = AudioHealer::new(
                    shared2.clone(),
                    daemon_cfg.audio_heal_interval_sec,
                );
                tokio::spawn(async move { audio.run().await });
            }

            // 5. Streaming health
            let streaming = StreamingHealth::new(
                shared2.clone(),
                event_tx2.clone(),
                streaming_rx,
            );
            tokio::spawn(async move { streaming.run().await });

            // 6. Buffer manager
            if daemon_cfg.buffer_enabled {
                let buffer = BufferManager::new(
                    shared2.clone(),
                    event_tx2.clone(),
                    client_clone.clone(),
                    daemon_cfg.buffer_interval_sec,
                    daemon_cfg.buffer_min_free_ram_mb,
                );
                tokio::spawn(async move { buffer.run().await });
            }

            info!("All daemon background tasks spawned");
        });

        self.task_handles.push(init_handle);

        info!("Daemon manager started");
    }

    /// Graceful shutdown.
    pub async fn shutdown(&mut self) {
        info!("Shutting down daemon manager");
        for handle in self.task_handles.drain(..) {
            handle.abort();
        }
    }
}
