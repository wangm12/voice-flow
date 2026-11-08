pub mod enrollment;
pub mod fileutil;
pub mod filter;
pub mod gain;
pub mod models;
pub mod profile;
pub mod supervisor;
pub mod wav;
pub mod wire;

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidArgument(&'static str),
    Input(&'static str),
    Model(&'static str),
    Profile(&'static str),
    Output(&'static str),
    Protocol(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Error::InvalidArgument(message)
            | Error::Input(message)
            | Error::Model(message)
            | Error::Profile(message)
            | Error::Output(message)
            | Error::Protocol(message) => message,
        };
        f.write_str(message)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_do_not_include_payload_values() {
        let error = Error::Protocol("audio message is invalid");
        assert_eq!(error.to_string(), "audio message is invalid");
    }
}
