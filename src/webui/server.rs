// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along with
// this program. If not, see <https://www.gnu.org/licenses/>.
//

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::{any, get},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
use tonic_web::GrpcWebLayer;
use tower::{Layer, ServiceExt};
use tracing::{debug, error, info};

use crate::controller::grpc::PlayerServer;
use crate::player::Player;
use crate::proto::player::v1::player_service_server::PlayerServiceServer;
use crate::state::StateSnapshot;

use super::state as ws_state;

fn process_video_preview_permit() -> Arc<tokio::sync::Semaphore> {
    static PERMIT: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    Arc::clone(PERMIT.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1))))
}

#[derive(Embed)]
#[folder = "src/webui/svelte/dist/"]
#[allow_missing = true]
struct WebUiAssets;

/// Configuration for the web server (address + port).
#[derive(Debug, Clone)]
pub struct WebConfig {
    pub port: u16,
    pub address: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            address: "127.0.0.1".to_string(),
        }
    }
}

/// Shared state for the web UI.
///
/// All path fields are canonicalized at construction time so that path
/// comparisons (e.g. `strip_prefix`) work consistently regardless of how
/// the paths were originally specified in the config file.
#[derive(Clone)]
pub struct WebUiState {
    pub player: Arc<Player>,
    pub state_rx: watch::Receiver<Arc<StateSnapshot>>,
    pub broadcast_tx: broadcast::Sender<String>,
    /// Canonicalized path to the player config file (mtrack.yaml).
    pub config_path: PathBuf,
    /// Canonicalized path to the songs directory.
    /// codeql[rust/path-injection] Set once at startup from the player config file; not user-controlled.
    pub songs_path: PathBuf,
    /// Canonicalized path to the playlists directory (if configured).
    pub playlists_dir: Option<PathBuf>,
    /// Canonicalized path to the legacy playlist file (for backward compat).
    pub legacy_playlist_path: Option<PathBuf>,
    /// Shared waveform cache (sent to each WebSocket client on connect).
    pub waveform_cache: ws_state::WaveformCache,
    /// Canonicalized path to the profiles directory (if configured).
    pub profiles_dir: Option<PathBuf>,
    /// Active calibration session (at most one at a time).
    pub(crate) calibration:
        Arc<parking_lot::Mutex<Option<super::api::devices::CalibrationSession>>>,
    /// Process-wide single editor video-preview session permit.
    #[cfg_attr(not(feature = "video"), allow(dead_code))]
    pub(crate) video_preview_permit: Arc<tokio::sync::Semaphore>,
}

/// Canonicalizes a path, falling back to the original if canonicalization fails
/// (e.g. the path does not exist yet).
fn canonicalize_or_keep(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

/// Parameters for constructing a [`WebUiState`].
pub struct WebUiStateParams {
    pub player: Arc<Player>,
    pub state_rx: watch::Receiver<Arc<StateSnapshot>>,
    pub broadcast_tx: broadcast::Sender<String>,
    pub config_path: PathBuf,
    pub songs_path: PathBuf,
    pub playlists_dir: Option<PathBuf>,
    pub legacy_playlist_path: Option<PathBuf>,
    pub profiles_dir: Option<PathBuf>,
    pub waveform_cache: ws_state::WaveformCache,
}

impl WebUiState {
    /// Creates a new WebUiState, canonicalizing all path fields.
    pub fn new(params: WebUiStateParams) -> Self {
        WebUiState {
            player: params.player,
            state_rx: params.state_rx,
            broadcast_tx: params.broadcast_tx,
            config_path: canonicalize_or_keep(params.config_path),
            songs_path: canonicalize_or_keep(params.songs_path),
            playlists_dir: params.playlists_dir.map(canonicalize_or_keep),
            legacy_playlist_path: params.legacy_playlist_path.map(canonicalize_or_keep),
            profiles_dir: params.profiles_dir.map(canonicalize_or_keep),
            waveform_cache: params.waveform_cache,
            calibration: Arc::new(parking_lot::Mutex::new(None)),
            video_preview_permit: process_video_preview_permit(),
        }
    }
}

/// Handle to a running web UI server, returned so callers can shut it down.
///
/// When dropped, the server and background tasks are aborted.
pub struct WebUiHandle {
    _shutdown_tx: tokio::sync::oneshot::Sender<()>,
    server_handle: tokio::task::JoinHandle<()>,
    playback_poller_handle: tokio::task::JoinHandle<()>,
    state_poller_handle: tokio::task::JoinHandle<()>,
    log_poller_handle: tokio::task::JoinHandle<()>,
    waveform_poller_handle: tokio::task::JoinHandle<()>,
    waveform_prewarmer_handle: tokio::task::JoinHandle<()>,
    config_watcher_handle: tokio::task::JoinHandle<()>,
    /// The local address the server is bound to.
    #[allow(dead_code)]
    local_addr: std::net::SocketAddr,
}

impl WebUiHandle {
    /// Returns the local address the server is listening on.
    #[allow(dead_code)]
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }
}

impl Drop for WebUiHandle {
    fn drop(&mut self) {
        self.server_handle.abort();
        self.playback_poller_handle.abort();
        self.state_poller_handle.abort();
        self.log_poller_handle.abort();
        self.waveform_poller_handle.abort();
        self.waveform_prewarmer_handle.abort();
        self.config_watcher_handle.abort();
    }
}

/// Starts the unified web server with dashboard, gRPC-Web, and REST API.
///
/// The server serves:
/// - `/` — Web UI SPA (dashboard, config editor, etc.)
/// - `/ws` — WebSocket for real-time state streaming (playback, fixtures, logs)
/// - `/player.v1.PlayerService/*` — gRPC-Web endpoints for player control
/// - `/api/*` — REST endpoints for config/song/playlist/lighting CRUD
/// - Static assets from the embedded Svelte dist directory
pub async fn start(
    webui_state: WebUiState,
    address: String,
    port: u16,
) -> Result<WebUiHandle, Box<dyn std::error::Error + Send + Sync>> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    // Start background pollers that feed the broadcast channel
    let playback_poller_handle = tokio::spawn(ws_state::playback_poller(
        webui_state.player.clone(),
        webui_state.broadcast_tx.clone(),
    ));
    let state_poller_handle = tokio::spawn(ws_state::state_poller(
        webui_state.state_rx.clone(),
        webui_state.broadcast_tx.clone(),
    ));
    let log_poller_handle = tokio::spawn(ws_state::log_poller(webui_state.broadcast_tx.clone()));
    let waveform_poller_handle = tokio::spawn(ws_state::waveform_poller(
        webui_state.player.clone(),
        webui_state.broadcast_tx.clone(),
        webui_state.waveform_cache.clone(),
    ));
    let waveform_prewarmer_handle = tokio::spawn(ws_state::waveform_prewarmer(
        webui_state.player.clone(),
        webui_state.waveform_cache.clone(),
    ));
    let config_watcher_handle = tokio::spawn(ws_state::config_watcher(
        webui_state.player.clone(),
        webui_state.broadcast_tx.clone(),
    ));

    // Build the app router
    let api_router = super::api::router().route_layer(axum::middleware::from_fn_with_state(
        webui_state.clone(),
        super::api::lock_guard,
    ));
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .nest("/api", api_router)
        .route("/player.v1.PlayerService/{method}", any(grpc_web_handler))
        .route("/", get(index_handler))
        .fallback(get(static_handler))
        .with_state(webui_state);

    let ip: std::net::IpAddr = match address.parse() {
        Ok(ip) => ip,
        Err(e) => {
            return Err(format!("Invalid web UI address '{}': {}", address, e).into());
        }
    };
    let addr = std::net::SocketAddr::from((ip, port));
    info!("Web UI listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;

    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap_or_else(|e| error!("Web UI server error: {}", e));
    });

    Ok(WebUiHandle {
        _shutdown_tx: shutdown_tx,
        server_handle,
        playback_poller_handle,
        state_poller_handle,
        log_poller_handle,
        waveform_poller_handle,
        waveform_prewarmer_handle,
        config_watcher_handle,
        local_addr,
    })
}

/// Handles gRPC-Web requests by adapting between axum and tonic body types.
async fn grpc_web_handler(
    State(state): State<WebUiState>,
    request: axum::extract::Request,
) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("none")
        .to_string();

    debug!(
        "gRPC-Web request: {} {} (content-type: {})",
        method, uri, content_type
    );

    let grpc_svc = match state.player.config_store() {
        Some(store) => {
            PlayerServiceServer::new(PlayerServer::with_config_store(state.player.clone(), store))
        }
        None => PlayerServiceServer::new(PlayerServer::new(state.player.clone())),
    };
    let grpc_web = GrpcWebLayer::new().layer(grpc_svc);

    match grpc_web.oneshot(request).await {
        Ok(response) => {
            debug!("gRPC-Web response status: {}", response.status());
            let (parts, body) = response.into_parts();
            let body = axum::body::Body::new(body);
            http::Response::from_parts(parts, body)
        }
        Err(e) => {
            error!("gRPC-Web handler error: {}", e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(axum::body::Body::from(format!("{e}")))
                .unwrap()
        }
    }
}

/// Serves the embedded index.html for the SPA.
async fn index_handler() -> impl IntoResponse {
    match WebUiAssets::get("index.html") {
        Some(content) => Html(
            std::str::from_utf8(content.data.as_ref())
                .unwrap_or("<h1>Error loading web UI</h1>")
                .to_string(),
        )
        .into_response(),
        None => (StatusCode::NOT_FOUND, "Web UI assets not found").into_response(),
    }
}

/// Serves embedded static assets (CSS, JS, etc.).
async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Try exact path first
    if let Some(content) = WebUiAssets::get(path) {
        let mime = mime_type_for_path(path);
        return Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .body(axum::body::Body::from(content.data.to_vec()))
            .unwrap();
    }

    // For SPA routing: return index.html for non-file paths
    // (paths without extensions are assumed to be SPA routes)
    if !path.contains('.') {
        if let Some(content) = WebUiAssets::get("index.html") {
            return Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(axum::body::Body::from(content.data.to_vec()))
                .unwrap();
        }
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(axum::body::Body::from("Not found"))
        .unwrap()
}

/// Returns the MIME type for a file path based on its extension.
fn mime_type_for_path(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Handles WebSocket upgrade requests for the dashboard.
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<WebUiState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

/// Handles a single WebSocket connection for the dashboard.
///
/// Sends fixture metadata on connect, then streams playback, fixture state,
/// and log messages from the broadcast channel.
async fn handle_ws(socket: WebSocket, state: WebUiState) {
    let (mut sender, mut receiver) = socket.split();

    // Send metadata on connect (fixture tags, types for stage layout).
    // Computed on-demand so it reflects the current DMX engine state
    // (which may have changed via hot-reload since startup).
    let metadata_json = match state.player.broadcast_handles() {
        Some(handles) => ws_state::build_metadata_json(handles.lighting_system.as_ref()),
        None => ws_state::build_metadata_json(None),
    };
    if sender
        .send(Message::Text(metadata_json.into()))
        .await
        .is_err()
    {
        return;
    }

    // Send cached waveform for the current song on connect
    let waveform_msg = {
        state
            .player
            .get_playlist()
            .current()
            .and_then(|current_song| {
                let song_name = current_song.name().to_string();
                state
                    .waveform_cache
                    .lock()
                    .get(&song_name)
                    .cloned()
                    .map(|track_peaks| {
                        let tracks_json: Vec<serde_json::Value> = track_peaks
                            .into_iter()
                            .map(|(name, peaks)| {
                                serde_json::json!({
                                    "name": name,
                                    "peaks": peaks,
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "type": "waveform",
                            "song_name": song_name,
                            "tracks": tracks_json,
                        })
                        .to_string()
                    })
            })
    };
    if let Some(msg) = waveform_msg {
        if sender.send(Message::Text(msg.into())).await.is_err() {
            return;
        }
    }

    // Subscribe to state broadcasts
    let mut rx = state.broadcast_tx.subscribe();

    // Spawn a task to forward broadcasts to this client
    let mut send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if sender.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Drain incoming messages (we only use server→client for now)
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Close(_) = msg {
                break;
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => { recv_task.abort(); }
        _ = &mut recv_task => { send_task.abort(); }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn mime_type_html() {
        assert_eq!(mime_type_for_path("index.html"), "text/html; charset=utf-8");
    }

    #[test]
    fn mime_type_css() {
        assert_eq!(
            mime_type_for_path("styles/app.css"),
            "text/css; charset=utf-8"
        );
    }

    #[test]
    fn mime_type_js() {
        assert_eq!(
            mime_type_for_path("bundle.js"),
            "application/javascript; charset=utf-8"
        );
    }

    #[test]
    fn mime_type_json() {
        assert_eq!(mime_type_for_path("data.json"), "application/json");
    }

    #[test]
    fn mime_type_png() {
        assert_eq!(mime_type_for_path("logo.png"), "image/png");
    }

    #[test]
    fn mime_type_svg() {
        assert_eq!(mime_type_for_path("icon.svg"), "image/svg+xml");
    }

    #[test]
    fn mime_type_ico() {
        assert_eq!(mime_type_for_path("favicon.ico"), "image/x-icon");
    }

    #[test]
    fn mime_type_woff() {
        assert_eq!(mime_type_for_path("font.woff"), "font/woff");
    }

    #[test]
    fn mime_type_woff2() {
        assert_eq!(mime_type_for_path("font.woff2"), "font/woff2");
    }

    #[test]
    fn mime_type_unknown() {
        assert_eq!(mime_type_for_path("file.xyz"), "application/octet-stream");
    }

    #[test]
    fn mime_type_no_extension() {
        assert_eq!(mime_type_for_path("README"), "application/octet-stream");
    }

    #[test]
    fn web_config_default() {
        let config = WebConfig::default();
        assert_eq!(config.port, 8080);
        assert_eq!(config.address, "127.0.0.1");
    }

    use crate::player::PlayerDevices;
    use crate::playlist;
    use crate::songs::{Song, Songs};
    use std::collections::HashMap;

    fn test_webui_state() -> (WebUiState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();

        let config_path = dir.path().join("mtrack.yaml");
        std::fs::write(&config_path, "songs: songs\n").unwrap();

        let playlist_path = dir.path().join("playlist.yaml");
        std::fs::write(&playlist_path, "songs:\n  - Song A\n").unwrap();

        let songs_path = dir.path().join("songs");
        std::fs::create_dir(&songs_path).unwrap();

        let mut map = HashMap::new();
        map.insert(
            "Song A".to_string(),
            Arc::new(Song::new_for_test("Song A", &["track1"])),
        );
        let songs = Arc::new(Songs::new(map));
        let playlist_config = crate::config::Playlist::new(&["Song A".to_string()]);
        let pl = playlist::Playlist::new("test", &playlist_config, songs.clone()).unwrap();
        let devices = PlayerDevices {
            audio: None,
            mappings: None,
            midi: None,
            dmx_engine: None,
            sample_engine: None,
            trigger_engine: None,
        };
        let mut playlists = std::collections::HashMap::new();
        playlists.insert(
            "all_songs".to_string(),
            playlist::from_songs(songs.clone()).unwrap(),
        );
        playlists.insert(pl.name().to_string(), pl);
        let player = Arc::new(
            crate::player::Player::new_with_devices(devices, playlists, "test".to_string(), None)
                .unwrap(),
        );

        let (broadcast_tx, _) = broadcast::channel(16);
        let (_state_tx, state_rx) =
            watch::channel(Arc::new(crate::state::StateSnapshot::default()));

        let state = WebUiState::new(WebUiStateParams {
            player,
            state_rx,
            broadcast_tx,
            config_path,
            songs_path,
            playlists_dir: None,
            legacy_playlist_path: Some(playlist_path),
            profiles_dir: None,
            waveform_cache: ws_state::new_waveform_cache(),
        });

        (state, dir)
    }

    async fn start_test_server() -> (WebUiHandle, String, tempfile::TempDir) {
        let (state, dir) = test_webui_state();
        let handle = start(state, "127.0.0.1".to_string(), 0)
            .await
            .expect("server should start");
        let base_url = format!("http://{}", handle.local_addr());
        (handle, base_url, dir)
    }

    #[tokio::test]
    async fn server_serves_index() {
        let (_handle, base_url, _dir) = start_test_server().await;

        let resp = reqwest::get(&base_url).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("html"), "expected HTML response");
    }

    #[tokio::test]
    async fn server_serves_spa_route() {
        let (_handle, base_url, _dir) = start_test_server().await;

        let resp = reqwest::get(format!("{}/dashboard", base_url))
            .await
            .unwrap();
        // SPA routes should return index.html
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn server_static_not_found() {
        let (_handle, base_url, _dir) = start_test_server().await;

        let resp = reqwest::get(format!("{}/nonexistent.xyz", base_url))
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn server_api_config() {
        let (_handle, base_url, _dir) = start_test_server().await;

        let resp = reqwest::get(format!("{}/api/config", base_url))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("songs:"));
    }

    #[tokio::test]
    async fn server_api_playlist() {
        let (_handle, base_url, _dir) = start_test_server().await;

        let resp = reqwest::get(format!("{}/api/playlist", base_url))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn server_api_validate_config() {
        let (_handle, base_url, _dir) = start_test_server().await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/api/config/validate", base_url))
            .body("songs: songs\n")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn server_api_songs_empty() {
        let (_handle, base_url, _dir) = start_test_server().await;

        let resp = reqwest::get(format!("{}/api/songs", base_url))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn server_api_lighting_empty() {
        let (_handle, base_url, _dir) = start_test_server().await;

        let resp = reqwest::get(format!("{}/api/lighting", base_url))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn server_local_addr() {
        let (handle, _, _dir) = start_test_server().await;
        let addr = handle.local_addr();
        assert_ne!(addr.port(), 0, "should have resolved to a real port");
    }

    #[tokio::test]
    async fn server_drop_shuts_down() {
        let (handle, base_url, _dir) = start_test_server().await;

        // Verify server is up
        let resp = reqwest::get(&base_url).await.unwrap();
        assert_eq!(resp.status(), 200);

        // Drop the handle
        drop(handle);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Server should be down — connection should fail
        let result = reqwest::get(&base_url).await;
        assert!(result.is_err(), "server should be shut down");
    }

    #[tokio::test]
    async fn server_serves_css_asset() {
        let css_file = WebUiAssets::iter()
            .find(|f| f.ends_with(".css"))
            .expect("no CSS asset found in embedded files");
        let (_handle, base_url, _dir) = start_test_server().await;

        let resp = reqwest::get(format!("{}/{}", base_url, css_file))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            content_type.contains("css"),
            "expected CSS content type, got: {}",
            content_type
        );
    }

    #[tokio::test]
    async fn server_serves_js_asset() {
        let js_file = WebUiAssets::iter()
            .find(|f| f.ends_with(".js"))
            .expect("no JS asset found in embedded files");
        let (_handle, base_url, _dir) = start_test_server().await;

        let resp = reqwest::get(format!("{}/{}", base_url, js_file))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            content_type.contains("javascript"),
            "expected JS content type, got: {}",
            content_type
        );
    }

    #[tokio::test]
    async fn server_grpc_web_endpoint() {
        let (_handle, base_url, _dir) = start_test_server().await;

        // Send a gRPC-Web request to the PlayerService
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/player.v1.PlayerService/GetPlaylist", base_url))
            .header("content-type", "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .body(vec![0u8; 5]) // minimal gRPC frame (compressed flag + 4 byte length)
            .send()
            .await
            .unwrap();

        // Should get a response (even if the gRPC call fails, we exercise the handler)
        let status = resp.status().as_u16();
        assert!(
            status == 200 || status == 415 || status == 400,
            "unexpected status: {}",
            status
        );
    }

    #[tokio::test]
    async fn server_start_invalid_address() {
        let (state, _dir) = test_webui_state();
        let result = start(state, "not-an-ip".to_string(), 0).await;
        assert!(result.is_err());
    }

    #[cfg(not(feature = "video"))]
    #[tokio::test]
    async fn video_preview_websocket_explicitly_reports_disabled_build() {
        use futures_util::StreamExt;

        let (handle, _, _dir) = start_test_server().await;
        let ws_url = format!(
            "ws://{}/api/songs/Song%20A/video-preview/ws",
            handle.local_addr()
        );
        let (socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
        let (_, mut read) = socket.split();
        let message = read.next().await.unwrap().unwrap().into_text().unwrap();
        let response: serde_json::Value = serde_json::from_str(&message).unwrap();

        assert_eq!(response["type"], "error");
        assert_eq!(response["state"], "disabled");
        assert!(response["error"].as_str().unwrap().contains("disabled"));
    }

    #[cfg(feature = "video")]
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial(video_preview_global)]
    async fn video_preview_websocket_streams_frame_and_releases_on_disconnect() {
        use futures_util::{SinkExt, StreamExt};
        use std::{collections::HashMap, process::Command};

        let song_dir = tempfile::tempdir().unwrap();
        let media = song_dir.path().join("clip.mkv");
        let status = Command::new("timeout")
            .args([
                "10s",
                "gst-launch-1.0",
                "-q",
                "videotestsrc",
                "pattern=blue",
                "num-buffers=8",
                "!",
                "video/x-raw,format=RGBA,width=4,height=4,framerate=4/1",
                "!",
                "videoconvert",
                "!",
                "avenc_ffv1",
                "!",
                "matroskamux",
                "!",
                "filesink",
                &format!("location={}", media.display()),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let song_config = crate::config::Song::new(
            "Preview Song",
            None,
            None,
            None,
            None,
            None,
            Vec::new(),
            HashMap::new(),
            Vec::new(),
        );
        let song = Arc::new(crate::songs::Song::new(song_dir.path(), &song_config).unwrap());
        let mut songs = HashMap::new();
        songs.insert("Preview Song".to_string(), song);
        let (state, _state_dir) = super::super::api::test_helpers::test_state_with_registry(
            Arc::new(crate::songs::Songs::new(songs)),
        );
        let observed_state = state.clone();
        let handle = start(state, "127.0.0.1".to_string(), 0).await.unwrap();
        let ws_url = format!(
            "ws://{}/api/songs/Preview%20Song/video-preview/ws",
            handle.local_addr()
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
        socket.send(tokio_tungstenite::tungstenite::Message::Text(
            r##"{"type":"start","request_id":"ws-1","config":{"canvas":{"width":4,"height":4,"frame_rate":4,"background":"#000000"},"tracks":[{"name":"screen","clips":[{"id":"clip","start":0,"duration":2,"file":"clip.mkv"}]}]},"position_ms":250,"autoplay":false}"##.into()
        )).await.unwrap();

        let mut got_frame = false;
        let mut got_paused = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while !(got_frame && got_paused) {
            let message = tokio::time::timeout_at(deadline, socket.next())
                .await
                .expect("timed out waiting for preview output")
                .unwrap()
                .unwrap();
            match message {
                tokio_tungstenite::tungstenite::Message::Binary(frame) => {
                    let header = super::super::video_preview::decode_frame_message(&frame).unwrap();
                    assert_eq!(header.epoch, 1);
                    assert_eq!(header.position_ms, 250);
                    assert!(header.jpeg.starts_with(&[0xff, 0xd8]));
                    got_frame = true;
                }
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    assert_ne!(value["type"], "error", "{value}");
                    got_paused |= value["state"] == "paused";
                }
                _ => {}
            }
        }
        socket.close(None).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while observed_state.video_preview_permit.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("preview permit was not released after disconnect");
    }

    #[cfg(feature = "video")]
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial(video_preview_global)]
    async fn video_preview_pipelined_lifecycle_commands_do_not_deadlock_admission() {
        use futures_util::{SinkExt, StreamExt};
        use std::collections::HashSet;

        let (handle, _, _dir) = start_test_server().await;
        let ws_url = format!(
            "ws://{}/api/songs/Song%20A/video-preview/ws",
            handle.local_addr()
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
        const COMMANDS: usize = 40;

        for request_id in 0..COMMANDS {
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    format!(r#"{{"type":"play","request_id":"pipeline-{request_id}"}}"#).into(),
                ))
                .await
                .unwrap();
        }

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut correlated = HashSet::new();
        while correlated.len() < COMMANDS {
            let message = tokio::time::timeout_at(deadline, socket.next())
                .await
                .expect("pipelined command admission deadlocked")
                .expect("preview socket closed before all commands completed")
                .unwrap();
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                let request_id = value["request_id"].as_str().unwrap();
                assert_eq!(value["type"], "error", "unexpected response: {value}");
                assert_eq!(value["state"], "stopped", "unexpected response: {value}");
                correlated.insert(request_id.to_string());
            }
        }
        assert_eq!(correlated.len(), COMMANDS);
    }

    #[cfg(feature = "video")]
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial(video_preview_global)]
    async fn active_player_excludes_preview_start_play_and_seek_but_allows_pause_and_stop() {
        use futures_util::{SinkExt, StreamExt};

        let song_dir = tempfile::tempdir().unwrap();
        crate::testutil::write_wav(
            song_dir.path().join("long.wav"),
            vec![vec![0.0_f32; 80_000]],
            8_000,
        )
        .unwrap();
        let song_config = crate::config::Song::new(
            "Playing Song",
            None,
            None,
            None,
            None,
            None,
            vec![crate::config::Track::new(
                "audio".to_string(),
                "long.wav",
                None,
            )],
            HashMap::new(),
            Vec::new(),
        );
        let song = Arc::new(crate::songs::Song::new(song_dir.path(), &song_config).unwrap());
        let songs = Arc::new(crate::songs::Songs::new(HashMap::from([(
            "Playing Song".to_string(),
            song,
        )])));
        let (mut state, _state_dir) =
            super::super::api::test_helpers::test_state_with_registry(songs.clone());
        let audio = Arc::new(crate::audio::mock::Device::get("preview-playing"));
        let devices = PlayerDevices {
            audio: Some(audio.clone()),
            mappings: Some(Arc::new(HashMap::from([("audio".to_string(), vec![1])]))),
            midi: None,
            dmx_engine: None,
            sample_engine: None,
            trigger_engine: None,
        };
        let mut playlists = HashMap::new();
        playlists.insert(
            "all_songs".to_string(),
            playlist::from_songs(songs).unwrap(),
        );
        state.player = Arc::new(
            crate::player::Player::new_with_devices(
                devices,
                playlists,
                "all_songs".to_string(),
                None,
            )
            .unwrap(),
        );
        let player = state.player.clone();
        let handle = start(state, "127.0.0.1".to_string(), 0).await.unwrap();
        let ws_url = format!(
            "ws://{}/api/songs/Playing%20Song/video-preview/ws",
            handle.local_addr()
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                r##"{"type":"start","request_id":"paused-start","config":{"canvas":{"width":4,"height":4,"frame_rate":4,"background":"#000000"},"tracks":[]},"position_ms":37,"autoplay":false}"##.into(),
            ))
            .await
            .unwrap();
        let paused = loop {
            let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
                .await
                .expect("paused preview timed out")
                .expect("preview socket closed")
                .unwrap();
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if value["request_id"] == "paused-start" {
                    break value;
                }
            }
        };
        assert_eq!(paused["state"], "paused");
        assert_eq!(paused["epoch"], 1);
        assert_eq!(paused["position_ms"], 37);

        player.play().await.unwrap();
        let active_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !audio.is_playing() && std::time::Instant::now() < active_deadline {
            tokio::task::yield_now().await;
        }
        assert!(audio.is_playing(), "mock audio device never became active");
        assert!(player.is_playing().await);

        for (command, expected_position) in [
            (
                r##"{"type":"start","request_id":"blocked-start","config":{"canvas":{"width":4,"height":4,"frame_rate":4,"background":"#000000"},"tracks":[]},"position_ms":88,"autoplay":false}"##,
                88,
            ),
            (r#"{"type":"play","request_id":"blocked-play"}"#, 37),
            (
                r#"{"type":"seek","request_id":"blocked-seek","position_ms":99}"#,
                37,
            ),
        ] {
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    command.into(),
                ))
                .await
                .unwrap();
            let response: serde_json::Value = loop {
                let message = socket.next().await.unwrap().unwrap();
                if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
                    break serde_json::from_str(&text).unwrap();
                }
            };
            assert_eq!(response["type"], "error");
            assert_eq!(response["epoch"], 1);
            assert_eq!(response["state"], "paused");
            assert_eq!(response["position_ms"], expected_position);
            assert!(response["error"]
                .as_str()
                .unwrap()
                .contains("player is playing"));
            assert!(player.is_playing().await);
            assert!(audio.is_playing());
        }

        for (command, request_id, expected_state) in [
            (
                r#"{"type":"pause","request_id":"allowed-pause"}"#,
                "allowed-pause",
                "paused",
            ),
            (
                r#"{"type":"stop","request_id":"allowed-stop"}"#,
                "allowed-stop",
                "stopped",
            ),
        ] {
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    command.into(),
                ))
                .await
                .unwrap();
            let response: serde_json::Value = loop {
                let message = socket.next().await.unwrap().unwrap();
                if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
                    break serde_json::from_str(&text).unwrap();
                }
            };
            assert_eq!(response["type"], "state", "{response}");
            assert_eq!(response["request_id"], request_id);
            assert_eq!(response["state"], expected_state);
            assert!(player.is_playing().await);
            assert!(audio.is_playing());
        }
        player.stop().await.unwrap();
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    #[serial_test::serial(video_preview_global)]
    async fn video_preview_reports_malformed_and_invalid_start_with_request_ids() {
        use futures_util::{SinkExt, StreamExt};

        let (handle, _, _dir) = start_test_server().await;
        let ws_url = format!(
            "ws://{}/api/songs/Song%20A/video-preview/ws",
            handle.local_addr()
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"type":"play"}"#.into(),
            ))
            .await
            .unwrap();
        let malformed: serde_json::Value = serde_json::from_str(
            &tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(malformed["type"], "error");
        assert_eq!(malformed["request_id"], "protocol");
        assert!(malformed["error"]
            .as_str()
            .unwrap()
            .contains("invalid video preview command"));

        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                r##"{"type":"start","request_id":"invalid-start","config":{"canvas":{"width":0,"height":4,"frame_rate":4,"background":"#000000"},"tracks":[]},"position_ms":77,"autoplay":false}"##.into(),
            ))
            .await
            .unwrap();
        let invalid: serde_json::Value = serde_json::from_str(
            &tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(invalid["type"], "error");
        assert_eq!(invalid["request_id"], "invalid-start");
        assert_eq!(invalid["position_ms"], 77);
        assert!(invalid["error"].as_str().unwrap().contains("width"));
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    #[serial_test::serial(video_preview_global)]
    async fn video_preview_transport_responses_keep_each_request_id() {
        use futures_util::{SinkExt, StreamExt};

        let (handle, _, _dir) = start_test_server().await;
        let ws_url = format!(
            "ws://{}/api/songs/Song%20A/video-preview/ws",
            handle.local_addr()
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
        let commands = [
            (r#"{"type":"play","request_id":"play-id"}"#, "play-id"),
            (r#"{"type":"pause","request_id":"pause-id"}"#, "pause-id"),
            (
                r#"{"type":"seek","request_id":"seek-id","position_ms":321}"#,
                "seek-id",
            ),
            (r#"{"type":"stop","request_id":"stop-id"}"#, "stop-id"),
        ];

        for (command, request_id) in commands {
            socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    command.into(),
                ))
                .await
                .unwrap();
            let response: serde_json::Value = serde_json::from_str(
                &tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .into_text()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(response["request_id"], request_id, "{response}");
            if request_id == "stop-id" {
                assert_eq!(response["type"], "state");
                assert_eq!(response["state"], "stopped");
            } else {
                assert_eq!(response["type"], "error");
                assert!(response["error"]
                    .as_str()
                    .unwrap()
                    .contains("has not been started"));
            }
        }
    }

    #[cfg(feature = "video")]
    #[tokio::test]
    #[serial_test::serial(video_preview_global)]
    async fn second_video_preview_websocket_reports_busy() {
        use futures_util::StreamExt;

        let (handle, _, _dir) = start_test_server().await;
        let ws_url = format!(
            "ws://{}/api/songs/Song%20A/video-preview/ws",
            handle.local_addr()
        );
        let (_first, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
        let (second, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
        let (_, mut read) = second.split();
        let message = read.next().await.unwrap().unwrap().into_text().unwrap();
        let response: serde_json::Value = serde_json::from_str(&message).unwrap();

        assert_eq!(response["type"], "error");
        assert_eq!(response["state"], "busy");
    }

    #[tokio::test]
    async fn server_websocket_receives_waveform() {
        use futures_util::StreamExt;

        let (state, _dir) = test_webui_state();
        // Pre-populate waveform cache for current song "Song A"
        state.waveform_cache.lock().insert(
            "Song A".to_string(),
            vec![("track1".to_string(), vec![0.5, 0.8, 0.3])],
        );
        let handle = start(state, "127.0.0.1".to_string(), 0)
            .await
            .expect("server should start");
        let addr = handle.local_addr();
        let ws_url = format!("ws://{}/ws", addr);

        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("WebSocket connect failed");

        let (_, mut read) = ws_stream.split();

        // First message: metadata
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), read.next())
            .await
            .unwrap();

        // Second message: waveform (from pre-populated cache)
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), read.next())
            .await
            .expect("timed out waiting for waveform message")
            .expect("stream ended")
            .expect("WS error");

        let text = msg.into_text().expect("expected text message");
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["type"], "waveform");
        assert_eq!(parsed["song_name"], "Song A");
        assert!(parsed["tracks"].is_array());
        let tracks = parsed["tracks"].as_array().unwrap();
        assert_eq!(tracks[0]["name"], "track1");
    }

    #[tokio::test]
    async fn server_websocket_receives_metadata() {
        use futures_util::StreamExt;

        let (handle, _, _dir) = start_test_server().await;
        let addr = handle.local_addr();
        let ws_url = format!("ws://{}/ws", addr);

        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("WebSocket connect failed");

        let (_, mut read) = ws_stream.split();

        // First message should be the metadata JSON
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), read.next())
            .await
            .expect("timed out waiting for WS message")
            .expect("stream ended")
            .expect("WS error");

        let text = msg.into_text().expect("expected text message");
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["type"], "metadata");
    }

    #[tokio::test]
    async fn server_websocket_receives_playback() {
        use futures_util::StreamExt;

        let (handle, _, _dir) = start_test_server().await;
        let addr = handle.local_addr();
        let ws_url = format!("ws://{}/ws", addr);

        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("WebSocket connect failed");

        let (_, mut read) = ws_stream.split();

        // Skip the metadata message
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), read.next())
            .await
            .unwrap();

        // Next messages should include a playback update (poller runs at 200ms)
        let msg = tokio::time::timeout(std::time::Duration::from_secs(3), read.next())
            .await
            .expect("timed out waiting for playback message")
            .expect("stream ended")
            .expect("WS error");

        let text = msg.into_text().expect("expected text message");
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        // Could be playback, state, or waveform — just verify it's valid JSON
        assert!(parsed["type"].is_string());
    }
}
