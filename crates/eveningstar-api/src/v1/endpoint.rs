//! Endpoints: the method and path of each operation, what its requests
//! carry, and how the device answers.

use core::convert::Infallible;
use core::marker::PhantomData;

use super::ErrorCode;

/// An HTTP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
}

impl Method {
    /// The method's name, as written in a request.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// An operation's endpoint: its method and path, the bodies of its requests
/// and successful responses, and the errors it answers with.
pub trait Endpoint {
    /// The method of the operation's requests.
    const METHOD: Method;
    /// The operation's path, relative to [`PATH`](super::PATH).
    const PATH: &'static str;
    /// The status code of a successful response.
    const STATUS: u16;
    /// What the operation does, in one line.
    const SUMMARY: &'static str;
    /// What the operation does, in more detail than the summary.
    const DESCRIPTION: Option<&'static str> = None;

    /// The body of a request.
    type Request: RequestBody;
    /// The body of a successful response.
    type Response: ResponseBody;
    /// The errors the operation answers with, besides those of reading the
    /// request body.
    type Error: Errors;
}

/// The body of a request: [`Empty`] or [`Json`].
pub trait RequestBody {
    /// The value the body holds.
    type Value;
    /// The errors of reading the body.
    type Error: Errors;
}

/// The body of a response: [`Empty`] or [`Json`].
pub trait ResponseBody {
    /// The value the body holds.
    type Value;
}

/// No body, holding `()`.
#[derive(Debug)]
pub enum Empty {}

impl RequestBody for Empty {
    type Value = ();
    type Error = Infallible;
}

impl ResponseBody for Empty {
    type Value = ();
}

/// A JSON body holding a `T`. A request with one has the `Content-Type`
/// `application/json` (see [`is_json_content_type`]).
#[derive(Debug)]
pub struct Json<T>(PhantomData<T>);

/// Whether the value of a `Content-Type` header names JSON: the media type
/// `application/json`, in any case, with or without parameters such as
/// `charset=utf-8`.
///
/// A device that reads a request body as JSON only after this check can't be
/// sent one by a form on another site, as forms send other types, and a request
/// of this type from another site needs the device's consent first.
#[must_use]
pub fn is_json_content_type(value: &str) -> bool {
    let media_type = value
        .split_once(';')
        .map_or(value, |(media_type, _)| media_type);
    media_type.trim().eq_ignore_ascii_case("application/json")
}

impl<T> RequestBody for Json<T> {
    type Value = T;
    type Error = BodyError;
}

impl<T> ResponseBody for Json<T> {
    type Value = T;
}

/// A set of [`ErrorCode`]s, as a type whose values are the codes.
pub trait Errors: Copy {
    /// Every code in the set.
    const CODES: &'static [ErrorCode];

    /// The code of this error.
    fn code(self) -> ErrorCode;
}

impl Errors for Infallible {
    const CODES: &'static [ErrorCode] = &[];

    fn code(self) -> ErrorCode {
        match self {}
    }
}

/// Defines an [`Errors`] enum whose variants are named after their codes.
macro_rules! errors {
    ($(#[$attr:meta])* $name:ident { $($code:ident),* $(,)? }) => {
        $(#[$attr])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $(
                #[doc = concat!("[`ErrorCode::", stringify!($code), "`].")]
                $code,
            )*
        }

        impl Errors for $name {
            const CODES: &'static [ErrorCode] = &[$(ErrorCode::$code),*];

            fn code(self) -> ErrorCode {
                match self {
                    $(Self::$code => ErrorCode::$code,)*
                }
            }
        }
    };
}

errors! {
    /// The errors of reading a [`Json`] request body.
    BodyError { BadRequest, UnsupportedMediaType, PayloadTooLarge }
}

errors! {
    /// The errors of finding the operation a request is for.
    RouteError { NotFound, MethodNotAllowed }
}
