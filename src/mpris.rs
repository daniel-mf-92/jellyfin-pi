use log::debug;

#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub art_url: String,
    pub is_playing: bool,
    pub position_ms: i64,
    pub duration_ms: i64,
}

pub struct MprisPublisher {
    path: String,
}

impl MprisPublisher {
    pub fn new() -> Self {
        Self { path: "/tmp/jellyfin-pi-now-playing.json".into() }
    }

    pub fn update(&self, state: &NowPlaying) {
        let json = format!(
            r#"{{"title":"{}","artist":"{}","art_url":"{}","status":"{}","position_ms":{},"duration_ms":{}}}"#,
            state.title.replace('"', "\\\""),
            state.artist.replace('"', "\\\""),
            state.art_url.replace('"', "\\\""),
            if state.is_playing { "Playing" } else { "Paused" },
            state.position_ms,
            state.duration_ms,
        );
        if let Err(e) = std::fs::write(&self.path, &json) {
            debug!("MPRIS: write failed: {}", e);
        }
    }

    pub fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
