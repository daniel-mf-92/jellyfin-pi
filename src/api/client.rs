use reqwest::Client;
use std::fmt;

use crate::api::models::*;
use crate::config::AppConfig;

// Fields to request when fetching items
const ITEM_FIELDS: &str = "CanDelete,Chapters,ChildCount,CumulativeRunTimeTicks,DateCreated,Genres,MediaSourceCount,MediaSources,MediaStreams,Overview,Path,PrimaryImageAspectRatio,Taglines,Trickplay";

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ApiError {
    Network(reqwest::Error),
    Auth(String),
    NotFound,
    Server(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Network(e) => write!(f, "Network error: {e}"),
            ApiError::Auth(msg) => write!(f, "Auth error: {msg}"),
            ApiError::NotFound => write!(f, "Not found"),
            ApiError::Server(msg) => write!(f, "Server error: {msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        if e.status() == Some(reqwest::StatusCode::NOT_FOUND) {
            ApiError::NotFound
        } else if e.status() == Some(reqwest::StatusCode::UNAUTHORIZED) {
            ApiError::Auth("Unauthorized".into())
        } else {
            ApiError::Network(e)
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub struct JellyfinClient {
    http: Client,
    pub server_url: String,
    pub access_token: Option<String>,
    pub user_id: Option<String>,
    device_id: String,
    device_name: String,
    client_name: String,
    client_version: String,
}

impl JellyfinClient {
    /// Create a new client from application config.
    pub fn new(config: &AppConfig) -> Self {
        Self {
            http: Client::new(),
            server_url: config.server.url.clone(),
            access_token: None,
            user_id: None,
            device_id: config.server.device_id.clone(),
            device_name: config.server.device_name.clone(),
            client_name: config.server.client_name.clone(),
            client_version: config.server.client_version.clone(),
        }
    }

    /// Build the `X-Emby-Authorization` header value.
    pub fn auth_header(&self) -> String {
        let token_part = self
            .access_token
            .as_deref()
            .map(|t| format!(", Token=\"{t}\""))
            .unwrap_or_default();

        format!(
            "MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"{}",
            self.client_name,
            self.device_name,
            self.device_id,
            self.client_version,
            token_part,
        )
    }

    // -----------------------------------------------------------------------
    // Helper: check response status and return body or error
    // -----------------------------------------------------------------------

    async fn check_response(
        &self,
        resp: reqwest::Response,
    ) -> ApiResult<reqwest::Response> {
        let status = resp.status();
        if status.is_success() {
            Ok(resp)
        } else if status == reqwest::StatusCode::NOT_FOUND {
            Err(ApiError::NotFound)
        } else if status == reqwest::StatusCode::UNAUTHORIZED {
            Err(ApiError::Auth("Unauthorized".into()))
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(ApiError::Server(format!("{status}: {body}")))
        }
    }

    // -----------------------------------------------------------------------
    // System / Auth
    // -----------------------------------------------------------------------

    /// GET /Users/Public
    pub async fn get_public_users(&self) -> ApiResult<Vec<UserDto>> {
        let url = format!("{}/Users/Public", self.server_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// POST /Users/AuthenticateByName
    ///
    /// On success the access token and user ID are stored on the client.
    pub async fn authenticate(
        &mut self,
        username: &str,
        password: &str,
    ) -> ApiResult<AuthenticationResult> {
        let url = format!("{}/Users/AuthenticateByName", self.server_url);

        #[derive(serde::Serialize)]
        struct Body<'a> {
            #[serde(rename = "Username")]
            username: &'a str,
            #[serde(rename = "Pw")]
            pw: &'a str,
        }

        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&Body {
                username,
                pw: password,
            })
            .send()
            .await?;

        let resp = self.check_response(resp).await?;
        let result: AuthenticationResult = resp.json().await?;

        self.access_token = Some(result.access_token.clone());
        self.user_id = Some(result.user.id.clone());

        Ok(result)
    }

    /// GET /System/Info/Public
    pub async fn get_public_system_info(&self) -> ApiResult<PublicSystemInfo> {
        let url = format!("{}/System/Info/Public", self.server_url);
        let resp = self.http.get(&url).send().await?;
        let resp = self.check_response(resp).await?;
        Ok(resp.json().await?)
    }

    // -----------------------------------------------------------------------
    // User views / home screen
    // -----------------------------------------------------------------------

    /// GET /UserViews — returns the user's media library views (Movies, Shows, etc.)
    pub async fn get_user_views(&self) -> ApiResult<Vec<BaseItemDto>> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!("{}/Users/{}/Views", self.server_url, user_id);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        let result: QueryResult = resp.json().await?;
        Ok(result.items)
    }

    /// GET /UserItems/Resume
    pub async fn get_resume_items(&self, limit: i32) -> ApiResult<Vec<BaseItemDto>> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!("{}/Users/{}/Items/Resume", self.server_url, user_id);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .query(&[
                ("Limit", limit.to_string()),
                ("MediaTypes", "Video".into()),
                ("Fields", ITEM_FIELDS.into()),
            ])
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        let result: QueryResult = resp.json().await?;
        Ok(result.items)
    }

    /// GET /Shows/NextUp
    pub async fn get_next_up(&self, limit: i32) -> ApiResult<Vec<BaseItemDto>> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!("{}/Shows/NextUp", self.server_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .query(&[
                ("UserId", user_id.to_string()),
                ("Limit", limit.to_string()),
                ("Fields", ITEM_FIELDS.into()),
            ])
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        let result: QueryResult = resp.json().await?;
        Ok(result.items)
    }

    /// GET /Items/Latest
    pub async fn get_latest_media(
        &self,
        parent_id: &str,
        limit: i32,
    ) -> ApiResult<Vec<BaseItemDto>> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!("{}/Users/{}/Items/Latest", self.server_url, user_id);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .query(&[
                ("ParentId", parent_id),
                ("Limit", &limit.to_string()),
                ("Fields", ITEM_FIELDS),
            ])
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        // Latest endpoint returns a plain array, not a QueryResult wrapper
        Ok(resp.json().await?)
    }

    // -----------------------------------------------------------------------
    // Items / browsing
    // -----------------------------------------------------------------------

    /// GET /Items — generic item query with many optional filters.
    pub async fn get_items(
        &self,
        parent_id: Option<&str>,
        item_types: Option<&str>,
        sort_by: Option<&str>,
        sort_order: Option<&str>,
        start_index: i32,
        limit: i32,
        filters: Option<&str>,
    ) -> ApiResult<QueryResult> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!("{}/Users/{}/Items", self.server_url, user_id);

        let mut params: Vec<(&str, String)> = vec![
            ("StartIndex", start_index.to_string()),
            ("Limit", limit.to_string()),
            ("Fields", ITEM_FIELDS.into()),
            ("Recursive", "true".into()),
        ];

        if let Some(v) = parent_id {
            params.push(("ParentId", v.into()));
        }
        if let Some(v) = item_types {
            params.push(("IncludeItemTypes", v.into()));
        }
        if let Some(v) = sort_by {
            params.push(("SortBy", v.into()));
        }
        if let Some(v) = sort_order {
            params.push(("SortOrder", v.into()));
        }
        if let Some(v) = filters {
            params.push(("Filters", v.into()));
        }

        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .query(&params)
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// GET /Users/{userId}/Items/{itemId} — fetch a single item by ID.
    pub async fn get_item(&self, item_id: &str) -> ApiResult<BaseItemDto> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!(
            "{}/Users/{}/Items/{}",
            self.server_url, user_id, item_id
        );
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .query(&[("Fields", ITEM_FIELDS)])
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// GET /Shows/{seriesId}/Seasons
    pub async fn get_seasons(&self, series_id: &str) -> ApiResult<Vec<BaseItemDto>> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!("{}/Shows/{}/Seasons", self.server_url, series_id);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .query(&[
                ("UserId", user_id),
                ("Fields", ITEM_FIELDS),
            ])
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        let result: QueryResult = resp.json().await?;
        Ok(result.items)
    }

    /// GET /Shows/{seriesId}/Episodes
    pub async fn get_episodes(
        &self,
        series_id: &str,
        season_id: &str,
    ) -> ApiResult<Vec<BaseItemDto>> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!("{}/Shows/{}/Episodes", self.server_url, series_id);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .query(&[
                ("UserId", user_id),
                ("SeasonId", season_id),
                ("Fields", ITEM_FIELDS),
            ])
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        let result: QueryResult = resp.json().await?;
        Ok(result.items)
    }

    /// GET /Items/{id}/Similar
    pub async fn get_similar(
        &self,
        item_id: &str,
        limit: i32,
    ) -> ApiResult<Vec<BaseItemDto>> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!("{}/Items/{}/Similar", self.server_url, item_id);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .query(&[
                ("UserId", user_id.to_string()),
                ("Limit", limit.to_string()),
                ("Fields", ITEM_FIELDS.into()),
            ])
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        let result: QueryResult = resp.json().await?;
        Ok(result.items)
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// GET /Search/Hints
    pub async fn search(
        &self,
        query: &str,
        limit: i32,
    ) -> ApiResult<Vec<SearchHint>> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!("{}/Search/Hints", self.server_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .query(&[
                ("UserId", user_id.to_string()),
                ("SearchTerm", query.into()),
                ("Limit", limit.to_string()),
            ])
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        let result: SearchHintResult = resp.json().await?;
        Ok(result.search_hints)
    }

    // -----------------------------------------------------------------------
    // Playback
    // -----------------------------------------------------------------------

    /// POST /Items/{id}/PlaybackInfo
    pub async fn get_playback_info(
        &self,
        item_id: &str,
    ) -> ApiResult<PlaybackInfoResponse> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!("{}/Items/{}/PlaybackInfo", self.server_url, item_id);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .query(&[("UserId", user_id)])
            .json(&serde_json::json!({}))
            .send()
            .await?;
        let resp = self.check_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// POST /Sessions/Playing
    pub async fn report_playback_start(
        &self,
        info: &PlaybackStartInfo,
    ) -> ApiResult<()> {
        let url = format!("{}/Sessions/Playing", self.server_url);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(info)
            .send()
            .await?;
        self.check_response(resp).await?;
        Ok(())
    }

    /// POST /Sessions/Playing/Progress
    pub async fn report_playback_progress(
        &self,
        info: &PlaybackProgressInfo,
    ) -> ApiResult<()> {
        let url = format!("{}/Sessions/Playing/Progress", self.server_url);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(info)
            .send()
            .await?;
        self.check_response(resp).await?;
        Ok(())
    }

    /// POST /Sessions/Playing/Stopped
    pub async fn report_playback_stopped(
        &self,
        info: &PlaybackStopInfo,
    ) -> ApiResult<()> {
        let url = format!("{}/Sessions/Playing/Stopped", self.server_url);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(info)
            .send()
            .await?;
        self.check_response(resp).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // User actions
    // -----------------------------------------------------------------------

    /// POST or DELETE /UserFavoriteItems/{id}
    pub async fn toggle_favorite(
        &self,
        item_id: &str,
        is_favorite: bool,
    ) -> ApiResult<()> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!(
            "{}/Users/{}/FavoriteItems/{}",
            self.server_url, user_id, item_id
        );

        let resp = if is_favorite {
            self.http
                .post(&url)
                .header("Authorization", self.auth_header())
                .send()
                .await?
        } else {
            self.http
                .delete(&url)
                .header("Authorization", self.auth_header())
                .send()
                .await?
        };

        self.check_response(resp).await?;
        Ok(())
    }

    /// POST /UserPlayedItems/{id}
    pub async fn mark_played(&self, item_id: &str) -> ApiResult<()> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!(
            "{}/Users/{}/PlayedItems/{}",
            self.server_url, user_id, item_id
        );
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        self.check_response(resp).await?;
        Ok(())
    }

    /// DELETE /UserPlayedItems/{id}
    pub async fn mark_unplayed(&self, item_id: &str) -> ApiResult<()> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| ApiError::Auth("Not authenticated".into()))?;

        let url = format!(
            "{}/Users/{}/PlayedItems/{}",
            self.server_url, user_id, item_id
        );
        let resp = self
            .http
            .delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        self.check_response(resp).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Images
    // -----------------------------------------------------------------------

    /// Build an image URL for the given item. Not async — pure string building.
    pub fn image_url(
        &self,
        item_id: &str,
        image_type: &str,
        max_size: i32,
        tag: Option<&str>,
    ) -> String {
        let mut url = format!(
            "{}/Items/{}/Images/{}?maxWidth={}&maxHeight={}",
            self.server_url, item_id, image_type, max_size, max_size
        );
        if let Some(t) = tag {
            url.push_str(&format!("&tag={t}"));
        }
        url
    }
}
