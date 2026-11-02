use std::{env, net::IpAddr, path::PathBuf};

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

/// Which browser origins may call the HTTP API cross-origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorsPolicy {
    /// No CORS headers: only the dashboard served by this server (or a dev proxy) can call the API.
    SameOrigin,
    /// Explicit allow-list from `AEROBOARD_CORS_ORIGINS`.
    Origins(Vec<String>),
    /// `AEROBOARD_CORS_ORIGINS=*`: any website may call the API, including DELETE.
    Any,
}

impl CorsPolicy {
    fn from_env() -> Self {
        // `CORS_ORIGIN` is the pre-0.2 name; keep honouring it.
        let raw = env::var("AEROBOARD_CORS_ORIGINS").or_else(|_| env::var("CORS_ORIGIN")).unwrap_or_default();
        let origins: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if origins.is_empty() {
            Self::SameOrigin
        } else if origins.iter().any(|origin| origin == "*") {
            Self::Any
        } else {
            Self::Origins(origins)
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Interface both servers bind to. Defaults to loopback; set `AEROBOARD_HOST=0.0.0.0`
    /// to accept telemetry and dashboard traffic from other machines.
    pub host: IpAddr,
    pub http_port: u16,
    pub flight_port: u16,
    pub storage_mode: StorageMode,
    pub data_dir: PathBuf,
    pub greptime_url: String,
    pub web_dist_dir: PathBuf,
    /// Largest gRPC Flight message accepted from clients (audio/video batches).
    pub max_message_bytes: usize,
    /// Dashboard defaults file (`AEROBOARD_DASHBOARD_CONFIG`); `None` means
    /// `./aeroboard.toml` if it exists.
    pub dashboard_config_path: Option<PathBuf>,
    pub cors: CorsPolicy,
    /// Batches buffered for each WebSocket subscriber before it is told it lagged.
    pub live_queue_batches: usize,
    /// Batches buffered between ingestion and storage before ingestion waits.
    pub persist_queue_batches: usize,
}

impl AppConfig {
    pub fn from_env() -> Self {
        Self {
            host: read_host(),
            http_port: read_number("AEROBOARD_HTTP_PORT", 3000),
            flight_port: read_number("AEROBOARD_FLIGHT_PORT", 50051),
            storage_mode: StorageMode::from_env(),
            data_dir: env::var("AEROBOARD_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(".aeroboard")),
            greptime_url: env::var("AEROBOARD_GREPTIME_URL")
                .unwrap_or_else(|_| "http://localhost:4000".to_string()),
            web_dist_dir: env::var("AEROBOARD_WEB_DIST_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("../aeroboard-web/dist")),
            max_message_bytes: read_number::<usize>("AEROBOARD_MAX_MESSAGE_MB", 64) * 1024 * 1024,
            dashboard_config_path: env::var("AEROBOARD_DASHBOARD_CONFIG")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            cors: CorsPolicy::from_env(),
            live_queue_batches: read_number("AEROBOARD_LIVE_QUEUE", 256),
            persist_queue_batches: read_number("AEROBOARD_PERSIST_QUEUE", 256),
        }
    }

    /// Defaults suitable for tests: filesystem storage under `data_dir`.
    #[cfg(test)]
    pub fn for_tests(data_dir: PathBuf, web_dist_dir: PathBuf) -> Self {
        Self {
            host: IpAddr::from([127, 0, 0, 1]),
            http_port: 3000,
            flight_port: 50051,
            storage_mode: StorageMode::Filesystem,
            data_dir,
            greptime_url: "http://localhost:4000".to_string(),
            web_dist_dir,
            max_message_bytes: 64 * 1024 * 1024,
            dashboard_config_path: None,
            cors: CorsPolicy::SameOrigin,
            live_queue_batches: 16,
            persist_queue_batches: 16,
        }
    }
}

fn read_host() -> IpAddr {
    let default = IpAddr::from([127, 0, 0, 1]);
    match env::var("AEROBOARD_HOST") {
        Ok(value) if !value.trim().is_empty() => {
            let value = value.trim();
            let value = if value.eq_ignore_ascii_case("localhost") { "127.0.0.1" } else { value };
            value.parse().unwrap_or_else(|_| {
                tracing::warn!("Ignoring invalid AEROBOARD_HOST={value:?}; binding to {default}");
                default
            })
        }
        _ => default,
    }
}

fn read_number<T: std::str::FromStr + Copy + std::fmt::Display>(name: &str, default: T) -> T {
    match env::var(name) {
        Ok(value) => value.trim().parse::<T>().unwrap_or_else(|_| {
            tracing::warn!("Ignoring invalid {name}={value:?}; using {default}");
            default
        }),
        Err(_) => default,
    }
}
