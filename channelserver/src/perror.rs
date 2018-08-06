use std::fmt;

use failure::{Backtrace, Context, Fail};

/*
#[allow(dead_code)]
pub type Result<T> = result::Result<T, Error>;

#[allow(dead_code)]
pub type HandlerResult<T> = result::Result<T, HandlerError>;
*/

#[derive(Debug)]
pub struct HandlerError {
    inner: Context<HandlerErrorKind>,
}

#[derive(Clone, Eq, PartialEq, Debug, Fail)]
pub enum HandlerErrorKind {
    #[fail(display = "Excess Data Exchanged")]
    XSDataErr,
    #[fail(display = "Excess Messages")]
    XSMessageErr,
    #[fail(display = "Connection Expired")]
    ExpiredErr,
    #[fail(display = "Channel Shutdown Requested")]
    ShutdownErr,
}

/*
#[allow(dead_code)]
impl HandlerError {
    pub fn kind(&self) -> &HandlerErrorKind {
        self.inner.get_context()
    }
}
*/

impl Fail for HandlerError {
    fn cause(&self) -> Option<&Fail> {
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
