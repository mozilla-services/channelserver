use std::env;

use config::{Config, ConfigError, Environment, File};

static PREFIX: &str = "PAIR";

#[derive(Debug, Deserialize, Clone)]
pub struct Settings {
    pub hostname: String,             // server hostname (localhost)
    pub port: u16,                    // server port (8000)
    pub max_channel_connections: u8,  // Max connections per channel (3)
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
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let mut settings = Config::default();

        settings.set_default("debug", false)?;
        settings.set_default("verbose", false)?;
        settings.set_default("max_exchanges", 3)?;
        settings.set_default("conn_lifespan", 300)?;
        settings.set_default("client_timeout", 30)?;
        settings.set_default("max_channel_connections", 3)?;
        settings.set_default("max_data", 0)?;
        settings.set_default("port", 8000)?;
        settings.set_default("hostname", "0.0.0.0".to_owned())?;
        settings.set_default("mmdb_loc", "mmdb/latest/GeoLite2-City.mmdb".to_owned())?;
        settings.set_default("statsd_host", "localhost:8125".to_owned())?;
        settings.set_default("trusted_proxy_list", "".to_owned())?;
        settings.set_default("ip_reputation_server", "".to_owned())?;
        settings.set_default("iprep_min", 0)?;
        settings.set_default("ip_violation", "channel_abuse".to_owned())?;
        settings.set_default("heartbeat", 5)?;
        // Get the run environment
        let env = env::var("RUN_MODE").unwrap_or("development".to_owned());
        // start with any local config file.
        settings.merge(File::with_name(&format!("config/{}", env)).required(false))?;
        // Add/overwrite with the environments
        settings.merge(Environment::with_prefix(PREFIX))?;
        settings.try_into()
    }
}
