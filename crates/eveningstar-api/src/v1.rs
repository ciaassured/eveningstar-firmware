//! Version 1 of the API, served under [`PATH`].

#![cfg_attr(
    feature = "schema",
    allow(
        clippy::large_stack_frames,
        reason = "the schemas are built on the host, which has more stack than the chip the limit is for"
    )
)]

use core::convert::Infallible;

use serde::{Deserialize, Serialize};

use crate::DeviceId;

mod endpoint;
#[cfg(feature = "schema")]
mod openapi;

pub use endpoint::{
    BodyError, Empty, Endpoint, Errors, Json, Method, RequestBody, ResponseBody, RouteError,
    is_json_content_type,
};
#[cfg(feature = "schema")]
pub use openapi::openapi;

/// Path prefix of every version 1 endpoint.
pub const PATH: &str = "/api/v1";

/// Defines [`Operation`], with a variant holding each endpoint listed.
macro_rules! operations {
    ($($endpoint:ident),* $(,)?) => {
        /// Every operation of the API, each holding its [`Endpoint`].
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Operation {
            $(
                #[doc = concat!("[`", stringify!($endpoint), "`].")]
                $endpoint($endpoint),
            )*
        }

        impl Operation {
            /// Every operation.
            pub const ALL: &[Self] = &[$(Self::$endpoint($endpoint)),*];

            /// The operation's name: the name of its endpoint's type.
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$endpoint(_) => stringify!($endpoint),)*
                }
            }

            /// The method of the operation's requests.
            #[must_use]
            pub const fn method(self) -> Method {
                match self {
                    $(Self::$endpoint(_) => <$endpoint as Endpoint>::METHOD,)*
                }
            }

            /// The operation's path, relative to [`PATH`].
            #[must_use]
            pub const fn path(self) -> &'static str {
                match self {
                    $(Self::$endpoint(_) => <$endpoint as Endpoint>::PATH,)*
                }
            }

            /// What a session needs to be allowed to call the operation.
            #[must_use]
            pub const fn capability(self) -> Capability {
                match self {
                    $(Self::$endpoint(_) => <$endpoint as Endpoint>::CAPABILITY,)*
                }
            }

            /// Adds the operation to `document`.
            #[cfg(feature = "schema")]
            fn document(self, document: &mut openapi::Document) {
                match self {
                    $(Self::$endpoint(_) => document.operation::<$endpoint>(self.name()),)*
                }
            }
        }
    };
}

operations!(GetSystem, GetRebootStatus, Reboot);

impl Operation {
    /// The operation a request with `method` and `path`, relative to
    /// [`PATH`], is for.
    ///
    /// # Errors
    ///
    /// [`RouteError::NotFound`] if no operation has the path, and
    /// [`RouteError::MethodNotAllowed`] if none with the path has the method.
    pub fn find(method: &str, path: &str) -> Result<Self, RouteError> {
        let mut error = RouteError::NotFound;
        for &operation in Self::ALL {
            if operation.path() == path {
                if operation.method().as_str() == method {
                    return Ok(operation);
                }
                error = RouteError::MethodNotAllowed;
            }
        }
        Err(error)
    }
}

/// The endpoint answering with the device's [`System`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetSystem;

impl Endpoint for GetSystem {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/system";
    const STATUS: u16 = 200;
    const CAPABILITY: Capability = Capability::Read;
    const SUMMARY: &'static str = "Get the device's identity and uptime";

    type Request = Empty;
    type Response = Json<System>;
    type Error = Infallible;
}

/// The endpoint answering with the device's [`RebootStatus`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetRebootStatus;

impl Endpoint for GetRebootStatus {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/system/reboot";
    const STATUS: u16 = 200;
    const CAPABILITY: Capability = Capability::Read;
    const SUMMARY: &'static str = "Get the pending reboot and its holds";

    type Request = Empty;
    type Response = Json<RebootStatus>;
    type Error = Infallible;
}

/// The endpoint rebooting the device as a [`RebootRequest`] asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reboot;

impl Endpoint for Reboot {
    const METHOD: Method = Method::Post;
    const PATH: &'static str = GetRebootStatus::PATH;
    const STATUS: u16 = 202;
    const CAPABILITY: Capability = Capability::Operate;
    const SUMMARY: &'static str = "Reboot the device";
    const DESCRIPTION: Option<&'static str> = Some(
        "A graceful reboot waits for the holds the reboot status lists to be \
        released, up to a deadline. The device answers before it reboots, and \
        the reboot drops the connection.",
    );

    type Request = Json<RebootRequest>;
    type Response = Empty;
    type Error = Infallible;
}

/// What a session needs to be allowed to call an operation. Later versions may
/// add capabilities.
///
/// The device doesn't enforce capabilities yet: they say what each operation
/// is, so that a client can offer only what a session may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Reading the device's state.
    Read,
    /// Operational actions, such as a reboot, that leave its configuration as it is.
    Operate,
    /// Changing the device's persistent configuration.
    Configure,
    /// A capability this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// Longest device name: the limit of a DHCP hostname or a Wi-Fi SSID.
pub const DEVICE_NAME_MAX_LEN: usize = 32;

/// The device's identity and uptime.
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

/// A request to reboot the device.
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

/// The pending graceful reboot, if any, and what holds off graceful reboots.
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
    /// The request has a body, and its `Content-Type` is not `application/json`.
    UnsupportedMediaType,
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

impl ErrorCode {
    /// The HTTP status code of responses with this code, or `None` for
    /// [`Unknown`](Self::Unknown).
    #[must_use]
    pub const fn status(self) -> Option<u16> {
        match self {
            Self::BadRequest => Some(400),
            Self::NotFound => Some(404),
            Self::MethodNotAllowed => Some(405),
            Self::PayloadTooLarge => Some(413),
            Self::UnsupportedMediaType => Some(415),
            Self::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_operation_by_its_method_and_path() {
        for &operation in Operation::ALL {
            let found = Operation::find(operation.method().as_str(), operation.path());
            assert_eq!(found, Ok(operation), "{}", operation.name());
        }
    }

    #[test]
    fn finds_operations_by_exact_method_and_path() {
        assert_eq!(
            Operation::find("POST", "/system/reboot"),
            Ok(Operation::Reboot(Reboot))
        );
        assert_eq!(
            Operation::find("POST", "/system"),
            Err(RouteError::MethodNotAllowed)
        );
        assert_eq!(
            Operation::find("get", "/system"),
            Err(RouteError::MethodNotAllowed)
        );
        for path in ["", "/", "/system/", "/systems", "/system/reboot/now"] {
            assert_eq!(
                Operation::find("GET", path),
                Err(RouteError::NotFound),
                "{path}"
            );
        }
    }

    #[test]
    fn operations_name_the_capability_they_need() {
        let capabilities: Vec<_> = Operation::ALL
            .iter()
            .map(|operation| (operation.name(), operation.capability()))
            .collect();
        assert_eq!(
            capabilities,
            [
                ("GetSystem", Capability::Read),
                ("GetRebootStatus", Capability::Read),
                ("Reboot", Capability::Operate),
            ]
        );
        assert_eq!(
            serde_json::to_string(&Capability::Operate).unwrap(),
            r#""operate""#
        );
        let later: Capability = serde_json::from_str(r#""from_the_future""#).unwrap();
        assert_eq!(later, Capability::Unknown);
    }

    #[test]
    fn error_codes_of_error_sets_have_statuses() {
        for &code in BodyError::CODES.iter().chain(RouteError::CODES) {
            assert!(code.status().is_some(), "{code:?}");
        }
        assert_eq!(
            BodyError::PayloadTooLarge.code(),
            ErrorCode::PayloadTooLarge
        );
        assert_eq!(RouteError::NotFound.code(), ErrorCode::NotFound);
        assert_eq!(
            BodyError::UnsupportedMediaType.code(),
            ErrorCode::UnsupportedMediaType
        );
    }

    #[test]
    fn accepts_the_json_media_type_in_any_case_with_parameters() {
        for value in [
            "application/json",
            "Application/JSON",
            "application/json; charset=utf-8",
            "application/json;charset=UTF-8",
            "  application/json  ",
        ] {
            assert!(is_json_content_type(value), "{value}");
        }
    }

    #[test]
    fn rejects_the_media_types_a_form_can_send() {
        for value in [
            "",
            "text/plain",
            "text/plain; charset=utf-8",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
            "application/jsonp",
            "application/json5",
            "application/vnd.api+json",
            "text/json",
            "application/json/",
            "x-application/json",
            "json",
        ] {
            assert!(!is_json_content_type(value), "{value}");
        }
    }

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
                ErrorCode::UnsupportedMediaType,
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
