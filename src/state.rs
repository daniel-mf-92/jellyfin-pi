use std::sync::Arc;
use tokio::sync::RwLock;
use log::{info, debug};
use crate::api::models::{UserDto, BaseItemDto};

#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    Login,
    Home,
    Detail { item_id: String },
    Library { library_id: String, title: String },
    Search,
    Settings,
    Player { item_id: String },
}

impl Screen {
    pub fn name(&self) -> &str {
        match self {
            Screen::Login => "login",
            Screen::Home => "home",
            Screen::Detail { .. } => "detail",
            Screen::Library { .. } => "library",
            Screen::Search => "search",
            Screen::Settings => "settings",
            Screen::Player { .. } => "player",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub current_screen: Screen,
    pub screen_stack: Vec<Screen>,
    pub current_user: Option<UserDto>,
    pub access_token: Option<String>,
    pub server_url: String,
    pub server_name: String,
    // Playback session
    pub play_session_id: Option<String>,
    pub playing_item_id: Option<String>,
    pub playing_media_source_id: Option<String>,
    // Playback tracking session ID (local SQLite)
    pub tracking_session_id: Option<i64>,
    // Screensaver
    pub idle_seconds: u32,
    pub screensaver_timeout: u32,
}

pub struct StateManager {
    state: Arc<RwLock<AppState>>,
}

impl StateManager {
    pub fn new(server_url: String) -> Self {
        let state = AppState {
            current_screen: Screen::Login,
            screen_stack: Vec::new(),
            current_user: None,
            access_token: None,
            server_url,
            server_name: String::new(),
            play_session_id: None,
            playing_item_id: None,
            playing_media_source_id: None,
            tracking_session_id: None,
            idle_seconds: 0,
            screensaver_timeout: 0, // disabled — TV appliance mode
        };
        info!("StateManager initialized on Login screen");
        StateManager {
            state: Arc::new(RwLock::new(state)),
        }
    }

    pub async fn get_state(&self) -> AppState {
        let state = self.state.read().await;
        state.clone()
    }

    pub async fn navigate_to(&self, screen: Screen) {
        let mut state = self.state.write().await;
        let previous_name = state.current_screen.name().to_string();
        let new_name = screen.name().to_string();
        let previous = std::mem::replace(&mut state.current_screen, screen);
        state.screen_stack.push(previous);
        info!("Navigate: {} -> {} (stack depth: {})", previous_name, new_name, state.screen_stack.len());
    }

    pub async fn go_back(&self) -> Option<Screen> {
        let mut state = self.state.write().await;
        if let Some(previous) = state.screen_stack.pop() {
            let from_name = state.current_screen.name().to_string();
            let to_name = previous.name().to_string();
            state.current_screen = previous;
            info!("Navigate back: {} -> {} (stack depth: {})", from_name, to_name, state.screen_stack.len());
            Some(state.current_screen.clone())
        } else {
            // Stack is empty: if not already on Home, go to Home
            if state.current_screen != Screen::Home {
                let from_name = state.current_screen.name().to_string();
                state.current_screen = Screen::Home;
                info!("Navigate back: {} -> home (stack was empty, defaulting to home)", from_name);
                Some(Screen::Home)
            } else {
                debug!("Go back requested but already on Home with empty stack");
                None
            }
        }
    }

    pub async fn navigate_replace(&self, screen: Screen) {
        let mut state = self.state.write().await;
        let previous_name = state.current_screen.name().to_string();
        let new_name = screen.name().to_string();
        state.current_screen = screen;
        info!("Navigate replace: {} -> {} (stack unchanged, depth: {})", previous_name, new_name, state.screen_stack.len());
    }

    pub async fn set_user(&self, user: UserDto, token: String) {
        let mut state = self.state.write().await;
        info!("User authenticated: {}", &user.name);
        state.current_user = Some(user);
        state.access_token = Some(token);
    }

    pub async fn logout(&self) {
        let mut state = self.state.write().await;
        let user_name = state.current_user
            .as_ref()
            .map(|u| u.name.as_str())
            .unwrap_or("unknown")
            .to_string();
        state.current_user = None;
        state.access_token = None;
        state.play_session_id = None;
        state.playing_item_id = None;
        state.playing_media_source_id = None;
        state.tracking_session_id = None;
        state.screen_stack.clear();
        state.current_screen = Screen::Login;
        info!("User '{}' logged out, state reset to Login", user_name);
    }

    pub async fn start_playback(&self, item_id: String, session_id: String, media_source_id: String) {
        let mut state = self.state.write().await;
        info!("Starting playback: item={}, session={}", item_id, session_id);
        state.play_session_id = Some(session_id);
        state.playing_item_id = Some(item_id.clone());
        state.playing_media_source_id = Some(media_source_id);
        // Push current screen onto stack and navigate to Player
        let previous = std::mem::replace(
            &mut state.current_screen,
            Screen::Player { item_id },
        );
        state.screen_stack.push(previous);
    }

    pub async fn stop_playback(&self) {
        let mut state = self.state.write().await;
        let item_id = state.playing_item_id.take();
        state.play_session_id = None;
        state.playing_media_source_id = None;
        state.tracking_session_id = None;
        info!("Stopped playback: item={}", item_id.as_deref().unwrap_or("none"));
        // Go back from player screen
        if let Some(previous) = state.screen_stack.pop() {
            state.current_screen = previous;
        } else {
            state.current_screen = Screen::Home;
        }
        debug!("Returned to {} after stopping playback", state.current_screen.name());
    }

    pub async fn reset_idle(&self) {
        let mut state = self.state.write().await;
        if state.idle_seconds > 0 {
            debug!("Idle timer reset (was {}s)", state.idle_seconds);
        }
        state.idle_seconds = 0;
    }

    pub async fn tick_idle(&self) -> bool {
        let mut state = self.state.write().await;
        state.idle_seconds += 1;
        let triggered = state.idle_seconds >= state.screensaver_timeout;
        if triggered && state.idle_seconds == state.screensaver_timeout {
            info!("Screensaver timeout reached ({}s)", state.screensaver_timeout);
        }
        triggered
    }

    pub async fn is_authenticated(&self) -> bool {
        let state = self.state.read().await;
        state.access_token.is_some()
    }

    pub async fn current_screen_name(&self) -> String {
        let state = self.state.read().await;
        state.current_screen.name().to_string()
    }

    pub async fn get_screen_param(&self) -> Option<String> {
        let state = self.state.read().await;
        match &state.current_screen {
            Screen::Detail { item_id } => Some(item_id.clone()),
            Screen::Library { library_id, .. } => Some(library_id.clone()),
            Screen::Player { item_id } => Some(item_id.clone()),
            _ => None,
        }
    }

    pub async fn set_tracking_session(&self, session_id: Option<i64>) {
        let mut state = self.state.write().await;
        state.tracking_session_id = session_id;
    }

    pub async fn get_tracking_session(&self) -> Option<i64> {
        let state = self.state.read().await;
        state.tracking_session_id
    }

}
