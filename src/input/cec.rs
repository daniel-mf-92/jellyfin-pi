use log::{info, warn, debug};
use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use tokio::sync::mpsc;
use super::controller::InputAction;

pub fn spawn_cec_listener(tx: mpsc::UnboundedSender<InputAction>) -> Option<std::thread::JoinHandle<()>> {
    if Command::new("cec-client").arg("--help").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
        warn!("cec-client not found, HDMI-CEC disabled");
        return None;
    }

    let handle = std::thread::spawn(move || {
        info!("CEC: starting listener");
        loop {
            let mut child = match Command::new("cec-client")
                .args(["-d", "8", "-t", "r", "-o", "JellyfinTV"])
                .stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    warn!("CEC: failed to start: {}. Retry in 10s", e);
                    std::thread::sleep(std::time::Duration::from_secs(10));
                    continue;
                }
            };

            let stdout = match child.stdout.take() {
                Some(s) => s,
                None => { let _ = child.kill(); std::thread::sleep(std::time::Duration::from_secs(5)); continue; }
            };

            for line in BufReader::new(stdout).lines().flatten() {
                if let Some(action) = parse_cec_key(&line) {
                    debug!("CEC: {:?}", action);
                    if tx.send(action).is_err() { let _ = child.kill(); return; }
                }
            }

            warn!("CEC: cec-client exited, restarting in 5s");
            let _ = child.wait();
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });

    Some(handle)
}

fn parse_cec_key(line: &str) -> Option<InputAction> {
    if !line.contains("key pressed:") { return None; }
    let key = line.split("key pressed:").nth(1)?.trim().split('(').next()?.trim().to_lowercase();
    match key.as_str() {
        "up" => Some(InputAction::Up),
        "down" => Some(InputAction::Down),
        "left" => Some(InputAction::Left),
        "right" => Some(InputAction::Right),
        "select" | "enter" => Some(InputAction::Select),
        "exit" | "back" => Some(InputAction::Back),
        "root menu" | "setup menu" | "contents menu" => Some(InputAction::Menu),
        "play" | "pause" | "play function" | "pause function" => Some(InputAction::PlayPause),
        "stop" => Some(InputAction::Back),
        "rewind" | "fast rewind" | "backward" => Some(InputAction::SeekBack),
        "fast forward" | "forward" => Some(InputAction::SeekForward),
        "channel up" | "page up" => Some(InputAction::Up),
        "channel down" | "page down" => Some(InputAction::Down),
        "volume up" => Some(InputAction::VolumeUp),
        "volume down" => Some(InputAction::VolumeDown),
        "mute" | "mute function" => Some(InputAction::Mute),
        "display information" => Some(InputAction::ContextMenu),
        "f1 (blue)" => Some(InputAction::Search),
        "f2 (red)" => Some(InputAction::ContextMenu),
        "f3 (green)" => Some(InputAction::Home),
        "f4 (yellow)" => Some(InputAction::Menu),
        _ => { debug!("CEC: unhandled key: {}", key); None }
    }
}
