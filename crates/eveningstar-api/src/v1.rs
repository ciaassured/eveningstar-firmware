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

/// Body of every error response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub error: ErrorCode,
}

/// What went wrong, independent of the HTTP status code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No endpoint at the requested path.
    NotFound,
    /// The endpoint does not support the request's method.
    MethodNotAllowed,
    /// A code this version of the crate does not know.
    #[serde(other)]
    Unknown,
}
