//! Version 1 of the API, served under [`PATH`].

use serde::{Deserialize, Serialize};

use crate::DeviceId;

/// Path prefix of every version 1 endpoint.
pub const PATH: &str = "/api/v1";

/// Longest device name: the limit of a DHCP hostname or a Wi-Fi SSID.
pub const DEVICE_NAME_MAX_LEN: usize = 32;

/// `GET /system`: the device's identity and uptime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct System {
    /// The device's name on networks, e.g. `eveningstar-0kd7-9c6`.
    pub name: heapless::String<DEVICE_NAME_MAX_LEN>,
    /// The device's globally unique ID, e.g. `4WSG-0KD7-9C6`.
    pub id: DeviceId,
    /// Seconds since the device started.
    pub uptime_s: u64,
}

/// `POST /system/reboot`: reboots the device.
///
/// The device answers 202 Accepted, with no body, before it reboots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebootRequest {
    pub mode: RebootMode,
}

/// How the device reboots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebootMode {
    /// Once the device's work in progress has finished, within a deadline.
    Graceful,
    /// At once, abandoning work in progress.
    Immediate,
}

/// Body of every error response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub error: ErrorCode,
}

/// What went wrong, independent of the HTTP status code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request's body is not what the endpoint expects.
    BadRequest,
    /// No endpoint at the requested path.
    NotFound,
    /// The endpoint does not support the request's method.
    MethodNotAllowed,
    /// The request's body is larger than the device can read.
    PayloadTooLarge,
    /// A code this version of the crate does not know.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reboot_requests() {
        let request: RebootRequest = serde_json::from_str(r#"{"mode":"immediate"}"#).unwrap();
        assert_eq!(request.mode, RebootMode::Immediate);
        let request: RebootRequest = serde_json::from_str(r#"{"mode":"graceful"}"#).unwrap();
        assert_eq!(request.mode, RebootMode::Graceful);
        assert!(serde_json::from_str::<RebootRequest>(r#"{"mode":"later"}"#).is_err());
        assert!(serde_json::from_str::<RebootRequest>("{}").is_err());
    }

    #[test]
    fn error_codes_are_snake_case() {
        let error = Error {
            error: ErrorCode::PayloadTooLarge,
        };
        let json = serde_json::to_string(&error).unwrap();
        assert_eq!(json, r#"{"error":"payload_too_large"}"#);
        assert_eq!(serde_json::from_str::<Error>(&json).unwrap(), error);
    }

    #[test]
    fn unknown_error_codes_parse() {
        let error: Error = serde_json::from_str(r#"{"error":"teapot"}"#).unwrap();
        assert_eq!(error.error, ErrorCode::Unknown);
    }
}
