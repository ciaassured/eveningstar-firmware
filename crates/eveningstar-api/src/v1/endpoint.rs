//! Endpoints: the method and path of each operation, what its requests
//! carry, and how the device answers.

use core::convert::Infallible;
use core::marker::PhantomData;

use serde::de::{DeserializeOwned, IntoDeserializer, value};

use super::{ErrorCode, Scope};

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
    /// The operation's path, relative to [`PATH`]. A segment written `{name}`
    /// is a path parameter: it matches any one segment that
    /// [`Params`](Self::Params) parses.
    ///
    /// [`PATH`]: super::PATH
    const PATH: &'static str;
    /// The status code of a successful response.
    const STATUS: u16;
    /// What a session needs to be allowed to call the operation.
    const SCOPE: Scope;
    /// What the operation does, in one line.
    const SUMMARY: &'static str;
    /// What the operation does, in more detail than the summary.
    const DESCRIPTION: Option<&'static str> = None;

    /// The parameters of the operation's path.
    type Params: PathParams;
    /// The body of a request.
    type Request: RequestBody;
    /// The body of a successful response.
    type Response: ResponseBody;
    /// The errors the operation answers with, besides those of reading the
    /// request body and of answering with the response body.
    type Error: Errors;
}

/// The parameters of a path: `()` for a path with none, or a [`PathParam`] for
/// a path with one.
pub trait PathParams: Sized {
    /// The parameters `segments` hold, or `None` if one doesn't parse.
    /// `segments` are the segments of a path that its endpoint's path names
    /// with `{name}`, in order.
    fn parse<'a>(segments: &mut impl Iterator<Item = &'a str>) -> Option<Self>;
}

impl PathParams for () {
    fn parse<'a>(_segments: &mut impl Iterator<Item = &'a str>) -> Option<Self> {
        Some(())
    }
}

/// A path parameter: the value of one segment of a path, deserialised from a
/// string holding the segment as it was sent, without percent-decoding.
pub trait PathParam: DeserializeOwned {}

impl<T: PathParam> PathParams for T {
    fn parse<'a>(segments: &mut impl Iterator<Item = &'a str>) -> Option<Self> {
        let segment: value::StrDeserializer<'_, value::Error> =
            segments.next()?.into_deserializer();
        T::deserialize(segment).ok()
    }
}

/// The parameters `path` holds, if it is one of the paths `template` describes:
/// each segment of `template` written `{name}` matches any one segment, and
/// every other segment only itself. `None` if `path` has other segments, or if
/// `P` doesn't parse the parameters' segments or leaves one of them.
pub(super) fn path_params<P: PathParams>(template: &str, path: &str) -> Option<P> {
    let mut templates = template.split('/');
    let mut segments = path.split('/');
    loop {
        match (templates.next(), segments.next()) {
            (None, None) => break,
            (Some(template), Some(segment))
                if param_name(template).is_some() || template == segment => {}
            _ => return None,
        }
    }

    let mut values = template
        .split('/')
        .zip(path.split('/'))
        .filter(|&(template, _)| param_name(template).is_some())
        .map(|(_, segment)| segment);
    let params = P::parse(&mut values)?;
    values.next().is_none().then_some(params)
}

/// The name of the path parameter a segment of a path template is, if it is
/// one: `name` for `{name}`.
pub(super) fn param_name(segment: &str) -> Option<&str> {
    segment.strip_prefix('{')?.strip_suffix('}')
}

/// The body of a request: [`Empty`] or [`Json`].
pub trait RequestBody {
    /// The value the body holds.
    type Value;
    /// The errors of reading the body.
    type Error: Errors;
}

/// The body of a response: [`Empty`], [`Json`] or [`Bytes`].
pub trait ResponseBody {
    /// The value the body holds.
    type Value: ?Sized;
    /// The errors of answering with the body.
    type Error: Errors;
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
    type Error = Infallible;
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
    type Error = Infallible;
}

/// A binary body, with the `Content-Type` `application/octet-stream`.
///
/// A `GET` request can ask for part of the body with a `Range` header, and make
/// that conditional with an `If-Range` header (see [`Selection`]). The answer
/// is the whole body with the endpoint's status code, the part with
/// [`PARTIAL_STATUS`](Self::PARTIAL_STATUS), or [`RangeError`].
///
/// [`Selection`]: super::Selection
#[derive(Debug)]
pub enum Bytes {}

impl Bytes {
    /// The status code of a response with part of the body.
    pub const PARTIAL_STATUS: u16 = 206;
}

impl ResponseBody for Bytes {
    type Value = [u8];
    type Error = RangeError;
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
    /// The errors of answering with a [`Bytes`] response body.
    RangeError { RangeNotSatisfiable }
}

errors! {
    /// The errors of finding the operation a request is for.
    RouteError { NotFound, MethodNotAllowed }
}
