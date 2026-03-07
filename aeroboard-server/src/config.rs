use std::{env, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageMode {
    Filesystem,
    Greptime,
    LiveOnly,
}

impl StorageMode {
    pub fn from_env() -> Self {
        match env::var("AEROBOARD_STORAGE")
            .unwrap_or_else(|_| "filesystem".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "greptime" => Self::Greptime,
            "live" | "live-only" | "live_only" | "none" => Self::LiveOnly,
            _ => Self::Filesystem,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem",
            Self::Greptime => "greptime",
            Self::LiveOnly => "live",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub http_port: u16,
    pub flight_port: u16,
    pub storage_mode: StorageMode,
    pub data_dir: PathBuf,
    pub greptime_url: String,
    pub web_dist_dir: PathBuf,
}

impl AppConfig {
    pub fn from_env() -> Self {
        Self {
            http_port: read_u16("AEROBOARD_HTTP_PORT", 3000),
            flight_port: read_u16("AEROBOARD_FLIGHT_PORT", 50051),
            storage_mode: StorageMode::from_env(),
            data_dir: env::var("AEROBOARD_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(".aeroboard")),
            greptime_url: env::var("AEROBOARD_GREPTIME_URL")
                .unwrap_or_else(|_| "http://localhost:4000".to_string()),
            web_dist_dir: env::var("AEROBOARD_WEB_DIST_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("../aeroboard-web/dist")),
        }
    }
}

fn read_u16(name: &str, default: u16) -> u16 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(default)
}