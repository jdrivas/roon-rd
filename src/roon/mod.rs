use roon_api::{Info, RoonApi, CoreEvent, Services, Parsed};
use roon_api::transport::{Transport, Zone, QueueItem};
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

/// Message types for WebSocket updates
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type")]
pub enum WsMessage {
    #[serde(rename = "zones_changed")]
    ZonesChanged,
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
}

const CONFIG_PATH: &str = "roon-rd-config.json";

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
                    if let Some((_raw_msg, parsed)) = msg {
                        match parsed {
                            Parsed::RoonState(roon_state) => {
                                // Save state to persist authorization token
                                if let Err(e) = RoonApi::save_roon_state(CONFIG_PATH, roon_state) {
                                    log::error!("Failed to save Roon state: {}", e);
                                }
                            }
                            Parsed::Zones(zones_changed) => {
                                log::debug!("Zones changed, updating zone map");
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

                                // Broadcast zone change via WebSocket
                                let _ = ws_tx.send(WsMessage::ZonesChanged);
                            }
                            Parsed::ZonesRemoved(zones_removed) => {
                                log::debug!("Zones removed");
                                let mut zone_map = zones.write().await;

                                // Remove zones that are gone
                                for zone_id in zones_removed {
                                    zone_map.remove(&zone_id);
                                }

                                // Broadcast zone change via WebSocket
                                let _ = ws_tx.send(WsMessage::ZonesChanged);
                            }
                            Parsed::ZonesSeek(zones_seek) => {
                                log::debug!("Zone seek position updated");
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
                                log::debug!("Received JPEG image: {} ({} bytes)", image_key, data.len());
                                images.write().await.insert(image_key, ImageData {
                                    content_type: "image/jpeg".to_string(),
                                    data,
                                });
                            }
                            Parsed::Png((image_key, data)) => {
                                log::debug!("Received PNG image: {} ({} bytes)", image_key, data.len());
                                images.write().await.insert(image_key, ImageData {
                                    content_type: "image/png".to_string(),
                                    data,
                                });
                            }
                            Parsed::BrowseResult(result, session_key) => {
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
            Ok(())
        } else {
            Err("Transport service not available".to_string())
        }
    }

    /// Play from a specific queue item
    pub async fn play_from_queue_item(&self, zone_id: &str, queue_item_id: u32) -> Result<(), String> {
        let transport = self.transport_service.read().await;
        if let Some(transport) = transport.as_ref() {
            transport.play_from_here(zone_id, queue_item_id).await;
            Ok(())
        } else {
            Err("Transport service not available".to_string())
        }
    }

    /// Seek to a position in a zone
    pub async fn seek_zone(&self, zone_id: &str, seconds: i32) -> Result<(), String> {
        use roon_api::transport::Seek;

        let transport = self.transport_service.read().await;
        if let Some(transport) = transport.as_ref() {
            transport.seek(zone_id, &Seek::Absolute, seconds).await;
            Ok(())
        } else {
            Err("Transport service not available".to_string())
        }
    }

    /// Mute or unmute an output
    pub async fn mute_output(&self, zone_id: &str, mute: bool) -> Result<(), String> {
        use roon_api::transport::volume::Mute;

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
            Ok(())
        } else {
            Err("Transport service not available".to_string())
        }
    }
}
