use serde::{Deserialize, Serialize};
use std::error::Error;
use std::collections::HashMap;
use std::time::Duration;
use mdns_sd::{ServiceDaemon, ServiceEvent};

/// mDNS service type for dCS devices (StreamUnlimited Engine)
const DCS_SERVICE_TYPE: &str = "_sueS800Device._tcp.local.";

/// Represents a discovered dCS device on the network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcsDevice {
    /// Friendly name (e.g., "dCS Vivaldi Upsampler")
    pub name: String,
    /// Device type (e.g., "Vivaldi Upsampler", "Network Bridge")
    pub unit_type: String,
    /// Unique identifier (e.g., "Vivaldi-68c90b2ddc26")
    pub uuid: String,
    /// Hostname (e.g., "dcs-vivaldi.local")
    pub hostname: String,
    /// IP address
    pub ip: Option<String>,
    /// Port (usually 80)
    pub port: u16,
    /// Manufacturer
    pub manufacturer: Option<String>,
}

impl DcsDevice {
    /// Get the host string suitable for API calls (hostname:port or ip:port)
    pub fn host(&self) -> String {
        if self.port == 80 {
            self.ip.clone().unwrap_or_else(|| self.hostname.clone())
        } else {
            format!("{}:{}", self.ip.clone().unwrap_or_else(|| self.hostname.clone()), self.port)
        }
    }
}

/// Discover dCS devices on the network using mDNS
/// 
/// Browses for `_sueS800Device._tcp` services which dCS devices advertise.
/// Returns a list of discovered devices with their metadata.
pub fn discover_devices(timeout_secs: u64) -> Result<Vec<DcsDevice>, Box<dyn Error + Send + Sync>> {
    log::info!("Starting dCS device discovery ({}s timeout)", timeout_secs);

    let mdns = ServiceDaemon::new()?;
    let receiver = mdns.browse(DCS_SERVICE_TYPE)?;

    let mut devices: HashMap<String, DcsDevice> = HashMap::new();
    let timeout = Duration::from_secs(timeout_secs);
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        log::debug!("Resolved dCS service: {:?}", info);

                        // Parse TXT records for device metadata
                        let properties = info.get_properties();
                        
                        let name = properties.get_property_val_str("name")
                            .unwrap_or_else(|| info.get_fullname())
                            .replace("\\", "")
                            .to_string();
                        
                        let unit_type = properties.get_property_val_str("dcsunittype")
                            .unwrap_or("Unknown")
                            .replace("\\", "")
                            .to_string();
                        
                        let uuid = properties.get_property_val_str("uuid")
                            .unwrap_or_else(|| info.get_fullname())
                            .to_string();
                        
                        let ip = properties.get_property_val_str("ip")
                            .map(|s| s.to_string())
                            .or_else(|| {
                                // Fallback to addresses from service info
                                info.get_addresses().iter().next().map(|a| a.to_string())
                            });
                        
                        let manufacturer = properties.get_property_val_str("manufacturer")
                            .map(|s| s.replace("\\", "").to_string());

                        let hostname = info.get_hostname().trim_end_matches('.').to_string();
                        let port = info.get_port();

                        let device = DcsDevice {
                            name,
                            unit_type,
                            uuid: uuid.clone(),
                            hostname,
                            ip,
                            port,
                            manufacturer,
                        };

                        log::info!("Discovered dCS device: {} ({}) at {}", 
                                   device.name, device.unit_type, device.host());
                        
                        devices.insert(uuid, device);
                    }
                    ServiceEvent::SearchStarted(_) => {
                        log::debug!("mDNS search started for dCS devices");
                    }
                    ServiceEvent::ServiceFound(_, _) => {
                        // Service found but not yet resolved, wait for ServiceResolved
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        log::debug!("dCS service removed: {}", fullname);
                    }
                    ServiceEvent::SearchStopped(_) => {
                        log::debug!("mDNS search stopped");
                        break;
                    }
                }
            }
            Err(flume::RecvTimeoutError::Timeout) => {
                // Continue waiting
            }
            Err(flume::RecvTimeoutError::Disconnected) => {
                log::debug!("mDNS receiver disconnected");
                break;
            }
        }
    }

    // Stop browsing
    let _ = mdns.stop_browse(DCS_SERVICE_TYPE);

    let result: Vec<DcsDevice> = devices.into_values().collect();
    log::info!("dCS discovery complete: found {} device(s)", result.len());

    Ok(result)
}

// Global in-memory device cache using std::sync for thread safety
use std::sync::{Mutex, OnceLock};

static DEVICE_CACHE: OnceLock<Mutex<Vec<DcsDevice>>> = OnceLock::new();

/// Get the device cache, initializing with discovery if needed
fn get_device_cache() -> &'static Mutex<Vec<DcsDevice>> {
    DEVICE_CACHE.get_or_init(|| {
        log::info!("Initializing dCS device cache...");
        let devices = discover_devices(3).unwrap_or_else(|e| {
            log::warn!("Failed to discover dCS devices: {}", e);
            Vec::new()
        });
        log::info!("dCS cache initialized with {} device(s)", devices.len());
        Mutex::new(devices)
    })
}

/// Refresh the device cache by running discovery again
pub fn refresh_device_cache() -> Result<Vec<DcsDevice>, Box<dyn std::error::Error + Send + Sync>> {
    log::info!("Discovering dCS devices...");
    let devices = discover_devices(3)?;
    
    if let Ok(mut cache) = get_device_cache().lock() {
        *cache = devices.clone();
    }
    log::info!("dCS cache refreshed with {} device(s)", devices.len());
    
    Ok(devices)
}

/// Get cached devices (runs discovery on first call)
pub fn get_cached_devices() -> Vec<DcsDevice> {
    get_device_cache()
        .lock()
        .map(|cache| cache.clone())
        .unwrap_or_default()
}

/// Find a dCS device that matches a Roon zone name
/// 
/// Matches zone names like "dCS Vivaldi Upsampler" to devices with unit_type "Vivaldi Upsampler"
/// or zone names containing "Network Bridge" to devices with that unit type.
pub fn find_device_for_zone(zone_name: &str) -> Option<DcsDevice> {
    let devices = get_cached_devices();
    
    log::debug!("find_device_for_zone: looking for zone '{}' in {} cached devices", 
               zone_name, devices.len());
    
    // Try to match zone name to device unit_type or name
    for device in &devices {
        log::trace!("  Checking device '{}' (unit_type: '{}')", device.name, device.unit_type);
        // Check if zone name contains the device's unit type (e.g., "Vivaldi Upsampler", "Network Bridge")
        if zone_name.contains(&device.unit_type) {
            log::debug!("Matched zone '{}' to dCS device '{}' ({})", 
                       zone_name, device.name, device.unit_type);
            return Some(device.clone());
        }
        
        // Also check if zone name starts with "dCS" and contains part of the device name
        if zone_name.starts_with("dCS") {
            // Extract key words from device name for matching
            let name_parts: Vec<&str> = device.name.split_whitespace().collect();
            for part in name_parts {
                if part.len() > 3 && zone_name.contains(part) {
                    log::debug!("Matched zone '{}' to dCS device '{}' via name part '{}'", 
                               zone_name, device.name, part);
                    return Some(device.clone());
                }
            }
        }
    }
    
    log::debug!("No dCS device match found for zone '{}'", zone_name);
    None
}

/// dCS API base URL helper
fn api_url(host: &str, endpoint: &str, path: &str, roles: &str) -> String {
    format!(
        "http://{}/api/{}?path={}&roles={}",
        host,
        endpoint,
        urlencoding::encode(path),
        urlencoding::encode(roles)
    )
}

/// Represents audio format information from the dCS device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcsAudioFormat {
    pub bit_depth: Option<i32>,
    pub sample_rate: Option<i32>,
    pub input_mode: Option<String>,
}

/// Represents playback information from the dCS device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcsPlaybackInfo {
    pub state: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub service_id: Option<String>,
    pub duration: Option<i32>,
    pub audio_format: Option<AudioFormatDetails>,
}

/// Audio format details from player data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFormatDetails {
    pub sample_frequency: Option<i32>,
    pub bits_per_sample: Option<i32>,
    pub nr_audio_channels: Option<i32>,
}

/// Device settings from the dCS device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcsDeviceSettings {
    pub display_brightness: Option<i32>,
    pub display_off: Option<bool>,
    pub sync_mode: Option<String>,
}

/// Generic response type for dCS API getValue responses
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DcsValueResponse {
    #[serde(rename = "i32_")]
    i32_value: Option<i32>,
    #[serde(rename = "i64_")]
    i64_value: Option<i64>,
    #[serde(rename = "string_")]
    string_value: Option<String>,
    #[serde(rename = "bool_")]
    bool_value: Option<bool>,
    #[serde(rename = "type")]
    value_type: Option<String>,
}

/// Get current audio format from dCS device
pub async fn get_audio_format(host: &str) -> Result<DcsAudioFormat, Box<dyn Error>> {
    log::info!("Fetching audio format from dCS device: {}", host);

    let client = reqwest::Client::new();

    // Query bit depth
    let bit_depth_url = api_url(host, "getData", "dcsworker:/dcs/currentBitDepth", "value");
    let bit_depth_resp: Vec<DcsValueResponse> = client.get(&bit_depth_url).send().await?.json().await?;
    let bit_depth = bit_depth_resp.first().and_then(|v| v.i32_value);

    // Query sample rate
    let sample_rate_url = api_url(host, "getData", "dcsworker:/dcs/inputSampleRateCurrent", "value");
    let sample_rate_resp: Vec<DcsValueResponse> = client.get(&sample_rate_url).send().await?.json().await?;
    let sample_rate = sample_rate_resp.first().and_then(|v| v.i32_value);

    // Query input mode
    let input_mode_url = api_url(host, "getData", "dcsworker:/dcs/settings/inputMode", "value");
    let input_mode_resp: Vec<DcsValueResponse> = client.get(&input_mode_url).send().await?.json().await?;
    let input_mode = input_mode_resp.first().and_then(|v| v.string_value.clone());

    let format = DcsAudioFormat {
        bit_depth,
        sample_rate,
        input_mode,
    };

    log::debug!("dCS audio format retrieved: bit_depth={:?}, sample_rate={:?}, input_mode={:?}",
                format.bit_depth, format.sample_rate, format.input_mode);

    Ok(format)
}

/// Get current playback information from dCS device
pub async fn get_playback_info(host: &str) -> Result<DcsPlaybackInfo, Box<dyn Error>> {
    log::info!("Fetching playback info from dCS device: {}", host);

    let client = reqwest::Client::new();
    let url = api_url(host, "getData", "/player/data", "title,value");

    let response = client.get(&url).send().await?;
    let text = response.text().await?;

    // Parse the complex JSON response
    // The response is an array with two elements: first is empty string, second is the data object
    let json: serde_json::Value = serde_json::from_str(&text)?;

    if let Some(data) = json.get(1) {
        let state = data["state"].as_str().map(|s| s.to_string());

        // Extract track metadata
        let track_roles = &data["trackRoles"];
        let media_data = &track_roles["mediaData"];
        let meta_data = &media_data["metaData"];

        let title = track_roles["title"].as_str().map(|s| s.to_string());
        let artist = meta_data["artist"].as_str().map(|s| s.to_string());
        let album = meta_data["album"].as_str().map(|s| s.to_string());
        let service_id = meta_data["serviceID"].as_str().map(|s| s.to_string());

        // Extract duration
        let duration = data["status"]["duration"].as_i64().map(|d| d as i32);

        // Extract audio format from resources
        let resources = &media_data["resources"];
        let audio_format = if let Some(resource) = resources.get(0) {
            Some(AudioFormatDetails {
                sample_frequency: resource["sampleFrequency"].as_i64().map(|v| v as i32),
                bits_per_sample: resource["bitsPerSample"].as_i64().map(|v| v as i32),
                nr_audio_channels: resource["nrAudioChannels"].as_i64().map(|v| v as i32),
            })
        } else {
            None
        };

        let playback_info = DcsPlaybackInfo {
            state,
            title,
            artist,
            album,
            service_id,
            duration,
            audio_format,
        };

        log::debug!(
            "dCS playback info retrieved: state={:?}, title={:?}, artist={:?}, album={:?}, \
             service_id={:?}, duration={:?}, sample_frequency={:?}, bits_per_sample={:?}, channels={:?}",
            playback_info.state,
            playback_info.title,
            playback_info.artist,
            playback_info.album,
            playback_info.service_id,
            playback_info.duration,
            playback_info.audio_format.as_ref().and_then(|f| f.sample_frequency),
            playback_info.audio_format.as_ref().and_then(|f| f.bits_per_sample),
            playback_info.audio_format.as_ref().and_then(|f| f.nr_audio_channels)
        );

        Ok(playback_info)
    } else {
        Err("Invalid response format from dCS API".into())
    }
}

/// Get device settings from dCS device
pub async fn get_device_settings(host: &str) -> Result<DcsDeviceSettings, Box<dyn Error>> {
    log::info!("Fetching device settings from dCS device: {}", host);

    let client = reqwest::Client::new();

    // Query display brightness
    let brightness_url = api_url(host, "getData", "dcsworker:/dcs/unitSettings/displayBrightness", "value");
    let brightness_resp: Vec<DcsValueResponse> = client.get(&brightness_url).send().await?.json().await?;
    let display_brightness = brightness_resp.first().and_then(|v| v.i32_value);

    // Query display off state
    let display_off_url = api_url(host, "getData", "dcsworker:/dcs/unitSettings/displayOff", "value");
    let display_off_resp: Vec<DcsValueResponse> = client.get(&display_off_url).send().await?.json().await?;
    let display_off = display_off_resp.first().and_then(|v| v.bool_value);

    // Query sync mode
    let sync_mode_url = api_url(host, "getData", "dcsworker:/dcs/unitSettings/syncMode", "value");
    let sync_mode_resp: Vec<DcsValueResponse> = client.get(&sync_mode_url).send().await?.json().await?;
    let sync_mode = sync_mode_resp.first().and_then(|v| v.string_value.clone());

    let settings = DcsDeviceSettings {
        display_brightness,
        display_off,
        sync_mode,
    };

    log::debug!("dCS device settings retrieved: display_brightness={:?}, display_off={:?}, sync_mode={:?}",
                settings.display_brightness, settings.display_off, settings.sync_mode);

    Ok(settings)
}

/// Upsampler settings from the dCS device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcsUpsamplerSettings {
    pub output_sample_rate: Option<i32>,
    pub filter: Option<i32>,
}

/// Digital input information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcsInputInfo {
    pub current_input: Option<String>,
    pub available_inputs: Vec<String>,
}

/// Play mode information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcsPlayMode {
    pub mode: Option<String>,
}

/// Menu item from getRows response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcsMenuItem {
    pub title: String,
    pub item_type: String,
    pub path: String,
    pub value: Option<serde_json::Value>,
}

/// Menu response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DcsMenu {
    pub title: String,
    pub path: String,
    pub items: Vec<DcsMenuItem>,
}

/// Get upsampler settings from dCS device
pub async fn get_upsampler_settings(host: &str) -> Result<DcsUpsamplerSettings, Box<dyn Error>> {
    log::info!("Fetching upsampler settings from dCS device: {}", host);

    let client = reqwest::Client::new();

    // Query output sample rate
    let output_rate_url = api_url(host, "getData", "dcsworker:/dcs/settings/outputSampleRate", "value");
    let output_rate_resp: Vec<DcsValueResponse> = client.get(&output_rate_url).send().await?.json().await?;
    let output_sample_rate = output_rate_resp.first().and_then(|v| v.i32_value);

    // Query filter setting
    let filter_url = api_url(host, "getData", "dcsworker:/dcs/controls/filter", "value");
    let filter_resp: Vec<DcsValueResponse> = client.get(&filter_url).send().await?.json().await?;
    let filter = filter_resp.first().and_then(|v| v.i32_value);

    let settings = DcsUpsamplerSettings {
        output_sample_rate,
        filter,
    };

    log::debug!("dCS upsampler settings retrieved: output_sample_rate={:?}, filter={:?}",
                settings.output_sample_rate, settings.filter);

    Ok(settings)
}

/// Get current playback position in milliseconds
pub async fn get_playback_position(host: &str) -> Result<i64, Box<dyn Error>> {
    log::info!("Fetching playback position from dCS device: {}", host);

    let client = reqwest::Client::new();
    let url = api_url(host, "getData", "/player/data/playTime", "value,path");

    let response: Vec<DcsValueResponse> = client.get(&url).send().await?.json().await?;

    if let Some(first) = response.first() {
        if let Some(i64_val) = first.i64_value {
            log::debug!("dCS playback position retrieved: {} ms", i64_val);
            return Ok(i64_val);
        }
    }

    log::debug!("dCS playback position: No value available");
    Err("No playback position available".into())
}

/// Get digital input information
pub async fn get_input_info(host: &str) -> Result<DcsInputInfo, Box<dyn Error>> {
    log::info!("Fetching input info from dCS device: {}", host);

    let client = reqwest::Client::new();

    // Get current input
    let current_url = api_url(host, "getData", "dcsUiMenu:/ui/currentDigital", "title,value");
    let response = client.get(&current_url).send().await?;
    let text = response.text().await?;
    let json: serde_json::Value = serde_json::from_str(&text)?;

    // Extract current input path and get its title
    let current_path = json.get(1)
        .and_then(|v| v.get("string_"))
        .and_then(|v| v.as_str());

    let mut current_input = None;
    if let Some(path) = current_path {
        // Get the title for this input
        let input_url = api_url(host, "getData", path, "title,value");
        let input_response = client.get(&input_url).send().await?;
        let input_text = input_response.text().await?;
        let input_json: serde_json::Value = serde_json::from_str(&input_text)?;
        current_input = input_json.get(0).and_then(|v| v.as_str()).map(|s| s.to_string());
    }

    // List of known digital inputs
    let available_inputs = vec![
        "Network".to_string(),
        "AES1".to_string(),
        "SPDIF1".to_string(),
        "SPDIF2".to_string(),
    ];

    let input_info = DcsInputInfo {
        current_input,
        available_inputs,
    };

    log::debug!("dCS input info retrieved: current_input={:?}, available_inputs={:?}",
                input_info.current_input, input_info.available_inputs);

    Ok(input_info)
}

/// Get play mode
pub async fn get_play_mode(host: &str) -> Result<DcsPlayMode, Box<dyn Error>> {
    log::info!("Fetching play mode from dCS device: {}", host);

    let client = reqwest::Client::new();
    let url = api_url(host, "getData", "settings:/mediaPlayer/playMode", "value");

    let response = client.get(&url).send().await?;
    let text = response.text().await?;
    let json: serde_json::Value = serde_json::from_str(&text)?;

    let mode = json.get(0)
        .and_then(|v| v.get("playerPlayMode"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let play_mode = DcsPlayMode { mode };

    log::debug!("dCS play mode retrieved: mode={:?}", play_mode.mode);

    Ok(play_mode)
}

/// Get menu items using getRows
pub async fn get_menu(host: &str, path: &str) -> Result<DcsMenu, Box<dyn Error>> {
    log::info!("Fetching menu from dCS device: {} -> {}", host, path);

    let client = reqwest::Client::new();

    // Build getRows URL
    let url = format!(
        "http://{}/api/getRows?path={}&roles=title,icon,type,path,value&from=0&to=100",
        host,
        urlencoding::encode(path)
    );

    let response = client.get(&url).send().await?;
    let text = response.text().await?;
    let json: serde_json::Value = serde_json::from_str(&text)?;

    // Check for error
    if let Some(error) = json.get("error") {
        let error_msg = error.get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(format!("API error: {}", error_msg).into());
    }

    // Extract menu title and path
    let roles = json.get("roles").ok_or("Missing roles in response")?;
    let title = roles.get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();
    let menu_path = roles.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(path)
        .to_string();

    // Extract rows
    let rows = json.get("rows")
        .and_then(|v| v.as_array())
        .ok_or("Missing rows in response")?;

    let mut items = Vec::new();
    for row in rows {
        if let Some(row_array) = row.as_array() {
            if row_array.len() >= 4 {
                let item_title = row_array[0].as_str().unwrap_or("").to_string();
                let item_type = row_array[2].as_str().unwrap_or("unknown").to_string();
                let item_path = row_array[3].as_str().unwrap_or("").to_string();
                let value = if !row_array[4].is_null() {
                    Some(row_array[4].clone())
                } else {
                    None
                };

                items.push(DcsMenuItem {
                    title: item_title,
                    item_type,
                    path: item_path,
                    value,
                });
            }
        }
    }

    let menu = DcsMenu {
        title,
        path: menu_path,
        items,
    };

    log::debug!("dCS menu retrieved: title={:?}, path={:?}, item_count={}",
                menu.title, menu.path, menu.items.len());

    Ok(menu)
}

/// Set display brightness (0-15 range)
pub async fn set_display_brightness(host: &str, brightness: i32) -> Result<(), Box<dyn Error>> {
    log::info!("Setting display brightness on dCS device: {} -> {}", host, brightness);

    if brightness < 0 || brightness > 15 {
        return Err("Brightness must be between 0 and 15".into());
    }

    let client = reqwest::Client::new();

    // Build setData URL with JSON value
    let value_json = format!("{{\"type\":\"i32_\",\"i32_\":{}}}", brightness);
    let url = format!(
        "http://{}/api/setData?path={}&role=value&value={}",
        host,
        urlencoding::encode("dcsworker:/dcs/unitSettings/displayBrightness"),
        urlencoding::encode(&value_json)
    );

    let response = client.get(&url).send().await?;
    let text = response.text().await?;

    // API returns "true" on success
    if text.trim() == "true" {
        log::debug!("dCS display brightness set successfully: {}", brightness);
        Ok(())
    } else {
        log::debug!("dCS display brightness set failed: {}", text);
        Err(format!("Failed to set brightness: {}", text).into())
    }
}

/// Set display on/off state
pub async fn set_display_off(host: &str, off: bool) -> Result<(), Box<dyn Error>> {
    log::info!("Setting display off on dCS device: {} -> {}", host, off);

    let client = reqwest::Client::new();

    // Build setData URL with JSON value
    let value_json = format!("{{\"type\":\"bool_\",\"bool_\":{}}}", off);
    let url = format!(
        "http://{}/api/setData?path={}&role=value&value={}",
        host,
        urlencoding::encode("dcsworker:/dcs/unitSettings/displayOff"),
        urlencoding::encode(&value_json)
    );

    let response = client.get(&url).send().await?;
    let text = response.text().await?;

    // API returns "true" on success
    if text.trim() == "true" {
        log::debug!("dCS display off set successfully: {}", off);
        Ok(())
    } else {
        log::debug!("dCS display off set failed: {}", text);
        Err(format!("Failed to set display off: {}", text).into())
    }
}
