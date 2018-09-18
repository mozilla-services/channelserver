use std::env;

use config::{Config, ConfigError, Environment, File};

static PREFIX: &str = "PAIR";

#[derive(Debug, Deserialize, Clone)]
pub struct Settings {
    pub hostname: String,           // server hostname (localhost)
    pub port: u16,                  // server port (8000)
    pub max_clients: u8,            // Max clients per channel (2)
    pub timeout: u64,               // seconds before channel timeout (300)
    pub max_exchanges: u8,          // Max number of messages before channel shutdown (3)
    pub max_data: u64,              // Max amount of data octets to exchange (0 ; unlimited)
    pub debug: bool,                // In debug mode? (false)
    pub verbose: bool,              // Verbose Errors? (false)
    pub mmdb_loc: String,           // MaxMind database path ("mmdb/latest/GeoLite2-City.mmdb")
    pub statsd_host: String,        // Metric statsd host (localhost)
    pub trusted_proxy_list: String, // comma delimited list of proxy hosts ("")
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let mut settings = Config::default();

        settings.set_default("debug", false)?;
        settings.set_default("verbose", false)?;
        settings.set_default("max_exchanges", 3)?;
        settings.set_default("timeout", 300)?;
        settings.set_default("max_clients", 2)?;
        settings.set_default("max_data", 0)?;
        settings.set_default("port", 8000)?;
        settings.set_default("hostname", "0.0.0.0".to_owned())?;
        settings.set_default("mmdb_loc", "mmdb/latest/GeoLite2-City.mmdb".to_owned())?;
        settings.set_default("statsd_host", "localhost:8125".to_owned())?;
        settings.set_default("trusted_proxy_list", "".to_owned())?;
        // Get the run environment
        let env = env::var("RUN_MODE").unwrap_or("development".to_owned());
        // start with any local config file.
        settings.merge(File::with_name(&format!("config/{}", env)).required(false))?;
        // Add/overwrite with the environments
        settings.merge(Environment::with_prefix(PREFIX))?;
        settings.try_into()
    }
}
