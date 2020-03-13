use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter, Result};
use std::io;

use actix::prelude::{Actor, Context};

use slog::slog_o;
use slog::Drain;
use slog_async;
use slog_mozlog_json::MozLogJson;
use slog_term;

#[derive(Clone, Debug)]
pub struct MozLogger {
    pub log: slog::Logger,
}

#[allow(dead_code)]
pub enum ErrorLevel {
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

impl Debug for ErrorLevel {
    fn fmt(&self, f: &mut Formatter) -> Result {
        write!(
            f,
            "{}",
            match self {
                ErrorLevel::Debug => "Debug",
                ErrorLevel::Info => "Info",
                ErrorLevel::Warn => "Warn",
                ErrorLevel::Error => "Error",
                ErrorLevel::Critical => "Critical",
            }
        )
    }
}

impl MozLogger {
    pub fn new() -> Self {
        let json_drain = MozLogJson::new(io::stdout())
            .logger_name(format!(
                "{}-{}",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION")
            ))
            .msg_type(format!("{}:log", env!("CARGO_PKG_NAME")))
            .build()
            .fuse();
        let drain = slog_async::Async::new(json_drain).build().fuse();
        Self {
            log: slog::Logger::root(drain, slog_o!()).new(slog_o!()),
        }
    }

    pub fn new_json() -> Self {
        Self::new()
    }

    pub fn new_human() -> Self {
        let decorator = slog_term::TermDecorator::new().build();
        let drain = slog_term::CompactFormat::new(decorator).build().fuse();
        let drain = slog_async::Async::new(drain).build().fuse();

        Self {
            log: slog::Logger::root(drain, slog_o!()).new(slog_o!()),
        }
    }
}

impl Default for MozLogger {
    fn default() -> Self {
        Self::new_json()
    }
}

impl Actor for MozLogger {
    type Context = Context<Self>;
}

#[derive(Debug)]
pub struct LogMessage {
    pub level: ErrorLevel,
    pub msg: String,
    pub attributes: Option<HashMap<String, String>>,
}

impl Display for LogMessage {
    fn fmt(&self, f: &mut Formatter) -> Result {
        let level = match &self.level {
            ErrorLevel::Debug => "DEBUG",
            ErrorLevel::Info => "INFO",
            ErrorLevel::Warn => "WARN",
            ErrorLevel::Error => "ERROR",
            ErrorLevel::Critical => "CRITICAL",
        };
        let mut msg = format!("{}: {}", level, self.msg);
        if let Some(ref attributes) = self.attributes {
            msg = format!("{} :: {:?}", msg, attributes);
        }
        Ok(write!(f, "{}", msg)?)
    }
}
