use std::fmt;
use std::io;

use backtrace::Backtrace;
use thiserror::Error;

#[derive(Debug)]
#[allow(dead_code)]
pub struct HandlerError {
    pub kind: HandlerErrorKind,
    pub backtrace: Box<Backtrace>,
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl std::error::Error for HandlerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.kind.source()
    }
}

impl<T> From<T> for HandlerError
where
    HandlerErrorKind: From<T>,
{
    fn from(item: T) -> Self {
        HandlerError {
            kind: HandlerErrorKind::from(item),
            backtrace: Box::new(Backtrace::new()),
        }
    }
}

#[derive(Debug, Error)]
pub enum HandlerErrorKind {
    #[error("Excess Data Exchanged: {:?}", _0)]
    XSDataErr(String),
    #[error("Excess Messages: {:?}", _0)]
    XSMessageErr(String),
    #[error(transparent)]
    IOError(#[from] io::Error),
    #[error(transparent)]
    MetricsError(#[from] cadence::MetricError),
    #[error("Bad remote address: {:?}", _0)]
    BadRemoteAddrError(String),
}
