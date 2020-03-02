use std::fmt;
use std::io;

use cadence;
use failure::{Backtrace, Context, Fail};

#[derive(Debug)]
pub struct HandlerError {
    inner: Context<HandlerErrorKind>,
}

#[derive(Clone, Eq, PartialEq, Debug, Fail)]
pub enum HandlerErrorKind {
    #[fail(display = "Excess Data Exchanged: {:?}", _0)]
    XSDataErr(String),
    #[fail(display = "Excess Messages: {:?}", _0)]
    XSMessageErr(String),
    #[fail(display = "IO Error: {:?}", _0)]
    IOError(String),
    #[fail(display = "Could not start metrics: {:?}", _0)]
    MetricsError(String),
    #[fail(display = "Bad remote address: {:?}", _0)]
    BadRemoteAddrError(String),
}

impl Fail for HandlerError {
    fn cause(&self) -> Option<&dyn Fail> {
        self.inner.cause()
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        self.inner.backtrace()
    }
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

impl From<HandlerErrorKind> for HandlerError {
    fn from(kind: HandlerErrorKind) -> HandlerError {
        Context::new(kind).into()
    }
}

impl From<Context<HandlerErrorKind>> for HandlerError {
    fn from(inner: Context<HandlerErrorKind>) -> HandlerError {
        HandlerError { inner }
    }
}

impl From<io::Error> for HandlerError {
    fn from(err: io::Error) -> HandlerError {
        Context::new(HandlerErrorKind::IOError(format!("{:?}", err))).into()
    }
}

impl From<cadence::MetricError> for HandlerError {
    fn from(err: cadence::MetricError) -> HandlerError {
        Context::new(HandlerErrorKind::MetricsError(format!("{:?}", err))).into()
    }
}
