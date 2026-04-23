use log::{debug, error, info, warn};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use super::vlc::{PlayerError, PlayerEvent, PlayerResult, TrackInfo};

const MPV_SOCKET_PATH: &str = "/tmp/pi-media-player-mpv.sock";
const MPV_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

pub struct MpvPlayer {
    child: Arc<Mutex<Option<Child>>>,
    event_tx: mpsc::UnboundedSender<PlayerEvent>,
    event_rx: Option<mpsc::UnboundedReceiver<PlayerEvent>>,
    socket_path: String,
    stored_volume: Arc<Mutex<f64>>,
    is_muted: Arc<AtomicBool>,
    request_id: AtomicI64,
}

impl MpvPlayer {
    pub fn new() -> PlayerResult<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        info!("MpvPlayer created (socket={})", MPV_SOCKET_PATH);
        Ok(Self {
            child: Arc::new(Mutex::new(None)),
            event_tx, event_rx: Some(event_rx),
            socket_path: MPV_SOCKET_PATH.to_string(),
            stored_volume: Arc::new(Mutex::new(100.0)),
            is_muted: Arc::new(AtomicBool::new(false)),
            request_id: AtomicI64::new(1),
        })
    }

    pub fn take_event_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<PlayerEvent>> {
        self.event_rx.take()
    }

    async fn send_json(&self, command: &[Value]) -> PlayerResult<Value> {
        let stream = match timeout(MPV_COMMAND_TIMEOUT, UnixStream::connect(&self.socket_path)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(PlayerError::Socket(format!("mpv socket connect: {}", e))),
            Err(_) => return Err(PlayerError::Socket("mpv socket timeout".into())),
        };
        let (reader, mut writer) = stream.into_split();
        let req_id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let msg = json!({"command": command, "request_id": req_id});
        let cmd_bytes = format!("{}\n", msg);
        if let Err(e) = timeout(MPV_COMMAND_TIMEOUT, writer.write_all(cmd_bytes.as_bytes())).await {
            return Err(PlayerError::Socket(format!("write timeout: {}", e)));
        }
        let buf_reader = BufReader::new(reader);
        let mut lines = buf_reader.lines();
        loop {
            match timeout(Duration::from_millis(500), lines.next_line()).await {
                Ok(Ok(Some(line))) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() { continue; }
                    if let Ok(resp) = serde_json::from_str::<Value>(trimmed) {
                        if resp.get("request_id").and_then(|v| v.as_i64()) == Some(req_id) {
                            debug!("mpv: {:?} -> {}", command, trimmed);
                            return Ok(resp);
                        }
                    }
                }
                Ok(Ok(None)) | Ok(Err(_)) | Err(_) => break,
            }
        }
        Ok(json!({"error": "timeout"}))
    }

    pub async fn send_command(&self, cmd: &str) -> PlayerResult<String> {
        let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
        match parts[0] {
            "rate" => {
                if let Some(val) = parts.get(1).and_then(|s| s.parse::<f64>().ok()) {
                    self.send_json(&[json!("set_property"), json!("speed"), json!(val)]).await?;
                }
            }
            "subdelay" => {
                if let Some(val) = parts.get(1).and_then(|s| s.parse::<f64>().ok()) {
                    self.send_json(&[json!("set_property"), json!("sub-delay"), json!(val / 1000.0)]).await?;
                }
            }
            _ => debug!("Unknown mpv command: {}", cmd),
        }
        Ok(String::new())
    }

    async fn kill_existing(&self) {
        let mut child_guard = self.child.lock().await;
        if let Some(ref mut child) = *child_guard {
            info!("Killing existing mpv process");
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        *child_guard = None;
        let _ = tokio::fs::remove_file(&self.socket_path).await;
    }

    pub async fn play_url(&self, url: &str, start_position_ms: Option<i64>) -> PlayerResult<()> {
        info!("mpv play_url: {}", url);

        // Kill ALL media players system-wide (single-instance enforcement)
        super::kill_all_media_players();

        self.kill_existing().await;
        let mut args = vec![
            "--fullscreen".into(), "--input-ipc-server".into(), self.socket_path.clone(),
            "--hwdec=auto".into(), "--vo=dmabuf-wayland".into(), "--gpu-context=wayland".into(),
            "--ao=alsa".into(), "--audio-device=alsa/default".into(),
            "--cache=yes".into(), "--demuxer-max-bytes=100MiB".into(),
            "--demuxer-max-back-bytes=50MiB".into(), "--demuxer-readahead-secs=5".into(),
            "--network-timeout=30".into(),
            "--no-terminal".into(), "--keep-open=no".into(),
            "--sub-font-size=48".into(), "--osd-level=0".into(),
        ];
        if let Some(ms) = start_position_ms {
            args.push(format!("--start={:.3}", ms as f64 / 1000.0));
        }
        args.push("--".into());
        args.push(url.to_string());
        info!("Launching: mpv {}", args.join(" "));
        let child = Command::new("mpv").args(&args)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .kill_on_drop(true).spawn()
            .map_err(|e| PlayerError::Vlc(format!("failed to launch mpv: {}", e)))?;
        { let mut g = self.child.lock().await; *g = Some(child); }
        for i in 0..25 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if tokio::fs::metadata(&self.socket_path).await.is_ok() {
                if UnixStream::connect(&self.socket_path).await.is_ok() {
                    info!("mpv IPC ready after {}ms", (i+1)*250);
                    self.event_tx.send(PlayerEvent::Playing).ok();
                    return Ok(());
                }
            }
        }
        Err(PlayerError::Socket("mpv IPC socket unavailable".into()))
    }

    pub async fn queue_url(&self, url: &str) -> PlayerResult<()> {
        self.send_json(&[json!("loadfile"), json!(url), json!("append")]).await?; Ok(())
    }
    pub async fn get_chapter_count(&self) -> PlayerResult<i32> {
        let r = self.send_json(&[json!("get_property"), json!("chapter-list/count")]).await?;
        Ok(r.get("data").and_then(|v| v.as_i64()).unwrap_or(0) as i32)
    }
    pub async fn set_chapter(&self, index: i32) -> PlayerResult<()> {
        self.send_json(&[json!("set_property"), json!("chapter"), json!(index)]).await?; Ok(())
    }
    pub async fn pause(&self) -> PlayerResult<()> {
        self.send_json(&[json!("set_property"), json!("pause"), json!(true)]).await?;
        self.event_tx.send(PlayerEvent::Paused).ok(); Ok(())
    }
    pub async fn resume(&self) -> PlayerResult<()> {
        self.send_json(&[json!("set_property"), json!("pause"), json!(false)]).await?;
        self.event_tx.send(PlayerEvent::Playing).ok(); Ok(())
    }
    pub async fn toggle_pause(&self) -> PlayerResult<()> {
        self.send_json(&[json!("cycle"), json!("pause")]).await?; Ok(())
    }
    pub async fn stop(&self) -> PlayerResult<()> {
        let _ = self.send_json(&[json!("quit")]).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        self.kill_existing().await;
        self.event_tx.send(PlayerEvent::Stopped).ok(); Ok(())
    }
    pub async fn seek_to(&self, position_ms: i64) -> PlayerResult<()> {
        self.send_json(&[json!("seek"), json!(position_ms as f64 / 1000.0), json!("absolute")]).await?; Ok(())
    }
    pub async fn seek_relative(&self, offset_seconds: f64) -> PlayerResult<()> {
        self.send_json(&[json!("seek"), json!(offset_seconds), json!("relative")]).await?; Ok(())
    }
    pub async fn set_volume(&self, volume: f64) -> PlayerResult<()> {
        let vol = volume.clamp(0.0, 100.0);
        self.send_json(&[json!("set_property"), json!("volume"), json!(vol)]).await?;
        { let mut s = self.stored_volume.lock().await; *s = vol; }
        self.is_muted.store(false, Ordering::SeqCst);
        self.event_tx.send(PlayerEvent::VolumeChanged(vol)).ok(); Ok(())
    }
    pub async fn get_volume(&self) -> PlayerResult<f64> {
        let r = self.send_json(&[json!("get_property"), json!("volume")]).await?;
        Ok(r.get("data").and_then(|v| v.as_f64()).unwrap_or(100.0))
    }
    pub async fn set_mute(&self, muted: bool) -> PlayerResult<()> {
        self.send_json(&[json!("set_property"), json!("mute"), json!(muted)]).await?;
        self.is_muted.store(muted, Ordering::SeqCst);
        self.event_tx.send(PlayerEvent::MuteChanged(muted)).ok(); Ok(())
    }
    pub async fn toggle_mute(&self) -> PlayerResult<()> {
        self.set_mute(!self.is_muted.load(Ordering::SeqCst)).await
    }
    pub async fn set_audio_track(&self, track_id: i32) -> PlayerResult<()> {
        self.send_json(&[json!("set_property"), json!("aid"), json!(track_id + 1)]).await?; Ok(())
    }
    pub async fn set_subtitle_track(&self, track_id: i32) -> PlayerResult<()> {
        if track_id < 0 {
            self.send_json(&[json!("set_property"), json!("sid"), json!("no")]).await?;
        } else {
            self.send_json(&[json!("set_property"), json!("sid"), json!(track_id + 1)]).await?;
        }
        Ok(())
    }
    pub async fn get_position_ms(&self) -> PlayerResult<i64> {
        let r = self.send_json(&[json!("get_property"), json!("playback-time")]).await?;
        Ok((r.get("data").and_then(|v| v.as_f64()).unwrap_or(0.0) * 1000.0) as i64)
    }
    pub async fn get_duration_ms(&self) -> PlayerResult<i64> {
        let r = self.send_json(&[json!("get_property"), json!("duration")]).await?;
        Ok((r.get("data").and_then(|v| v.as_f64()).unwrap_or(0.0) * 1000.0) as i64)
    }
    pub async fn get_tracks(&self) -> PlayerResult<(Vec<TrackInfo>, Vec<TrackInfo>)> {
        let r = self.send_json(&[json!("get_property"), json!("track-list")]).await?;
        let mut audio = Vec::new(); let mut subs = Vec::new();
        if let Some(tracks) = r.get("data").and_then(|v| v.as_array()) {
            for t in tracks {
                let tt = t.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let id = t.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let lang = t.get("lang").and_then(|v| v.as_str()).map(String::from);
                let codec = t.get("codec").and_then(|v| v.as_str()).map(String::from);
                let is_default = t.get("default").and_then(|v| v.as_bool()).unwrap_or(false);
                let display = if !title.is_empty() { title } else { lang.clone().unwrap_or(format!("Track {}", id)) };
                let info = TrackInfo { id: id - 1, title: display, language: lang, codec, is_default };
                match tt { "audio" => audio.push(info), "sub" => subs.push(info), _ => {} }
            }
        }
        Ok((audio, subs))
    }
    pub async fn is_playing(&self) -> bool {
        self.child.lock().await.is_some()
    }
    pub async fn run_event_loop(&self) {
        {
            let mut g = self.child.lock().await;
            if let Some(ref mut child) = *g {
                match child.try_wait() {
                    Ok(Some(status)) => { info!("mpv exited: {}", status); *g = None; self.event_tx.send(PlayerEvent::EndOfFile).ok(); return; }
                    Ok(None) => {}
                    Err(e) => { error!("mpv check error: {}", e); *g = None; self.event_tx.send(PlayerEvent::Error(e.to_string())).ok(); return; }
                }
            } else { return; }
        }
        if let Ok(pos) = self.get_position_ms().await {
            if let Ok(dur) = self.get_duration_ms().await {
                self.event_tx.send(PlayerEvent::PositionChanged { position_ms: pos, duration_ms: dur }).ok();
            }
        }
    }
}

impl Drop for MpvPlayer {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.socket_path); }
}
