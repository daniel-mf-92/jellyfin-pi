use log::info;
use std::process::Command;

pub enum PowerAction {
    Shutdown,
    Reboot,
    Suspend,
}

pub fn execute(action: PowerAction) -> Result<(), String> {
    let (args, label) = match action {
        PowerAction::Shutdown => (vec!["poweroff"], "shutdown"),
        PowerAction::Reboot => (vec!["reboot"], "reboot"),
        PowerAction::Suspend => (vec!["suspend"], "suspend"),
    };
    info!("Power action: {}", label);
    Command::new("systemctl")
        .args(&args)
        .spawn()
        .map_err(|e| format!("Failed to {}: {}", label, e))?;
    Ok(())
}
