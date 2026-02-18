use figment::{Figment, providers::{Format, Toml, Env, Serialized}};
use serde::{Deserialize, Serialize};

/// Default config file location: ~/.config/roon-rd/config.toml
fn default_config_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("roon-rd")
        .join("config.toml")
}

/// Application configuration, resolved from layered sources.
///
/// Precedence (lowest to highest):
///   1. Code defaults (in this struct)
///   2. Config file (~/.config/roon-rd/config.toml or --config path)
///   3. Environment variables (ROON_RD__*)
///   4. CLI arguments
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_carousel_interval")]
    pub carousel_interval: u32,

    /// Path to a single interchange libretto JSON file (legacy, still supported)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libretto: Option<String>,

    /// Directory containing *.interchange.json files for multi-opera support
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libretto_dir: Option<String>,
}

fn default_port() -> u16 { 3000 }
fn default_carousel_interval() -> u32 { 30 }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            carousel_interval: default_carousel_interval(),
            libretto: None,
            libretto_dir: None,
        }
    }
}

/// CLI overrides for server configuration.
///
/// All fields are Option so that unset CLI args don't overwrite
/// values from config file or environment variables.
/// `skip_serializing_if = "Option::is_none"` ensures unset fields
/// are not included in the figment merge.
#[derive(Debug, Clone, Serialize)]
pub struct CliOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub carousel_interval: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub libretto: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub libretto_dir: Option<String>,
}

/// Map a figment metadata provider name to a human-friendly source label.
///
/// Figment metadata names observed:
///   - "TOML file" for Toml::file()
///   - "roon_rd::config::CliOverrides" for Serialized::defaults(CliOverrides{..})
///   - "roon_rd::config::AppConfig" for Serialized::defaults(AppConfig::default())
///   - env vars: "environment variable(s)"
fn source_label(meta_name: &str) -> &'static str {
    // Order matters: check most specific patterns first
    if meta_name.contains("CliOverrides") {
        "cli"
    } else if meta_name.contains("TOML") || meta_name.contains(".toml") {
        "config-file"
    } else if meta_name.contains("environment") || meta_name.contains("env") {
        "env"
    } else if meta_name.contains("AppConfig") || meta_name.contains("Default") {
        "default"
    } else {
        "default"
    }
}

/// Load the application config from all sources.
///
/// Precedence (lowest to highest):
///   1. Code defaults
///   2. Config file
///   3. Environment variables (ROON_RD__PORT, ROON_RD__LIBRETTO_DIR, etc.)
///   4. CLI overrides
pub fn load_config(
    config_path: Option<&str>,
    cli_overrides: CliOverrides,
) -> Result<AppConfig, figment::Error> {
    let config_file = config_path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_config_path);

    let figment = Figment::new()
        // 1. Code defaults (lowest priority)
        .merge(Serialized::defaults(AppConfig::default()))
        // 2. Config file (optional — missing file is fine)
        .merge(Toml::file(&config_file))
        // 3. Environment variables: ROON_RD__PORT, ROON_RD__LIBRETTO_DIR, etc.
        //    Double underscore separates prefix from key.
        .merge(Env::prefixed("ROON_RD__"))
        // 4. CLI overrides (highest priority)
        .merge(Serialized::defaults(cli_overrides));

    let config: AppConfig = figment.extract()?;

    // Log config file location
    if config_file.exists() {
        log::info!("Config file: {}", config_file.display());
    } else {
        log::info!("Config file: {} (not found, using defaults)", config_file.display());
    }

    // Log each config value with its provenance
    let keys = &["port", "carousel_interval", "libretto", "libretto_dir"];
    for key in keys {
        let source = figment.find_metadata(key)
            .map(|meta| source_label(&meta.name))
            .unwrap_or("default");

        match *key {
            "port" => log::info!("  port = {} ({})", config.port, source),
            "carousel_interval" => log::info!("  carousel_interval = {} ({})", config.carousel_interval, source),
            "libretto" => match &config.libretto {
                Some(v) => log::info!("  libretto = {:?} ({})", v, source),
                None => log::info!("  libretto = <not set>"),
            },
            "libretto_dir" => match &config.libretto_dir {
                Some(v) => log::info!("  libretto_dir = {:?} ({})", v, source),
                None => log::info!("  libretto_dir = <not set>"),
            },
            _ => {}
        }
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = load_config(
            Some("/nonexistent/path/config.toml"),
            CliOverrides {
                port: None,
                carousel_interval: None,
                libretto: None,
                libretto_dir: None,
            },
        ).unwrap();

        assert_eq!(config.port, 3000);
        assert_eq!(config.carousel_interval, 30);
        assert!(config.libretto.is_none());
        assert!(config.libretto_dir.is_none());
    }

    #[test]
    fn test_cli_overrides() {
        let config = load_config(
            Some("/nonexistent/path/config.toml"),
            CliOverrides {
                port: Some(8080),
                carousel_interval: None,
                libretto: None,
                libretto_dir: Some("/tmp/librettos".into()),
            },
        ).unwrap();

        assert_eq!(config.port, 8080);
        assert_eq!(config.carousel_interval, 30); // default
        assert_eq!(config.libretto_dir.as_deref(), Some("/tmp/librettos"));
    }

    #[test]
    fn test_toml_config() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("roon-rd-test-config");
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.toml");

        let mut f = std::fs::File::create(&config_path).unwrap();
        writeln!(f, "port = 9090").unwrap();
        writeln!(f, "carousel_interval = 15").unwrap();
        writeln!(f, r#"libretto_dir = "/home/user/librettos""#).unwrap();
        drop(f);

        let config = load_config(
            Some(config_path.to_str().unwrap()),
            CliOverrides {
                port: None,
                carousel_interval: None,
                libretto: None,
                libretto_dir: None,
            },
        ).unwrap();

        assert_eq!(config.port, 9090);
        assert_eq!(config.carousel_interval, 15);
        assert_eq!(config.libretto_dir.as_deref(), Some("/home/user/librettos"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_provenance_labels() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("roon-rd-test-provenance");
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.toml");

        let mut f = std::fs::File::create(&config_path).unwrap();
        writeln!(f, "port = 9090").unwrap();
        writeln!(f, r#"libretto_dir = "/tmp/libs""#).unwrap();
        drop(f);

        // Build figment with a CLI override on carousel_interval
        let config_file = config_path.clone();
        let cli = CliOverrides {
            port: None,
            carousel_interval: Some(10),
            libretto: None,
            libretto_dir: None,
        };

        let figment = Figment::new()
            .merge(Serialized::defaults(AppConfig::default()))
            .merge(Toml::file(&config_file))
            .merge(Env::prefixed("ROON_RD__"))
            .merge(Serialized::defaults(cli));

        let _config: AppConfig = figment.extract().unwrap();

        // Print actual metadata names for each key
        for key in &["port", "carousel_interval", "libretto", "libretto_dir"] {
            let meta_name = figment.find_metadata(key)
                .map(|m| m.name.as_ref())
                .unwrap_or("<none>");
            let label = figment.find_metadata(key)
                .map(|m| source_label(&m.name))
                .unwrap_or("default");
            println!("  {}: meta_name={:?} -> label={:?}", key, meta_name, label);
        }

        // Verify labels
        assert_eq!(
            figment.find_metadata("port").map(|m| source_label(&m.name)),
            Some("config-file"),
            "port should come from config-file"
        );
        assert_eq!(
            figment.find_metadata("carousel_interval").map(|m| source_label(&m.name)),
            Some("cli"),
            "carousel_interval should come from cli"
        );
        // libretto is not set anywhere except defaults, and defaults don't produce a value for Option::None
        // so find_metadata returns None -> "default"
        assert_eq!(
            figment.find_metadata("libretto").map(|m| source_label(&m.name)).unwrap_or("default"),
            "default",
            "libretto should be default (not set)"
        );
        assert_eq!(
            figment.find_metadata("libretto_dir").map(|m| source_label(&m.name)),
            Some("config-file"),
            "libretto_dir should come from config-file"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_cli_overrides_beat_toml() {
        use std::io::Write;

        let dir = std::env::temp_dir().join("roon-rd-test-config-override");
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.toml");

        let mut f = std::fs::File::create(&config_path).unwrap();
        writeln!(f, "port = 9090").unwrap();
        drop(f);

        let config = load_config(
            Some(config_path.to_str().unwrap()),
            CliOverrides {
                port: Some(4000),
                carousel_interval: None,
                libretto: None,
                libretto_dir: None,
            },
        ).unwrap();

        assert_eq!(config.port, 4000); // CLI wins
        assert_eq!(config.carousel_interval, 30); // default

        std::fs::remove_dir_all(&dir).ok();
    }
}
