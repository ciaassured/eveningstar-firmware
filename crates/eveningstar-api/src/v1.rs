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

/// Most holders a [`RebootStatus`] can list.
pub const REBOOT_HOLDERS_MAX: usize = 16;

/// `GET /system/reboot`: the pending graceful reboot, if any, and what holds
/// off graceful reboots.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebootStatus {
    pub pending: Option<PendingReboot>,
    /// The holders a graceful reboot waits for, each with live holds.
    pub holds: heapless::Vec<RebootHold, REBOOT_HOLDERS_MAX>,
}

/// A requested graceful reboot, waiting for holds to be released.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingReboot {
    pub reason: RebootReason,
    /// Seconds until the device reboots even if holds remain.
    pub remaining_s: u64,
}

/// Why a reboot was requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebootReason {
    /// The Ethernet link was down, or had no IPv4 address, for too long.
    LinkDown,
    /// A client requested it through the API.
    Api,
    /// A reason this version of the crate does not know.
    #[serde(other)]
    Unknown,
}

/// Live holds of one holder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebootHold {
    pub holder: RebootHolder,
    pub count: u16,
}

/// What holds off a graceful reboot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebootHolder {
    /// The HTTP server, while it answers a reboot request.
    Http,
    /// A holder this version of the crate does not know.
    #[serde(other)]
    Unknown,
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
    fn reboot_status_round_trips() {
        let mut holds = heapless::Vec::new();
        holds
            .push(RebootHold {
                holder: RebootHolder::Http,
                count: 1,
            })
            .unwrap();
        let status = RebootStatus {
            pending: Some(PendingReboot {
                reason: RebootReason::LinkDown,
                remaining_s: 287,
            }),
            holds,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(
            json,
            r#"{"pending":{"reason":"link_down","remaining_s":287},"holds":[{"holder":"http","count":1}]}"#
        );
        assert_eq!(serde_json::from_str::<RebootStatus>(&json).unwrap(), status);

        let idle: RebootStatus = serde_json::from_str(r#"{"pending":null,"holds":[]}"#).unwrap();
        assert_eq!(idle.pending, None);
        assert!(idle.holds.is_empty());
    }

    #[test]
    fn unknown_reboot_reasons_and_holders_parse() {
        let hold: RebootHold =
            serde_json::from_str(r#"{"holder":"config_commit","count":2}"#).unwrap();
        assert_eq!(hold.holder, RebootHolder::Unknown);
        let pending: PendingReboot =
            serde_json::from_str(r#"{"reason":"ota_update","remaining_s":1}"#).unwrap();
        assert_eq!(pending.reason, RebootReason::Unknown);
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
