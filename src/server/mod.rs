use axum::{
    routing::{get, post},
    Router,
    Json,
    extract::{State, Path, ws::{WebSocket, WebSocketUpgrade}},
    response::{Html, IntoResponse, Response},
    http::{StatusCode, header},
};
use tower_http::cors::CorsLayer;
use std::sync::Arc;
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};
use crate::roon::RoonClient;
use futures_util::StreamExt;

#[derive(Clone)]
pub struct AppState {
    pub roon_client: Arc<Mutex<RoonClient>>,
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

#[derive(Serialize, Deserialize)]
pub struct NowPlayingInfo {
    pub zone_id: String,
    pub zone_name: String,
    pub state: String,
    pub track: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub position_seconds: Option<i64>,
    pub length_seconds: Option<u32>,
    pub image_key: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct NowPlayingResponse {
    pub now_playing: Vec<NowPlayingInfo>,
    pub count: usize,
}

#[derive(Serialize, Deserialize)]
pub struct ReconnectResponse {
    pub success: bool,
    pub message: String,
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
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            color: #eee;
            height: 100vh;
            overflow: auto;
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
        .zone-header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            margin-bottom: 15px;
        }
        .zone-name {
            font-size: 1.2rem;
            font-weight: 500;
        }
        .zone-state {
            padding: 4px 10px;
            border-radius: 8px;
            font-size: 0.8rem;
            text-transform: lowercase;
        }
        .zone-state.playing {
            background: #2ecc71;
            color: #000;
        }
        .zone-state.paused {
            background: #f39c12;
            color: #000;
        }
        .zone-state.stopped {
            background: #555;
            color: #fff;
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
        .no-playing {
            text-align: center;
            padding: 40px;
            color: #666;
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
        .zone-controls {
            display: flex;
            align-items: center;
            justify-content: flex-end;
            gap: 8px;
            margin-top: 15px;
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
        .control-btn:hover {
            background: rgba(255, 255, 255, 0.2);
            border-color: rgba(255, 255, 255, 0.3);
        }
        .control-btn:active {
            transform: scale(0.95);
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
            width: 66.666%;
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
            .track-title {
                font-size: 1.8rem;
            }
            .track-artist {
                font-size: 1.4rem;
            }
            .track-album {
                font-size: 1.2rem;
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
        const placeholderSvg = '<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path d="M12 3v10.55c-.59-.34-1.27-.55-2-.55-2.21 0-4 1.79-4 4s1.79 4 4 4 4-1.79 4-4V7h4V3h-6z"/></svg>';

        function formatTime(seconds) {
            if (seconds == null) return '0:00';
            const mins = Math.floor(seconds / 60);
            const secs = Math.floor(seconds % 60);
            return `${mins}:${secs.toString().padStart(2, '0')}`;
        }

        function renderZone(zone) {
            const stateClass = zone.state.toLowerCase();

            // For stopped zones, only show the header with status and controls
            if (zone.state.toLowerCase() === 'stopped') {
                return `
                    <div class="zone stopped" data-zone-id="${zone.zone_id}">
                        <div class="zone-header">
                            <span class="zone-name">${zone.zone_name}</span>
                            <span class="zone-state ${stateClass}">stopped</span>
                        </div>
                        <div class="zone-controls">
                            <button class="control-btn" onclick="sendControl('${zone.zone_id}', 'play')">▶ Play</button>
                        </div>
                    </div>
                `;
            }

            // For playing/paused zones, show full details
            const progress = zone.length_seconds > 0
                ? ((zone.position_seconds || 0) / zone.length_seconds * 100)
                : 0;

            // Use image endpoint if image_key is available
            const albumArt = zone.image_key
                ? `<img class="album-art" src="/image/${encodeURIComponent(zone.image_key)}" alt="Album Art">`
                : `<div class="album-art-placeholder">${placeholderSvg}</div>`;

            const isPlaying = zone.state.toLowerCase() === 'playing';
            const playPauseBtn = isPlaying
                ? `<button class="control-btn" onclick="sendControl('${zone.zone_id}', 'pause')">⏸ Pause</button>`
                : `<button class="control-btn" onclick="sendControl('${zone.zone_id}', 'play')">▶ Play</button>`;

            return `
                <div class="zone" data-zone-id="${zone.zone_id}">
                    <div class="zone-header">
                        <span class="zone-name">${zone.zone_name}</span>
                        <span class="zone-state ${stateClass}">${zone.state.toLowerCase()}</span>
                    </div>
                    ${zone.track ? `
                        <div class="zone-content">
                            ${albumArt}
                            <div class="track-details">
                                <div class="track-details-top">
                                    <div class="track-info">
                                        <div class="track-title">${zone.track}</div>
                                        ${zone.artist ? `<div class="track-artist">${zone.artist}</div>` : ''}
                                        ${zone.album ? `<div class="track-album">${zone.album}</div>` : ''}
                                    </div>
                                    <div class="progress-container">
                                        <div class="progress-bar" onclick="handleSeek(event, '${zone.zone_id}', ${zone.length_seconds || 0})" data-length="${zone.length_seconds || 0}">
                                            <div class="progress-fill" style="width: ${progress}%"></div>
                                        </div>
                                        <div class="progress-time">
                                            <span>${formatTime(zone.position_seconds)}</span>
                                            <span>${formatTime(zone.length_seconds)}</span>
                                        </div>
                                    </div>
                                </div>
                                <div class="zone-controls">
                                    <button class="control-btn" onclick="showQueue('${zone.zone_id}')">▹≡</button>
                                    <div style="width: 8px;"></div>
                                    <button class="control-btn" onclick="sendControl('${zone.zone_id}', 'previous')">⏮</button>
                                    ${playPauseBtn}
                                    <button class="control-btn" onclick="sendControl('${zone.zone_id}', 'stop')">⏹ Stop</button>
                                    <button class="control-btn" onclick="sendControl('${zone.zone_id}', 'next')">⏭</button>
                                </div>
                            </div>
                        </div>
                    ` : '<div class="no-playing">No track loaded</div>'}
                </div>
            `;
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

            // Merge available zones with now playing data
            const mergedZones = availableZones.map(zone => {
                // Find matching now playing data
                const nowPlaying = nowPlayingZones.find(np => np.zone_id === zone.zone_id);

                if (nowPlaying) {
                    // Zone has now playing data
                    return {
                        zone_id: zone.zone_id,
                        zone_name: zone.display_name,
                        state: nowPlaying.state,
                        track: nowPlaying.track,
                        artist: nowPlaying.artist,
                        album: nowPlaying.album,
                        position_seconds: nowPlaying.position_seconds,
                        length_seconds: nowPlaying.length_seconds,
                        image_key: nowPlaying.image_key
                    };
                } else {
                    // Zone is stopped
                    return {
                        zone_id: zone.zone_id,
                        zone_name: zone.display_name,
                        state: zone.state
                    };
                }
            });

            // Filter by selected zone
            let zonesToShow = mergedZones;
            if (selectedZone !== 'all') {
                zonesToShow = mergedZones.filter(z => z.zone_id === selectedZone);
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
            } else {
                container.innerHTML = zonesToShow.map(renderZone).join('');
            }
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
            const isMuted = muteState[zoneId] || false;
            const newMuteState = !isMuted;

            try {
                const response = await fetch(`/mute/${encodeURIComponent(zoneId)}`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ mute: newMuteState })
                });
                if (response.ok) {
                    muteState[zoneId] = newMuteState;
                    const muteBtn = document.getElementById(`mute-${zoneId}`);
                    if (muteBtn) {
                        if (newMuteState) {
                            muteBtn.classList.add('muted');
                            muteBtn.textContent = '🔊 Unmute';
                        } else {
                            muteBtn.classList.remove('muted');
                            muteBtn.textContent = '🔇 Mute';
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

                zoneElement.insertAdjacentHTML('beforeend', overlayHtml);
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
                        // Update zones and now playing data
                        updateZones();
                        updateNowPlaying();
                    } else if (msg.type === 'connection_changed') {
                        // Update connection status
                        updateStatus();
                        if (msg.connected) {
                            // When reconnected, refresh all data
                            updateZones();
                            updateNowPlaying();
                        }
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

    // Handle incoming and outgoing messages
    loop {
        tokio::select! {
            // Receive updates from broadcast channel
            result = rx.recv() => {
                match result {
                    Ok(msg) => {
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

/// Start the web server
pub async fn start_server(client: Arc<Mutex<RoonClient>>, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let state = AppState {
        roon_client: client,
    };

    let app = Router::new()
        .route("/", get(spa_handler))
        .route("/ws", get(ws_handler))
        .route("/status", get(status_handler))
        .route("/reconnect", post(reconnect_handler))
        .route("/zones", get(zones_handler))
        .route("/now-playing", get(now_playing_handler))
        .route("/queue/:zone_id", get(queue_handler))
        .route("/image/:image_key", get(image_handler))
        .route("/control/:zone_id", post(control_handler))
        .route("/seek/:zone_id", post(seek_handler))
        .route("/mute/:zone_id", post(mute_handler))
        .route("/play-from-queue/:zone_id", post(play_from_queue_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    println!("\n=== Roon Remote Display Server ===");
    println!("Starting server on http://{}", addr);
    println!("\nOpen http://localhost:{} in your browser", port);
    println!("\nAPI endpoints:");
    println!("  GET /status          - Get connection status (JSON)");
    println!("  GET /zones           - Get available zones (JSON)");
    println!("  GET /now-playing     - Get currently playing tracks (JSON)");
    println!("  GET /image/:key      - Get album art image");
    println!("\nPress Ctrl+C to stop the server\n");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn spa_handler() -> Html<&'static str> {
    Html(SPA_HTML)
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
            image_key: item.image_key,
        }
    }).collect();

    Json(QueueResponse { items })
}

async fn now_playing_handler(State(state): State<AppState>) -> Json<NowPlayingResponse> {
    let client = state.roon_client.lock().await;

    let zones = client.get_zones().await;
    let now_playing: Vec<NowPlayingInfo> = zones.into_iter()
        .filter_map(|zone| {
            // Only include zones that have something playing or paused
            if zone.now_playing.is_some() {
                let now_playing = zone.now_playing.as_ref()?;

                let three_line = &now_playing.three_line;
                let (track, artist, album) = (
                    Some(three_line.line1.clone()),
                    if !three_line.line2.is_empty() {
                        Some(three_line.line2.clone())
                    } else {
                        None
                    },
                    if !three_line.line3.is_empty() {
                        Some(three_line.line3.clone())
                    } else {
                        None
                    },
                );

                Some(NowPlayingInfo {
                    zone_id: zone.zone_id,
                    zone_name: zone.display_name,
                    state: format!("{:?}", zone.state),
                    track,
                    artist,
                    album,
                    position_seconds: now_playing.seek_position,
                    length_seconds: now_playing.length,
                    image_key: now_playing.image_key.clone(),
                })
            } else {
                None
            }
        })
        .collect();

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
