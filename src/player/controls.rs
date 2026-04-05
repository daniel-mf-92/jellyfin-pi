// ---------------------------------------------------------------------------
// Playback Controls — speed, subtitle delay, audio delay
// ---------------------------------------------------------------------------
//
// Integration notes:
//
// 1. Add to player/mod.rs:
//      pub mod controls;
//      pub use controls::PlaybackControls;
//
// 2. In main.rs (or wherever VlcPlayer is driven), create a PlaybackControls
//    instance alongside VlcPlayer. When the user changes speed or subtitle
//    delay, call the appropriate method on PlaybackControls, then send the
//    returned command string through VlcPlayer::send_command(). Example:
//
//      let mut controls = PlaybackControls::new();
//      let cmd = controls.speed_up();
//      vlc_player.send_command(&cmd).await?;
//      // Update UI with controls.speed_label()
//
// 3. On playback stop or new media, call controls.reset_all() and send each
//    returned command to VLC to restore defaults.
//
// VLC IPC command reference (sent over Unix socket as "{cmd}\n"):
//   rate <float>    — set playback speed (e.g. "rate 1.5")
//   subdelay <int>  — set subtitle delay in ms (positive = later)
//   audiodelay <int> — set audio delay in ms
// ---------------------------------------------------------------------------

/// Available playback speeds, ordered slowest to fastest.
pub const SPEEDS: &[f64] = &[0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

/// Index of 1.0x in SPEEDS.
const DEFAULT_SPEED_INDEX: usize = 3;

/// Manages playback speed and subtitle/audio delay state.
///
/// Each mutating method returns the VLC IPC command string that the caller
/// must send over the Unix socket via `VlcPlayer::send_command`.
pub struct PlaybackControls {
    /// Index into [`SPEEDS`] for the current playback speed.
    current_speed_index: usize,
    /// Subtitle delay in milliseconds (positive = subs appear later).
    subtitle_delay_ms: i64,
}

impl PlaybackControls {
    /// Create controls with default values: 1.0x speed, 0ms subtitle delay.
    pub fn new() -> Self {
        Self {
            current_speed_index: DEFAULT_SPEED_INDEX,
            subtitle_delay_ms: 0,
        }
    }

    // -------------------------------------------------------------------
    // Speed control
    // -------------------------------------------------------------------

    /// Get the current playback speed value (e.g. 1.0, 1.5).
    pub fn current_speed(&self) -> f64 {
        SPEEDS[self.current_speed_index]
    }

    /// Get the current speed as a display string (e.g. "1.5x", "1x").
    pub fn speed_label(&self) -> String {
        let speed = self.current_speed();
        // Avoid unnecessary decimal for whole numbers (1x not 1.0x)
        if speed.fract() == 0.0 {
            format!("{}x", speed as i32)
        } else {
            // Trim trailing zeros: 1.50 -> 1.5, 0.25 -> 0.25
            let s = format!("{:.2}", speed);
            let s = s.trim_end_matches('0');
            let s = s.trim_end_matches('.');
            format!("{}x", s)
        }
    }

    /// Cycle to the next faster speed. Wraps around to slowest.
    /// Returns the VLC `rate` command string.
    pub fn speed_up(&mut self) -> String {
        if self.current_speed_index + 1 < SPEEDS.len() {
            self.current_speed_index += 1;
        } else {
            self.current_speed_index = 0;
        }
        self.rate_command()
    }

    /// Cycle to the next slower speed. Wraps around to fastest.
    /// Returns the VLC `rate` command string.
    pub fn speed_down(&mut self) -> String {
        if self.current_speed_index > 0 {
            self.current_speed_index -= 1;
        } else {
            self.current_speed_index = SPEEDS.len() - 1;
        }
        self.rate_command()
    }

    /// Set a specific playback speed. If the exact value is in [`SPEEDS`],
    /// the index is updated to match; otherwise the closest speed is used.
    /// Returns the VLC `rate` command string.
    pub fn set_speed(&mut self, speed: f64) -> String {
        // Find the closest matching speed in our preset list
        let mut best_idx = DEFAULT_SPEED_INDEX;
        let mut best_diff = f64::MAX;
        for (i, &s) in SPEEDS.iter().enumerate() {
            let diff = (s - speed).abs();
            if diff < best_diff {
                best_diff = diff;
                best_idx = i;
            }
        }
        self.current_speed_index = best_idx;
        self.rate_command()
    }

    /// Reset speed to 1.0x. Returns the VLC `rate` command string.
    pub fn reset_speed(&mut self) -> String {
        self.current_speed_index = DEFAULT_SPEED_INDEX;
        self.rate_command()
    }

    // -------------------------------------------------------------------
    // Subtitle delay
    // -------------------------------------------------------------------

    /// Adjust subtitle delay by `delta_ms` milliseconds.
    /// Positive values make subtitles appear later; negative, earlier.
    /// Returns the VLC `subdelay` command string.
    pub fn adjust_subtitle_delay(&mut self, delta_ms: i64) -> String {
        self.subtitle_delay_ms += delta_ms;
        self.subdelay_command()
    }

    /// Get the subtitle delay as a human-readable string.
    /// Examples: "+250ms", "-100ms", "0ms".
    pub fn subtitle_delay_label(&self) -> String {
        if self.subtitle_delay_ms > 0 {
            format!("+{}ms", self.subtitle_delay_ms)
        } else if self.subtitle_delay_ms < 0 {
            format!("{}ms", self.subtitle_delay_ms) // negative sign included
        } else {
            "0ms".to_string()
        }
    }

    /// Reset subtitle delay to 0ms. Returns the VLC `subdelay` command string.
    pub fn reset_subtitle_delay(&mut self) -> String {
        self.subtitle_delay_ms = 0;
        self.subdelay_command()
    }

    // -------------------------------------------------------------------
    // Reset all
    // -------------------------------------------------------------------

    /// Reset speed and subtitle delay to defaults. Returns all VLC commands
    /// needed to restore the player state.
    pub fn reset_all(&mut self) -> Vec<String> {
        let mut cmds = Vec::new();

        // Only emit commands for values that differ from defaults
        if self.current_speed_index != DEFAULT_SPEED_INDEX {
            self.current_speed_index = DEFAULT_SPEED_INDEX;
            cmds.push(self.rate_command());
        }
        if self.subtitle_delay_ms != 0 {
            self.subtitle_delay_ms = 0;
            cmds.push(self.subdelay_command());
        }

        // If nothing was changed, still return commands to guarantee a
        // known state (useful after seeking or track switching)
        if cmds.is_empty() {
            cmds.push(self.rate_command());
            cmds.push(self.subdelay_command());
        }

        cmds
    }

    // -------------------------------------------------------------------
    // VLC command builders (private)
    // -------------------------------------------------------------------

    /// Build the VLC `rate` command for the current speed.
    fn rate_command(&self) -> String {
        format!("rate {}", self.current_speed())
    }

    /// Build the VLC `subdelay` command for the current subtitle delay.
    fn subdelay_command(&self) -> String {
        format!("subdelay {}", self.subtitle_delay_ms)
    }
}

impl Default for PlaybackControls {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_defaults() {
        let c = PlaybackControls::new();
        assert_eq!(c.current_speed(), 1.0);
        assert_eq!(c.speed_label(), "1x");
        assert_eq!(c.subtitle_delay_ms, 0);
        assert_eq!(c.subtitle_delay_label(), "0ms");
    }

    #[test]
    fn speed_up_cycles() {
        let mut c = PlaybackControls::new(); // 1.0x (index 3)
        let cmd = c.speed_up();
        assert_eq!(c.current_speed(), 1.25);
        assert_eq!(cmd, "rate 1.25");
        assert_eq!(c.speed_label(), "1.25x");
    }

    #[test]
    fn speed_up_wraps() {
        let mut c = PlaybackControls::new();
        c.current_speed_index = SPEEDS.len() - 1; // 2.0x
        let cmd = c.speed_up();
        assert_eq!(c.current_speed(), 0.25);
        assert_eq!(cmd, "rate 0.25");
    }

    #[test]
    fn speed_down_cycles() {
        let mut c = PlaybackControls::new(); // 1.0x
        let cmd = c.speed_down();
        assert_eq!(c.current_speed(), 0.75);
        assert_eq!(cmd, "rate 0.75");
    }

    #[test]
    fn speed_down_wraps() {
        let mut c = PlaybackControls::new();
        c.current_speed_index = 0; // 0.25x
        let cmd = c.speed_down();
        assert_eq!(c.current_speed(), 2.0);
        assert_eq!(cmd, "rate 2");
    }

    #[test]
    fn set_speed_exact() {
        let mut c = PlaybackControls::new();
        let cmd = c.set_speed(1.5);
        assert_eq!(c.current_speed(), 1.5);
        assert_eq!(cmd, "rate 1.5");
    }

    #[test]
    fn set_speed_closest() {
        let mut c = PlaybackControls::new();
        let cmd = c.set_speed(1.3); // closest to 1.25
        assert_eq!(c.current_speed(), 1.25);
        assert_eq!(cmd, "rate 1.25");
    }

    #[test]
    fn reset_speed() {
        let mut c = PlaybackControls::new();
        c.speed_up();
        c.speed_up();
        let cmd = c.reset_speed();
        assert_eq!(c.current_speed(), 1.0);
        assert_eq!(cmd, "rate 1");
    }

    #[test]
    fn subtitle_delay_adjust() {
        let mut c = PlaybackControls::new();
        let cmd = c.adjust_subtitle_delay(250);
        assert_eq!(c.subtitle_delay_ms, 250);
        assert_eq!(cmd, "subdelay 250");
        assert_eq!(c.subtitle_delay_label(), "+250ms");

        let cmd = c.adjust_subtitle_delay(-350);
        assert_eq!(c.subtitle_delay_ms, -100);
        assert_eq!(cmd, "subdelay -100");
        assert_eq!(c.subtitle_delay_label(), "-100ms");
    }

    #[test]
    fn subtitle_delay_reset() {
        let mut c = PlaybackControls::new();
        c.adjust_subtitle_delay(500);
        let cmd = c.reset_subtitle_delay();
        assert_eq!(c.subtitle_delay_ms, 0);
        assert_eq!(cmd, "subdelay 0");
        assert_eq!(c.subtitle_delay_label(), "0ms");
    }

    #[test]
    fn reset_all_returns_commands() {
        let mut c = PlaybackControls::new();
        c.speed_up();
        c.adjust_subtitle_delay(200);
        let cmds = c.reset_all();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0], "rate 1");
        assert_eq!(cmds[1], "subdelay 0");
        assert_eq!(c.current_speed(), 1.0);
        assert_eq!(c.subtitle_delay_ms, 0);
    }

    #[test]
    fn reset_all_at_defaults_still_returns_commands() {
        let mut c = PlaybackControls::new();
        let cmds = c.reset_all();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0], "rate 1");
        assert_eq!(cmds[1], "subdelay 0");
    }

    #[test]
    fn speed_label_formatting() {
        let mut c = PlaybackControls::new();
        c.current_speed_index = 0; // 0.25
        assert_eq!(c.speed_label(), "0.25x");
        c.current_speed_index = 1; // 0.5
        assert_eq!(c.speed_label(), "0.5x");
        c.current_speed_index = 3; // 1.0
        assert_eq!(c.speed_label(), "1x");
        c.current_speed_index = 7; // 2.0
        assert_eq!(c.speed_label(), "2x");
        c.current_speed_index = 6; // 1.75
        assert_eq!(c.speed_label(), "1.75x");
    }
}
