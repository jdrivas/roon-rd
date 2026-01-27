use roon_api::{Info, RoonApi, CoreEvent, Services, Parsed};
use roon_api::transport::{Transport, Zone, QueueItem, State};
use roon_api::image::{Image, Args as ImageArgs, Scaling, Scale, Format};
use roon_api::browse::{Browse, BrowseOpts, LoadOpts};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, Notify};

/// Image data with content type
#[derive(Clone, Debug)]
pub struct ImageData {
    pub content_type: String,
    pub data: Vec<u8>,
}

/// Simplified zone data for WebSocket updates and HTTP responses
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WsZoneData {
    pub zone_id: String,
    pub zone_name: String,
    pub state: String,
    pub track: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub position_seconds: Option<i64>,
    pub length_seconds: Option<u32>,
    pub image_key: Option<String>,
    pub is_muted: Option<bool>,
    pub dcs_format: Option<String>,
}

/// Message types for WebSocket updates
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type")]
pub enum WsMessage {
    #[serde(rename = "zones_changed")]
    ZonesChanged {
        now_playing: Vec<WsZoneData>,
        #[serde(skip_serializing)]  // Don't send raw zones to web clients, only for TUI
        raw_zones: Vec<Zone>,
        #[serde(skip_serializing)]  // Don't send raw JSON to web clients, only for TUI
        raw_json: Option<String>,
    },
    #[serde(rename = "connection_changed")]
    ConnectionChanged { connected: bool },
    #[serde(rename = "seek_updated")]
    SeekUpdated {
        zone_id: String,
        seek_position: Option<i64>,
        queue_time_remaining: i64,
    },
    #[serde(rename = "queue_changed")]
    QueueChanged {
        zone_id: String,
    },
}

/// Wrapper for Roon API client with state management
pub struct RoonClient {
    api: RoonApi,
    zones: Arc<RwLock<HashMap<String, Zone>>>,
    zones_raw_json: Arc<RwLock<Option<String>>>, // Last raw JSON from Roon for zones_changed
    queues: Arc<RwLock<HashMap<String, Vec<QueueItem>>>>, // zone_id -> queue items
    active_queue_zone: Arc<RwLock<Option<String>>>, // zone_id of currently subscribed queue
    queue_ready: Arc<Notify>, // Notifies when queue data arrives for active zone
    connected: Arc<RwLock<bool>>,
    core_name: Arc<RwLock<Option<String>>>,
    images: Arc<RwLock<HashMap<String, ImageData>>>,
    image_service: Arc<RwLock<Option<Image>>>,
    transport_service: Arc<RwLock<Option<Transport>>>,
    browse_service: Arc<RwLock<Option<Browse>>>,
    ws_tx: broadcast::Sender<WsMessage>,
    pending_stops: Arc<tokio::sync::Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>, // zone_id -> delayed stop task
}

const CONFIG_PATH: &str = "roon-rd-config.json";

/// Delay in milliseconds to wait for dCS device to update after a zone change
/// This gives the dCS device time to process the new stream before browsers fetch the format
const DCS_UPDATE_DELAY_MS: u64 = 200;

/// Delay in milliseconds before broadcasting a Stopped state
/// This prevents UI flickering during track transitions (Playing -> Stopped -> Loading -> Playing)
/// If another event arrives within this window, the stop is cancelled
const STOP_BROADCAST_DELAY_MS: u64 = 500;

/// Build WebSocket zone data from zones Arc (standalone function for use in event handlers)
/// Returns both the simplified WsZoneData, the raw Zones from Roon, and the raw JSON string
async fn build_ws_zone_data_from_zones(zones: Arc<RwLock<HashMap<String, Zone>>>, zones_raw_json: Arc<RwLock<Option<String>>>) -> (Vec<WsZoneData>, Vec<Zone>, Option<String>) {
    use crate::dcs;

    let zones_vec: Vec<Zone> = zones.read().await.values().cloned().collect();
    let raw_zones = zones_vec.clone();  // Keep a copy of raw zones
    log::debug!("build_ws_zone_data_from_zones: Processing {} zones", zones_vec.len());

    // Process all zones in parallel
    let zone_futures: Vec<_> = zones_vec.into_iter().map(|zone| {
        async move {
            let zone_id = zone.zone_id.clone();
            let zone_name = zone.display_name.clone();
            let zone_state = format!("{:?}", zone.state);

            log::debug!("Processing zone: {} ({}), state: {}", zone_name, zone_id, zone_state);

            // Fetch dCS format on-demand if this is a dCS Vivaldi zone in Playing state
            let dcs_format = if zone.display_name.starts_with("dCS Vivaldi")
                && format!("{:?}", zone.state).to_lowercase() == "playing" {

                log::debug!("Zone {} is dCS Vivaldi in Playing state, fetching format...", zone_name);

                match dcs::get_playback_info("dcs-vivaldi.local").await {
                    Ok(playback_info) => {
                        log::debug!("dCS playback info retrieved for {}: {:?}", zone_name, playback_info);
                        // Extract format from audio_format field
                        if let Some(audio_format) = playback_info.audio_format {
                            // Only return format if bits_per_sample is valid (non-zero)
                            if let Some(bits) = audio_format.bits_per_sample {
                                if bits > 0 {
                                    let sample_rate_str = if let Some(freq) = audio_format.sample_frequency {
                                        if freq >= 1000 {
                                            format!("{} kHz", freq / 1000)
                                        } else {
                                            format!("{} Hz", freq)
                                        }
                                    } else {
                                        String::new()
                                    };

                                    let bit_depth_str = format!("{} bit", bits);

                                    if !sample_rate_str.is_empty() {
                                        let format_str = format!("{} {}", sample_rate_str, bit_depth_str);
                                        log::debug!("dCS format for {}: {}", zone_name, format_str);
                                        Some(format_str)
                                    } else {
                                        log::debug!("dCS format missing sample rate for {}", zone_name);
                                        None
                                    }
                                } else {
                                    log::debug!("dCS format has bits_per_sample=0 for {}, not displaying", zone_name);
                                    None
                                }
                            } else {
                                log::debug!("dCS format missing bits_per_sample for {}", zone_name);
                                None
                            }
                        } else {
                            log::debug!("dCS playback info missing audio_format for {}", zone_name);
                            None
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to get dCS playback info for {}: {}", zone_name, e);
                        None
                    }
                }
            } else {
                log::debug!("Zone {} not eligible for dCS format (name: {}, state: {})",
                           zone_id, zone.display_name, format!("{:?}", zone.state));
                None
            };

            // Extract track info if available
            let (track, artist, album, position_seconds, length_seconds, image_key) =
                if let Some(now_playing) = zone.now_playing.as_ref() {
                    let three_line = &now_playing.three_line;
                    (
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
                        now_playing.seek_position,
                        now_playing.length,
                        now_playing.image_key.clone(),
                    )
                } else {
                    (None, None, None, None, None, None)
                };

            // Extract is_muted from the first output with volume info
            let is_muted = zone.outputs.iter()
                .find_map(|output| output.volume.as_ref().and_then(|v| v.is_muted));

            // Build zone name with selected output display name
            let zone_name = if let Some(output) = zone.outputs.first() {
                // Check if there's a selected source control
                if let Some(source_controls) = &output.source_controls {
                    if let Some(selected_source) = source_controls.iter()
                        .find(|sc| sc.status == roon_api::transport::Status::Selected) {
                        format!("{} ({})", zone.display_name, selected_source.display_name)
                    } else {
                        zone.display_name.clone()
                    }
                } else {
                    zone.display_name.clone()
                }
            } else {
                zone.display_name.clone()
            };

            let ws_data = WsZoneData {
                zone_id: zone.zone_id,
                zone_name: zone_name.clone(),
                state: format!("{:?}", zone.state),
                track: track.clone(),
                artist,
                album,
                position_seconds,
                length_seconds,
                image_key,
                is_muted,
                dcs_format: dcs_format.clone(),
            };

            log::debug!("Built WsZoneData for {}: track={:?}, dcs_format={:?}",
                       zone_name, track, dcs_format);

            ws_data
        }
    }).collect();

    let result = futures_util::future::join_all(zone_futures).await;
    let raw_json = zones_raw_json.read().await.clone();
    log::debug!("build_ws_zone_data_from_zones: Returning {} zone data items, {} raw zones, and raw JSON ({})",
                result.len(), raw_zones.len(), if raw_json.is_some() { "present" } else { "absent" });
    for item in &result {
        log::debug!("  Zone {}: state={}, track={:?}, dcs_format={:?}",
                   item.zone_name, item.state, item.track, item.dcs_format);
    }
    (result, raw_zones, raw_json)
}

/// Get the local IP address of this machine
fn get_local_ip() -> String {
    use std::net::UdpSocket;

    // Connect to a public DNS server (doesn't actually send data)
    // This is the most reliable way to get the local IP that would be used for network communication
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if let Ok(()) = socket.connect("8.8.8.8:80") {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }

    // Fallback to localhost if we can't determine the IP
    "localhost".to_string()
}

impl RoonClient {
    /// Create a new Roon client
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // Get local IP address for display name
        let local_ip = get_local_ip();
        let display_name = format!("Roon Remote Display @ {}", local_ip);

        // Construct Info manually instead of using the macro
        let extension_id = format!("com.momentlabs.io.{}", env!("CARGO_PKG_NAME"));
        let info = Info::new(
            extension_id,
            Box::leak(display_name.into_boxed_str()), // Convert to &'static str
            env!("CARGO_PKG_VERSION"),
            Some("Momentlabs"),
            "david@momentlabs.io",
            Some(env!("CARGO_PKG_REPOSITORY"))
        );
        let api = RoonApi::new(info);

        // Create broadcast channel for WebSocket updates (capacity of 100 messages)
        let (ws_tx, _) = broadcast::channel(100);

        Ok(RoonClient {
            api,
            zones: Arc::new(RwLock::new(HashMap::new())),
            zones_raw_json: Arc::new(RwLock::new(None)),
            queues: Arc::new(RwLock::new(HashMap::new())),
            active_queue_zone: Arc::new(RwLock::new(None)),
            queue_ready: Arc::new(Notify::new()),
            connected: Arc::new(RwLock::new(false)),
            core_name: Arc::new(RwLock::new(None)),
            images: Arc::new(RwLock::new(HashMap::new())),
            image_service: Arc::new(RwLock::new(None)),
            transport_service: Arc::new(RwLock::new(None)),
            browse_service: Arc::new(RwLock::new(None)),
            ws_tx,
            pending_stops: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        })
    }

    /// Get a WebSocket subscriber
    pub fn subscribe_ws(&self) -> broadcast::Receiver<WsMessage> {
        self.ws_tx.subscribe()
    }

    /// Reconnect to Roon Core (triggers a new discovery)
    pub async fn reconnect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("Reconnecting to Roon Core...");
        self.connect().await
    }

    /// Start the Roon API connection
    pub async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("Connecting to Roon Core...");

        // Prepare for connection
        let provided: HashMap<String, roon_api::Svc> = HashMap::new();

        // Request transport, image, and browse services
        let services = Some(vec![
            Services::Transport(Transport::new()),
            Services::Image(Image::new()),
            Services::Browse(Browse::new()),
        ]);

        // Get state callback - load from config file
        let get_roon_state = || {
            RoonApi::load_roon_state(CONFIG_PATH)
        };

        // Clone Arc references for the handler
        let zones = self.zones.clone();
        let zones_raw_json = self.zones_raw_json.clone();
        let queues = self.queues.clone();
        let active_queue_zone = self.active_queue_zone.clone();
        let queue_ready = self.queue_ready.clone();
        let connected = self.connected.clone();
        let core_name = self.core_name.clone();
        let images = self.images.clone();
        let image_service = self.image_service.clone();
        let transport_service = self.transport_service.clone();
        let browse_service = self.browse_service.clone();
        let ws_tx = self.ws_tx.clone();
        let pending_stops = self.pending_stops.clone();

        // Start discovery
        let result = self.api.start_discovery(
            Box::new(get_roon_state),
            provided,
            services,
        ).await;

        if let Some((mut handlers, mut core_rx)) = result {
            log::info!("Roon API started. Please authorize this extension in Roon Settings > Extensions");

            // Spawn handler for core events and transport updates
            handlers.spawn(async move {
                while let Some((core_event, msg)) = core_rx.recv().await {
                    match core_event {
                        CoreEvent::Discovered(core, token) => {
                            log::info!("Discovered Roon Core: {}, version {}", core.display_name, core.display_version);
                            if let Some(token) = token {
                                log::debug!("Using existing token: {}", token);
                            }
                        }
                        CoreEvent::Registered(mut core, _token) => {
                            log::info!("Registered with Roon Core: {}, version {}", core.display_name, core.display_version);

                            // Update connection state
                            *connected.write().await = true;
                            *core_name.write().await = Some(core.display_name.clone());

                            // Broadcast connection change
                            let _ = ws_tx.send(WsMessage::ConnectionChanged { connected: true });

                            // Subscribe to zone updates if we have transport service
                            if let Some(transport) = core.get_transport() {
                                log::info!("Subscribing to zone updates...");
                                transport.subscribe_zones().await;
                                *transport_service.write().await = Some(transport.clone());
                            }

                            // Store image service reference
                            if let Some(img) = core.get_image() {
                                log::info!("Image service available");
                                *image_service.write().await = Some(img.clone());
                            }

                            // Store browse service reference
                            if let Some(browse) = core.get_browse() {
                                log::info!("Browse service available");
                                *browse_service.write().await = Some(browse.clone());
                            }
                        }
                        CoreEvent::Lost(core) => {
                            log::warn!("Lost connection to Roon Core: {}, version {}", core.display_name, core.display_version);
                            *connected.write().await = false;
                            *core_name.write().await = None;
                            zones.write().await.clear();
                            *image_service.write().await = None;
                            *transport_service.write().await = None;
                            *browse_service.write().await = None;

                            // Broadcast connection change
                            let _ = ws_tx.send(WsMessage::ConnectionChanged { connected: false });
                        }
                        CoreEvent::None => {}
                    }

                    // Handle messages via Parsed enum
                    if let Some((raw_msg, parsed)) = msg {
                        match parsed {
                            Parsed::RoonState(roon_state) => {
                                // Save state to persist authorization token
                                if let Err(e) = RoonApi::save_roon_state(CONFIG_PATH, roon_state) {
                                    log::error!("Failed to save Roon state: {}", e);
                                }
                            }
                            Parsed::Zones(zones_changed) => {
                                log::debug!("Roon API Zones response:\n{:#?}", zones_changed);
                                log::debug!("Zones changed, updating zone map");

                                // Capture and store raw JSON for TUI display
                                let raw_json = serde_json::to_string_pretty(&raw_msg).ok();
                                *zones_raw_json.write().await = raw_json;

                                let mut zone_map = zones.write().await;

                                // Collect image keys to request
                                let mut image_keys_to_request = Vec::new();

                                // Update zones that changed
                                for zone in zones_changed {
                                    // If the zone has now_playing with an image_key, queue it for download
                                    if let Some(ref now_playing) = zone.now_playing {
                                        if let Some(ref image_key) = now_playing.image_key {
                                            // Check if we don't already have this image cached
                                            if !images.read().await.contains_key(image_key) {
                                                image_keys_to_request.push(image_key.clone());
                                            }
                                        }
                                    }
                                    zone_map.insert(zone.zone_id.clone(), zone);
                                }

                                // Release the zone_map lock before requesting images
                                drop(zone_map);

                                // Request any new images
                                if !image_keys_to_request.is_empty() {
                                    let img_svc = image_service.read().await;
                                    if let Some(img) = img_svc.as_ref() {
                                        for image_key in image_keys_to_request {
                                            log::info!("Proactively requesting album art: {}", image_key);
                                            let scaling = Scaling::new(Scale::Fit, 300, 300);
                                            let args = ImageArgs::new(Some(scaling), Some(Format::Jpeg));
                                            img.get_image(&image_key, args).await;
                                        }
                                    }
                                }

                                // Check zone states to determine broadcast strategy
                                let zones_snapshot = zones.read().await.clone();

                                // Categorize zones by state
                                let mut has_stopped_zones = Vec::new();
                                let mut has_non_stop_zones = Vec::new();
                                let has_dcs_playing_zones: Vec<_> = zones_snapshot.iter()
                                    .filter(|(_, zone)| zone.display_name.starts_with("dCS Vivaldi") && zone.state == State::Playing)
                                    .map(|(id, _)| id.clone())
                                    .collect();

                                for (zone_id, zone) in &zones_snapshot {
                                    match zone.state {
                                        State::Stopped => has_stopped_zones.push(zone_id.clone()),
                                        State::Loading | State::Playing | State::Paused => has_non_stop_zones.push(zone_id.clone()),
                                    }
                                }

                                // Handle pending stops for zones that are no longer stopped
                                let pending_stops_clone = pending_stops.clone();
                                for zone_id in &has_non_stop_zones {
                                    let mut pending = pending_stops_clone.lock().await;
                                    if let Some(task) = pending.remove(zone_id) {
                                        log::debug!("Cancelling pending stop for zone {} due to state change", zone_id);
                                        task.abort();
                                    }
                                }

                                // Handle stopped zones with delay logic
                                for zone_id in &has_stopped_zones {
                                    let mut pending = pending_stops_clone.lock().await;
                                    if let Some(existing_task) = pending.remove(zone_id) {
                                        // There's already a pending stop - cancel it and broadcast immediately
                                        log::debug!("Zone {} stopped again - broadcasting immediately (user intent)", zone_id);
                                        existing_task.abort();
                                        drop(pending); // Release lock before broadcasting

                                        let zones_clone = zones.clone();
                                        let zones_raw_json_clone = zones_raw_json.clone();
                                        let ws_tx_clone = ws_tx.clone();
                                        tokio::spawn(async move {
                                            let (zone_data, raw_zones, raw_json) = build_ws_zone_data_from_zones(zones_clone, zones_raw_json_clone).await;
                                            log::debug!("Broadcasting immediate stop for zone (double-stop detected)");
                                            let _ = ws_tx_clone.send(WsMessage::ZonesChanged { now_playing: zone_data, raw_zones, raw_json });
                                        });
                                    } else {
                                        // No pending stop - start delayed broadcast
                                        log::debug!("Zone {} stopped - delaying broadcast by {}ms", zone_id, STOP_BROADCAST_DELAY_MS);

                                        let ws_tx_clone = ws_tx.clone();
                                        let zones_clone = zones.clone();
                                        let zones_raw_json_clone = zones_raw_json.clone();
                                        let zone_id_clone = zone_id.clone();
                                        let pending_stops_clone2 = pending_stops_clone.clone();

                                        let task = tokio::spawn(async move {
                                            tokio::time::sleep(tokio::time::Duration::from_millis(STOP_BROADCAST_DELAY_MS)).await;

                                            // Build and broadcast stop
                                            let (zone_data, raw_zones, raw_json) = build_ws_zone_data_from_zones(zones_clone, zones_raw_json_clone).await;
                                            log::debug!("Broadcasting delayed stop for zone {}", zone_id_clone);
                                            let _ = ws_tx_clone.send(WsMessage::ZonesChanged { now_playing: zone_data, raw_zones, raw_json });

                                            // Remove self from pending map
                                            pending_stops_clone2.lock().await.remove(&zone_id_clone);
                                        });

                                        pending.insert(zone_id.clone(), task);
                                    }
                                }

                                // Skip broadcasting if we only have "loading" state zones
                                let has_non_loading = zones_snapshot.values()
                                    .any(|zone| zone.state != State::Loading);

                                // Broadcast non-stop zone updates immediately
                                if !has_non_stop_zones.is_empty() && has_non_loading {
                                    if !has_dcs_playing_zones.is_empty() {
                                        // If we have dCS zones in Playing state, give dCS device a moment to update
                                        log::debug!("Found {} dCS zones in Playing state, waiting {}ms for dCS to update",
                                                   has_dcs_playing_zones.len(), DCS_UPDATE_DELAY_MS);

                                        let ws_tx_clone = ws_tx.clone();
                                        let zones_clone = zones.clone();
                                        let zones_raw_json_clone = zones_raw_json.clone();

                                        tokio::spawn(async move {
                                            // Brief delay to let dCS device process the new stream
                                            tokio::time::sleep(tokio::time::Duration::from_millis(DCS_UPDATE_DELAY_MS)).await;

                                            // Build zone data with dCS format
                                            let (zone_data, raw_zones, raw_json) = build_ws_zone_data_from_zones(zones_clone, zones_raw_json_clone).await;

                                            // Broadcast zone change with full data
                                            log::debug!("Broadcasting zone change (after dCS delay) with {} zones of data", zone_data.len());
                                            for item in &zone_data {
                                                log::debug!("  Broadcasting zone {}: state={}, track={:?}, dcs_format={:?}",
                                                           item.zone_name, item.state, item.track, item.dcs_format);
                                            }
                                            let _ = ws_tx_clone.send(WsMessage::ZonesChanged { now_playing: zone_data, raw_zones, raw_json });
                                        });
                                    } else {
                                        // No dCS zones in Playing state, build and broadcast immediately
                                        let zones_clone = zones.clone();
                                        let zones_raw_json_clone = zones_raw_json.clone();
                                        let ws_tx_clone = ws_tx.clone();
                                        tokio::spawn(async move {
                                            let (zone_data, raw_zones, raw_json) = build_ws_zone_data_from_zones(zones_clone, zones_raw_json_clone).await;
                                            log::debug!("Broadcasting zone change (no dCS delay) with {} zones of data", zone_data.len());
                                            for item in &zone_data {
                                                log::debug!("  Broadcasting zone {}: state={}, track={:?}, dcs_format={:?}",
                                                           item.zone_name, item.state, item.track, item.dcs_format);
                                            }
                                            let _ = ws_tx_clone.send(WsMessage::ZonesChanged { now_playing: zone_data, raw_zones, raw_json });
                                        });
                                    }
                                } else if has_stopped_zones.is_empty() && !has_non_loading {
                                    log::debug!("Skipping zone change broadcast - all zones in 'loading' state");
                                }
                            }
                            Parsed::ZonesRemoved(zones_removed) => {
                                log::debug!("Roon API ZonesRemoved response:\n{:#?}", zones_removed);
                                log::debug!("Zones removed");
                                let mut zone_map = zones.write().await;

                                // Remove zones that are gone
                                for zone_id in zones_removed {
                                    zone_map.remove(&zone_id);
                                }

                                // Broadcast zone change with full data via WebSocket
                                let zones_clone = zones.clone();
                                let zones_raw_json_clone = zones_raw_json.clone();
                                let ws_tx_clone = ws_tx.clone();
                                tokio::spawn(async move {
                                    let (zone_data, raw_zones, raw_json) = build_ws_zone_data_from_zones(zones_clone, zones_raw_json_clone).await;
                                    log::debug!("Broadcasting zone removal with {} zones of data", zone_data.len());
                                    for item in &zone_data {
                                        log::debug!("  Broadcasting zone {}: state={}, track={:?}, dcs_format={:?}",
                                                   item.zone_name, item.state, item.track, item.dcs_format);
                                    }
                                    let _ = ws_tx_clone.send(WsMessage::ZonesChanged { now_playing: zone_data, raw_zones, raw_json });
                                });
                            }
                            Parsed::ZonesSeek(zones_seek) => {
                                log::trace!("Roon API ZonesSeek response:\n{:#?}", zones_seek);
                                log::trace!("Zone seek position updated");
                                let mut zone_map = zones.write().await;

                                // Update seek positions and broadcast individual updates
                                for zone_seek in zones_seek {
                                    if let Some(zone) = zone_map.get_mut(&zone_seek.zone_id) {
                                        if let Some(now_playing) = zone.now_playing.as_mut() {
                                            now_playing.seek_position = zone_seek.seek_position;
                                        }
                                    }

                                    // Broadcast seek update for this zone
                                    let _ = ws_tx.send(WsMessage::SeekUpdated {
                                        zone_id: zone_seek.zone_id,
                                        seek_position: zone_seek.seek_position,
                                        queue_time_remaining: zone_seek.queue_time_remaining,
                                    });
                                }
                            }
                            Parsed::Jpeg((image_key, data)) => {
                                log::debug!("Roon API JPEG image response: key={}, size={} bytes", image_key, data.len());
                                images.write().await.insert(image_key, ImageData {
                                    content_type: "image/jpeg".to_string(),
                                    data,
                                });
                            }
                            Parsed::Png((image_key, data)) => {
                                log::debug!("Roon API PNG image response: key={}, size={} bytes", image_key, data.len());
                                images.write().await.insert(image_key, ImageData {
                                    content_type: "image/png".to_string(),
                                    data,
                                });
                            }
                            Parsed::BrowseResult(result, session_key) => {
                                log::debug!("Roon API BrowseResult response:\n{:#?}", result);
                                log::info!("Browse result - action: {:?}", result.action);
                                if let Some(item) = &result.item {
                                    log::info!("  Item title: {}", item.title);
                                    if let Some(subtitle) = &item.subtitle {
                                        log::info!("  Item subtitle: {}", subtitle);
                                    }
                                }
                                if let Some(list) = &result.list {
                                    log::info!("  List title: {}", list.title);
                                    if let Some(subtitle) = &list.subtitle {
                                        log::info!("  List subtitle: {}", subtitle);
                                    }

                                    // Auto-load the list to see what items are available
                                    let browse_svc = browse_service.read().await;
                                    if let Some(browse) = browse_svc.as_ref() {
                                        log::info!("  Loading list items...");
                                        let opts = LoadOpts {
                                            multi_session_key: session_key,
                                            offset: 0,
                                            count: Some(list.count.min(20)), // Limit to first 20 items
                                            set_display_offset: 0,
                                            ..Default::default()
                                        };
                                        browse.load(&opts).await;
                                    }
                                }
                            }
                            Parsed::LoadResult(result, session_key) => {
                                log::debug!("Roon API LoadResult response:\n{:#?}", result);
                                log::info!("Load result - {} items", result.items.len());
                                let mut now_playing_item_key = None;

                                for (i, item) in result.items.iter().enumerate() {
                                    log::info!("  Item {}: {}", i, item.title);
                                    if let Some(subtitle) = &item.subtitle {
                                        log::info!("    Subtitle: {}", subtitle);
                                    }

                                    // Look for "Now Playing" or "Queue" items to browse into
                                    if (item.title.contains("Now Playing") || item.title.contains("Queue"))
                                        && item.item_key.is_some() && now_playing_item_key.is_none() {
                                        now_playing_item_key = item.item_key.clone();
                                    }
                                }

                                // If we found a "Now Playing" item, browse into it
                                if let Some(item_key) = now_playing_item_key {
                                    let browse_svc = browse_service.read().await;
                                    if let Some(browse) = browse_svc.as_ref() {
                                        log::info!("Browsing into Now Playing/Queue...");
                                        let opts = BrowseOpts {
                                            multi_session_key: session_key,
                                            item_key: Some(item_key),
                                            ..Default::default()
                                        };
                                        browse.browse(&opts).await;
                                    }
                                }
                            }
                            Parsed::Queue(queue_items) => {
                                log::debug!("Roon API Queue response:\n{:#?}", queue_items);
                                log::info!("Queue snapshot - {} items", queue_items.len());

                                // Store queue for the active subscribed zone
                                let active_zone = active_queue_zone.read().await;
                                if let Some(zone_id) = active_zone.as_ref() {
                                    log::info!("Storing queue for active zone: {}", zone_id);
                                    queues.write().await.insert(zone_id.clone(), queue_items);

                                    // Notify waiting subscribers that queue data has arrived
                                    queue_ready.notify_waiters();
                                } else {
                                    log::warn!("Received queue data but no active queue zone subscription");
                                }
                            }
                            Parsed::QueueChanges(queue_changes) => {
                                log::debug!("Roon API QueueChanges response:\n{:#?}", queue_changes);
                                log::info!("Queue changes - {} operations", queue_changes.len());

                                // Apply changes to the active zone's queue
                                let active_zone = active_queue_zone.read().await;
                                if let Some(zone_id) = active_zone.as_ref() {
                                    let mut queues_map = queues.write().await;
                                    if let Some(queue) = queues_map.get_mut(zone_id) {
                                        for change in queue_changes {
                                            log::info!("  Operation: {:?} at index {}", change.operation, change.index);

                                            match change.operation {
                                                roon_api::transport::QueueOperation::Insert => {
                                                    if let Some(items) = &change.items {
                                                        let idx = change.index;
                                                        for (i, item) in items.iter().enumerate() {
                                                            log::info!("    Inserting: {} (id: {})", item.two_line.line1, item.queue_item_id);
                                                            // Items from queue changes are already QueueItem
                                                            queue.insert(idx + i, item.clone());
                                                        }
                                                    }
                                                }
                                                roon_api::transport::QueueOperation::Remove => {
                                                    let count = change.count.unwrap_or(1);
                                                    let idx = change.index;
                                                    log::info!("    Removing {} items at index {}", count, idx);
                                                    for _ in 0..count {
                                                        if idx < queue.len() {
                                                            queue.remove(idx);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        log::info!("Queue updated for zone {} - now has {} items", zone_id, queue.len());

                                        // Notify frontend via WebSocket that queue has changed
                                        let _ = ws_tx.send(WsMessage::QueueChanged {
                                            zone_id: zone_id.clone(),
                                        });
                                    } else {
                                        log::warn!("Received queue changes for zone {} but no queue cached", zone_id);
                                    }
                                } else {
                                    log::warn!("Received queue changes but no active queue zone subscription");
                                }
                            }
                            Parsed::Error(err) => {
                                log::error!("Roon API error: {}", err);
                            }
                            _ => {}
                        }
                    }
                }
            });

            // Keep handlers alive
            tokio::spawn(async move {
                while let Some(_) = handlers.join_next().await {
                    // Keep running
                }
            });
        } else {
            return Err("Failed to start Roon discovery".into());
        }

        Ok(())
    }

    /// Check if connected to Roon Core
    pub async fn is_connected(&self) -> bool {
        *self.connected.read().await
    }

    /// Get the name of the connected Roon Core
    pub async fn get_core_name(&self) -> Option<String> {
        self.core_name.read().await.clone()
    }

    /// Get all available zones
    pub async fn get_zones(&self) -> Vec<Zone> {
        self.zones.read().await.values().cloned().collect()
    }

    /// Build WebSocket zone data with dCS format
    /// This method calls the standalone function with the zones Arc
    /// Returns the simplified WsZoneData, raw Zones from Roon, and raw JSON string
    pub async fn build_ws_zone_data(&self) -> (Vec<WsZoneData>, Vec<Zone>, Option<String>) {
        build_ws_zone_data_from_zones(self.zones.clone(), self.zones_raw_json.clone()).await
    }

    /// OLD IMPLEMENTATION - kept for reference but not used
    #[allow(dead_code)]
    async fn build_ws_zone_data_old(&self) -> Vec<WsZoneData> {
        use crate::dcs;

        let zones = self.get_zones().await;

        // Process all zones in parallel
        let zone_futures: Vec<_> = zones.into_iter().map(|zone| {
            async move {
                // Fetch dCS format on-demand if this is a dCS Vivaldi zone in Playing state
                let dcs_format = if zone.display_name.starts_with("dCS Vivaldi")
                    && format!("{:?}", zone.state).to_lowercase() == "playing" {

                    match dcs::get_playback_info("dcs-vivaldi.local").await {
                        Ok(playback_info) => {
                            // Extract format from audio_format field
                            if let Some(audio_format) = playback_info.audio_format {
                                // Only return format if bits_per_sample is valid (non-zero)
                                if let Some(bits) = audio_format.bits_per_sample {
                                    if bits > 0 {
                                        let sample_rate_str = if let Some(freq) = audio_format.sample_frequency {
                                            if freq >= 1000 {
                                                format!("{} kHz", freq / 1000)
                                            } else {
                                                format!("{} Hz", freq)
                                            }
                                        } else {
                                            String::new()
                                        };

                                        let bit_depth_str = format!("{} bit", bits);

                                        if !sample_rate_str.is_empty() {
                                            Some(format!("{} {}", sample_rate_str, bit_depth_str))
                                        } else {
                                            None
                                        }
                                    } else {
                                        log::debug!("dCS format has bits_per_sample=0, not displaying");
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to get dCS playback info: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };

                // Extract track info if available
                let (track, artist, album, position_seconds, length_seconds, image_key) =
                    if let Some(now_playing) = zone.now_playing.as_ref() {
                        let three_line = &now_playing.three_line;
                        (
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
                            now_playing.seek_position,
                            now_playing.length,
                            now_playing.image_key.clone(),
                        )
                    } else {
                        (None, None, None, None, None, None)
                    };

                // Extract is_muted from the first output with volume info
                let is_muted = zone.outputs.iter()
                    .find_map(|output| output.volume.as_ref().and_then(|v| v.is_muted));

                // Build zone name with selected output display name
                let zone_name = if let Some(output) = zone.outputs.first() {
                    // Check if there's a selected source control
                    if let Some(source_controls) = &output.source_controls {
                        if let Some(selected_source) = source_controls.iter()
                            .find(|sc| sc.status == roon_api::transport::Status::Selected) {
                            format!("{} ({})", zone.display_name, selected_source.display_name)
                        } else {
                            zone.display_name.clone()
                        }
                    } else {
                        zone.display_name.clone()
                    }
                } else {
                    zone.display_name.clone()
                };

                WsZoneData {
                    zone_id: zone.zone_id,
                    zone_name,
                    state: format!("{:?}", zone.state),
                    track,
                    artist,
                    album,
                    position_seconds,
                    length_seconds,
                    image_key,
                    is_muted,
                    dcs_format,
                }
            }
        }).collect();

        futures_util::future::join_all(zone_futures).await
    }

    /// Get queue for a specific zone
    pub async fn get_queue(&self, zone_id: &str) -> Option<Vec<QueueItem>> {
        self.queues.read().await.get(zone_id).cloned()
    }

    /// Subscribe to queue updates for a specific zone
    /// Unsubscribes from any previously active queue subscription first
    /// Waits for the queue data to arrive with a 2 second timeout
    pub async fn subscribe_to_queue(&self, zone_id: &str) {
        let transport_svc = self.transport_service.read().await;
        if let Some(transport) = transport_svc.as_ref() {
            // Check if we're already subscribed to this zone
            let current_zone = self.active_queue_zone.read().await;
            if current_zone.as_deref() == Some(zone_id) {
                log::debug!("Already subscribed to queue for zone {}", zone_id);
                return;
            }
            drop(current_zone);

            // Unsubscribe from previous queue if any
            let mut active_zone = self.active_queue_zone.write().await;
            if active_zone.is_some() {
                log::info!("Unsubscribing from previous queue...");
                transport.unsubscribe_queue().await;
            }

            // Subscribe to new queue
            log::info!("Subscribing to queue for zone {}...", zone_id);
            transport.subscribe_queue(zone_id, 50).await;

            // Update active zone tracking
            *active_zone = Some(zone_id.to_string());
            drop(active_zone);

            // Wait for queue data to arrive (with 2 second timeout)
            log::debug!("Waiting for queue data to arrive...");
            let timeout = tokio::time::Duration::from_secs(2);
            match tokio::time::timeout(timeout, self.queue_ready.notified()).await {
                Ok(_) => {
                    log::debug!("Queue data received for zone {}", zone_id);
                }
                Err(_) => {
                    log::warn!("Timeout waiting for queue data for zone {}", zone_id);
                }
            }
        } else {
            log::warn!("Transport service not available for queue subscription");
        }
    }

    /// Request an image from Roon
    /// Returns the request ID if successful, None if the service is unavailable
    pub async fn request_image(&self, image_key: &str, width: u32, height: u32) -> Option<usize> {
        log::debug!("request_image called for key: {}", image_key);
        let image_service = self.image_service.read().await;
        if let Some(image) = image_service.as_ref() {
            log::debug!("Image service available, sending request");
            let scaling = Scaling::new(Scale::Fit, width, height);
            let args = ImageArgs::new(Some(scaling), Some(Format::Jpeg));
            let result = image.get_image(image_key, args).await;
            log::debug!("Image request result: {:?}", result);
            result
        } else {
            log::warn!("Image service not available");
            None
        }
    }

    /// Get a cached image by key
    pub async fn get_image(&self, image_key: &str) -> Option<ImageData> {
        self.images.read().await.get(image_key).cloned()
    }

    /// Wait for authorization from Roon Core
    /// Prints a message every `interval_secs` seconds while waiting
    /// Returns true if connected, false if timed out (when timeout_secs is Some)
    pub async fn wait_for_authorization(&self, interval_secs: u64, timeout_secs: Option<u64>) -> bool {
        use tokio::time::{interval, Duration, Instant};

        let start = Instant::now();
        let mut check_interval = interval(Duration::from_secs(interval_secs));

        // Skip the first immediate tick
        check_interval.tick().await;

        loop {
            // Check if already connected
            if self.is_connected().await {
                return true;
            }

            // Check for timeout
            if let Some(timeout) = timeout_secs {
                if start.elapsed() >= Duration::from_secs(timeout) {
                    return false;
                }
            }

            // Wait for next interval
            check_interval.tick().await;

            // Check again after waiting
            if self.is_connected().await {
                return true;
            }

            // Print waiting message
            println!("Still waiting for authorization in Roon Settings > Extensions...");
        }
    }

    /// Control playback for a zone
    pub async fn control_zone(&self, zone_id: &str, control: &str) -> Result<(), String> {
        use roon_api::transport::Control;

        log::debug!("Roon API control_zone request: zone_id={}, control={}", zone_id, control);

        let transport = self.transport_service.read().await;
        if let Some(transport) = transport.as_ref() {
            let control_enum = match control {
                "play" => Control::Play,
                "pause" => Control::Pause,
                "playpause" => Control::PlayPause,
                "stop" => Control::Stop,
                "previous" => Control::Previous,
                "next" => Control::Next,
                _ => return Err(format!("Invalid control: {}", control)),
            };

            transport.control(zone_id, &control_enum).await;
            log::debug!("Roon API control_zone completed successfully");
            Ok(())
        } else {
            Err("Transport service not available".to_string())
        }
    }

    /// Play from a specific queue item
    pub async fn play_from_queue_item(&self, zone_id: &str, queue_item_id: u32) -> Result<(), String> {
        log::debug!("Roon API play_from_queue_item request: zone_id={}, queue_item_id={}", zone_id, queue_item_id);

        let transport = self.transport_service.read().await;
        if let Some(transport) = transport.as_ref() {
            transport.play_from_here(zone_id, queue_item_id).await;
            log::debug!("Roon API play_from_queue_item completed successfully");
            Ok(())
        } else {
            Err("Transport service not available".to_string())
        }
    }

    /// Seek to a position in a zone
    pub async fn seek_zone(&self, zone_id: &str, seconds: i32) -> Result<(), String> {
        use roon_api::transport::Seek;

        log::debug!("Roon API seek_zone request: zone_id={}, seconds={}", zone_id, seconds);

        let transport = self.transport_service.read().await;
        if let Some(transport) = transport.as_ref() {
            transport.seek(zone_id, &Seek::Absolute, seconds).await;
            log::debug!("Roon API seek_zone completed successfully");
            Ok(())
        } else {
            Err("Transport service not available".to_string())
        }
    }

    /// Mute or unmute an output
    pub async fn mute_output(&self, zone_id: &str, mute: bool) -> Result<(), String> {
        use roon_api::transport::volume::Mute;

        log::debug!("Roon API mute_output request: zone_id={}, mute={}", zone_id, mute);

        // First, get the zone to find its output ID
        let zones = self.zones.read().await;
        let zone = zones.get(zone_id).ok_or("Zone not found")?;

        if zone.outputs.is_empty() {
            return Err("Zone has no outputs".to_string());
        }

        let output_id = &zone.outputs[0].output_id;

        let transport = self.transport_service.read().await;
        if let Some(transport) = transport.as_ref() {
            let mute_enum = if mute { Mute::Mute } else { Mute::Unmute };
            transport.mute(output_id, &mute_enum).await;
            log::debug!("Roon API mute_output completed successfully");
            Ok(())
        } else {
            Err("Transport service not available".to_string())
        }
    }
}
