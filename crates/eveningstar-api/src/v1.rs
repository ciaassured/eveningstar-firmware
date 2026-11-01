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
mod range;
mod storage;

pub use endpoint::{
    BodyError, Bytes, Empty, Endpoint, Errors, Json, Method, PathParam, PathParams, RangeError,
    RequestBody, ResponseBody, RouteError, is_json_content_type,
};
#[cfg(feature = "schema")]
pub use openapi::openapi;
pub use range::{ContentRange, EntityTag, Selection};
pub use storage::{
    AppDescriptor, AppImage, AppSlot, BOOTLOADER_NAME_MAX_LEN, BoardTable, BootState,
    BootloaderInfo, BootloaderStatus, CheckStatus, Checks, Contents, ENTRY_FIELDS_MAX, EntryField,
    EntryValidity, FLASH_TEXT_MAX_LEN, FlashText, GetInternalFlash, GetPartitionTable,
    GetRegionContent, GetRegions, GetStorage, ImageFault, ImageStatus, InternalFlash,
    LABEL_MAX_LEN, Label, LayoutMatch, Limit, MatchStatus, OtaState, OtadataEntry, PartitionKind,
    REGIONS_MAX, ROLES_MAX, Region, RegionContentError, RegionInfo, Regions, Role,
    STORAGE_DEVICES_MAX, Sha256, SlotName, StorageDevice, StorageDevices, StorageId, StorageKind,
    StorageStatus, TABLE_ENTRIES_MAX, TableEnd, TableEntry, TableRead, TableSearch, TableSummary,
    Tier,
};

/// Path prefix of every version 1 endpoint.
pub const PATH: &str = "/api/v1";

/// Defines [`Operation`], with a variant holding each endpoint listed, and
/// [`Call`], with a variant holding each endpoint and its path's parameters.
macro_rules! operations {
    ($($(#[$attr:meta])* $endpoint:ident),* $(,)?) => {
        /// Every operation of the API, each holding its [`Endpoint`].
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Operation {
            $(
                $(#[$attr])*
                #[doc = concat!("[`", stringify!($endpoint), "`].")]
                $endpoint($endpoint),
            )*
        }

        /// The operation a request is for, holding its [`Endpoint`] and the
        /// parameters the request's path holds.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Call {
            $(
                $(#[$attr])*
                #[doc = concat!("[`", stringify!($endpoint), "`].")]
                $endpoint($endpoint, <$endpoint as Endpoint>::Params),
            )*
        }

        impl Operation {
            /// Every operation.
            pub const ALL: &[Self] = &[$($(#[$attr])* Self::$endpoint($endpoint)),*];

            /// The operation a request with `method` and `path`, relative to
            /// [`PATH`], is for, with the parameters `path` holds.
            ///
            /// # Errors
            ///
            /// [`RouteError::NotFound`] if no operation has the path, including
            /// when a segment that is a path parameter doesn't parse, and
            /// [`RouteError::MethodNotAllowed`] if none with the path has the
            /// method.
            pub fn find(method: &str, path: &str) -> Result<Call, RouteError> {
                let mut error = RouteError::NotFound;
                $(
                    $(#[$attr])*
                    if let Some(params) =
                        endpoint::path_params(<$endpoint as Endpoint>::PATH, path)
                    {
                        if <$endpoint as Endpoint>::METHOD.as_str() == method {
                            return Ok(Call::$endpoint($endpoint, params));
                        }
                        error = RouteError::MethodNotAllowed;
                    }
                )*
                Err(error)
            }

            /// The operation's name: the name of its endpoint's type.
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $($(#[$attr])* Self::$endpoint(_) => stringify!($endpoint),)*
                }
            }

            /// The method of the operation's requests.
            #[must_use]
            pub const fn method(self) -> Method {
                match self {
                    $($(#[$attr])* Self::$endpoint(_) => <$endpoint as Endpoint>::METHOD,)*
                }
            }

            /// The operation's path, relative to [`PATH`].
            #[must_use]
            pub const fn path(self) -> &'static str {
                match self {
                    $($(#[$attr])* Self::$endpoint(_) => <$endpoint as Endpoint>::PATH,)*
                }
            }

            /// What a session needs to be allowed to call the operation.
            #[must_use]
            pub const fn scope(self) -> Scope {
                match self {
                    $($(#[$attr])* Self::$endpoint(_) => <$endpoint as Endpoint>::SCOPE,)*
                }
            }

            /// Adds the operation to `document`.
            #[cfg(feature = "schema")]
            fn document(self, document: &mut openapi::Document) {
                match self {
                    $(
                        $(#[$attr])*
                        Self::$endpoint(_) => document.operation::<$endpoint>(self.name()),
                    )*
                }
            }
        }

        impl Call {
            /// The operation called.
            #[must_use]
            pub const fn operation(self) -> Operation {
                match self {
                    $(
                        $(#[$attr])*
                        Self::$endpoint(endpoint, _) => Operation::$endpoint(endpoint),
                    )*
                }
            }
        }
    };
}

operations!(
    GetSystem,
    GetRebootStatus,
    Reboot,
    #[cfg(test)]
    GetTestContent,
);

/// The endpoint answering with the device's [`System`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetSystem;

impl Endpoint for GetSystem {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/system";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "Get the device's identity and uptime";

    type Params = ();
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
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "Get the pending reboot and its holds";

    type Params = ();
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
    const SCOPE: Scope = Scope::Operate;
    const SUMMARY: &'static str = "Reboot the device";
    const DESCRIPTION: Option<&'static str> = Some(
        "A graceful reboot waits for the holds the reboot status lists to be \
        released, up to a deadline. The device answers before it reboots, and \
        the reboot drops the connection.",
    );

    type Params = ();
    type Request = Json<RebootRequest>;
    type Response = Empty;
    type Error = Infallible;
}

/// An endpoint of the tests, with a path parameter and a binary body.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetTestContent;

#[cfg(test)]
impl Endpoint for GetTestContent {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/test/{item}/content";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "Get an item's content";

    type Params = TestItem;
    type Request = Empty;
    type Response = Bytes;
    type Error = Infallible;
}

/// An item of the tests, named by a path parameter.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TestItem {
    /// The first item.
    First,
    /// The item after the first.
    SecondItem,
}

#[cfg(test)]
impl PathParam for TestItem {}

/// What a session needs to be allowed to call an operation. Later versions may
/// add scopes.
///
/// The device doesn't enforce scopes yet: they say what each operation
/// is, so that a client can offer only what a session may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Reading the device's state.
    Read,
    /// Operational actions, such as a reboot, that leave its configuration as it is.
    Operate,
    /// Changing the device's persistent configuration.
    Configure,
    /// A scope this version of the crate does not know.
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
    /// The running firmware is an update that hasn't been confirmed, and the
    /// device's flash isn't laid out as that firmware needs to confirm it. The
    /// reboot returns to the previous firmware.
    CannotConfirm,
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
    /// The request's `Range` header asks for several ranges, or for none
    /// that is part of the body.
    RangeNotSatisfiable,
    /// The region holds configuration, secrets or factory data, which never
    /// leave the device, whatever the session may do.
    Sensitive,
    /// The device can't read the region this boot, as its flash isn't laid
    /// out as the firmware expects.
    Unavailable,
    /// The device is serving another download, which it serves one at a
    /// time. The `Retry-After` header says when to ask again.
    Busy,
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
            Self::RangeNotSatisfiable => Some(416),
            Self::Sensitive => Some(403),
            Self::Unavailable => Some(409),
            Self::Busy => Some(503),
            Self::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_operation_without_parameters_by_its_method_and_path() {
        for &operation in Operation::ALL {
            if operation.path().contains('{') {
                continue;
            }
            let found = Operation::find(operation.method().as_str(), operation.path());
            assert_eq!(
                found.map(Call::operation),
                Ok(operation),
                "{}",
                operation.name()
            );
        }
    }

    #[test]
    fn finds_operations_by_exact_method_and_path() {
        assert_eq!(
            Operation::find("POST", "/system/reboot"),
            Ok(Call::Reboot(Reboot, ()))
        );
        assert_eq!(
            Operation::find("GET", "/system"),
            Ok(Call::GetSystem(GetSystem, ()))
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
    fn finds_operations_with_the_parameters_of_their_paths() {
        assert_eq!(
            Operation::find("GET", "/test/first/content"),
            Ok(Call::GetTestContent(GetTestContent, TestItem::First))
        );
        assert_eq!(
            Operation::find("GET", "/test/second_item/content"),
            Ok(Call::GetTestContent(GetTestContent, TestItem::SecondItem))
        );
        assert_eq!(
            Operation::find("POST", "/test/first/content"),
            Err(RouteError::MethodNotAllowed)
        );
    }

    #[test]
    fn parameters_that_do_not_parse_are_not_found() {
        for path in [
            "/test/third/content",
            "/test/First/content",
            "/test/second-item/content",
            "/test/second%5Fitem/content",
            "/test/%66irst/content",
            "/test/first%20/content",
            "/test/{item}/content",
            "/test//content",
            "/test/first/second_item/content",
            "/test/first/content/",
            "/test/first",
            "/test/content",
            "/test/first/contents",
            "/tests/first/content",
        ] {
            assert_eq!(
                Operation::find("GET", path),
                Err(RouteError::NotFound),
                "{path}"
            );
            // Without a path, no method is allowed or not.
            assert_eq!(
                Operation::find("POST", path),
                Err(RouteError::NotFound),
                "{path}"
            );
        }
    }

    #[test]
    fn path_parameters_take_every_segment_they_name() {
        // A template naming more parameters than its type parses never matches.
        assert_eq!(
            endpoint::path_params::<TestItem>("/test/{a}/{b}", "/test/first/first"),
            None
        );
        // Nor does one naming fewer.
        assert_eq!(
            endpoint::path_params::<TestItem>("/test/first", "/test/first"),
            None
        );
        assert_eq!(
            endpoint::path_params::<()>("/test/{item}", "/test/first"),
            None
        );
        assert_eq!(endpoint::path_params::<()>("/test", "/test"), Some(()));
        assert_eq!(endpoint::path_params::<()>("/test", "/test/"), None);
        assert_eq!(endpoint::path_params::<()>("/test", ""), None);
    }

    #[test]
    fn calls_name_their_operations() {
        assert_eq!(
            Call::GetTestContent(GetTestContent, TestItem::First).operation(),
            Operation::GetTestContent(GetTestContent)
        );
        assert_eq!(
            Call::Reboot(Reboot, ()).operation(),
            Operation::Reboot(Reboot)
        );
    }

    #[test]
    fn operations_name_the_scope_they_need() {
        let scopes: Vec<_> = Operation::ALL
            .iter()
            .map(|operation| (operation.name(), operation.scope()))
            .collect();
        assert_eq!(
            scopes,
            [
                ("GetSystem", Scope::Read),
                ("GetRebootStatus", Scope::Read),
                ("Reboot", Scope::Operate),
                ("GetTestContent", Scope::Read),
            ]
        );
        assert_eq!(
            serde_json::to_string(&Scope::Operate).unwrap(),
            r#""operate""#
        );
        let later: Scope = serde_json::from_str(r#""from_the_future""#).unwrap();
        assert_eq!(later, Scope::Unknown);
    }

    #[test]
    fn error_codes_of_error_sets_have_statuses() {
        let codes = BodyError::CODES
            .iter()
            .chain(RangeError::CODES)
            .chain(RouteError::CODES);
        for &code in codes {
            assert!(code.status().is_some(), "{code:?}");
        }
        assert_eq!(RangeError::RangeNotSatisfiable.code().status(), Some(416));
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
        assert_eq!(
            serde_json::to_string(&ErrorCode::RangeNotSatisfiable).unwrap(),
            r#""range_not_satisfiable""#
        );
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
                ErrorCode::RangeNotSatisfiable,
                ErrorCode::Sensitive,
                ErrorCode::Unavailable,
                ErrorCode::Busy,
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
