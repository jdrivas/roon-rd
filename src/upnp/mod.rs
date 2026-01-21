use rupnp::ssdp::{SearchTarget, URN};
use std::time::Duration;

/// Represents a discovered UPnP device
#[derive(Debug, Clone)]
pub struct UpnpDevice {
    pub location: String,
    pub usn: String,
    pub server: Option<String>,
    pub device_type: Option<String>,
}

/// Discover UPnP devices on the network using SSDP
pub async fn discover_devices(timeout_secs: u64) -> Result<Vec<UpnpDevice>, Box<dyn std::error::Error>> {
    log::info!("Starting UPnP device discovery ({}s timeout)", timeout_secs);

    let search_target = SearchTarget::RootDevice;
    let timeout = Duration::from_secs(timeout_secs);

    let mut devices = Vec::new();

    // Use rupnp's SSDP discovery
    let discovery = rupnp::discover(&search_target, timeout).await?;

    tokio::pin!(discovery);

    while let Some(device_result) = futures_util::StreamExt::next(&mut discovery).await {
        match device_result {
            Ok(device) => {
                // Get basic device info
                let location = device.url().to_string();

                let upnp_device = UpnpDevice {
                    location: location.clone(),
                    usn: "unknown".to_string(), // rupnp doesn't expose USN directly
                    server: None,
                    device_type: Some(device.device_type().to_string()),
                };

                log::debug!("Discovered UPnP device:\n{:#?}", upnp_device);
                devices.push(upnp_device);
            }
            Err(e) => {
                log::warn!("Error discovering device: {}", e);
            }
        }
    }

    log::info!("Discovery complete: found {} device(s)", devices.len());
    Ok(devices)
}

/// Discover specifically MediaRenderer devices (audio/video players)
pub async fn discover_media_renderers(timeout_secs: u64) -> Result<Vec<UpnpDevice>, Box<dyn std::error::Error>> {
    log::info!("Starting MediaRenderer discovery ({}s timeout)", timeout_secs);

    // MediaRenderer URN
    let media_renderer_urn = URN::device("schemas-upnp-org", "MediaRenderer", 1).into();
    let timeout = Duration::from_secs(timeout_secs);

    let mut devices = Vec::new();

    let discovery = rupnp::discover(&media_renderer_urn, timeout).await?;

    tokio::pin!(discovery);

    while let Some(device_result) = futures_util::StreamExt::next(&mut discovery).await {
        match device_result {
            Ok(device) => {
                let location = device.url().to_string();

                let upnp_device = UpnpDevice {
                    location: location.clone(),
                    usn: "unknown".to_string(),
                    server: None,
                    device_type: Some("MediaRenderer".to_string()),
                };

                log::debug!("Discovered MediaRenderer:\n{:#?}", upnp_device);
                devices.push(upnp_device);
            }
            Err(e) => {
                log::warn!("Error discovering MediaRenderer: {}", e);
            }
        }
    }

    log::info!("MediaRenderer discovery complete: found {} renderer(s)", devices.len());
    Ok(devices)
}

/// Get detailed device information
pub async fn get_device_info(location: &str) -> Result<DeviceInfo, Box<dyn std::error::Error>> {
    log::info!("Fetching device info from: {}", location);
    let device = rupnp::Device::from_url(location.parse()?).await?;

    let device_info = DeviceInfo {
        friendly_name: device.friendly_name().to_string(),
        manufacturer: None, // Not directly accessible in rupnp 2.0
        model_name: None, // Not directly accessible in rupnp 2.0
        model_number: None,
        serial_number: None,
        device_type: device.device_type().to_string(),
    };

    log::debug!("UPnP get_device_info response:\n{:#?}", device_info);

    Ok(device_info)
}

/// Detailed device information
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub friendly_name: String,
    pub manufacturer: Option<String>,
    pub model_name: Option<String>,
    pub model_number: Option<String>,
    pub serial_number: Option<String>,
    pub device_type: String,
}

/// Get position info from a MediaRenderer (includes current track metadata)
pub async fn get_position_info(device_location: &str) -> Result<PositionInfo, Box<dyn std::error::Error>> {
    let device = rupnp::Device::from_url(device_location.parse()?).await?;

    // Find AVTransport service
    let service = device
        .find_service(&URN::service("schemas-upnp-org", "AVTransport", 1))
        .ok_or("AVTransport service not found")?;

    // Call GetPositionInfo action
    let args = "<InstanceID>0</InstanceID>";
    let response = service.action(device.url(), "GetPositionInfo", args).await?;

    log::debug!("UPnP GetPositionInfo raw response:\n{:#?}", response);

    // Parse response - this is a simplified version
    // In reality, we'd need to parse the XML response properly
    let position_info = PositionInfo {
        track: response.get("Track").unwrap_or(&"0".to_string()).clone(),
        track_duration: response.get("TrackDuration").unwrap_or(&"00:00:00".to_string()).clone(),
        track_metadata: response.get("TrackMetaData").unwrap_or(&"".to_string()).clone(),
        track_uri: response.get("TrackURI").unwrap_or(&"".to_string()).clone(),
        rel_time: response.get("RelTime").unwrap_or(&"00:00:00".to_string()).clone(),
    };

    log::debug!("UPnP GetPositionInfo parsed response:\n{:#?}", position_info);

    Ok(position_info)
}

/// Position information from AVTransport GetPositionInfo
#[derive(Debug, Clone)]
pub struct PositionInfo {
    pub track: String,
    pub track_duration: String,
    pub track_metadata: String,  // DIDL-Lite XML
    pub track_uri: String,
    pub rel_time: String,
}

/// Parse DIDL-Lite metadata to extract audio format information
pub fn parse_audio_format(didl_xml: &str) -> Option<AudioFormat> {
    // This is a simplified parser - would need more robust XML parsing
    // Look for <res> element attributes

    if didl_xml.is_empty() {
        return None;
    }

    // Extract sampleFrequency
    let sample_rate = extract_attribute(didl_xml, "sampleFrequency");
    let bits_per_sample = extract_attribute(didl_xml, "bitsPerSample");
    let channels = extract_attribute(didl_xml, "nrAudioChannels");
    let bitrate = extract_attribute(didl_xml, "bitrate");

    if sample_rate.is_some() || bits_per_sample.is_some() {
        Some(AudioFormat {
            sample_rate,
            bits_per_sample,
            channels,
            bitrate,
        })
    } else {
        None
    }
}

/// Audio format information extracted from DIDL-Lite
#[derive(Debug, Clone)]
pub struct AudioFormat {
    pub sample_rate: Option<String>,
    pub bits_per_sample: Option<String>,
    pub channels: Option<String>,
    pub bitrate: Option<String>,
}

/// Helper to extract XML attribute value
fn extract_attribute(xml: &str, attr_name: &str) -> Option<String> {
    let pattern = format!(r#"{}="([^"]*)""#, attr_name);
    if let Some(caps) = regex::Regex::new(&pattern).ok()?.captures(xml) {
        caps.get(1).map(|m| m.as_str().to_string())
    } else {
        None
    }
}
