pub mod library;

use axum::{
    routing::{get, post},
    Router,
    Json,
    extract::{State, Path, ws::{WebSocket, WebSocketUpgrade}},
    response::{Html, IntoResponse, Response},
    http::{StatusCode, header},
};
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;
use std::time::Duration;
use std::sync::Arc;
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};
use crate::roon::RoonClient;
use futures_util::StreamExt;
use std::net::IpAddr;

/// mDNS responder handle - keeps the service registered while alive
struct MdnsHandle {
    _responder: libmdns::Responder,
    _service: libmdns::Service,
}

/// Register mDNS service with custom hostname "roon-rd" so roon-rd.local resolves to this machine
fn register_mdns(port: u16, local_ip: &str) -> Option<MdnsHandle> {
    // Parse the local IP to filter mDNS responses to just this IP
    let ip_list: Vec<IpAddr> = local_ip.parse().ok().into_iter().collect();
    
    // Create responder with custom hostname "roon-rd" (will become roon-rd.local)
    // Use spawn_with_ip_list_and_hostname to specify our custom hostname
    let rt = tokio::runtime::Handle::current();
    match libmdns::Responder::spawn_with_ip_list_and_hostname(&rt, ip_list, "roon-rd".to_string()) {
        Ok(responder) => {
            // Register HTTP service - this also advertises the A record for the hostname
            let service = responder.register(
                "_http._tcp",
                "Roon Remote Display",
                port,
                &["path=/"],
            );
            log::info!("Registered mDNS: roon-rd.local:{}", port);
            Some(MdnsHandle {
                _responder: responder,
                _service: service,
            })
        }
        Err(e) => {
            log::warn!("Failed to create mDNS responder: {}", e);
            None
        }
    }
}

/// Get the local IP address of this machine
fn get_local_ip() -> String {
    use std::net::UdpSocket;

    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if let Ok(()) = socket.connect("8.8.8.8:80") {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}


#[derive(Clone)]
pub struct AppState {
    pub roon_client: Arc<Mutex<RoonClient>>,
    pub carousel_interval_ms: u32,
    /// Loaded timed interchange libretto JSON (raw value, served as-is)
    /// Used for single-file --libretto mode (legacy)
    pub libretto_json: Option<Arc<serde_json::Value>>,
    /// Multi-file libretto library (from --libretto-dir)
    pub library: Option<Arc<library::LibrettoLibrary>>,
}

#[derive(Serialize, Deserialize)]
pub struct StatusResponse {
    pub connected: bool,
    pub core_name: Option<String>,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub struct ZoneInfo {
    pub zone_id: String,
    pub display_name: String,
    pub state: String,
    pub devices: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ZonesResponse {
    pub zones: Vec<ZoneInfo>,
    pub count: usize,
}

// NowPlayingInfo removed - now using WsZoneData from roon module
// This eliminates code duplication between WebSocket and HTTP responses

#[derive(Serialize, Deserialize)]
pub struct NowPlayingResponse {
    pub now_playing: Vec<crate::roon::WsZoneData>,
    pub count: usize,
}

#[derive(Serialize, Deserialize)]
pub struct ReconnectResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
pub struct VersionResponse {
    pub version: String,
}

/// Embedded SPA HTML
const SPA_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Roon Remote Display</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }
        /* Hide scrollbars */
        html, body {
            overflow: hidden;
            scrollbar-width: none; /* Firefox */
            -ms-overflow-style: none; /* IE and Edge */
        }
        html::-webkit-scrollbar, body::-webkit-scrollbar {
            display: none; /* Chrome, Safari, Opera */
        }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            color: #eee;
            height: 100vh;
            margin: 0;
            padding: 0;
        }
        .container {
            width: 100%;
            height: 100vh;
            display: flex;
            flex-direction: column;
            padding: 20px;
        }
        .status {
            display: inline-block;
            padding: 0;
            border: none;
            border-radius: 0;
            background: none;
            font-size: 1.15rem;
            font-weight: 500;
        }
        .status.connected {
            color: #2ecc71;
            background: none;
        }
        .status.disconnected {
            color: #e74c3c;
            background: none;
        }
        .reconnect-btn {
            padding: 6px 14px;
            background: #3498db;
            color: #fff;
            border: none;
            border-radius: 6px;
            cursor: pointer;
            font-size: 0.85rem;
            font-weight: 500;
            transition: background 0.2s;
        }
        .reconnect-btn:hover {
            background: #2980b9;
        }
        .reconnect-btn:active {
            background: #1f6ca0;
        }
        .reconnect-btn:disabled {
            background: #95a5a6;
            cursor: not-allowed;
            opacity: 0.6;
        }
        .fullscreen-btn {
            padding: 6px 12px;
            background: rgba(255, 255, 255, 0.1);
            color: #fff;
            border: 1px solid rgba(255, 255, 255, 0.2);
            border-radius: 6px;
            cursor: pointer;
            font-size: 1.2rem;
            line-height: 1;
            transition: all 0.2s;
        }
        .fullscreen-btn:hover {
            background: rgba(255, 255, 255, 0.15);
            border-color: rgba(255, 255, 255, 0.3);
        }
        .fullscreen-btn:active {
            background: rgba(255, 255, 255, 0.2);
        }
        .zone-selector {
            display: flex;
            justify-content: space-between;
            align-items: center;
            gap: 8px;
            flex-wrap: wrap;
            margin-bottom: 20px;
            flex-shrink: 0;
        }
        #zones-container {
            flex: 1;
            overflow-y: auto;
            display: flex;
            flex-direction: column;
        }
        .zone-dropdown {
            display: flex;
            align-items: center;
            gap: 10px;
        }
        .zone-dropdown label {
            font-size: 0.9rem;
            color: #aaa;
        }
        .zone-dropdown select {
            background: rgba(255, 255, 255, 0.1);
            border: 1px solid rgba(255, 255, 255, 0.2);
            color: #fff;
            padding: 8px 16px;
            border-radius: 8px;
            cursor: pointer;
            font-size: 0.9rem;
            outline: none;
            transition: all 0.2s ease;
        }
        .zone-dropdown select:hover {
            background: rgba(255, 255, 255, 0.15);
            border-color: rgba(255, 255, 255, 0.3);
        }
        .zone-dropdown select:focus {
            background: rgba(255, 255, 255, 0.15);
            border-color: #3498db;
        }
        .zone-dropdown select option {
            background: #1a1a2e;
            color: #fff;
        }
        .zone-dropdown select option.version {
            font-size: 0.75rem;
            color: #888;
            font-style: italic;
            margin-top: 1.2em;
            padding-top: 0.5em;
        }
        .zone {
            background: rgba(255, 255, 255, 0.05);
            border-radius: 12px;
            padding: 20px;
            margin-bottom: 20px;
            border: 1px solid rgba(255, 255, 255, 0.1);
            flex-shrink: 0;
            height: calc(100vh - 100px);
            display: flex;
            flex-direction: column;
            justify-content: center;
            position: relative;
        }
        .zone.stopped {
            height: auto;
            min-height: auto;
        }
        .zone-content {
            display: flex;
            gap: 20px;
        }
        .album-art {
            width: calc(100vh - 200px);
            height: calc(100vh - 200px);
            max-width: 50vw;
            max-height: 50vw;
            min-width: 200px;
            min-height: 200px;
            border-radius: 8px;
            object-fit: cover;
            background: #222;
            flex-shrink: 0;
        }
        .album-art-placeholder {
            width: calc(100vh - 200px);
            height: calc(100vh - 200px);
            max-width: 50vw;
            max-height: 50vw;
            min-width: 200px;
            min-height: 200px;
            border-radius: 8px;
            background: linear-gradient(135deg, #333 0%, #222 100%);
            display: flex;
            align-items: center;
            justify-content: center;
            flex-shrink: 0;
        }
        .album-art-placeholder svg {
            width: 40%;
            height: 40%;
            fill: #555;
        }
        .track-details {
            flex: 1;
            min-width: 0;
            display: flex;
            flex-direction: column;
            justify-content: space-between;
            height: calc(100vh - 200px);
            max-height: 50vw;
            min-height: 200px;
            position: relative;
        }
        .track-info {
            margin-bottom: 15px;
        }
        .track-title {
            font-size: 1.1rem;
            font-weight: 500;
            margin-bottom: 5px;
            color: #fff;
        }
        .track-artist {
            font-size: 0.95rem;
            color: #aaa;
            margin-bottom: 3px;
        }
        .track-album {
            font-size: 0.85rem;
            color: #777;
        }
        .track-format {
            font-size: 0.8rem;
            color: #888;
            margin-top: 5px;
        }
        .progress-container {
            margin-top: 15px;
        }
        .progress-bar {
            height: 4px;
            background: #333;
            border-radius: 2px;
            overflow: hidden;
            margin-bottom: 8px;
        }
        .progress-fill {
            height: 100%;
            background: linear-gradient(90deg, #3498db, #2ecc71);
            transition: width 0.5s ease;
        }
        .progress-time {
            display: flex;
            justify-content: space-between;
            font-size: 0.8rem;
            color: #777;
        }
        .current-time {
            font-family: 'Courier New', Courier, monospace;
        }
        .queue-info {
            display: flex;
            justify-content: space-between;
            align-items: center;
            font-size: 0.85rem;
            color: #666;
            margin-top: 2px;
        }
        .queue-count {
            opacity: 0.8;
        }
        .queue-time {
            opacity: 0.8;
            margin-left: auto;
        }
        .queue-time-value {
            font-family: 'Courier New', Courier, monospace;
        }
        .no-playing {
            text-align: center;
            padding: 40px;
            color: #666;
        }
        .stopped-state {
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 10px;
            font-size: 1.5rem;
        }
        .stopped-zone-info {
            display: flex;
            align-items: center;
            gap: 10px;
        }
        .stopped-status {
            color: #888;
            font-size: 1.5rem;
        }
        .loading {
            text-align: center;
            padding: 40px;
            color: #666;
        }
        @keyframes pulse {
            0%, 100% { opacity: 1; }
            50% { opacity: 0.5; }
        }
        .loading-text {
            animation: pulse 1.5s infinite;
        }
        .track-details-top {
            flex: 0 0 auto;
        }
        .artist-images {
            position: relative;
            margin-top: 12px;
            flex: 1;
            min-height: 0;
            overflow: hidden;
            cursor: pointer;
            transition: opacity 0.3s ease-in-out;
        }
        .artist-images.hidden {
            opacity: 0;
        }
        .artist-image {
            position: absolute;
            top: 0;
            left: 0;
            width: 100%;
            height: 100%;
            object-fit: contain;
            border-radius: 4px;
            opacity: 0;
            transition: opacity 0.5s ease-in-out;
        }
        .artist-image.active {
            opacity: 1;
        }
        .zone-controls-container {
            display: flex;
            flex-direction: column;
            align-items: flex-end;
            align-self: flex-end;
            margin-top: 12px;
        }
        .zone-name-label {
            font-weight: 600;
            font-size: 1rem;
            color: #ecf0f1;
            margin-bottom: 8px;
            width: 100%;
            text-align: right;
        }
        .zone-controls {
            display: flex;
            align-items: center;
            gap: 8px;
            min-width: fit-content;
            flex-shrink: 0;
        }
        .zone.stopped .zone-controls {
            display: flex;
            align-items: center;
            justify-content: flex-end;
            gap: 8px;
            margin-top: 15px;
            min-width: fit-content;
            flex-shrink: 0;
        }
        .control-btn {
            background: rgba(255, 255, 255, 0.1);
            border: 1px solid rgba(255, 255, 255, 0.2);
            color: #fff;
            padding: 8px 12px;
            border-radius: 6px;
            cursor: pointer;
            font-size: 0.9rem;
            transition: all 0.2s ease;
            display: flex;
            align-items: center;
            gap: 4px;
            white-space: nowrap;
            flex-shrink: 0;
        }
        .control-btn svg {
            width: 1.5em;
            height: 1.5em;
            flex-shrink: 0;
        }
        .control-btn:hover {
            background: rgba(255, 255, 255, 0.2);
            border-color: rgba(255, 255, 255, 0.3);
        }
        .control-btn:active {
            transform: scale(0.95);
        }
        .control-btn.pause-active {
            color: #4ade80;  /* Green color indicating active playback */
        }
        .control-btn.pause-active:hover {
            color: #86efac;  /* Lighter green on hover */
        }
        .control-btn.play-paused {
            color: #3498db;  /* Blue color indicating paused state */
        }
        .control-btn.play-paused:hover {
            color: #5dade2;  /* Lighter blue on hover */
        }
        .control-btn.play-stopped {
            color: rgba(255, 255, 255, 0.9);  /* White color indicating stopped state */
        }
        .control-btn.play-stopped:hover {
            color: rgba(255, 255, 255, 1);  /* Brighter white on hover */
        }
        .control-btn.muted {
            color: #dc2626;  /* Bright red color indicating muted */
        }
        .control-btn.muted:hover {
            color: #ef4444;  /* Lighter red on hover */
        }
        .progress-bar {
            height: 4px;
            background: #333;
            border-radius: 2px;
            overflow: hidden;
            margin-bottom: 8px;
            cursor: pointer;
            position: relative;
        }
        .progress-bar:hover {
            height: 6px;
        }
        #zones-container.blurred {
            filter: blur(8px);
            pointer-events: none;
        }
        .auth-overlay {
            position: fixed;
            top: 50%;
            left: 50%;
            transform: translate(-50%, -50%);
            background: rgba(0, 0, 0, 0.9);
            border: 2px solid rgba(255, 255, 255, 0.3);
            border-radius: 12px;
            padding: 40px;
            text-align: center;
            z-index: 1000;
            max-width: 500px;
            display: none;
        }
        .auth-overlay.visible {
            display: block;
        }
        .auth-overlay h2 {
            color: #fff;
            margin-bottom: 20px;
            font-size: 1.5rem;
        }
        .auth-overlay p {
            color: #aaa;
            line-height: 1.6;
            margin-bottom: 15px;
        }
        .queue-overlay {
            position: absolute;
            width: 100%;
            height: 100%;
            top: 0;
            left: 0;
            background: rgba(50, 100, 150, 0.2);
            backdrop-filter: blur(10px);
            z-index: 2000;
            display: none;
            flex-direction: column;
            border-radius: 12px;
        }
        .queue-overlay.visible {
            display: flex;
        }
        .queue-header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 15px;
            border-bottom: 1px solid rgba(255, 255, 255, 0.1);
        }
        .queue-title {
            font-size: 1.3rem;
            font-weight: 600;
            color: #fff;
        }
        .queue-close {
            background: rgba(255, 255, 255, 0.1);
            border: 1px solid rgba(255, 255, 255, 0.2);
            color: #fff;
            width: 30px;
            height: 30px;
            border-radius: 6px;
            cursor: pointer;
            font-size: 1.1rem;
            display: flex;
            align-items: center;
            justify-content: center;
            transition: all 0.2s ease;
        }
        .queue-close:hover {
            background: rgba(255, 255, 255, 0.2);
        }
        .queue-content {
            flex: 1;
            overflow-y: auto;
            padding: 15px;
        }
        .queue-item {
            padding: 10px;
            border-bottom: 1px solid rgba(255, 255, 255, 0.05);
            display: flex;
            gap: 10px;
            align-items: center;
        }
        .queue-item:hover {
            background: rgba(255, 255, 255, 0.05);
        }
        .queue-item-index {
            color: #666;
            font-size: 0.75rem;
            min-width: 25px;
        }
        .queue-item-info {
            flex: 1;
        }
        .queue-item-title {
            color: #fff;
            font-weight: 500;
            margin-bottom: 3px;
            font-size: 0.9rem;
        }
        .queue-item-artist {
            color: #aaa;
            font-size: 0.75rem;
        }
        .queue-item-length {
            color: #666;
            font-size: 0.75rem;
        }

        /* Large screen mode - for fullscreen on big monitors */
        @media (min-width: 1500px) {
            .zone-name-label {
                font-size: 1.3rem;
            }
            .track-title {
                font-size: 1.8rem;
            }
            .track-artist {
                font-size: 1.4rem;
            }
            .track-album {
                font-size: 1.2rem;
            }
            .track-format {
                font-size: 1.1rem;
            }
            .control-btn {
                font-size: 1.3rem;
                padding: 12px 18px;
            }
            .progress-time {
                font-size: 1.1rem;
            }
        }
    </style>
</head>
<body>
    <div class="container">
        <nav id="zone-selector" class="zone-selector">
            <div style="display: flex; align-items: center; gap: 8px;">
                <div id="connection-status" class="status disconnected">Connecting...</div>
                <button id="reconnect-btn" class="reconnect-btn" onclick="reconnectToRoon()" style="display: none;">Reconnect to Roon Server</button>
            </div>
            <div style="display: flex; align-items: center; gap: 8px;">
                <button id="fullscreen-btn" class="fullscreen-btn" onclick="toggleFullscreen()" title="Toggle Fullscreen">⛶</button>
                <div class="zone-dropdown">
                    <select id="zone-select">
                        <option value="all">All Zones</option>
                    </select>
                </div>
            </div>
        </nav>
        <main id="zones-container">
            <div class="loading">
                <span class="loading-text">Loading...</span>
            </div>
        </main>
    </div>

    <div id="auth-overlay" class="auth-overlay">
        <h2>Roon Core Not Connected</h2>
        <p>Please authorize this extension in:</p>
        <p><strong>Roon Settings &gt; Extensions &gt; Roon Remote Display</strong></p>
        <p>Once authorized, the connection will be established automatically.</p>
    </div>

    <script>
        let selectedZone = 'all';
        let availableZones = [];  // All zones from /zones
        let nowPlayingZones = [];  // Playing/paused zones from /now-playing
        let timeDisplayMode = {};  // Track time display mode per zone: true = show remaining, false = show total
        let artistImageCarousels = {};  // Track carousel timers per zone
        let artistImagesVisible = {};  // Track artist image visibility per zone: true = visible (default), false = hidden
        const placeholderSvg = '<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path d="M12 3v10.55c-.59-.34-1.27-.55-2-.55-2.21 0-4 1.79-4 4s1.79 4 4 4 4-1.79 4-4V7h4V3h-6z"/></svg>';

        // SVG icons for different zone types
        const zoneIcons = {
            speaker: '<svg viewBox="0 0 24 24" fill="currentColor" style="width: 40px; height: 40px; vertical-align: middle; margin-left: 8px;"><path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z"/></svg>',
            headphones: '<svg viewBox="0 0 24 24" fill="currentColor" style="width: 40px; height: 40px; vertical-align: middle; margin-left: 8px;"><path d="M12 1c-4.97 0-9 4.03-9 9v7c0 1.66 1.34 3 3 3h3v-8H5v-2c0-3.87 3.13-7 7-7s7 3.13 7 7v2h-4v8h3c1.66 0 3-1.34 3-3v-7c0-4.97-4.03-9-9-9z"/></svg>',
            computer: '<svg viewBox="0 0 24 24" fill="currentColor" style="width: 40px; height: 40px; vertical-align: middle; margin-left: 8px;"><path d="M20 18c1.1 0 1.99-.9 1.99-2L22 6c0-1.1-.9-2-2-2H4c-1.1 0-2 .9-2 2v10c0 1.1.9 2 2 2H0v2h24v-2h-4zM4 6h16v10H4V6z"/></svg>',
            phone: '<svg viewBox="0 0 24 24" fill="currentColor" style="width: 40px; height: 40px; vertical-align: middle; margin-left: 8px;"><path d="M17 1.01L7 1c-1.1 0-2 .9-2 2v18c0 1.1.9 2 2 2h10c1.1 0 2-.9 2-2V3c0-1.1-.9-1.99-2-1.99zM17 19H7V5h10v14z"/></svg>',
            network: '<svg viewBox="0 0 24 24" fill="currentColor" style="width: 40px; height: 40px; vertical-align: middle; margin-left: 8px;"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-1 17.93c-3.95-.49-7-3.85-7-7.93 0-.62.08-1.21.21-1.79L9 15v1c0 1.1.9 2 2 2v1.93zm6.9-2.54c-.26-.81-1-1.39-1.9-1.39h-1v-3c0-.55-.45-1-1-1H8v-2h2c.55 0 1-.45 1-1V7h2c1.1 0 2-.9 2-2v-.41c2.93 1.19 5 4.06 5 7.41 0 2.08-.8 3.97-2.1 5.39z"/></svg>',
            dac: '<svg viewBox="0 0 120 80" fill="none" stroke="white" stroke-width="0.8" style="width: 40px; height: 40px; vertical-align: middle; margin-left: 8px;">\
<!-- 3D perspective view: 444mm(w) x 151mm(h) x 435mm(d) with 25° Y-axis and 15° X-axis rotation -->\
<!-- Top surface (rotated down 25°) - more visible -->\
<path d="M 15 30 L 105 30 L 115 10 L 25 10 Z" fill="none" stroke="white" opacity="0.7"/>\
<!-- Right side surface -->\
<path d="M 105 30 L 115 10 L 115 50 L 105 65 Z" fill="none" stroke="white" opacity="0.6"/>\
<!-- Front panel (main face 444:151 ratio) -->\
<rect x="15" y="30" width="90" height="35" rx="0.5" fill="none" stroke="white" stroke-width="1.0"/>\
<!-- Display screen on left (vertically centered) -->\
<rect x="20" y="43.5" width="16" height="8" rx="0.3" fill="none" stroke="white" stroke-width="1.0"/>\
<!-- Rotary control knob on right -->\
<circle cx="92" cy="47.5" r="3.5" fill="none" stroke="white" stroke-width="1.0"/>\
<circle cx="92" cy="47.5" r="1" fill="white" stroke="none"/>\
</svg>',
            oldara: '<svg viewBox="0 0 120 80" fill="none" stroke="white" stroke-width="0.8" style="width: 40px; height: 40px; vertical-align: middle; margin-left: 8px;">\
<!-- 3D perspective view: Oldara Player with curved face plate -->\
<!-- Top surface (rotated down 25°) with curve to meet face plate -->\
<path d="M 15 33.5 Q 60 28 105 33.5 L 115 13.5 Q 70 8 25 13.5 Z" fill="none" stroke="white" opacity="0.7"/>\
<!-- Right side surface with curves -->\
<path d="M 105 33.5 L 115 13.5 L 115 48.5 L 105 61.5" fill="none" stroke="white" opacity="0.6"/>\
<!-- Front panel with curved horizontal lines (top edge) - edges 20% smaller -->\
<path d="M 15 33.5 Q 60 28 105 33.5" fill="none" stroke="white" stroke-width="1.0"/>\
<!-- Front panel with curved horizontal lines (bottom edge) - edges 20% smaller -->\
<path d="M 15 61.5 Q 60 67 105 61.5" fill="none" stroke="white" stroke-width="1.0"/>\
<!-- Vertical edges with more rounding -->\
<path d="M 15 33.5 Q 17 47.5 15 61.5" fill="none" stroke="white" stroke-width="1.0"/>\
<path d="M 105 33.5 Q 103 47.5 105 61.5" fill="none" stroke="white" stroke-width="1.0"/>\
<!-- Round display on right (like dCS knob) -->\
<circle cx="88" cy="47.5" r="5" fill="none" stroke="white" stroke-width="1.0"/>\
<circle cx="88" cy="47.5" r="1.5" fill="white" stroke="none"/>\
</svg>',
            waveform: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="width: 40px; height: 40px; vertical-align: middle; margin-left: 8px;"><path d="M2 12h3l2-6 2 12 2-9 2 6 2-3h5"/></svg>',
            conversion: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" style="width: 40px; height: 40px; vertical-align: middle; margin-left: 8px;"><path d="M3 12h3v-3h2v6h2v-4h2"/><path d="M14 9c0 1.5 1 3 2.5 3s2.5-1.5 2.5-3-1-3-2.5-3-2.5 1.5-2.5 3z" fill="none"/><line x1="12" y1="6" x2="12" y2="15"/></svg>',
            default: '<svg viewBox="0 0 24 24" fill="currentColor" style="width: 40px; height: 40px; vertical-align: middle; margin-left: 8px;"><path d="M12 3v9.28c-.47-.17-.97-.28-1.5-.28C8.01 12 6 14.01 6 16.5S8.01 21 10.5 21c2.31 0 4.2-1.75 4.45-4H15V6h4V3h-7z"/></svg>'
        };

        // Detect zone icon based on zone name and device names
        function getZoneIcon(zoneName, devices) {
            const nameAndDevices = (zoneName + ' ' + (devices || []).join(' ')).toLowerCase();

            // Check for Oldara Player
            if (nameAndDevices.match(/oldara/)) {
                return zoneIcons.oldara;
            }

            // Check for audiophile DAC/high-end equipment patterns
            if (nameAndDevices.match(/dac|dcs|vivaldi|apex|upsampler|rossini|bartok|lina|mosaic|chord|hugo|dave|mscaler|ayre|berkeley|ps audio|directstream|antipodes|lumin|aurender|esoteric|accuphase|meitner|emm labs|weiss|nagra|mytek|benchmark|holo audio|rockna|totaldac/)) {
                return zoneIcons.dac;
            }

            // Check for headphone patterns
            if (nameAndDevices.match(/headphone|earphone|earbud|airpod|beats|sennheiser|akg|audeze/)) {
                return zoneIcons.headphones;
            }

            // Check for computer patterns
            if (nameAndDevices.match(/mac|pc|computer|laptop|desktop|imac|macbook/)) {
                return zoneIcons.computer;
            }

            // Check for phone/tablet patterns
            if (nameAndDevices.match(/phone|iphone|android|ipad|tablet|mobile/)) {
                return zoneIcons.phone;
            }

            // Check for network/streaming patterns
            if (nameAndDevices.match(/roon|chromecast|airplay|sonos|network|stream|bridge/)) {
                return zoneIcons.network;
            }

            // Default to speaker icon
            return zoneIcons.speaker;
        }

        function formatTime(seconds) {
            if (seconds == null) return '0:00';
            const hours = Math.floor(seconds / 3600);
            const mins = Math.floor((seconds % 3600) / 60);
            const secs = Math.floor(seconds % 60);

            if (hours > 0) {
                return `${hours}:${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
            } else {
                return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
            }
        }

        // Determine if queue time should be shown
        // Shows when: multiple queue items OR grouped work (queue time > current track remaining)
        function hasQueueTimeToShow(zone) {
            if (!zone.queue_time_remaining || zone.queue_time_remaining <= 0) return false;
            // Multiple items in queue
            if (zone.queue_items_remaining > 1) return true;
            // Single item but it's a grouped work (queue time exceeds current track remaining by >30s)
            const trackRemaining = (zone.length_seconds || 0) - (zone.position_seconds || 0);
            return zone.queue_time_remaining > trackRemaining + 30;
        }

        function startArtistImageCarousel(zoneId) {
            // Stop existing carousel for this zone if any
            stopArtistImageCarousel(zoneId);

            const container = document.querySelector(`.artist-images[data-zone-id="${zoneId}"]`);
            if (!container) return;

            // Only consider visible images (not hidden due to load errors)
            const getVisibleImages = () => Array.from(container.querySelectorAll('.artist-image')).filter(img => img.style.display !== 'none');
            
            let visibleImages = getVisibleImages();
            if (visibleImages.length <= 1) return; // No need for carousel with 0 or 1 image

            const rotateImage = () => {
                // Re-check visible images in case some failed to load
                visibleImages = getVisibleImages();
                if (visibleImages.length <= 1) {
                    stopArtistImageCarousel(zoneId);
                    return;
                }
                
                // Find current active image index among visible images
                const currentActive = visibleImages.findIndex(img => img.classList.contains('active'));
                if (currentActive >= 0) {
                    visibleImages[currentActive].classList.remove('active');
                }
                
                // Move to next visible image
                const nextIndex = (currentActive + 1) % visibleImages.length;
                visibleImages[nextIndex].classList.add('active');
            };

            // Start the carousel with configurable interval
            const intervalId = setInterval(rotateImage, __CAROUSEL_INTERVAL__);

            // Store the interval ID so we can stop it later
            artistImageCarousels[zoneId] = intervalId;
        }

        function stopArtistImageCarousel(zoneId) {
            if (artistImageCarousels[zoneId]) {
                clearInterval(artistImageCarousels[zoneId]);
                delete artistImageCarousels[zoneId];
            }
        }

        function handleArtistImageError(img) {
            // Hide the failed image
            img.style.display = 'none';
            img.classList.remove('active');
            
            // Find the container and check remaining visible images
            const container = img.closest('.artist-images');
            if (!container) return;
            
            const zoneId = container.dataset.zoneId;
            const visibleImages = Array.from(container.querySelectorAll('.artist-image')).filter(i => i.style.display !== 'none');
            
            // If no visible images remain, hide the container
            if (visibleImages.length === 0) {
                container.style.display = 'none';
                stopArtistImageCarousel(zoneId);
                return;
            }
            
            // If the hidden image was active, activate the first visible one
            if (!container.querySelector('.artist-image.active:not([style*="display: none"])')) {
                visibleImages[0].classList.add('active');
            }
            
            // Restart carousel with remaining images (or stop if only 1 left)
            stopArtistImageCarousel(zoneId);
            if (visibleImages.length > 1 && !container.classList.contains('hidden')) {
                startArtistImageCarousel(zoneId);
            }
        }

        function toggleArtistImages(zoneId) {
            const container = document.querySelector(`.artist-images[data-zone-id="${zoneId}"]`);
            if (!container) return;

            // Toggle visibility state (default is visible/true)
            const isCurrentlyVisible = artistImagesVisible[zoneId] !== false;
            artistImagesVisible[zoneId] = !isCurrentlyVisible;

            if (artistImagesVisible[zoneId]) {
                // Show images
                container.classList.remove('hidden');
                // Restart carousel if multiple images
                const images = container.querySelectorAll('.artist-image');
                if (images.length > 1) {
                    startArtistImageCarousel(zoneId);
                }
            } else {
                // Hide images
                container.classList.add('hidden');
                // Stop carousel when hidden
                stopArtistImageCarousel(zoneId);
            }
        }

        function formatZoneName(zoneName) {
            // Check if zone name has output in parentheses
            const match = zoneName.match(/^(.+?)\s+\((.+)\)$/);
            if (match) {
                const name = match[1];
                const output = match[2];
                return `${name} <span style="font-size: 0.85em;">(${output})</span>`;
            }
            return zoneName;
        }

        function formatTrackTitle(title) {
            if (!title) return '';
            
            // Short titles don't need formatting
            if (title.length < 20) return title;
            
            let mainPart = title;
            let bracketedSuffix = '';
            
            // Case 2: Extract bracketed text at end (e.g., "[2019 Remaster]" or "(Remastered 2020)")
            // Match square brackets containing anything, or parentheses with remaster-related keywords
            const bracketMatch = title.match(/^(.+?)\s*(\[[^\]]+\]|\((?:[^)]*(?:Remaster|Version|Edition|Mix|Recording|Live|Mono|Stereo|Deluxe|Anniversary|Bonus|Original|Digital|Session Demo)[^)]*)\))$/i);
            if (bracketMatch) {
                mainPart = bracketMatch[1].trim();
                bracketedSuffix = bracketMatch[2];
            }
            
            // Case 1a: Opera pattern - break after "Act X:" or "Atto Primo/Secondo/etc:"
            // e.g., "Le nozze di Figaro, K.492 / Act 1:N1. 5. Duettino..."
            // e.g., "Le nozze di Figaro, K. 492: Atto Primo: Se a caso..."
            // e.g., "Le nozze di Figaro, K. 492, Act I: Recit..."
            let formattedMain = mainPart;
            const operaMatch = mainPart.match(/^(.+?[,:\/]\s*(?:Act\s+(?:\d+|[IV]+)|Atto\s+(?:Primo|Secondo|Terzo|Quarto|Quinto)):?)\s*(.+)$/i);
            
            if (operaMatch) {
                const operaAndAct = operaMatch[1].trim();
                const songTitle = operaMatch[2].trim();
                formattedMain = operaAndAct + '<br>' + songTitle;
            }
            // Case 1b: Classical music pattern - "<Piece Name>, <Catalogue Number>: <Movement Number>. <Movement Name>"
            // Break at colon before movement number (Roman or Arabic numerals)
            else {
                const classicalMatch = mainPart.match(/^(.+?:\s*)((?:M{0,3}(?:CM|CD|D?C{0,3})(?:XC|XL|L?X{0,3})(?:IX|IV|V?I{0,3})|\d+)\..*)$/i);
                
                if (classicalMatch) {
                    // Break before the movement number, keeping colon with piece/catalogue
                    const pieceAndCatalogue = classicalMatch[1].trim();
                    const movement = classicalMatch[2].trim();
                    formattedMain = pieceAndCatalogue + '<br>' + movement;
                } else {
                    // Fallback: break at comma if no classical pattern found
                    const commaIndex = mainPart.indexOf(',');
                    if (commaIndex > 20 && commaIndex < mainPart.length - 10) {
                        const beforeComma = mainPart.substring(0, commaIndex + 1).trim();
                        const afterComma = mainPart.substring(commaIndex + 1).trim();
                        formattedMain = beforeComma + '<br>' + afterComma;
                    }
                }
            }
            
            // Add bracketed suffix on its own line if present
            if (bracketedSuffix) {
                return formattedMain + '<br><span style="font-size: 0.85em; opacity: 0.8;">' + bracketedSuffix + '</span>';
            }
            
            return formattedMain;
        }

        function renderZone(zone) {
            const stateClass = zone.state.toLowerCase();

            // Calculate progress and album art
            const progress = zone.length_seconds > 0
                ? ((zone.position_seconds || 0) / zone.length_seconds * 100)
                : 0;

            // Use image endpoint if image_key is available
            const albumArt = zone.image_key
                ? `<img class="album-art" src="/image/${encodeURIComponent(zone.image_key)}" alt="Album Art">`
                : `<div class="album-art-placeholder">${placeholderSvg}</div>`;

            const state = zone.state.toLowerCase();
            const isPlaying = state === 'playing';
            const isPaused = state === 'paused';
            const isStopped = state === 'stopped';

            const playPauseBtn = isPlaying
                ? `<button class="control-btn play-pause-btn pause-active" onclick="sendControl('${zone.zone_id}', 'pause')">
                    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                        <rect x="5" y="3" width="2" height="10"/>
                        <rect x="9" y="3" width="2" height="10"/>
                    </svg>
                </button>`
                : `<button class="control-btn play-pause-btn ${isPaused ? 'play-paused' : 'play-stopped'}" onclick="sendControl('${zone.zone_id}', 'play')">
                    <svg viewBox="0 0 16 16" fill="currentColor" stroke="none">
                        <path d="M5 3l8 5-8 5V3z"/>
                    </svg>
                </button>`;

            // Render mute button based on server state
            const isMuted = zone.is_muted === true;
            const muteBtn = isMuted
                ? `<button class="control-btn muted" id="mute-${zone.zone_id}" onclick="toggleMute('${zone.zone_id}')">
                    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M8 3L5 6H2v4h3l3 3V3z"/>
                        <line x1="11" y1="6" x2="14" y2="9"/>
                        <line x1="14" y1="6" x2="11" y2="9"/>
                    </svg>
                </button>`
                : `<button class="control-btn" id="mute-${zone.zone_id}" onclick="toggleMute('${zone.zone_id}')">
                    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M8 3L5 6H2v4h3l3 3V3z"/>
                        <path d="M11 6.5c.43 1.1.43 2.3 0 3.4"/>
                        <path d="M13.5 4.5c1 2 1 5.5 0 7.5"/>
                    </svg>
                </button>`;

            return `
                <div class="zone${isStopped ? ' stopped' : ''}" data-zone-id="${zone.zone_id}">
                    ${!isStopped ? `
                        <div class="zone-content">
                            ${albumArt}
                            <div class="track-details">
                                <div class="track-details-top">
                                    <div class="track-info">
                                        <div class="track-title">${formatTrackTitle(zone.track)}</div>
                                        ${zone.artist ? `<div class="track-artist">${zone.artist}</div>` : ''}
                                        ${zone.album ? `<div class="track-album">${zone.album}</div>` : ''}
                                        <div class="track-format">${zone.dcs_format || ''}</div>
                                    </div>
                                    <div class="progress-container">
                                        <div class="progress-bar" onclick="handleSeek(event, '${zone.zone_id}', ${zone.length_seconds || 0})" data-length="${zone.length_seconds || 0}">
                                            <div class="progress-fill" style="width: ${progress}%"></div>
                                        </div>
                                        <div class="progress-time">
                                            <span class="current-time">${formatTime(zone.position_seconds)}</span>
                                            <span class="total-time" onclick="toggleTimeDisplay('${zone.zone_id}')" style="cursor: pointer;" title="Click to toggle between total time and time remaining"><span class="queue-time-value">${formatTime(zone.length_seconds)}</span></span>
                                        </div>
                                        <div class="queue-info">
                                            ${zone.queue_items_remaining > 1 ? `<span class="queue-count">${zone.queue_items_remaining - 1} track${zone.queue_items_remaining !== 2 ? 's' : ''} in the queue</span>` : ''}
                                            ${hasQueueTimeToShow(zone) ? `<span class="queue-time"><span class="queue-time-value">${formatTime(zone.queue_time_remaining || 0)}</span> ${zone.queue_items_remaining > 1 ? 'remaining' : 'total remaining'}</span>` : ''}
                                        </div>
                                    </div>
                                </div>
                                ${zone.artist_image_keys && zone.artist_image_keys.length > 0 ? `
                                <div class="artist-images" data-zone-id="${zone.zone_id}" onclick="toggleArtistImages('${zone.zone_id}')">
                                    ${zone.artist_image_keys.map((key, index) => `<img src="/image/${key}" class="artist-image ${index === 0 ? 'active' : ''}" data-index="${index}" alt="Artist" onerror="handleArtistImageError(this)" />`).join('')}
                                </div>
                                ` : ''}
                                <div class="zone-controls-container">
                                    <div class="zone-name-label">${formatZoneName(zone.zone_name)}</div>
                                    <div class="zone-controls">
                                        <button class="control-btn" onclick="showQueue('${zone.zone_id}')">
                                            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                                <rect x="2" y="3" width="12" height="10" rx="1"/>
                                                <line x1="5" y1="6" x2="11" y2="6"/>
                                                <line x1="5" y1="9" x2="11" y2="9"/>
                                            </svg>
                                        </button>
                                        <div style="width: 8px;"></div>
                                        <button class="control-btn" onclick="sendControl('${zone.zone_id}', 'previous')">
                                            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                                <path d="M11 3L5 8l6 5V3z"/>
                                                <line x1="4" y1="3" x2="4" y2="13"/>
                                            </svg>
                                        </button>
                                        ${playPauseBtn}
                                        <button class="control-btn" onclick="sendControl('${zone.zone_id}', 'next')">
                                            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                                <path d="M5 3l6 5-6 5V3z"/>
                                                <line x1="12" y1="3" x2="12" y2="13"/>
                                            </svg>
                                        </button>
                                        ${muteBtn}
                                    </div>
                                </div>
                            </div>
                        </div>
                    ` : `
                        <div class="stopped-state">
                            <div class="stopped-zone-info">
                                <span>${formatZoneName(zone.zone_name)}</span>
                            </div>
                            <div class="stopped-status">Stopped</div>
                        </div>
                    `}
                </div>
            `;
        }

        // Create a new zone DOM element from zone data
        function createZoneElement(zone) {
            const div = document.createElement('div');
            div.innerHTML = renderZone(zone);
            const element = div.firstElementChild;

            // Start carousel for artist images if they exist
            if (zone.artist_image_keys && zone.artist_image_keys.length > 1) {
                // Use setTimeout to ensure DOM is ready
                setTimeout(() => startArtistImageCarousel(zone.zone_id), 0);
            }

            return element;
        }

        // Update an existing zone element with new data
        function updateZoneElement(element, zone) {
            const stateClass = zone.state.toLowerCase();

            // Update zone name label
            const zoneNameLabel = element.querySelector('.zone-name-label');
            if (zoneNameLabel) zoneNameLabel.innerHTML = formatZoneName(zone.zone_name);

            // Check state BEFORE updating the class
            const wasStopped = element.classList.contains('stopped');
            const isStopped = zone.state.toLowerCase() === 'stopped';

            // Update class on zone element
            element.className = isStopped ? 'zone stopped' : 'zone';

            // If zone changed from/to stopped state, need to rebuild content

            if (wasStopped !== isStopped) {
                // State changed between stopped and playing/paused, rebuild the zone
                const newElement = createZoneElement(zone);
                element.replaceWith(newElement);
                return;
            }

            // If still stopped, just update the header (already done above)
            if (isStopped) {
                return;
            }

            // Update playing/paused zone details
            if (zone.track) {
                const trackTitle = element.querySelector('.track-title');
                const trackArtist = element.querySelector('.track-artist');
                const trackAlbum = element.querySelector('.track-album');
                const trackFormat = element.querySelector('.track-format');
                const progressFill = element.querySelector('.progress-fill');
                const progressTimes = element.querySelectorAll('.progress-time span');
                const albumArt = element.querySelector('.album-art, .album-art-placeholder');

                if (trackTitle) trackTitle.innerHTML = formatTrackTitle(zone.track);
                if (trackArtist) trackArtist.textContent = zone.artist || '';
                if (trackAlbum) trackAlbum.textContent = zone.album || '';
                // Only update track format if we have format data
                if (trackFormat && zone.dcs_format) {
                    trackFormat.textContent = zone.dcs_format;
                }

                // Update progress bar
                const progress = zone.length_seconds > 0
                    ? ((zone.position_seconds || 0) / zone.length_seconds * 100)
                    : 0;
                if (progressFill) progressFill.style.width = `${progress}%`;

                // Update time displays
                if (progressTimes.length >= 2) {
                    progressTimes[0].textContent = formatTime(zone.position_seconds);

                    // Update total/remaining time based on toggle mode
                    const showRemaining = timeDisplayMode[zone.zone_id] || false;
                    if (showRemaining) {
                        const remaining = zone.length_seconds - (zone.position_seconds || 0);
                        progressTimes[1].innerHTML = `<span class="queue-time-value">${formatTime(remaining)}</span> remaining`;
                    } else {
                        progressTimes[1].innerHTML = `<span class="queue-time-value">${formatTime(zone.length_seconds)}</span>`;
                    }
                }

                // Update queue info (track count and time remaining)
                // Note: queue_items_remaining INCLUDES current track, so subtract 1 for display
                // queue_time_remaining is total time for all remaining tracks including current
                // Also show time for grouped works (operas/symphonies) even with 1 queue item
                const queueInfo = element.querySelector('.queue-info');
                if (queueInfo) {
                    const upcomingTracks = zone.queue_items_remaining - 1;
                    const showQueueTime = hasQueueTimeToShow(zone);
                    
                    if (upcomingTracks > 0 || showQueueTime) {
                        let queueCount = queueInfo.querySelector('.queue-count');
                        let queueTime = queueInfo.querySelector('.queue-time');
                        
                        // Show track count only if there are multiple queue items
                        if (upcomingTracks > 0) {
                            const countText = `${upcomingTracks} track${upcomingTracks !== 1 ? 's' : ''} in the queue`;
                            if (queueCount) {
                                queueCount.textContent = countText;
                            } else {
                                queueCount = document.createElement('span');
                                queueCount.className = 'queue-count';
                                queueCount.textContent = countText;
                                queueInfo.insertBefore(queueCount, queueInfo.firstChild);
                            }
                        } else if (queueCount) {
                            queueCount.remove();
                        }
                        
                        // Show time remaining (for multiple items or grouped works)
                        if (showQueueTime) {
                            const remainingLabel = upcomingTracks > 0 ? 'remaining' : 'total remaining';
                            const timeText = `<span class="queue-time-value">${formatTime(zone.queue_time_remaining || 0)}</span> ${remainingLabel}`;
                            if (queueTime) {
                                queueTime.innerHTML = timeText;
                            } else {
                                queueTime = document.createElement('span');
                                queueTime.className = 'queue-time';
                                queueTime.innerHTML = timeText;
                                queueInfo.appendChild(queueTime);
                            }
                        } else if (queueTime) {
                            queueTime.remove();
                        }
                    } else {
                        // No queue items and no grouped work, clear the info
                        queueInfo.innerHTML = '';
                    }
                }

                // Update album art if changed
                if (zone.image_key && albumArt) {
                    if (albumArt.tagName === 'IMG') {
                        const currentSrc = albumArt.getAttribute('src');
                        const newSrc = `/image/${encodeURIComponent(zone.image_key)}`;
                        if (currentSrc !== newSrc) {
                            albumArt.setAttribute('src', newSrc);
                        }
                    } else {
                        // Was placeholder, now has image - rebuild
                        const newElement = createZoneElement(zone);
                        element.replaceWith(newElement);
                        return;
                    }
                }

                // Update artist images if changed
                const artistImagesContainer = element.querySelector('.artist-images');
                if (zone.artist_image_keys && zone.artist_image_keys.length > 0) {
                    // Check if artist images changed
                    const currentKeys = artistImagesContainer
                        ? Array.from(artistImagesContainer.querySelectorAll('.artist-image'))
                            .map(img => img.getAttribute('src').split('/').pop())
                        : [];

                    const newKeys = zone.artist_image_keys;
                    const keysChanged = currentKeys.length !== newKeys.length ||
                        !currentKeys.every((key, i) => key === newKeys[i]);

                    if (keysChanged) {
                        // Stop existing carousel
                        stopArtistImageCarousel(zone.zone_id);

                        if (artistImagesContainer) {
                            // Preserve visibility state
                            const wasHidden = artistImagesContainer.classList.contains('hidden');

                            // Update existing container with carousel structure
                            artistImagesContainer.innerHTML = zone.artist_image_keys
                                .map((key, index) => `<img src="/image/${encodeURIComponent(key)}" class="artist-image ${index === 0 ? 'active' : ''}" data-index="${index}" alt="Artist" onerror="handleArtistImageError(this)" />`)
                                .join('');

                            // Restore visibility state
                            if (wasHidden) {
                                artistImagesContainer.classList.add('hidden');
                            }

                            // Start carousel if multiple images and visible
                            if (zone.artist_image_keys.length > 1 && !wasHidden) {
                                setTimeout(() => startArtistImageCarousel(zone.zone_id), 0);
                            }
                        } else {
                            // Container doesn't exist, rebuild zone
                            const newElement = createZoneElement(zone);
                            element.replaceWith(newElement);
                            return;
                        }
                    }
                } else if (artistImagesContainer) {
                    // No artist images but container exists, remove it and stop carousel
                    stopArtistImageCarousel(zone.zone_id);
                    artistImagesContainer.remove();
                }

                // Update play/pause button
                const state = zone.state.toLowerCase();
                const isPlaying = state === 'playing';
                const isPaused = state === 'paused';
                const playPauseBtn = element.querySelector('.play-pause-btn');
                if (playPauseBtn) {
                    if (isPlaying) {
                        playPauseBtn.innerHTML = `
                            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                <rect x="5" y="3" width="2" height="10"/>
                                <rect x="9" y="3" width="2" height="10"/>
                            </svg>`;
                        playPauseBtn.setAttribute('onclick', `sendControl('${zone.zone_id}', 'pause')`);
                        playPauseBtn.classList.add('pause-active');
                        playPauseBtn.classList.remove('play-paused', 'play-stopped');
                    } else {
                        playPauseBtn.innerHTML = `
                            <svg viewBox="0 0 16 16" fill="currentColor" stroke="none">
                                <path d="M5 3l8 5-8 5V3z"/>
                            </svg>`;
                        playPauseBtn.setAttribute('onclick', `sendControl('${zone.zone_id}', 'play')`);
                        playPauseBtn.classList.remove('pause-active');
                        if (isPaused) {
                            playPauseBtn.classList.add('play-paused');
                            playPauseBtn.classList.remove('play-stopped');
                        } else {
                            playPauseBtn.classList.add('play-stopped');
                            playPauseBtn.classList.remove('play-paused');
                        }
                    }
                }

                // Update mute button based on zone.is_muted
                const muteBtn = element.querySelector(`#mute-${zone.zone_id}`);
                if (muteBtn) {
                    const isMuted = zone.is_muted === true;
                    if (isMuted) {
                        muteBtn.classList.add('muted');
                        muteBtn.innerHTML = `
                            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M8 3L5 6H2v4h3l3 3V3z"/>
                                <line x1="11" y1="6" x2="14" y2="9"/>
                                <line x1="14" y1="6" x2="11" y2="9"/>
                            </svg>`;
                    } else {
                        muteBtn.classList.remove('muted');
                        muteBtn.innerHTML = `
                            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M8 3L5 6H2v4h3l3 3V3z"/>
                                <path d="M11 6.5c.43 1.1.43 2.3 0 3.4"/>
                                <path d="M13.5 4.5c1 2 1 5.5 0 7.5"/>
                            </svg>`;
                    }
                }
            }
        }

        function updateZoneSelector(zones) {
            const zoneSelect = document.getElementById('zone-select');

            // Sort zones by display_name
            const sortedZones = zones.sort((a, b) => a.display_name.localeCompare(b.display_name));

            // Build options
            let html = '<option value="all">All Zones</option>';
            for (const zone of sortedZones) {
                html += `<option value="${zone.zone_id}">${zone.display_name}</option>`;
            }
            // Add version
            html += '<option disabled class="version">__VERSION__</option>';
            zoneSelect.innerHTML = html;

            // Set selected value
            zoneSelect.value = selectedZone;

            // Add change handler (remove old one if exists)
            zoneSelect.onchange = () => {
                selectedZone = zoneSelect.value;
                renderZones();
            };
        }

        function renderZones() {
            const container = document.getElementById('zones-container');

            // Use nowPlayingZones directly - it contains ALL zones including stopped ones
            // with complete data including is_muted
            let zonesToShow = nowPlayingZones;
            if (selectedZone !== 'all') {
                zonesToShow = nowPlayingZones.filter(z => z.zone_id === selectedZone);
            }

            // Sort zones: playing/paused first, then stopped
            zonesToShow.sort((a, b) => {
                const aState = a.state.toLowerCase();
                const bState = b.state.toLowerCase();

                // If both are stopped or both are playing/paused, keep original order
                const aIsStopped = aState === 'stopped';
                const bIsStopped = bState === 'stopped';

                if (aIsStopped === bIsStopped) return 0;

                // Playing/paused zones come first
                return aIsStopped ? 1 : -1;
            });

            if (zonesToShow.length === 0) {
                container.innerHTML = '<div class="no-playing">No zones found</div>';
                return;
            }

            // DOM manipulation approach - update existing elements or create new ones
            // Remove loading element if it exists
            const loadingElement = container.querySelector('.loading');
            if (loadingElement) {
                loadingElement.remove();
            }

            // Remove "no zones found" message if it exists
            const noPlayingElement = container.querySelector('.no-playing');
            if (noPlayingElement) {
                noPlayingElement.remove();
            }

            // Get current zone elements
            const existingZones = new Map();
            container.querySelectorAll('.zone').forEach(el => {
                const zoneId = el.getAttribute('data-zone-id');
                if (zoneId) {
                    existingZones.set(zoneId, el);
                }
            });

            // Track which zones we've processed
            const processedZones = new Set();

            // Update or create zones
            zonesToShow.forEach((zone, index) => {
                processedZones.add(zone.zone_id);
                const existingZone = existingZones.get(zone.zone_id);

                if (existingZone) {
                    // Update existing zone element
                    updateZoneElement(existingZone, zone);

                    // If element was replaced (state transition), get the new element
                    // The new element will have the same data-zone-id
                    const currentElement = container.querySelector(`[data-zone-id="${zone.zone_id}"]`);

                    if (currentElement) {
                        // Ensure correct order
                        const currentIndex = Array.from(container.children).indexOf(currentElement);
                        if (currentIndex !== index) {
                            if (index >= container.children.length) {
                                container.appendChild(currentElement);
                            } else {
                                container.insertBefore(currentElement, container.children[index]);
                            }
                        }
                    }
                } else {
                    // Create new zone element
                    const newZone = createZoneElement(zone);
                    if (index >= container.children.length) {
                        container.appendChild(newZone);
                    } else {
                        container.insertBefore(newZone, container.children[index]);
                    }
                }
            });

            // Remove zones that are no longer shown
            existingZones.forEach((element, zoneId) => {
                if (!processedZones.has(zoneId)) {
                    element.remove();
                }
            });
        }

        function updateAuthOverlay(roonConnected) {
            const zonesContainer = document.getElementById('zones-container');
            const authOverlay = document.getElementById('auth-overlay');

            if (!roonConnected) {
                // Show blur and overlay
                zonesContainer.classList.add('blurred');
                authOverlay.classList.add('visible');
            } else {
                // Hide blur and overlay
                zonesContainer.classList.remove('blurred');
                authOverlay.classList.remove('visible');
            }
        }

        async function updateStatus() {
            const statusEl = document.getElementById('connection-status');
            const reconnectBtn = document.getElementById('reconnect-btn');

            // Check WebSocket connection first
            if (!wsConnected) {
                statusEl.className = 'status disconnected';
                statusEl.textContent = 'Can\'t Connect To Server';
                reconnectBtn.style.display = 'none';  // Don't show reconnect button - server is down
                // Don't show auth overlay when server is down - just hide it
                updateAuthOverlay(true);  // Pass true to hide overlay
                return;
            }

            // WebSocket is connected, check Roon Core connection
            try {
                const response = await fetch('/status');
                const data = await response.json();

                if (data.connected) {
                    statusEl.className = 'status connected';
                    statusEl.textContent = data.core_name ? `Connected to Roon Server: ${data.core_name}` : 'Connected';
                    reconnectBtn.style.display = 'none';
                    updateAuthOverlay(true);  // Hide overlay - connected
                } else {
                    statusEl.className = 'status disconnected';
                    statusEl.textContent = 'Roon Core Not Connected';
                    reconnectBtn.style.display = 'inline-block';
                    updateAuthOverlay(false);  // Show overlay - need authorization
                }
            } catch (e) {
                statusEl.className = 'status disconnected';
                reconnectBtn.style.display = 'none';  // Don't show reconnect button - can't reach our server
                statusEl.textContent = 'Can\'t Connect To Server';
                // Don't show auth overlay for connection errors
                updateAuthOverlay(true);  // Pass true to hide overlay
            }
        }

        async function updateZones() {
            try {
                const response = await fetch('/zones');
                const data = await response.json();
                availableZones = data.zones;
                updateZoneSelector(data.zones);
            } catch (e) {
                console.error('Error fetching zones:', e);
            }
        }

        async function updateNowPlaying() {
            try {
                const response = await fetch('/now-playing');
                const data = await response.json();
                nowPlayingZones = data.now_playing;
                renderZones();
            } catch (e) {
                console.error('Error fetching now playing:', e);
            }
        }

        // Update seek position for a specific zone (called by WebSocket)
        function updateSeekPosition(zoneId, seekPosition, queueTimeRemaining) {
            const zoneElement = document.querySelector(`[data-zone-id="${zoneId}"]`);
            if (!zoneElement) return;

            // Update progress bar
            const progressFill = zoneElement.querySelector('.progress-fill');
            const currentTime = zoneElement.querySelector('.current-time');
            const totalTime = zoneElement.querySelector('.total-time');
            const queueTimeElement = zoneElement.querySelector('.queue-time');

            // Find the zone data to get total length
            const zoneData = nowPlayingZones.find(z => z.zone_id === zoneId);
            if (!zoneData || !zoneData.length_seconds) return;

            const position = seekPosition || 0;
            const length = zoneData.length_seconds;
            const percentage = (position / length) * 100;

            if (progressFill) {
                progressFill.style.width = `${percentage}%`;
            }

            if (currentTime) {
                currentTime.textContent = formatTime(position);
            }

            // Update total time display based on toggle mode
            if (totalTime) {
                const showRemaining = timeDisplayMode[zoneId] || false;
                if (showRemaining) {
                    const remaining = length - position;
                    totalTime.innerHTML = `<span class="queue-time-value">${formatTime(remaining)}</span> remaining`;
                } else {
                    totalTime.innerHTML = `<span class="queue-time-value">${formatTime(length)}</span>`;
                }
            }

            // Update queue time remaining (queue_time_remaining already includes current track time)
            // Also update for grouped works (operas/symphonies) even with 1 queue item
            if (queueTimeElement && queueTimeRemaining != null) {
                // Update the cached zone data for hasQueueTimeToShow check
                zoneData.queue_time_remaining = queueTimeRemaining;
                
                if (hasQueueTimeToShow(zoneData)) {
                    const remainingLabel = zoneData.queue_items_remaining > 1 ? 'remaining' : 'total remaining';
                    queueTimeElement.innerHTML = `<span class="queue-time-value">${formatTime(queueTimeRemaining)}</span> ${remainingLabel}`;
                    queueTimeElement.style.display = '';
                } else {
                    queueTimeElement.style.display = 'none';
                }
            }
        }

        // Update queue info display when queue changes (e.g., user clears queue or skips track)
        // Note: queue_items_remaining INCLUDES current track, so subtract 1 for display
        // Also handles grouped works (operas/symphonies) where 1 queue item contains many tracks
        function updateQueueInfo(zoneId, queueItemsRemaining, queueTimeRemaining) {
            const zoneElement = document.querySelector(`[data-zone-id="${zoneId}"]`);
            if (!zoneElement) return;

            const queueInfo = zoneElement.querySelector('.queue-info');
            if (!queueInfo) return;

            // Update the cached zone data
            const zoneData = nowPlayingZones.find(z => z.zone_id === zoneId);
            if (zoneData) {
                zoneData.queue_items_remaining = queueItemsRemaining;
                zoneData.queue_time_remaining = queueTimeRemaining;
            }

            const upcomingTracks = queueItemsRemaining - 1;
            const showQueueTime = zoneData ? hasQueueTimeToShow(zoneData) : (queueTimeRemaining > 0);
            
            if (upcomingTracks > 0 || showQueueTime) {
                let queueCount = queueInfo.querySelector('.queue-count');
                let queueTime = queueInfo.querySelector('.queue-time');

                // Show track count only if there are multiple queue items
                if (upcomingTracks > 0) {
                    const countText = `${upcomingTracks} track${upcomingTracks !== 1 ? 's' : ''} in the queue`;
                    if (queueCount) {
                        queueCount.textContent = countText;
                    } else {
                        queueCount = document.createElement('span');
                        queueCount.className = 'queue-count';
                        queueCount.textContent = countText;
                        queueInfo.insertBefore(queueCount, queueInfo.firstChild);
                    }
                } else if (queueCount) {
                    queueCount.remove();
                }

                // Show time remaining (for multiple items or grouped works)
                if (showQueueTime) {
                    const remainingLabel = upcomingTracks > 0 ? 'remaining' : 'total remaining';
                    const timeText = `<span class="queue-time-value">${formatTime(queueTimeRemaining || 0)}</span> ${remainingLabel}`;
                    if (queueTime) {
                        queueTime.innerHTML = timeText;
                    } else {
                        queueTime = document.createElement('span');
                        queueTime.className = 'queue-time';
                        queueTime.innerHTML = timeText;
                        queueInfo.appendChild(queueTime);
                    }
                } else if (queueTime) {
                    queueTime.remove();
                }
            } else {
                // No queue items and no grouped work, clear the info
                queueInfo.innerHTML = '';
            }
        }

        // Toggle between showing total time and time remaining
        function toggleTimeDisplay(zoneId) {
            // Toggle the mode
            timeDisplayMode[zoneId] = !timeDisplayMode[zoneId];

            // Find the zone element and update the display
            const zoneElement = document.querySelector(`[data-zone-id="${zoneId}"]`);
            if (!zoneElement) return;

            const totalTime = zoneElement.querySelector('.total-time');
            if (!totalTime) return;

            // Find the zone data
            const zoneData = nowPlayingZones.find(z => z.zone_id === zoneId);
            if (!zoneData || !zoneData.length_seconds) return;

            const showRemaining = timeDisplayMode[zoneId];
            if (showRemaining) {
                // Show time remaining
                const remaining = zoneData.length_seconds - (zoneData.position_seconds || 0);
                totalTime.innerHTML = `<span class="queue-time-value">${formatTime(remaining)}</span> remaining`;
            } else {
                // Show total time
                totalTime.innerHTML = `<span class="queue-time-value">${formatTime(zoneData.length_seconds)}</span>`;
            }
        }

        // Reconnect to Roon Server
        async function reconnectToRoon() {
            console.log('Reconnecting to Roon Server...');
            const reconnectBtn = document.getElementById('reconnect-btn');

            // Disable button and show loading state
            if (reconnectBtn) {
                reconnectBtn.disabled = true;
                reconnectBtn.textContent = 'Reconnecting...';
            }

            try {
                // Call the backend reconnect endpoint
                const response = await fetch('/reconnect', {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json',
                    },
                });

                const data = await response.json();
                console.log('Reconnect response:', data);

                if (data.success) {
                    // Wait a moment for the connection to establish
                    setTimeout(() => {
                        updateStatus();
                        if (reconnectBtn) {
                            reconnectBtn.disabled = false;
                            reconnectBtn.textContent = 'Reconnect to Roon Server';
                        }
                    }, 2000);
                } else {
                    console.error('Reconnect failed:', data.message);
                    if (reconnectBtn) {
                        reconnectBtn.disabled = false;
                        reconnectBtn.textContent = 'Reconnect to Roon Server';
                    }
                }
            } catch (e) {
                console.error('Error reconnecting:', e);
                if (reconnectBtn) {
                    reconnectBtn.disabled = false;
                    reconnectBtn.textContent = 'Reconnect to Roon Server';
                }
            }
        }

        // Toggle fullscreen mode
        function toggleFullscreen() {
            if (!document.fullscreenElement) {
                // Enter fullscreen
                document.documentElement.requestFullscreen().catch(err => {
                    console.error('Error attempting to enable fullscreen:', err);
                });
            } else {
                // Exit fullscreen
                if (document.exitFullscreen) {
                    document.exitFullscreen();
                }
            }
        }

        // Track mute state for each zone
        const muteState = {};

        async function sendControl(zoneId, control) {
            try {
                const response = await fetch(`/control/${encodeURIComponent(zoneId)}`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ control })
                });
                if (!response.ok) {
                    console.error('Control command failed:', await response.text());
                }
            } catch (e) {
                console.error('Error sending control:', e);
            }
        }

        async function toggleMute(zoneId) {
            // Get current mute state from zone data
            const zoneData = nowPlayingZones.find(z => z.zone_id === zoneId);
            const isMuted = zoneData?.is_muted || false;
            const newMuteState = !isMuted;

            try {
                const response = await fetch(`/mute/${encodeURIComponent(zoneId)}`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ mute: newMuteState })
                });
                if (response.ok) {
                    // Update local zone data immediately for responsiveness
                    if (zoneData) {
                        zoneData.is_muted = newMuteState;
                    }

                    // Update the mute button to reflect new state
                    const muteBtn = document.getElementById(`mute-${zoneId}`);
                    if (muteBtn) {
                        if (newMuteState) {
                            muteBtn.classList.add('muted');
                            // Muted icon with X
                            muteBtn.innerHTML = `
                                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M8 3L5 6H2v4h3l3 3V3z"/>
                                    <line x1="11" y1="6" x2="14" y2="9"/>
                                    <line x1="14" y1="6" x2="11" y2="9"/>
                                </svg>`;
                        } else {
                            muteBtn.classList.remove('muted');
                            // Unmuted icon with sound waves
                            muteBtn.innerHTML = `
                                <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M8 3L5 6H2v4h3l3 3V3z"/>
                                    <path d="M11 6.5c.43 1.1.43 2.3 0 3.4"/>
                                    <path d="M13.5 4.5c1 2 1 5.5 0 7.5"/>
                                </svg>`;
                        }
                    }
                } else {
                    console.error('Mute command failed:', await response.text());
                }
            } catch (e) {
                console.error('Error toggling mute:', e);
            }
        }

        function handleSeek(event, zoneId, lengthSeconds) {
            const progressBar = event.currentTarget;
            const rect = progressBar.getBoundingClientRect();
            const clickX = event.clientX - rect.left;
            const percentage = clickX / rect.width;
            const seekSeconds = Math.floor(percentage * lengthSeconds);

            fetch(`/seek/${encodeURIComponent(zoneId)}`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ seconds: seekSeconds })
            }).catch(e => console.error('Error seeking:', e));
        }

        async function showQueue(zoneId) {
            try {
                const response = await fetch(`/queue/${encodeURIComponent(zoneId)}`);
                const data = await response.json();

                // Find the zone element
                const zoneElement = document.querySelector(`[data-zone-id="${zoneId}"]`);
                if (!zoneElement) return;

                // Remove any existing overlay in this zone
                const existingOverlay = zoneElement.querySelector('.queue-overlay');
                if (existingOverlay) {
                    existingOverlay.remove();
                }

                // Build queue items HTML
                let queueItemsHtml;
                if (data.items && data.items.length > 0) {
                    queueItemsHtml = data.items.map((item, index) => `
                        <div class="queue-item" ondblclick="playFromQueue('${zoneId}', ${item.queue_item_id})" style="cursor: pointer;">
                            <div class="queue-item-index">${index + 1}</div>
                            <div class="queue-item-info">
                                <div class="queue-item-title">${item.title}</div>
                                ${item.artist ? `<div class="queue-item-artist">${item.artist}</div>` : ''}
                            </div>
                            <div class="queue-item-length">${formatTime(item.length)}</div>
                        </div>
                    `).join('');
                } else {
                    queueItemsHtml = '<div style="padding: 20px; text-align: center; color: #666;">Queue is empty</div>';
                }

                // Create and insert the overlay
                const overlayHtml = `
                    <div class="queue-overlay visible">
                        <div class="queue-header">
                            <div class="queue-title">Queue</div>
                            <button class="queue-close" onclick="hideQueue('${zoneId}')">×</button>
                        </div>
                        <div class="queue-content">
                            ${queueItemsHtml}
                        </div>
                    </div>
                `;

                // Insert overlay into track-details instead of zone
                const trackDetails = zoneElement.querySelector('.track-details');
                if (trackDetails) {
                    trackDetails.insertAdjacentHTML('beforeend', overlayHtml);
                }
            } catch (e) {
                console.error('Error loading queue:', e);
            }
        }

        function hideQueue(zoneId) {
            const zoneElement = document.querySelector(`[data-zone-id="${zoneId}"]`);
            if (zoneElement) {
                const overlay = zoneElement.querySelector('.queue-overlay');
                if (overlay) {
                    overlay.remove();
                }
            }
        }

        async function playFromQueue(zoneId, queueItemId) {
            try {
                const response = await fetch(`/play-from-queue/${encodeURIComponent(zoneId)}`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ queue_item_id: queueItemId })
                });

                if (response.ok) {
                    console.log('Playing from queue item:', queueItemId);
                    // Close the queue overlay after selecting a track
                    hideQueue(zoneId);
                } else {
                    console.error('Play from queue failed:', await response.text());
                }
            } catch (e) {
                console.error('Error playing from queue:', e);
            }
        }

        async function refreshQueueIfOpen(zoneId) {
            // Check if queue popup is currently open for this zone
            const zoneElement = document.querySelector(`[data-zone-id="${zoneId}"]`);
            if (!zoneElement) return;

            const existingOverlay = zoneElement.querySelector('.queue-overlay');
            if (!existingOverlay) return; // Queue not open for this zone

            console.log('Refreshing queue for zone:', zoneId);

            try {
                // Fetch updated queue data
                const response = await fetch(`/queue/${encodeURIComponent(zoneId)}`);
                const data = await response.json();

                // Update the queue content
                const queueContent = existingOverlay.querySelector('.queue-content');
                if (queueContent) {
                    let queueItemsHtml;
                    if (data.items && data.items.length > 0) {
                        queueItemsHtml = data.items.map((item, index) => `
                            <div class="queue-item" ondblclick="playFromQueue('${zoneId}', ${item.queue_item_id})" style="cursor: pointer;">
                                <div class="queue-item-index">${index + 1}</div>
                                <div class="queue-item-info">
                                    <div class="queue-item-title">${item.title}</div>
                                    ${item.artist ? `<div class="queue-item-artist">${item.artist}</div>` : ''}
                                </div>
                                <div class="queue-item-length">${formatTime(item.length)}</div>
                            </div>
                        `).join('');
                    } else {
                        queueItemsHtml = '<div style="padding: 20px; text-align: center; color: #666;">Queue is empty</div>';
                    }
                    queueContent.innerHTML = queueItemsHtml;
                }
            } catch (e) {
                console.error('Error refreshing queue:', e);
            }
        }

        // WebSocket connection for real-time updates
        let ws = null;
        let reconnectTimeout = null;
        let wsConnected = false;

        function connectWebSocket() {
            const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
            const wsUrl = `${protocol}//${window.location.host}/ws`;

            console.log('Connecting to WebSocket:', wsUrl);
            ws = new WebSocket(wsUrl);

            ws.onopen = () => {
                console.log('WebSocket connected');
                wsConnected = true;
                // Initial data load
                updateStatus();
                updateZones();
                updateNowPlaying();
            };

            ws.onmessage = (event) => {
                try {
                    const msg = JSON.parse(event.data);
                    console.log('WebSocket message:', msg);

                    if (msg.type === 'zones_changed') {
                        // Zone change message now includes full zone data
                        if (msg.now_playing) {
                            nowPlayingZones = msg.now_playing;
                            renderZones();
                        }
                        // Still update zones list (for zone selector)
                        updateZones();
                    } else if (msg.type === 'connection_changed') {
                        // Update connection status
                        updateStatus();
                        if (msg.connected) {
                            // When reconnected, refresh all data
                            updateZones();
                            updateNowPlaying();
                        }
                    } else if (msg.type === 'seek_updated') {
                        // Only update seek position for this specific zone
                        updateSeekPosition(msg.zone_id, msg.seek_position, msg.queue_time_remaining);
                    } else if (msg.type === 'queue_changed') {
                        // Queue has changed - update queue info display and refresh popup if open
                        updateQueueInfo(msg.zone_id, msg.queue_items_remaining, msg.queue_time_remaining);
                        refreshQueueIfOpen(msg.zone_id);
                    }
                } catch (e) {
                    console.error('Error parsing WebSocket message:', e);
                }
            };

            ws.onerror = (error) => {
                console.error('WebSocket error:', error);
            };

            ws.onclose = () => {
                console.log('WebSocket disconnected, reconnecting in 2 seconds...');
                ws = null;
                wsConnected = false;
                // Update status to show connection error
                updateStatus();
                // Reconnect after 2 seconds
                reconnectTimeout = setTimeout(connectWebSocket, 2000);
            };
        }

        // Start WebSocket connection
        connectWebSocket();

        // Cleanup on page unload
        window.addEventListener('beforeunload', () => {
            if (reconnectTimeout) clearTimeout(reconnectTimeout);
            if (ws) ws.close();
        });
    </script>
</body>
</html>
"#;

/// WebSocket handler
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

/// Handle individual WebSocket connection
async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let client = state.roon_client.lock().await;
    let mut rx = client.subscribe_ws();
    drop(client);  // Release the lock

    // Send initial state
    let client = state.roon_client.lock().await;
    let connected = client.is_connected().await;
    drop(client);

    let init_msg = crate::roon::WsMessage::ConnectionChanged { connected };
    if let Ok(json) = serde_json::to_string(&init_msg) {
        let _ = socket.send(axum::extract::ws::Message::Text(json)).await;
    }

    // Per-connection state: last libretto match key per zone (file_id:track_index or "none")
    // Used to deduplicate — only send LibrettoUpdate when the match actually changes
    let mut last_match: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    // Handle incoming and outgoing messages
    loop {
        tokio::select! {
            // Receive updates from broadcast channel
            result = rx.recv() => {
                match result {
                    Ok(msg) => {
                        // Check for libretto updates on ZonesChanged
                        if let crate::roon::WsMessage::ZonesChanged { ref now_playing, .. } = msg {
                            if let Some(ref lib) = state.library {
                                for zone in now_playing {
                                    let content = lib.match_and_extract(
                                        zone.album.as_deref(),
                                        zone.track.as_deref(),
                                        None, // disc_number not in WsZoneData yet
                                        None, // track_number not in WsZoneData yet
                                    );

                                    // Build a dedup key: "file_id:track_index" or "none"
                                    let match_key = match &content {
                                        Some(c) => format!("{}:{}", c.file_id, c.track_index),
                                        None => "none".to_string(),
                                    };

                                    let prev = last_match.get(&zone.zone_id);
                                    if prev.map(|p| p.as_str()) != Some(&match_key) {
                                        last_match.insert(zone.zone_id.clone(), match_key);

                                        let update = crate::roon::WsMessage::LibrettoUpdate {
                                            zone_id: zone.zone_id.clone(),
                                            content,
                                        };
                                        if let Ok(json) = serde_json::to_string(&update) {
                                            if socket.send(axum::extract::ws::Message::Text(json)).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Always forward the original message (ZonesChanged, SeekUpdated, etc.)
                        if let Ok(json) = serde_json::to_string(&msg) {
                            if socket.send(axum::extract::ws::Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            // Handle incoming WebSocket messages (ping/pong/close)
            result = socket.next() => {
                match result {
                    Some(Ok(msg)) => {
                        match msg {
                            axum::extract::ws::Message::Close(_) => break,
                            _ => {}  // Ignore other message types
                        }
                    }
                    _ => break,
                }
            }
        }
    }
}

/// Embedded libretto viewer HTML — a dedicated page for synchronized libretto display
const LIBRETTO_VIEW_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Libretto — Roon Remote Display</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: 'Georgia', 'Times New Roman', serif;
            background: #1a1a2e;
            color: #e0e0e0;
            min-height: 100vh;
        }
        #header {
            position: sticky;
            top: 0;
            z-index: 100;
            background: linear-gradient(180deg, #1a1a2e 85%, transparent);
            padding: 16px 24px 28px;
            border-bottom: 1px solid #2a2a4e;
        }
        #libretto-meta {
            font-size: 13px;
            color: #666;
            margin-bottom: 8px;
        }
        #libretto-meta .album { color: #888; }
        #libretto-meta .artist { color: #777; }
        #roon-track {
            font-size: 22px;
            color: #e0e0e0;
            font-weight: bold;
            margin-bottom: 4px;
        }
        #matched-track {
            font-size: 15px;
            color: #c9a84c;
            margin-bottom: 6px;
        }
        #matched-track .label { color: #666; font-size: 12px; }
        #time-display {
            font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
            font-size: 16px;
            color: #aaa;
            margin-bottom: 4px;
        }
        #time-display .current { color: #e0e0e0; font-weight: bold; }
        #time-display .total { color: #666; }
        #time-display .seg-time { color: #c9a84c; }
        #progress-bar {
            margin-top: 6px;
            height: 4px;
            background: #333;
            border-radius: 2px;
            overflow: hidden;
        }
        #progress-fill {
            height: 100%;
            background: #c9a84c;
            width: 0%;
            transition: width 0.3s linear;
        }
        #status {
            font-size: 11px;
            color: #555;
            margin-top: 6px;
        }
        #libretto {
            padding: 8px 24px 40vh;
        }
        .segment {
            display: flex;
            gap: 12px;
            padding: 6px 12px;
            border-radius: 6px;
            transition: background 0.3s, opacity 0.3s;
            opacity: 0.4;
            margin-bottom: 2px;
            align-items: baseline;
        }
        .segment.active {
            background: rgba(201, 168, 76, 0.15);
            opacity: 1.0;
        }
        .segment.past {
            opacity: 0.6;
        }
        .segment.future {
            opacity: 0.35;
        }
        .segment .seg-start {
            min-width: 52px;
            font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
            font-size: 11px;
            color: #555;
            padding-top: 4px;
            text-align: right;
            flex-shrink: 0;
        }
        .segment.active .seg-start {
            color: #c9a84c;
        }
        .segment .character {
            min-width: 120px;
            font-size: 13px;
            font-weight: 600;
            text-transform: uppercase;
            letter-spacing: 0.5px;
            color: #c9a84c;
            padding-top: 3px;
            text-align: right;
            flex-shrink: 0;
        }
        .segment .texts {
            flex: 1;
            display: flex;
            gap: 24px;
        }
        .segment .text {
            flex: 1;
            font-size: 18px;
            line-height: 1.5;
            color: #e8e8e8;
            white-space: pre-wrap;
        }
        .segment .translation {
            flex: 1;
            font-size: 15px;
            line-height: 1.5;
            color: #888;
            font-style: italic;
            white-space: pre-wrap;
        }
        .segment .direction-text {
            flex: 1 1 100%;
            font-size: 14px;
            line-height: 1.4;
            color: #9a8a5c;
            font-style: italic;
            white-space: pre-wrap;
        }
        .segment.active .text {
            color: #fff;
            font-size: 19px;
        }
        .segment.active .translation {
            color: #bbb;
            font-size: 16px;
        }
        .segment.active .character {
            color: #dfc06e;
        }
        #no-track {
            text-align: center;
            padding: 80px 24px;
            color: #666;
            font-size: 18px;
        }
        .ensemble-group {
            border-left: 3px solid rgba(201, 168, 76, 0.25);
            margin-left: 52px;
            padding-left: 8px;
            margin-bottom: 4px;
            border-radius: 0 6px 6px 0;
        }
        .ensemble-group.active {
            border-left-color: rgba(201, 168, 76, 0.7);
            background: rgba(201, 168, 76, 0.06);
        }
        .ensemble-group.past {
            border-left-color: rgba(201, 168, 76, 0.15);
        }
        .ensemble-group .segment {
            margin-bottom: 0;
        }
        .ensemble-group .segment .seg-start.group-cont {
            visibility: hidden;
        }
        @media (max-width: 600px) {
            .segment { flex-direction: column; gap: 2px; }
            .segment .texts { flex-direction: column; gap: 4px; }
            .segment .character { text-align: left; min-width: auto; }
            .segment .seg-start { text-align: left; min-width: auto; }
            #libretto { padding: 8px 12px 40vh; }
            .ensemble-group { margin-left: 0; }
        }
    </style>
</head>
<body>
    <div id="header">
        <div id="libretto-meta">Loading libretto...</div>
        <div id="roon-track">Connecting...</div>
        <div id="matched-track"></div>
        <div id="time-display"></div>
        <div id="progress-bar"><div id="progress-fill"></div></div>
        <div id="status"></div>
    </div>
    <div id="libretto">
        <div id="no-track">Waiting for playback...</div>
    </div>

    <script>
    // State — the SPA is a pure renderer; all matching happens server-side
    let currentContent = null;   // LibrettoContent from server (segments, metadata)
    let currentSegmentIndex = -1;
    let seekPosition = 0;
    let trackLength = 0;
    let autoScroll = true;
    // Maps segment index → { start, end } of its group range, or null if ungrouped
    let segmentGroupMap = [];

    function formatTime(secs) {
        if (secs == null || isNaN(secs)) return '--:--';
        const s = Math.floor(secs);
        const m = Math.floor(s / 60);
        const ss = s % 60;
        return `${m}:${ss.toString().padStart(2, '0')}`;
    }

    // Find the active segment index for a given seek position
    function findSegmentIndex(segments, position) {
        if (!segments || segments.length === 0) return -1;
        let idx = -1;
        for (let i = 0; i < segments.length; i++) {
            if (segments[i].start <= position) {
                idx = i;
            } else {
                break;
            }
        }
        return idx;
    }

    // Build group ranges: find consecutive runs of segments sharing the same group tag
    function buildGroupMap(segments) {
        const map = new Array(segments.length).fill(null);
        let i = 0;
        while (i < segments.length) {
            const g = segments[i].group;
            if (g) {
                const start = i;
                while (i < segments.length && segments[i].group === g) i++;
                const end = i - 1;
                if (end > start) { // Only treat as group if 2+ segments
                    for (let j = start; j <= end; j++) {
                        map[j] = { start, end };
                    }
                }
            } else {
                i++;
            }
        }
        return map;
    }

    // Render a single segment's inner HTML
    function renderSegmentInner(seg, index, isGroupCont) {
        const startStr = formatTime(seg.start);
        const startClass = isGroupCont ? 'seg-start group-cont' : 'seg-start';
        let html = `<div class="${startClass}">${startStr}</div>`;
        html += `<div class="character">${escapeHtml(seg.character || '')}</div>`;
        html += `<div class="texts">`;
        if (seg.direction) {
            html += `<div class="direction-text">${escapeHtml(seg.direction)}</div>`;
        } else {
            html += `<div class="text">${escapeHtml(seg.text || '')}</div>`;
            html += `<div class="translation">${escapeHtml(seg.translation || '')}</div>`;
        }
        html += `</div>`;
        return html;
    }

    // Render the libretto segments
    function renderSegments(segments) {
        const container = document.getElementById('libretto');
        if (!segments || segments.length === 0) {
            container.innerHTML = '<div id="no-track">No libretto segments for this track</div>';
            segmentGroupMap = [];
            return;
        }

        segmentGroupMap = buildGroupMap(segments);

        let html = '';
        let i = 0;
        while (i < segments.length) {
            const grp = segmentGroupMap[i];
            if (grp && i === grp.start) {
                // Render ensemble group container
                html += `<div class="ensemble-group future" id="group-${grp.start}">`;
                for (let j = grp.start; j <= grp.end; j++) {
                    const seg = segments[j];
                    const isGroupCont = j > grp.start;
                    html += `<div class="segment future" data-index="${j}" id="seg-${j}">`;
                    html += renderSegmentInner(seg, j, isGroupCont);
                    html += `</div>`;
                }
                html += `</div>`;
                i = grp.end + 1;
            } else {
                const seg = segments[i];
                html += `<div class="segment future" data-index="${i}" id="seg-${i}">`;
                html += renderSegmentInner(seg, i, false);
                html += `</div>`;
                i++;
            }
        }
        container.innerHTML = html;
    }

    function escapeHtml(text) {
        return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    }

    // Update time display
    function updateTimeDisplay(position) {
        const timeEl = document.getElementById('time-display');
        let html = `<span class="current">${formatTime(position)}</span>`;
        html += ` <span class="total">/ ${formatTime(trackLength)}</span>`;
        const segs = currentContent?.segments;
        if (segs && currentSegmentIndex >= 0 && currentSegmentIndex < segs.length) {
            const seg = segs[currentSegmentIndex];
            const nextSeg = segs[currentSegmentIndex + 1];
            html += `  &mdash;  <span class="seg-time">seg ${currentSegmentIndex + 1}/${segs.length}`;
            html += ` @ ${formatTime(seg.start)}`;
            if (nextSeg) html += ` &rarr; ${formatTime(nextSeg.start)}`;
            html += `</span>`;
        }
        timeEl.innerHTML = html;
    }

    // Update segment highlighting based on seek position
    function updateHighlight(position) {
        if (!currentContent) return;

        updateTimeDisplay(position);

        const newIndex = findSegmentIndex(currentContent.segments, position);
        if (newIndex === currentSegmentIndex) return;

        currentSegmentIndex = newIndex;

        // Determine the active range (expanded to full group if in a group)
        const grp = segmentGroupMap[newIndex];
        const activeStart = grp ? grp.start : newIndex;
        const activeEnd = grp ? grp.end : newIndex;

        // Update individual segment classes
        const segments = document.querySelectorAll('.segment[data-index]');
        segments.forEach((el) => {
            const i = parseInt(el.dataset.index);
            el.classList.remove('active', 'past', 'future');
            if (i < activeStart) el.classList.add('past');
            else if (i >= activeStart && i <= activeEnd) el.classList.add('active');
            else el.classList.add('future');
        });

        // Update ensemble group container classes
        document.querySelectorAll('.ensemble-group').forEach(el => {
            el.classList.remove('active', 'past', 'future');
            // Determine group state from its first child segment
            const firstSeg = el.querySelector('.segment[data-index]');
            if (firstSeg) {
                const gi = parseInt(firstSeg.dataset.index);
                const gInfo = segmentGroupMap[gi];
                if (gInfo) {
                    if (gInfo.end < activeStart) el.classList.add('past');
                    else if (gInfo.start > activeEnd) el.classList.add('future');
                    else el.classList.add('active');
                }
            }
        });

        // Auto-scroll: target the group container if in a group, else the segment
        if (autoScroll && newIndex >= 0) {
            const scrollTarget = grp
                ? document.getElementById(`group-${grp.start}`)
                : document.getElementById(`seg-${newIndex}`);
            if (scrollTarget) {
                scrollTarget.scrollIntoView({ behavior: 'smooth', block: 'center' });
            }
        }

        // Update progress bar
        if (trackLength > 0) {
            const pct = (position / trackLength) * 100;
            document.getElementById('progress-fill').style.width = `${pct}%`;
        }
    }

    // Handle zone data from WebSocket — only updates track title and position
    function handleZoneUpdate(zones) {
        // Find first playing zone with a track
        const playing = zones.find(z => z.state === 'Playing' && z.track);
        if (!playing) {
            document.getElementById('roon-track').textContent = 'Paused or stopped';
            return;
        }

        document.getElementById('roon-track').textContent = playing.track;
        trackLength = playing.length_seconds || trackLength;

        if (playing.position_seconds != null) {
            seekPosition = playing.position_seconds;
            updateHighlight(seekPosition);
        }
    }

    // Handle libretto content update from server
    function handleLibrettoUpdate(msg) {
        const content = msg.content;

        if (!content) {
            // No match for this zone
            currentContent = null;
            currentSegmentIndex = -1;
            document.getElementById('matched-track').innerHTML =
                `<span class="label">No libretto match for current track</span>`;
            document.getElementById('libretto-meta').textContent = '';
            document.getElementById('libretto').innerHTML =
                '<div id="no-track">No libretto match for this track</div>';
            return;
        }

        // Update metadata display
        const opera = content.opera_title || '';
        const album = content.album || '';
        const artist = content.artist || '';
        let metaHtml = '';
        if (opera) metaHtml += `<span class="album">${escapeHtml(opera)}</span>`;
        if (album && album !== opera) metaHtml += ` &mdash; <span class="album">${escapeHtml(album)}</span>`;
        if (artist) metaHtml += ` &mdash; <span class="artist">${escapeHtml(artist)}</span>`;
        document.getElementById('libretto-meta').innerHTML = metaHtml;

        // Update matched track display
        const method = content.match_method || 'server';
        const segCount = content.segments?.length || 0;
        document.getElementById('matched-track').innerHTML =
            `<span class="label">Matched (${method}):</span> ${escapeHtml(content.track_title)} (${segCount} segments)`;

        // Store and render
        currentContent = content;
        currentSegmentIndex = -1;
        renderSegments(content.segments);

        // Apply current seek position
        if (seekPosition > 0) {
            updateHighlight(seekPosition);
        }
    }

    // Connect to WebSocket for real-time updates
    function connectWebSocket() {
        const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
        const ws = new WebSocket(`${protocol}//${location.host}/ws`);

        ws.onopen = () => {
            document.getElementById('status').textContent = 'Connected';
        };

        ws.onmessage = (event) => {
            try {
                const msg = JSON.parse(event.data);
                if (msg.type === 'zones_changed') {
                    handleZoneUpdate(msg.now_playing || []);
                } else if (msg.type === 'libretto_update') {
                    handleLibrettoUpdate(msg);
                } else if (msg.type === 'seek_updated') {
                    seekPosition = msg.seek_position || 0;
                    updateHighlight(seekPosition);
                } else if (msg.type === 'connection_changed') {
                    document.getElementById('status').textContent =
                        msg.connected ? 'Connected to Roon' : 'Roon disconnected';
                }
            } catch (e) {
                console.error('WS parse error:', e);
            }
        };

        ws.onclose = () => {
            document.getElementById('status').textContent = 'Disconnected — reconnecting...';
            setTimeout(connectWebSocket, 3000);
        };

        ws.onerror = () => ws.close();
    }

    // Disable auto-scroll on manual scroll, re-enable after 5s idle
    let scrollTimer = null;
    document.addEventListener('scroll', () => {
        autoScroll = false;
        clearTimeout(scrollTimer);
        scrollTimer = setTimeout(() => { autoScroll = true; }, 5000);
    }, { passive: true });

    // Init
    connectWebSocket();
    </script>
</body>
</html>
"##;

// Route documentation - keep this in sync with the actual routes
const ROUTES: &[(&str, &str, &str)] = &[
    ("GET", "/", "Serve the web UI (SPA)"),
    ("WS", "/ws", "WebSocket connection for real-time updates"),
    ("GET", "/status", "Get Roon connection status (JSON)"),
    ("GET", "/version", "Get server version (JSON)"),
    ("POST", "/reconnect", "Reconnect to Roon Core"),
    ("GET", "/zones", "Get available Roon zones (JSON)"),
    ("GET", "/now-playing", "Get currently playing tracks (JSON)"),
    ("GET", "/queue/:zone_id", "Get queue for a specific zone (JSON)"),
    ("GET", "/image/:image_key", "Get album art image"),
    ("POST", "/control/:zone_id", "Control playback (play/pause/stop)"),
    ("POST", "/seek/:zone_id", "Seek to position in current track"),
    ("POST", "/mute/:zone_id", "Toggle mute for a zone"),
    ("POST", "/play-from-queue/:zone_id", "Play a specific item from queue"),
    ("GET", "/libretto", "Get loaded libretto JSON (single-file mode)"),
    ("GET", "/librettos", "List all loaded libretto files (library mode)"),
    ("GET", "/libretto/:file_id", "Get a specific libretto by file_id"),
    ("GET", "/libretto-view", "Serve the libretto viewer page"),
];

/// Start the web server
pub async fn start_server(client: Arc<Mutex<RoonClient>>, config: crate::config::AppConfig) -> Result<(), Box<dyn std::error::Error>> {
    let port = config.port;
    let carousel_interval = config.carousel_interval;
    let libretto_path = config.libretto.clone();

    // Start background dCS device discovery (non-blocking)
    // Discovery runs in background and notifies when complete
    crate::dcs::start_discovery();

    // Load timed interchange libretto if provided
    let libretto_json = if let Some(path) = &libretto_path {
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str::<serde_json::Value>(&contents) {
                Ok(value) => {
                    let track_count = value.get("tracks").and_then(|t| t.as_array()).map(|a| a.len()).unwrap_or(0);
                    log::info!("Loaded timed libretto from {}: {} tracks", path, track_count);
                    Some(Arc::new(value))
                }
                Err(e) => {
                    log::error!("Failed to parse libretto JSON from {}: {}", path, e);
                    None
                }
            }
            Err(e) => {
                log::error!("Failed to read libretto file {}: {}", path, e);
                None
            }
        }
    } else {
        None
    };

    // Load libretto library from directory if configured
    let lib = if let Some(dir) = &config.libretto_dir {
        match library::LibrettoLibrary::from_directory(std::path::Path::new(dir)) {
            Ok(lib) => Some(Arc::new(lib)),
            Err(e) => {
                log::error!("Failed to load libretto library from {}: {}", dir, e);
                None
            }
        }
    } else {
        None
    };

    let state = AppState {
        roon_client: client,
        carousel_interval_ms: carousel_interval * 1000, // Convert seconds to milliseconds
        libretto_json,
        library: lib,
    };

    let app = Router::new()
        .route("/", get(spa_handler))
        .route("/ws", get(ws_handler))
        .route("/status", get(status_handler))
        .route("/version", get(version_handler))
        .route("/reconnect", post(reconnect_handler))
        .route("/zones", get(zones_handler))
        .route("/now-playing", get(now_playing_handler))
        .route("/queue/:zone_id", get(queue_handler))
        .route("/image/:image_key", get(image_handler))
        .route("/control/:zone_id", post(control_handler))
        .route("/seek/:zone_id", post(seek_handler))
        .route("/mute/:zone_id", post(mute_handler))
        .route("/play-from-queue/:zone_id", post(play_from_queue_handler))
        .route("/libretto", get(libretto_handler))
        .route("/librettos", get(librettos_handler))
        .route("/libretto/:file_id", get(libretto_by_id_handler))
        .route("/libretto-view", get(libretto_view_handler))
        .layer(CorsLayer::permissive())
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    let local_ip = get_local_ip();
    
    // Register mDNS so roon-rd.local resolves to this machine
    // Keep _mdns alive for the lifetime of the server
    let _mdns = register_mdns(port, &local_ip);
    
    println!("\n=== Roon Remote Display Server v{} ({}) ===", env!("CARGO_PKG_VERSION"), env!("BUILD_HASH"));
    println!("Starting server on http://{}", addr);
    println!("\nAccess via:");
    println!("  Local:   http://localhost:{}", port);
    println!("  Network: http://{}:{}", local_ip, port);
    if _mdns.is_some() {
        println!("  mDNS:    http://roon-rd.local:{}", port);
    }
    if libretto_path.is_some() {
        println!("  Libretto: http://localhost:{}/libretto-view", port);
    }
    if let Some(dir) = &config.libretto_dir {
        println!("  Libretto dir: {}", dir);
    }
    println!("\nAPI endpoints:");

    // Print routes from the documentation array
    for (method, path, description) in ROUTES {
        println!("  {:<6} {:<30} - {}", method, path, description);
    }

    println!("\nPress Ctrl+C to stop the server\n");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    log::info!("Server accepting HTTP connections on {}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn spa_handler(State(state): State<AppState>) -> Html<String> {
    let html = SPA_HTML
        .replace("__VERSION__", &format!("v{}", env!("CARGO_PKG_VERSION")))
        .replace("__CAROUSEL_INTERVAL__", &state.carousel_interval_ms.to_string());
    Html(html)
}

/// Serve the timed interchange libretto JSON (legacy single-file mode)
async fn libretto_handler(State(state): State<AppState>) -> Response {
    match &state.libretto_json {
        Some(json) => Json(json.as_ref().clone()).into_response(),
        None => (StatusCode::NOT_FOUND, "No libretto loaded").into_response(),
    }
}

/// List all loaded libretto files in the library
async fn librettos_handler(State(state): State<AppState>) -> Response {
    match &state.library {
        Some(lib) => {
            let summaries = lib.list_files();
            Json(serde_json::json!({
                "files": summaries,
                "count": summaries.len(),
            })).into_response()
        }
        None => Json(serde_json::json!({
            "files": [],
            "count": 0,
        })).into_response(),
    }
}

/// Serve a specific libretto file by its file_id
async fn libretto_by_id_handler(
    State(state): State<AppState>,
    Path(file_id): Path<String>,
) -> Response {
    if let Some(lib) = &state.library {
        if let Some(loaded) = lib.librettos.get(&file_id) {
            return Json(loaded.json.as_ref().clone()).into_response();
        }
    }
    (StatusCode::NOT_FOUND, format!("Libretto '{}' not found", file_id)).into_response()
}

/// Serve the libretto viewer page
async fn libretto_view_handler(State(state): State<AppState>) -> Response {
    let has_libretto = state.libretto_json.is_some()
        || state.library.as_ref().map_or(false, |lib| !lib.librettos.is_empty());
    if !has_libretto {
        return (StatusCode::NOT_FOUND, Html("<h1>No libretto loaded</h1><p>Start the server with --libretto &lt;path&gt; or --libretto-dir &lt;dir&gt;</p>".to_string())).into_response();
    }
    Html(LIBRETTO_VIEW_HTML.to_string()).into_response()
}

async fn status_handler(State(state): State<AppState>) -> Json<StatusResponse> {
    let client = state.roon_client.lock().await;

    let connected = client.is_connected().await;
    let core_name = client.get_core_name().await;

    let message = if connected {
        "Connected to Roon Core".to_string()
    } else {
        "Not connected. Please authorize the extension in Roon Settings > Extensions".to_string()
    };

    Json(StatusResponse {
        connected,
        core_name,
        message,
    })
}

async fn version_handler() -> Json<VersionResponse> {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn reconnect_handler(State(state): State<AppState>) -> Json<ReconnectResponse> {
    let mut client = state.roon_client.lock().await;

    match client.reconnect().await {
        Ok(_) => {
            Json(ReconnectResponse {
                success: true,
                message: "Reconnection initiated. Please check status.".to_string(),
            })
        }
        Err(e) => {
            Json(ReconnectResponse {
                success: false,
                message: format!("Reconnection failed: {}", e),
            })
        }
    }
}

async fn zones_handler(State(state): State<AppState>) -> Json<ZonesResponse> {
    let client = state.roon_client.lock().await;

    let zones = client.get_zones().await;
    let zone_infos: Vec<ZoneInfo> = zones.into_iter().map(|zone| {
        let devices = zone.outputs.iter()
            .map(|output| output.display_name.clone())
            .collect();

        ZoneInfo {
            zone_id: zone.zone_id,
            display_name: zone.display_name,
            state: format!("{:?}", zone.state),
            devices,
        }
    }).collect();

    let count = zone_infos.len();

    Json(ZonesResponse {
        zones: zone_infos,
        count,
    })
}

#[derive(Serialize)]
struct QueueItemInfo {
    queue_item_id: u32,
    title: String,
    artist: Option<String>,
    album: Option<String>,
    length: u32,
    image_key: Option<String>,
    one_line: String,
    two_line_1: String,
    two_line_2: String,
}

#[derive(Serialize)]
struct QueueResponse {
    items: Vec<QueueItemInfo>,
}

async fn queue_handler(
    Path(zone_id): Path<String>,
    State(state): State<AppState>,
) -> Json<QueueResponse> {
    let client = state.roon_client.lock().await;

    // Subscribe to this zone's queue (will unsubscribe from previous zone if any)
    client.subscribe_to_queue(&zone_id).await;

    let queue_items = client.get_queue(&zone_id).await.unwrap_or_default();

    let items: Vec<QueueItemInfo> = queue_items.into_iter().map(|item| {
        QueueItemInfo {
            queue_item_id: item.queue_item_id,
            title: item.three_line.line1.clone(),
            artist: if !item.three_line.line2.is_empty() {
                Some(item.three_line.line2.clone())
            } else {
                None
            },
            album: if !item.three_line.line3.is_empty() {
                Some(item.three_line.line3.clone())
            } else {
                None
            },
            length: item.length,
            one_line: item.one_line.line1.clone(),
            two_line_1: item.two_line.line1.clone(),
            two_line_2: item.two_line.line2.clone(),
            image_key: item.image_key,
        }
    }).collect();

    Json(QueueResponse { items })
}

async fn now_playing_handler(State(state): State<AppState>) -> Json<NowPlayingResponse> {
    log::debug!("now_playing_handler called");

    // Use the RoonClient's build_ws_zone_data method which has all the debug logging
    let (now_playing, _raw_zones, _raw_json) = {
        let client = state.roon_client.lock().await;
        client.build_ws_zone_data().await
    }; // Lock is released here

    log::debug!("now_playing_handler returning {} zones", now_playing.len());

    let count = now_playing.len();

    Json(NowPlayingResponse {
        now_playing,
        count,
    })
}

async fn image_handler(
    State(state): State<AppState>,
    Path(image_key): Path<String>,
) -> Response {
    log::info!("Image request for key: {}", image_key);
    let client = state.roon_client.lock().await;

    // Check if image is already cached
    if let Some(image_data) = client.get_image(&image_key).await {
        log::info!("Serving cached image: {} ({} bytes)", image_key, image_data.data.len());
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, image_data.content_type.clone()),
             (header::CACHE_CONTROL, "public, max-age=3600".to_string())],
            image_data.data,
        ).into_response();
    }

    // Request the image from Roon
    log::info!("Image not cached, requesting from Roon...");
    if client.request_image(&image_key, 300, 300).await.is_some() {
        log::info!("Image request sent, waiting for response...");
        // Wait a bit for the image to arrive (with timeout)
        for i in 0..20 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            if let Some(image_data) = client.get_image(&image_key).await {
                log::info!("Image received after {}ms: {} ({} bytes)", i * 100, image_key, image_data.data.len());
                return (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, image_data.content_type.clone()),
                     (header::CACHE_CONTROL, "public, max-age=3600".to_string())],
                    image_data.data,
                ).into_response();
            }
        }
        log::warn!("Image request timed out after 2 seconds");
    } else {
        log::warn!("Failed to send image request to Roon");
    }

    // Image not found or timeout
    log::warn!("Returning 404 for image: {}", image_key);
    StatusCode::NOT_FOUND.into_response()
}

#[derive(Deserialize)]
struct ControlRequest {
    control: String,
}

async fn control_handler(
    State(state): State<AppState>,
    Path(zone_id): Path<String>,
    Json(payload): Json<ControlRequest>,
) -> Response {
    let client = state.roon_client.lock().await;

    match client.control_zone(&zone_id, &payload.control).await {
        Ok(_) => (StatusCode::OK, "Control command sent").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(Deserialize)]
struct SeekRequest {
    seconds: i32,
}

async fn seek_handler(
    State(state): State<AppState>,
    Path(zone_id): Path<String>,
    Json(payload): Json<SeekRequest>,
) -> Response {
    let client = state.roon_client.lock().await;

    match client.seek_zone(&zone_id, payload.seconds).await {
        Ok(_) => (StatusCode::OK, "Seek command sent").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(Deserialize)]
struct PlayFromQueueRequest {
    queue_item_id: u32,
}

async fn play_from_queue_handler(
    State(state): State<AppState>,
    Path(zone_id): Path<String>,
    Json(payload): Json<PlayFromQueueRequest>,
) -> Response {
    let client = state.roon_client.lock().await;

    match client.play_from_queue_item(&zone_id, payload.queue_item_id).await {
        Ok(_) => (StatusCode::OK, "Play from queue command sent").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(Deserialize)]
struct MuteRequest {
    mute: bool,
}

async fn mute_handler(
    State(state): State<AppState>,
    Path(zone_id): Path<String>,
    Json(payload): Json<MuteRequest>,
) -> Response {
    let client = state.roon_client.lock().await;

    match client.mute_output(&zone_id, payload.mute).await {
        Ok(_) => (StatusCode::OK, "Mute command sent").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}
