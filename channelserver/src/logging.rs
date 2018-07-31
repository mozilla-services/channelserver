use std::fmt::{Debug, Display, Formatter, Result};

use actix::prelude::{Actor, Context, Handler};

use slog;
use slog::Drain;
use slog_async;
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
        let decorator = slog_term::TermDecorator::new().build();
        let drain = slog_term::CompactFormat::new(decorator).build().fuse();
        let drain = slog_async::Async::new(drain).build().fuse();

        Self {
            log: slog::Logger::root(drain, o!()).new(o!()),
        }
    }
}

impl Default for MozLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl Actor for MozLogger {
    type Context = Context<Self>;
}

#[derive(Message, Debug)]
pub struct LogMessage {
    pub level: ErrorLevel,
    pub msg: String,
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
        /* TODO: Eventually add attributes
        let mut attrs = Vec::new();
        if Some(&self.attributes) {
            for (k, v) in &self.attributes {
                attrs.push(format!("{} => {}", k, v));
            }
        }
        write!(f, "## {}: {} ({})", level, self.msg, attrs.join(", "));
        */
        Ok(write!(f, "{}", self.msg)?)
    }
}

impl Handler<LogMessage> for MozLogger {
    type Result = ();

    fn handle(&mut self, msg: LogMessage, context: &mut Context<Self>) -> Self::Result {
        match &msg.level {
            ErrorLevel::Debug => slog_debug!(self.log, "{}", &msg),
            ErrorLevel::Info => slog_info!(self.log, "{}", &msg),
            ErrorLevel::Warn => slog_warn!(self.log, "{}", &msg),
            ErrorLevel::Error => slog_error!(self.log, "{}", &msg),
            ErrorLevel::Critical => slog_crit!(self.log, "{}", &msg),
        };
    }
}
