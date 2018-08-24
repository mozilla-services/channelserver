use std::env;

use config::{Config, ConfigError, Environment, File};

static PREFIX: &str = "PAIR";

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub hostname: String,  // server hostname (localhost)
    pub port: u16,         // server port (8000)
    pub max_clients: u8,   // Max clients per channel 2
    pub timeout: u64,      // seconds before channel timeout (300)
    pub max_exchanges: u8, // Max number of messages before channel shutdown (8)
    pub max_data: u64,     // Max amount of data octets to exchange (0 ; unlimited)
    pub debug: bool,       // In debug mode?
    pub verbose: bool,     // Verbose Errors?
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let mut settings = Config::default();

        settings.set_default("debug", false)?;
        settings.set_default("verbose", false)?;
        settings.set_default("max_exchanges", 0)?;
        settings.set_default("timeout", 300)?;
        settings.set_default("max_clients", 2)?;
        settings.set_default("max_data", 0)?;
        settings.set_default("port", 8000)?;
        settings.set_default("hostname", "0.0.0.0".to_owned())?;
        // Get the run environment
        let env = env::var("RUN_MODE").unwrap_or("development".to_owned());
        // start with any local config file.
        settings.merge(File::with_name(&format!("config/{}", env)).required(false))?;
        // Add/overwrite with the environments
        settings.merge(Environment::with_prefix(PREFIX))?;
        settings.try_into()
    }
}
