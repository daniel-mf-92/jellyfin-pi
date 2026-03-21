use evdev::{Device, InputEventKind, Key, AbsoluteAxisType};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::sleep;
use log::{info, warn, debug};
use anyhow::{Result, Context};

// ---------------------------------------------------------------------------
// Application-level input actions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InputAction {
    // Navigation
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
    Menu,
    Search,
    ContextMenu,

    // Media controls
    PlayPause,
    SeekForward,
    SeekBack,
    NextTrack,
    PrevTrack,
    VolumeUp,
    VolumeDown,
    Mute,

    // System
    Home,
    Screenshot,
}

// ---------------------------------------------------------------------------
// Axis tracking helpers
// ---------------------------------------------------------------------------

const AXIS_DEAD_ZONE: i32 = 12_000;
const KEY_REPEAT_INITIAL_DELAY: Duration = Duration::from_millis(400);
const KEY_REPEAT_INTERVAL: Duration = Duration::from_millis(150);
const AXIS_REPEAT_INTERVAL: Duration = Duration::from_millis(200);
const RECONNECT_SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Which logical direction an axis is pushed beyond the dead zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AxisDirection {
    Positive,
    Negative,
    Neutral,
}

impl AxisDirection {
    fn from_value(value: i32) -> Self {
        if value > AXIS_DEAD_ZONE {
            AxisDirection::Positive
        } else if value < -AXIS_DEAD_ZONE {
            AxisDirection::Negative
        } else {
            AxisDirection::Neutral
        }
    }
}

/// Per-axis state used to detect threshold crossings and drive repeat timers.
struct AxisState {
    direction: AxisDirection,
    last_emit: Option<Instant>,
}

impl Default for AxisState {
    fn default() -> Self {
        Self {
            direction: AxisDirection::Neutral,
            last_emit: None,
        }
    }
}

/// Per-direction key-repeat state for held d-pad / stick directions.
struct RepeatState {
    held_since: Option<Instant>,
    last_repeat: Option<Instant>,
}

impl Default for RepeatState {
    fn default() -> Self {
        Self {
            held_since: None,
            last_repeat: None,
        }
    }
}

impl RepeatState {
    /// Returns true if enough time has passed for another repeat emission.
    fn should_repeat(&self, now: Instant) -> bool {
        if let Some(held) = self.held_since {
            let elapsed = now.duration_since(held);
            if elapsed < KEY_REPEAT_INITIAL_DELAY {
                return false;
            }
            match self.last_repeat {
                Some(lr) => now.duration_since(lr) >= KEY_REPEAT_INTERVAL,
                None => true, // first repeat after initial delay
            }
        } else {
            false
        }
    }

    fn press(&mut self, now: Instant) {
        self.held_since = Some(now);
        self.last_repeat = None;
    }

    fn release(&mut self) {
        self.held_since = None;
        self.last_repeat = None;
    }

    fn mark_repeated(&mut self, now: Instant) {
        self.last_repeat = Some(now);
    }
}

// ---------------------------------------------------------------------------
// ControllerManager
// ---------------------------------------------------------------------------

pub struct ControllerManager {
    action_tx: mpsc::UnboundedSender<InputAction>,
    action_rx: Option<mpsc::UnboundedReceiver<InputAction>>,
}

impl ControllerManager {
    /// Create a new controller manager with an internal action channel.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            action_tx: tx,
            action_rx: Some(rx),
        }
    }

    /// Take the receiver end of the action channel.  
    /// This should be called exactly once by the main application loop before
    /// spawning the `run()` task.
    pub fn take_receiver(&mut self) -> mpsc::UnboundedReceiver<InputAction> {
        self.action_rx
            .take()
            .expect("take_receiver() called more than once")
    }

    /// Main loop: find the controller, read events, translate, and send
    /// actions.  Reconnects automatically when the device is lost.
    ///
    /// Intended to be spawned via `tokio::spawn`.
    pub async fn run(&self) -> Result<()> {
        loop {
            // --- Scan for the controller ---
            let device = loop {
                match Self::find_controller() {
                    Some(dev) => {
                        let name = dev
                            .name()
                            .unwrap_or("unknown")
                            .to_string();
                        info!(
                            "Switch Pro Controller found: {} ({})",
                            name,
                            dev.physical_path().unwrap_or("unknown path")
                        );
                        break dev;
                    }
                    None => {
                        debug!("Pro Controller not found, retrying in 2 s...");
                        sleep(RECONNECT_SCAN_INTERVAL).await;
                    }
                }
            };

            // --- Read events until disconnection ---
            if let Err(e) = self.read_loop(device).await {
                warn!("Controller disconnected or error: {:#}", e);
                info!("Will attempt to reconnect...");
                sleep(RECONNECT_SCAN_INTERVAL).await;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Scan `/dev/input/event*` for a device whose name contains
    /// "Pro Controller" or "Nintendo".
    fn find_controller() -> Option<Device> {
        let devices = evdev::enumerate();
        for (_path, device) in devices {
            if let Some(name) = device.name() {
                let lower = name.to_lowercase();
                if lower.contains("pro controller") || lower.contains("nintendo") {
                    return Some(device);
                }
            }
        }
        None
    }

    /// Blocking read loop wrapped in `tokio::task::spawn_blocking` so it
    /// does not starve the async runtime.
    async fn read_loop(&self, mut device: Device) -> Result<()> {
        let tx = self.action_tx.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut axis_states: HashMap<AbsoluteAxisType, AxisState> = HashMap::new();
            let mut repeat_states: HashMap<InputAction, RepeatState> = HashMap::new();

            // We use a small poll timeout so we can drive key-repeat even
            // when no new events arrive.
            loop {
                // Fetch pending events (non-blocking via poll).
                match device.fetch_events() {
                    Ok(events) => {
                        for ev in events {
                            match ev.kind() {
                                InputEventKind::Key(key) => {
                                    let value = ev.value();
                                    // Buttons
                                    if let Some(action) = Self::map_button(key, value) {
                                        let now = Instant::now();
                                        if value == 1 {
                                            // pressed
                                            let _ = tx.send(action.clone());
                                            repeat_states
                                                .entry(action)
                                                .or_default()
                                                .press(now);
                                        } else if value == 0 {
                                            // released
                                            repeat_states
                                                .entry(action)
                                                .or_default()
                                                .release();
                                        }
                                    }
                                    // D-pad (also reported as Key events)
                                    if let Some(action) = Self::map_dpad(key, value) {
                                        let now = Instant::now();
                                        if value == 1 {
                                            let _ = tx.send(action.clone());
                                            repeat_states
                                                .entry(action)
                                                .or_default()
                                                .press(now);
                                        } else if value == 0 {
                                            repeat_states
                                                .entry(action)
                                                .or_default()
                                                .release();
                                        }
                                    }
                                }
                                InputEventKind::AbsAxis(axis) => {
                                    let value = ev.value();
                                    Self::handle_axis(
                                        axis,
                                        value,
                                        &mut axis_states,
                                        &tx,
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        // ENODEV means the device was unplugged.
                        if e.raw_os_error() == Some(libc::ENODEV) {
                            return Err(anyhow::anyhow!("Device disconnected (ENODEV)"));
                        }
                        // Transient read errors (e.g. EAGAIN) – just continue.
                        debug!("evdev read error (non-fatal): {}", e);
                    }
                }

                // --- Drive key-repeat for held directions ---
                let now = Instant::now();
                let repeatable = [
                    InputAction::Up,
                    InputAction::Down,
                    InputAction::Left,
                    InputAction::Right,
                    InputAction::VolumeUp,
                    InputAction::VolumeDown,
                    InputAction::SeekForward,
                    InputAction::SeekBack,
                ];
                for action in &repeatable {
                    if let Some(rs) = repeat_states.get_mut(action) {
                        if rs.should_repeat(now) {
                            let _ = tx.send(action.clone());
                            rs.mark_repeated(now);
                        }
                    }
                }

                // Small sleep so we don't spin at 100 % CPU between event
                // batches.  This also sets the effective repeat-check cadence.
                std::thread::sleep(Duration::from_millis(5));
            }
        })
        .await
        .context("Controller read task panicked")?
    }

    /// Map a button key code to an `InputAction`.  
    /// Returns `None` for unmapped buttons or release events.
    fn map_button(key: Key, value: i32) -> Option<InputAction> {
        // Only act on press (value == 1). Ignore release (0) and repeat (2).
        if value != 1 {
            return None;
        }

        match key {
            // Face buttons (hid-nintendo / generic HID mapping)
            // BTN_SOUTH (304) – A on Switch Pro
            Key::BTN_SOUTH => Some(InputAction::Select),
            // BTN_EAST (305) – B
            Key::BTN_EAST => Some(InputAction::Back),
            // BTN_NORTH (307) – X
            Key::BTN_NORTH => Some(InputAction::ContextMenu),
            // BTN_WEST (308) – Y
            Key::BTN_WEST => Some(InputAction::Search),

            // Shoulder buttons
            Key::BTN_TL => Some(InputAction::SeekBack),
            Key::BTN_TR => Some(InputAction::SeekForward),

            // Triggers
            Key::BTN_TL2 => Some(InputAction::PrevTrack),
            Key::BTN_TR2 => Some(InputAction::PlayPause),

            // Meta buttons
            Key::BTN_SELECT => Some(InputAction::PrevTrack), // - button
            Key::BTN_START => Some(InputAction::Menu),       // + button
            Key::BTN_MODE => Some(InputAction::Home),        // Home button

            // Stick presses
            Key::BTN_THUMBR => Some(InputAction::Mute),
            // BTN_THUMBL is intentionally unmapped

            _ => None,
        }
    }

    /// Map d-pad key events to navigation actions.
    fn map_dpad(key: Key, value: i32) -> Option<InputAction> {
        if value != 1 {
            return None;
        }
        match key {
            Key::BTN_DPAD_UP => Some(InputAction::Up),
            Key::BTN_DPAD_DOWN => Some(InputAction::Down),
            Key::BTN_DPAD_LEFT => Some(InputAction::Left),
            Key::BTN_DPAD_RIGHT => Some(InputAction::Right),
            _ => None,
        }
    }

    /// Process an absolute axis event (analog sticks).
    fn handle_axis(
        axis: AbsoluteAxisType,
        value: i32,
        states: &mut HashMap<AbsoluteAxisType, AxisState>,
        tx: &mpsc::UnboundedSender<InputAction>,
    ) {
        let new_dir = AxisDirection::from_value(value);
        let state = states.entry(axis).or_default();
        let old_dir = state.direction;

        if new_dir == old_dir {
            // Still in the same zone – axis repeat is handled by the repeat
            // timer in the main loop for button-style repeats.  For axes we
            // use a simple time-gate here.
            if new_dir != AxisDirection::Neutral {
                let now = Instant::now();
                let should = match state.last_emit {
                    Some(t) => now.duration_since(t) >= AXIS_REPEAT_INTERVAL,
                    None => true,
                };
                if should {
                    if let Some(action) = Self::axis_to_action(axis, new_dir) {
                        let _ = tx.send(action);
                        state.last_emit = Some(now);
                    }
                }
            }
            return;
        }

        // Direction changed – emit immediately if entering a non-neutral zone.
        state.direction = new_dir;
        if new_dir != AxisDirection::Neutral {
            if let Some(action) = Self::axis_to_action(axis, new_dir) {
                let _ = tx.send(action);
                state.last_emit = Some(Instant::now());
            }
        } else {
            state.last_emit = None;
        }
    }

    /// Convert an axis + direction into a logical action.
    fn axis_to_action(axis: AbsoluteAxisType, dir: AxisDirection) -> Option<InputAction> {
        match axis {
            // Left stick – navigation
            AbsoluteAxisType::ABS_X => match dir {
                AxisDirection::Positive => Some(InputAction::Right),
                AxisDirection::Negative => Some(InputAction::Left),
                AxisDirection::Neutral => None,
            },
            AbsoluteAxisType::ABS_Y => match dir {
                // Y axis is inverted: negative = up
                AxisDirection::Negative => Some(InputAction::Up),
                AxisDirection::Positive => Some(InputAction::Down),
                AxisDirection::Neutral => None,
            },

            // Right stick – volume
            AbsoluteAxisType::ABS_RX => None, // horizontal right stick unused
            AbsoluteAxisType::ABS_RY => match dir {
                AxisDirection::Negative => Some(InputAction::VolumeUp),
                AxisDirection::Positive => Some(InputAction::VolumeDown),
                AxisDirection::Neutral => None,
            },

            _ => None,
        }
    }
}
