use std::env;

use config::{Config, ConfigError, Environment, File};
use serde::{Deserialize, Serialize};

static PREFIX: &str = "PAIR";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Settings {
    pub hostname: String,             // server hostname (localhost)
    pub port: u16,                    // server port (8000)
    pub max_channel_connections: u8,  // Max connections per channel (10)
    pub conn_lifespan: u64,           // Total connection lifespan in seconds (300)
    pub client_timeout: u64,          // Client timeout for pong responses (30)
    pub max_exchanges: u8,            // Max number of messages before channel shutdown (3)
    pub max_data: u64,                // Max amount of data octets to exchange (0 ; unlimited)
    pub debug: bool,                  // In debug mode? (false)
    pub verbose: bool,                // Verbose Errors? (false)
    pub mmdb_loc: String,             // MaxMind database path ("mmdb/latest/GeoLite2-City.mmdb")
    pub statsd_host: String,          // Metric statsd host (localhost)
    pub trusted_proxy_list: String,   // comma delimited list of proxy hosts ("")
    pub ip_reputation_server: String, // IP Reputation server. Leave blank to disable ("")
    pub iprep_min: u8,                // Minimum IP Reputation (0)
    pub ip_violation: String,         // Name of the abuse violation
    pub heartbeat: u64,               // Heartbeat rate in seconds for pings (5)
    pub human_logs: bool,             // Show "Human readable" logs (false)
    pub default_lang: String,         // Default language if none presented? (None)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hostname: "0.0.0.0".to_owned(),
            port: 8000,
            max_channel_connections: 3,
            conn_lifespan: 300,
            client_timeout: 30,
            max_exchanges: 10,
            max_data: 0,
            debug: false,
            verbose: false,
            mmdb_loc: "mmdb/latest/GeoLite2-City.mmdb".to_owned(),
            statsd_host: "localhost:8125".to_owned(),
            trusted_proxy_list: "".to_owned(),
            ip_reputation_server: "".to_owned(),
            iprep_min: 0,
            ip_violation: "channel_abuse".to_owned(),
            heartbeat: 5,
            human_logs: false,
            default_lang: "en".to_owned(),
        }
    }
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let mut settings = Config::try_from(&Settings::default())?;

        // Get the run environment
        let env = env::var("RUN_MODE").unwrap_or_else(|_| "development".to_owned());
        // start with any local config file.
        settings.merge(File::with_name(&format!("config/{}", env)).required(false))?;
        // Add/overwrite with the environments
        settings.merge(Environment::with_prefix(PREFIX))?;
        let ret: Settings = settings.try_into()?;
        Ok(ret)
    }
}
