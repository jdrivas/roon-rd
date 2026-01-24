use serde::{Deserialize, Serialize};
use std::error::Error;

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
