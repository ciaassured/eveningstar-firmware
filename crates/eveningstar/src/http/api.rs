//! The API under [`v1::PATH`]. Each request is routed to its
//! [`v1::Operation`], and its body read and its response written as the
//! operation's [`Endpoint`] declares. Errors are answered with a JSON
//! [`v1::Error`].
//!
//! A [`v1::Bytes`] body is written from a [`Source`], whole or in the part
//! the request's `Range` asks for, a chunk at a time.

use core::convert::Infallible;
use core::fmt;

use defmt::warn;
use eveningstar_api::v1::{self, Call, Endpoint, Errors, Operation};
use picoserve::ResponseSent;
use picoserve::extract::{FromRef, FromRequest, JsonRejection};
use picoserve::io::{Read, Write};
use picoserve::request::{Path, ReadAllBodyError, Request, RequestBody, RequestParts};
use picoserve::response::custom::{CustomBody, CustomResponse};
use picoserve::response::{
    Body, Connection, HeadersIter, IntoResponse, Json, Response, ResponseWriter, StatusCode,
};
use picoserve::routing::{MethodHandlerService, PathRouterService};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{AppState, TCP_BUFFER_LEN, system};

/// The headers of every answer the API gives, including its errors. The answers
/// are data: the browser mustn't read them as anything else, keep them, let
/// another site embed them, or render and frame them if someone opens one.
pub(super) const HEADERS: &[(&str, &str)] = &[
    ("X-Content-Type-Options", "nosniff"),
    ("Cache-Control", "no-store"),
    (
        "Content-Security-Policy",
        "default-src 'none'; frame-ancestors 'none'",
    ),
    ("Cross-Origin-Resource-Policy", "same-origin"),
    // The older form of `frame-ancestors`, for browsers that only know this one.
    ("X-Frame-Options", "DENY"),
];

/// Serves every request under [`v1::PATH`].
pub(super) struct Api;

impl PathRouterService<AppState> for Api {
    async fn call_path_router_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        state: &AppState,
        _path_parameters: (),
        path: Path<'_>,
        request: Request<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error> {
        dispatch(state, path.encoded(), request, writer).await
    }
}

/// Answers every request no other route takes with `404`, in the API's form,
/// not as the router's own plain text, which repeats the path it was asked for.
pub(super) struct NotFound;

impl PathRouterService<AppState> for NotFound {
    async fn call_path_router_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        _state: &AppState,
        _path_parameters: (),
        _path: Path<'_>,
        request: Request<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error> {
        let connection = request.body_connection.finalize().await?;
        write_error(v1::RouteError::NotFound.code(), connection, writer).await
    }
}

/// Serves requests to [`v1::PATH`] itself.
impl MethodHandlerService<AppState> for Api {
    async fn call_method_handler_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        state: &AppState,
        _path_parameters: (),
        _method: &str,
        request: Request<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error> {
        dispatch(state, "", request, writer).await
    }
}

/// Serves a request with `path`, relative to [`v1::PATH`], with its
/// operation's handler.
async fn dispatch<R: Read, W: ResponseWriter<Error = R::Error>>(
    state: &AppState,
    path: &str,
    request: Request<'_, R>,
    writer: W,
) -> Result<ResponseSent, W::Error> {
    match Operation::find(request.parts.method(), path) {
        Ok(Call::GetSystem(endpoint, params)) => {
            serve(endpoint, params, state, request, writer, system::get_system).await
        }
        Ok(Call::GetRebootStatus(endpoint, params)) => {
            serve(endpoint, params, state, request, writer, system::get_reboot).await
        }
        Ok(Call::Reboot(endpoint, params)) => {
            serve(
                endpoint,
                params,
                state,
                request,
                writer,
                system::post_reboot,
            )
            .await
        }
        Err(error) => {
            let connection = request.body_connection.finalize().await?;
            write_error(error.code(), connection, writer).await
        }
    }
}

/// Serves a request for `E` whose path holds `params`: reads its body, passes
/// both to `handler` with the part of `state` the handler takes, and writes the
/// handler's reply with `E`'s status code, or its error.
async fn serve<E, S, H, V, Rep, R, W>(
    _endpoint: E,
    params: E::Params,
    state: &AppState,
    mut request: Request<'_, R>,
    writer: W,
    handler: H,
) -> Result<ResponseSent, W::Error>
where
    E: Endpoint<Request: ReadBody, Response: WriteBody<V>>,
    S: FromRef<AppState>,
    H: AsyncFnOnce(S, E::Params, <E::Request as v1::RequestBody>::Value) -> Result<Rep, E::Error>,
    Rep: Reply<V>,
    R: Read,
    W: ResponseWriter<Error = R::Error>,
{
    let parts = request.parts;
    let body = E::Request::read(parts, request.body_connection.body()).await;
    let result = match body {
        Ok(body) => handler(S::from_ref(state), params, body)
            .await
            .map_err(Errors::code),
        Err(error) => Err(error.code()),
    };
    let connection = request.body_connection.finalize().await?;

    let reply = match result {
        Ok(reply) => reply,
        Err(code) => return write_error(code, connection, writer).await,
    };
    let (body, then) = reply.into_parts();
    let status = StatusCode::new(E::STATUS);
    let sent = E::Response::write(body, status, Rep::CLOSES, parts, connection, writer).await;
    then();
    sent
}

/// Writes a response with `code`.
pub(super) async fn write_error<R: Read, W: ResponseWriter<Error = R::Error>>(
    code: v1::ErrorCode,
    connection: Connection<'_, R>,
    writer: W,
) -> Result<ResponseSent, W::Error> {
    writer
        .write_response(connection, error_response(code))
        .await
}

/// Seconds a client is told to wait, in `Retry-After`, before asking again
/// after [`v1::ErrorCode::Busy`]. The device serves one download at a time,
/// and a whole app slot takes several seconds on a local network, so asking
/// sooner mostly finds it still busy.
const BUSY_RETRY_AFTER_S: &str = "5";

/// A response with `code`. One with [`v1::ErrorCode::Busy`] says when to ask
/// again, in `Retry-After`.
fn error_response(code: v1::ErrorCode) -> Response<impl HeadersIter, impl Body> {
    // Every code in the API's error sets has a status.
    let status = code
        .status()
        .map_or(StatusCode::INTERNAL_SERVER_ERROR, StatusCode::new);
    let retry_after = (code == v1::ErrorCode::Busy).then_some(("Retry-After", BUSY_RETRY_AFTER_S));
    Json(v1::Error { error: code })
        .into_response()
        .with_status_code(status)
        .with_headers(HEADERS)
        .with_headers(retry_after)
}

/// A request body the server reads.
trait ReadBody: v1::RequestBody {
    async fn read<'r, R: Read>(
        parts: RequestParts<'r>,
        body: RequestBody<'r, R>,
    ) -> Result<Self::Value, Self::Error>;
}

impl ReadBody for v1::Empty {
    fn read<'r, R: Read>(
        _parts: RequestParts<'r>,
        _body: RequestBody<'r, R>,
    ) -> impl Future<Output = Result<(), Infallible>> {
        core::future::ready(Ok(()))
    }
}

impl<T: DeserializeOwned> ReadBody for v1::Json<T> {
    async fn read<'r, R: Read>(
        parts: RequestParts<'r>,
        body: RequestBody<'r, R>,
    ) -> Result<T, v1::BodyError> {
        // Checked before anything is read, so no body of another type is parsed.
        let is_json = parts
            .headers()
            .get("Content-Type")
            .and_then(|value| value.as_str().ok())
            .is_some_and(v1::is_json_content_type);
        if !is_json {
            return Err(v1::BodyError::UnsupportedMediaType);
        }
        match Json::from_request(&(), parts, body).await {
            Ok(Json(value)) => Ok(value),
            Err(JsonRejection::IoError(ReadAllBodyError::BufferIsTooSmall { .. })) => {
                Err(v1::BodyError::PayloadTooLarge)
            }
            Err(_) => Err(v1::BodyError::BadRequest),
        }
    }
}

/// A response body the server writes from a `V`.
trait WriteBody<V>: v1::ResponseBody {
    /// Writes the response to `request` with `status` and a body holding
    /// `value`. If `closes`, the response tells the client the connection
    /// closes.
    async fn write<R: Read, W: ResponseWriter<Error = R::Error>>(
        value: V,
        status: StatusCode,
        closes: bool,
        request: RequestParts<'_>,
        connection: Connection<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error>;
}

impl WriteBody<()> for v1::Empty {
    async fn write<R: Read, W: ResponseWriter<Error = R::Error>>(
        (): (),
        status: StatusCode,
        closes: bool,
        _request: RequestParts<'_>,
        connection: Connection<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error> {
        // With no length, the client would read the body until the connection
        // closes.
        let response = Response::empty(status)
            .with_headers(("Content-Length", "0"))
            .with_headers(HEADERS)
            .with_headers(closes.then_some(("Connection", "close")));
        writer.write_response(connection, response).await
    }
}

impl<T: Serialize> WriteBody<T> for v1::Json<T> {
    async fn write<R: Read, W: ResponseWriter<Error = R::Error>>(
        value: T,
        status: StatusCode,
        closes: bool,
        _request: RequestParts<'_>,
        connection: Connection<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error> {
        let response = Json(value)
            .into_response()
            .with_status_code(status)
            .with_headers(HEADERS)
            .with_headers(closes.then_some(("Connection", "close")));
        writer.write_response(connection, response).await
    }
}

/// The source of a [`v1::Bytes`] body: its length, its tag, the name it is
/// saved under, and its bytes, read a chunk at a time as the response is
/// written.
pub(super) trait Source {
    /// The error of a read, which ends the body early.
    type Error: defmt::Format;

    /// The body's length in bytes.
    fn len(&self) -> u64;

    /// The body's strong entity tag, if it has one. A request whose
    /// `If-Range` differs from it is answered with the whole body.
    fn tag(&self) -> Option<v1::EntityTag<'_>>;

    /// The name the part `selection` of the body is saved under. Characters
    /// other than ASCII letters, digits, `.`, `-` and `_` are sent as `_`.
    fn filename(&self, selection: v1::Selection) -> impl fmt::Display;

    /// Reads the bytes from `offset` into `buffer`, filling it.
    ///
    /// A response reads each chunk of the body, at most [`CHUNK_LEN`] bytes,
    /// with one call, in order, once the previous chunk is written, and
    /// yields to the executor after writing it. A read that fails ends the
    /// body before its length, and the connection closes, so the client sees
    /// a short body.
    async fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<(), Self::Error>;
}

/// The most bytes of a binary body read and written at once: a quarter of a
/// TCP buffer, which keeps the chunk's buffer within the stack frame limit.
const CHUNK_LEN: usize = TCP_BUFFER_LEN / 4;

impl<S: Source> WriteBody<S> for v1::Bytes {
    /// Writes the part of the body `request` asks for. Every response with
    /// the body or a part of it tells the client the connection closes, as
    /// the connection closes once the body is written or a read of it fails.
    async fn write<R: Read, W: ResponseWriter<Error = R::Error>>(
        source: S,
        status: StatusCode,
        _closes: bool,
        request: RequestParts<'_>,
        connection: Connection<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error> {
        let header = |name| {
            request
                .headers()
                .get(name)
                .and_then(|value| value.as_str().ok())
        };
        // Range requests are defined for `GET` alone.
        let range = (request.method() == v1::Method::Get.as_str())
            .then(|| header("Range"))
            .flatten();
        let len = source.len();
        let tag = source.tag();
        let selection = v1::Selection::new(len, tag, range, header("If-Range"));
        let content_range = selection.content_range(len);

        let (status, first, count) = match selection {
            v1::Selection::Whole => (status, 0, len),
            v1::Selection::Part { first, last } => (
                StatusCode::new(v1::Bytes::PARTIAL_STATUS),
                first,
                last - first + 1,
            ),
            v1::Selection::Unsatisfiable => {
                return write_unsatisfiable(tag, content_range, connection, writer).await;
            }
        };

        let headers = [
            Some(("Content-Type", Header::Text("application/octet-stream"))),
            Some(("Content-Length", Header::Number(count))),
            Some(("Accept-Ranges", Header::Text("bytes"))),
            tag.map(|tag| ("ETag", Header::Tag(tag))),
            Some((
                "Content-Disposition",
                Header::Attachment(source.filename(selection)),
            )),
            content_range.map(|range| ("Content-Range", Header::Range(range))),
        ];
        // Custom, so the connection closes once the body is written: a body cut
        // short leaves nothing else to read on it.
        let response = CustomResponse::build(status)
            .with_headers(headers)
            .with_headers(HEADERS)
            .with_body(Chunks {
                source: &source,
                first,
                count,
            });
        response.write_to(connection, writer).await
    }
}

/// Writes the `416` response to a request for no part of a binary body whose
/// tag is `tag`, with the `Content-Range` `content_range`. Its body is the
/// JSON error, so it names no attachment.
async fn write_unsatisfiable<R: Read, W: ResponseWriter<Error = R::Error>>(
    tag: Option<v1::EntityTag<'_>>,
    content_range: Option<v1::ContentRange>,
    connection: Connection<'_, R>,
    writer: W,
) -> Result<ResponseSent, W::Error> {
    let response = error_response(v1::RangeError::RangeNotSatisfiable.code())
        .with_header("Accept-Ranges", "bytes")
        .with_headers(tag.map(|tag| ("ETag", tag)))
        .with_headers(content_range.map(|range| ("Content-Range", range)));
    writer.write_response(connection, response).await
}

/// The value of a header of a binary response. `F` writes the filename of an
/// attachment.
enum Header<'a, F> {
    Text(&'static str),
    Number(u64),
    Tag(v1::EntityTag<'a>),
    Range(v1::ContentRange),
    /// `attachment`, saved under the name `F` writes, its characters other
    /// than ASCII letters, digits, `.`, `-` and `_` written as `_`. The value
    /// can't then end the quoted name, or the header.
    Attachment(F),
}

impl<F: fmt::Display> fmt::Display for Header<'_, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.write_str(text),
            Self::Number(number) => write!(f, "{number}"),
            Self::Tag(tag) => write!(f, "{tag}"),
            Self::Range(range) => write!(f, "{range}"),
            Self::Attachment(filename) => {
                f.write_str("attachment; filename=\"")?;
                fmt::write(&mut Filename(f), format_args!("{filename}"))?;
                f.write_str("\"")
            }
        }
    }
}

/// Writes a filename to the formatter it holds, its characters other than
/// ASCII letters, digits, `.`, `-` and `_` as `_`.
struct Filename<'a, 'f>(&'a mut fmt::Formatter<'f>);

impl fmt::Write for Filename<'_, '_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            let safe = c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
            self.0.write_char(if safe { c } else { '_' })?;
        }
        Ok(())
    }
}

/// `count` bytes of `source` from `first`, written a chunk at a time.
struct Chunks<'a, S> {
    source: &'a S,
    first: u64,
    count: u64,
}

impl<S: Source> CustomBody for Chunks<'_, S> {
    async fn write_response_body<W: Write>(self, mut writer: W) -> Result<(), W::Error> {
        let mut buffer = [0; CHUNK_LEN];
        let mut offset = self.first;
        let end = self.first + self.count;
        while offset < end {
            let len = usize::try_from(end - offset).map_or(CHUNK_LEN, |left| left.min(CHUNK_LEN));
            let chunk = &mut buffer[..len];
            if let Err(error) = self.source.read(offset, chunk).await {
                warn!(
                    "Binary response body cut short after {} of {} bytes: {}",
                    offset - self.first,
                    self.count,
                    error
                );
                return Ok(());
            }
            writer.write_all(chunk).await?;
            offset += chunk.len() as u64;
            // A client reading as fast as the network allows would otherwise
            // hold the executor until its socket buffer fills.
            embassy_futures::yield_now().await;
        }
        Ok(())
    }
}

/// A handler's successful reply, with a body holding a `B`.
pub(super) trait Reply<B> {
    /// Whether the response tells the client the connection closes.
    const CLOSES: bool;

    /// The body, and what runs once the response is written, whether or not
    /// writing it succeeded.
    fn into_parts(self) -> (B, impl FnOnce());
}

impl<B> Reply<B> for B {
    const CLOSES: bool = false;

    fn into_parts(self) -> (B, impl FnOnce()) {
        (self, || ())
    }
}

/// A reply whose response tells the client the connection closes, with what
/// runs once the response is written, whether or not writing it succeeded.
/// Writing a response returns once the client has acknowledged it.
pub(super) struct Closing<B, F> {
    body: B,
    then: F,
}

impl<B, F: FnOnce()> Closing<B, F> {
    pub(super) fn new(body: B, then: F) -> Self {
        Self { body, then }
    }
}

impl<B, F: FnOnce()> Reply<B> for Closing<B, F> {
    const CLOSES: bool = true;

    fn into_parts(self) -> (B, impl FnOnce()) {
        (self.body, self.then)
    }
}
