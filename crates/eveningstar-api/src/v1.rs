//! Version 1 of the API, served under [`PATH`].

use serde::{Deserialize, Serialize};

use crate::DeviceId;

/// Path prefix of every version 1 endpoint.
pub const PATH: &str = "/api/v1";

/// Longest device name: the limit of a DHCP hostname or a Wi-Fi SSID.
pub const DEVICE_NAME_MAX_LEN: usize = 32;

/// `GET /system`: the device's identity and uptime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct System {
    /// The device's name on networks, e.g. `eveningstar-0kd7-9c6`.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::string::String", length(max = DEVICE_NAME_MAX_LEN))
    )]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RebootRequest {
    /// How the device reboots.
    pub mode: RebootMode,
}

/// How the device reboots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RebootStatus {
    /// The requested graceful reboot, if one is pending.
    pub pending: Option<PendingReboot>,
    /// The holders a graceful reboot waits for, each with live holds.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<RebootHold>", length(max = REBOOT_HOLDERS_MAX))
    )]
    pub holds: heapless::Vec<RebootHold, REBOOT_HOLDERS_MAX>,
}

/// A requested graceful reboot, waiting for holds to be released.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PendingReboot {
    /// Why the reboot was requested.
    pub reason: RebootReason,
    /// Seconds until the device reboots even if holds remain.
    pub remaining_s: u64,
}

/// Why a reboot was requested. Later versions may add reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RebootReason {
    /// The Ethernet link was down, or had no IPv4 address, for too long.
    LinkDown,
    /// A client requested it through the API.
    Api,
    /// A reason this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// Live holds of one holder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RebootHold {
    /// What holds off the reboot.
    pub holder: RebootHolder,
    /// How many holds the holder has.
    pub count: u16,
}

/// What holds off a graceful reboot. Later versions may add holders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RebootHolder {
    /// The HTTP server, while it answers a reboot request.
    Http,
    /// A holder this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// Body of every error response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Error {
    /// What went wrong.
    pub error: ErrorCode,
}

/// What went wrong, independent of the HTTP status code. Later versions may
/// add codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    #[cfg_attr(feature = "schema", schemars(skip))]
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

    #[cfg(feature = "schema")]
    mod schema {
        use serde_json::{Value, json};

        use super::*;
        use crate::schema_accepts;

        fn system(name: &str) -> Value {
            json!({ "name": name, "id": "4WSG-0KD7-9C6", "uptime_s": 61 })
        }

        fn status(holds: usize) -> Value {
            let hold = json!({ "holder": "http", "count": 1 });
            json!({ "pending": null, "holds": vec![hold; holds] })
        }

        #[test]
        fn serialised_values_match_their_schemas() {
            let system = System {
                name: "eveningstar-0kd7-9c6".try_into().unwrap(),
                id: DeviceId::from_mac([0x9c, 0xcc, 0x01, 0x36, 0x9d, 0x2c]),
                uptime_s: 61,
            };
            assert!(schema_accepts::<System>(
                &serde_json::to_value(system).unwrap()
            ));

            for mode in [RebootMode::Graceful, RebootMode::Immediate] {
                let request = serde_json::to_value(RebootRequest { mode }).unwrap();
                assert!(schema_accepts::<RebootRequest>(&request));
            }

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
            assert!(schema_accepts::<RebootStatus>(
                &serde_json::to_value(status).unwrap()
            ));

            for error in [
                ErrorCode::BadRequest,
                ErrorCode::NotFound,
                ErrorCode::MethodNotAllowed,
                ErrorCode::PayloadTooLarge,
            ] {
                let error = serde_json::to_value(Error { error }).unwrap();
                assert!(schema_accepts::<Error>(&error));
            }
        }

        #[test]
        fn schemas_bound_heapless_lengths() {
            let longest = "a".repeat(DEVICE_NAME_MAX_LEN);
            assert!(schema_accepts::<System>(&system(&longest)));
            assert!(!schema_accepts::<System>(&system(&format!("{longest}a"))));

            assert!(schema_accepts::<RebootStatus>(&status(REBOOT_HOLDERS_MAX)));
            assert!(!schema_accepts::<RebootStatus>(&status(
                REBOOT_HOLDERS_MAX + 1
            )));
        }

        #[test]
        fn schemas_reject_invalid_values() {
            assert!(!schema_accepts::<RebootRequest>(
                &json!({ "mode": "later" })
            ));
            assert!(!schema_accepts::<RebootRequest>(&json!({})));
            let mut system = system("eveningstar");
            system["uptime_s"] = json!(-1);
            assert!(!schema_accepts::<System>(&system));
            system["uptime_s"] = json!(1);
            system["id"] = json!("4WSG-0KD7-9C");
            assert!(!schema_accepts::<System>(&system));
        }

        #[test]
        fn schemas_leave_out_unknown_values() {
            assert!(!schema_accepts::<Error>(&json!({ "error": "unknown" })));
            let hold = json!({ "holder": "unknown", "count": 1 });
            assert!(!schema_accepts::<RebootHold>(&hold));
            let pending = json!({ "reason": "unknown", "remaining_s": 1 });
            assert!(!schema_accepts::<PendingReboot>(&pending));
        }
    }
}
