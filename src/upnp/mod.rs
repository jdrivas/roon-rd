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

/// Get raw XML from a UPnP device (pretty-printed)
pub async fn get_device_xml(location: &str) -> Result<String, Box<dyn std::error::Error>> {
    log::info!("Fetching raw XML from: {}", location);

    let response = reqwest::get(location).await?;
    let xml = response.text().await?;

    log::debug!("UPnP raw XML response:\n{}", xml);

    // Pretty-print the XML
    let pretty_xml = pretty_print_xml(&xml)?;

    Ok(pretty_xml)
}

/// Get service description (SCPD) XML for a specific service
pub async fn get_service_description(device_location: &str, service_type: &str) -> Result<String, Box<dyn std::error::Error>> {
    log::info!("Fetching service description for {} from: {}", service_type, device_location);

    // Validate the device location looks like a URL
    if !device_location.starts_with("http://") && !device_location.starts_with("https://") {
        return Err(format!("Invalid device URL: '{}'. Must start with http:// or https://", device_location).into());
    }

    // Fetch the device XML first
    let device_xml = reqwest::get(device_location).await?.text().await?;

    // Parse the service type to extract name and version
    // e.g., "AVTransport:2" or "urn:schemas-upnp-org:service:AVTransport:2"
    let service_name = if service_type.contains(':') {
        let parts: Vec<&str> = service_type.split(':').collect();
        if parts.len() >= 2 {
            parts[parts.len() - 2]
        } else {
            service_type
        }
    } else {
        service_type
    };

    // Look for SCPDURL in the device XML for the specified service
    // This is a simple regex-based approach - could be more robust with proper XML parsing
    let scpd_pattern = format!(r"<serviceType>.*{}.*</serviceType>.*?<SCPDURL>(.*?)</SCPDURL>", regex::escape(service_name));
    let re = regex::Regex::new(&scpd_pattern)?;

    let scpd_path = if let Some(caps) = re.captures(&device_xml) {
        caps.get(1)
            .ok_or("SCPD URL not found in device description")?
            .as_str()
    } else {
        return Err(format!("Service {} not found in device description", service_name).into());
    };

    // Construct full URL for SCPD
    let base_url = url::Url::parse(device_location)?;
    let scpd_url = if scpd_path.starts_with("http") {
        scpd_path.to_string()
    } else {
        base_url.join(scpd_path)?.to_string()
    };

    log::debug!("Fetching SCPD from: {}", scpd_url);

    // Fetch the SCPD XML
    let xml = reqwest::get(&scpd_url).await?.text().await?;

    log::debug!("UPnP SCPD XML response:\n{}", xml);

    // Pretty-print the XML
    let pretty_xml = pretty_print_xml(&xml)?;

    Ok(pretty_xml)
}

/// Pretty-print XML with indentation
fn pretty_print_xml(xml: &str) -> Result<String, Box<dyn std::error::Error>> {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    use quick_xml::Writer;
    use std::io::Cursor;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(event) => writer.write_event(event)?,
            Err(e) => return Err(format!("Error parsing XML: {}", e).into()),
        }
    }

    let result = writer.into_inner().into_inner();
    Ok(String::from_utf8(result)?)
}

/// Get detailed device information including available services
pub async fn get_device_info(location: &str) -> Result<DeviceInfo, Box<dyn std::error::Error>> {
    log::info!("Fetching device info from: {}", location);
    let device = rupnp::Device::from_url(location.parse()?).await?;

    // Collect available services
    let services: Vec<String> = device.services()
        .iter()
        .map(|s| s.service_type().to_string())
        .collect();

    let device_info = DeviceInfo {
        friendly_name: device.friendly_name().to_string(),
        manufacturer: None, // Not directly accessible in rupnp 2.0
        model_name: None, // Not directly accessible in rupnp 2.0
        model_number: None,
        serial_number: None,
        device_type: device.device_type().to_string(),
        services,
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
    pub services: Vec<String>,
}

/// Get position info from a MediaRenderer (includes current track metadata)
pub async fn get_position_info(device_location: &str) -> Result<PositionInfo, Box<dyn std::error::Error>> {
    let device = rupnp::Device::from_url(device_location.parse()?).await?;

    // Find AVTransport service (try version 2 first, then version 1)
    let service = device
        .find_service(&URN::service("schemas-upnp-org", "AVTransport", 2))
        .or_else(|| device.find_service(&URN::service("schemas-upnp-org", "AVTransport", 1)))
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

/// Get transport info from a MediaRenderer (playback state)
pub async fn get_transport_info(device_location: &str) -> Result<TransportInfo, Box<dyn std::error::Error>> {
    let device = rupnp::Device::from_url(device_location.parse()?).await?;

    // Find AVTransport service (try version 2 first, then version 1)
    let service = device
        .find_service(&URN::service("schemas-upnp-org", "AVTransport", 2))
        .or_else(|| device.find_service(&URN::service("schemas-upnp-org", "AVTransport", 1)))
        .ok_or("AVTransport service not found")?;

    // Call GetTransportInfo action
    let args = "<InstanceID>0</InstanceID>";
    let response = service.action(device.url(), "GetTransportInfo", args).await?;

    log::debug!("UPnP GetTransportInfo raw response:\n{:#?}", response);

    // Parse response
    let transport_info = TransportInfo {
        current_transport_state: response.get("CurrentTransportState").unwrap_or(&"UNKNOWN".to_string()).clone(),
        current_transport_status: response.get("CurrentTransportStatus").unwrap_or(&"OK".to_string()).clone(),
        current_speed: response.get("CurrentSpeed").unwrap_or(&"1".to_string()).clone(),
    };

    log::debug!("UPnP GetTransportInfo parsed response:\n{:#?}", transport_info);

    Ok(transport_info)
}

/// Transport information from AVTransport GetTransportInfo
#[derive(Debug, Clone)]
pub struct TransportInfo {
    pub current_transport_state: String,  // PLAYING, PAUSED_PLAYBACK, STOPPED, etc.
    pub current_transport_status: String,  // OK, ERROR_OCCURRED, etc.
    pub current_speed: String,  // Usually "1" for normal playback
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
