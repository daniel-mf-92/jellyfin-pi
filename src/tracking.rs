use log::{info, warn, error};
use std::path::PathBuf;
use std::sync::Mutex;
use chrono::{Utc, DateTime};

/// Lightweight local playback tracker backed by SQLite.
///
/// Records who watched what, for how long, and from which device.
/// Database lives alongside the app config at `~/.config/jellyfin-tv/playback.db`.

pub struct PlaybackTracker {
    conn: Mutex<rusqlite::Connection>,
}

/// A single completed (or in-progress) playback session row.
#[derive(Debug, Clone)]
pub struct PlaybackSession {
    pub id: i64,
    pub user_name: String,
    pub user_id: String,
    pub item_id: String,
    pub item_title: String,
    pub series_name: Option<String>,
    pub season_episode: Option<String>,
    pub device_name: String,
    pub play_method: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_secs: i64,
    pub position_ticks: i64,
    pub runtime_ticks: Option<i64>,
    pub completed: bool,
}

impl PlaybackTracker {
    /// Open (or create) the tracking database.
    pub fn new() -> Result<Self, rusqlite::Error> {
        let db_path = Self::db_path();
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        Self::migrate(&conn)?;
        info!("PlaybackTracker opened at {}", db_path.display());
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn db_path() -> PathBuf {
        crate::config::AppConfig::config_dir().join("playback.db")
    }

    fn migrate(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS playback_sessions (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                user_name       TEXT NOT NULL,
                user_id         TEXT NOT NULL,
                item_id         TEXT NOT NULL,
                item_title      TEXT NOT NULL,
                series_name     TEXT,
                season_episode  TEXT,
                device_name     TEXT NOT NULL,
                play_method     TEXT NOT NULL,
                started_at      TEXT NOT NULL,
                ended_at        TEXT,
                duration_secs   INTEGER NOT NULL DEFAULT 0,
                position_ticks  INTEGER NOT NULL DEFAULT 0,
                runtime_ticks   INTEGER,
                completed       INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_started ON playback_sessions(started_at);
            CREATE INDEX IF NOT EXISTS idx_sessions_user    ON playback_sessions(user_id);
            CREATE INDEX IF NOT EXISTS idx_sessions_item    ON playback_sessions(item_id);
            "
        )?;
        Ok(())
    }

    /// Record the start of a new playback session. Returns the session row ID.
    pub fn start_session(
        &self,
        user_name: &str,
        user_id: &str,
        item_id: &str,
        item_title: &str,
        series_name: Option<&str>,
        season_episode: Option<&str>,
        device_name: &str,
        play_method: &str,
        runtime_ticks: Option<i64>,
    ) -> Result<i64, rusqlite::Error> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO playback_sessions
                (user_name, user_id, item_id, item_title, series_name,
                 season_episode, device_name, play_method, started_at, runtime_ticks)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                user_name, user_id, item_id, item_title, series_name,
                season_episode, device_name, play_method, now, runtime_ticks,
            ],
        )?;
        let row_id = conn.last_insert_rowid();
        info!(
            "Tracking: session #{} started — {} playing \"{}\" on {}",
            row_id, user_name, item_title, device_name
        );
        Ok(row_id)
    }

    /// Update position for an in-progress session.
    pub fn update_position(&self, session_id: i64, position_ticks: i64) {
        let conn = self.conn.lock().unwrap();
        if let Err(e) = conn.execute(
            "UPDATE playback_sessions SET position_ticks = ?1 WHERE id = ?2",
            rusqlite::params![position_ticks, session_id],
        ) {
            warn!("Tracking: failed to update position for session #{}: {}", session_id, e);
        }
    }

    /// End a playback session, recording final duration and position.
    pub fn end_session(
        &self,
        session_id: i64,
        position_ticks: i64,
        runtime_ticks: Option<i64>,
    ) {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();

        // Calculate duration from started_at
        let started: Option<String> = conn
            .query_row(
                "SELECT started_at FROM playback_sessions WHERE id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .ok();

        let duration_secs = started
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|start| (Utc::now() - start.with_timezone(&Utc)).num_seconds().max(0))
            .unwrap_or(0);

        // Consider it "completed" if watched >90% of runtime
        let completed = runtime_ticks
            .filter(|&rt| rt > 0)
            .map(|rt| position_ticks as f64 / rt as f64 > 0.90)
            .unwrap_or(false);

        if let Err(e) = conn.execute(
            "UPDATE playback_sessions
                SET ended_at = ?1, duration_secs = ?2, position_ticks = ?3,
                    runtime_ticks = ?4, completed = ?5
              WHERE id = ?6",
            rusqlite::params![now, duration_secs, position_ticks, runtime_ticks, completed as i32, session_id],
        ) {
            error!("Tracking: failed to end session #{}: {}", session_id, e);
            return;
        }

        info!(
            "Tracking: session #{} ended — {}s watched, completed={}",
            session_id, duration_secs, completed
        );
    }

    /// Fetch the N most recent sessions (for diagnostics / UI).
    pub fn recent_sessions(&self, limit: i64) -> Vec<PlaybackSession> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, user_name, user_id, item_id, item_title, series_name,
                    season_episode, device_name, play_method, started_at, ended_at,
                    duration_secs, position_ticks, runtime_ticks, completed
               FROM playback_sessions
              ORDER BY started_at DESC
              LIMIT ?1"
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("Tracking: failed to query sessions: {}", e);
                return Vec::new();
            }
        };

        let rows = stmt.query_map([limit], |row| {
            Ok(PlaybackSession {
                id: row.get(0)?,
                user_name: row.get(1)?,
                user_id: row.get(2)?,
                item_id: row.get(3)?,
                item_title: row.get(4)?,
                series_name: row.get(5)?,
                season_episode: row.get(6)?,
                device_name: row.get(7)?,
                play_method: row.get(8)?,
                started_at: row.get(9)?,
                ended_at: row.get(10)?,
                duration_secs: row.get(11)?,
                position_ticks: row.get(12)?,
                runtime_ticks: row.get(13)?,
                completed: row.get::<_, i32>(14)? != 0,
            })
        });

        match rows {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                error!("Tracking: query error: {}", e);
                Vec::new()
            }
        }
    }
}
