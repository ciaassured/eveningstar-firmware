//! The API under [`v1::PATH`]. Each request is routed to its
//! [`v1::Operation`], and its body read and its response written as the
//! operation's [`Endpoint`] declares. Errors are answered with a JSON
//! [`v1::Error`].

use core::convert::Infallible;

use eveningstar_api::v1::{self, Endpoint, Errors, Operation};
use picoserve::ResponseSent;
use picoserve::extract::{FromRef, FromRequest, JsonRejection};
use picoserve::io::Read;
use picoserve::request::{Path, ReadAllBodyError, Request, RequestBody, RequestParts};
use picoserve::response::{Connection, HeadersIter, Json, Response, ResponseWriter, StatusCode};
use picoserve::routing::{MethodHandlerService, PathRouterService};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{AppState, system};

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
        Ok(Operation::GetSystem(endpoint)) => {
            serve(endpoint, state, request, writer, system::get_system).await
        }
        Ok(Operation::GetRebootStatus(endpoint)) => {
            serve(endpoint, state, request, writer, system::get_reboot).await
        }
        Ok(Operation::Reboot(endpoint)) => {
            serve(endpoint, state, request, writer, system::post_reboot).await
        }
        Err(error) => {
            let connection = request.body_connection.finalize().await?;
            write_error(error.code(), connection, writer).await
        }
    }
}

/// Serves a request for `E`: reads its body, passes it to `handler` with the
/// part of `state` the handler takes, and writes the handler's reply with
/// `E`'s status code, or its error.
async fn serve<E, S, H, Rep, R, W>(
    _endpoint: E,
    state: &AppState,
    mut request: Request<'_, R>,
    writer: W,
    handler: H,
) -> Result<ResponseSent, W::Error>
where
    E: Endpoint<Request: ReadBody, Response: WriteBody>,
    S: FromRef<AppState>,
    H: AsyncFnOnce(S, <E::Request as v1::RequestBody>::Value) -> Result<Rep, E::Error>,
    Rep: Reply<<E::Response as v1::ResponseBody>::Value>,
    R: Read,
    W: ResponseWriter<Error = R::Error>,
{
    let body = E::Request::read(request.parts, request.body_connection.body()).await;
    let result = match body {
        Ok(body) => handler(S::from_ref(state), body)
            .await
            .map_err(Errors::code),
        Err(error) => Err(error.code()),
    };
    let connection = request.body_connection.finalize().await?;

    let reply = match result {
        Ok(reply) => reply,
        Err(code) => return write_error(code, connection, writer).await,
    };
    let headers = Rep::CLOSES.then_some(("Connection", "close"));
    let (body, then) = reply.into_parts();
    let status = StatusCode::new(E::STATUS);
    let sent = E::Response::write(body, status, headers, connection, writer).await;
    then();
    sent
}

/// Writes a response with `code`.
pub(super) async fn write_error<R: Read, W: ResponseWriter<Error = R::Error>>(
    code: v1::ErrorCode,
    connection: Connection<'_, R>,
    writer: W,
) -> Result<ResponseSent, W::Error> {
    // Every code in the API's error sets has a status.
    let status = code
        .status()
        .map_or(StatusCode::INTERNAL_SERVER_ERROR, StatusCode::new);
    let response = Json(v1::Error { error: code })
        .into_response()
        .with_status_code(status)
        .with_headers(HEADERS);
    writer.write_response(connection, response).await
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

/// A response body the server writes.
trait WriteBody: v1::ResponseBody {
    /// Writes a response with `status`, `headers` and a body holding `value`.
    async fn write<R: Read, W: ResponseWriter<Error = R::Error>>(
        value: Self::Value,
        status: StatusCode,
        headers: impl HeadersIter,
        connection: Connection<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error>;
}

impl WriteBody for v1::Empty {
    async fn write<R: Read, W: ResponseWriter<Error = R::Error>>(
        (): (),
        status: StatusCode,
        headers: impl HeadersIter,
        connection: Connection<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error> {
        // With no length, the client would read the body until the connection
        // closes.
        let response = Response::empty(status)
            .with_headers(("Content-Length", "0"))
            .with_headers(HEADERS)
            .with_headers(headers);
        writer.write_response(connection, response).await
    }
}

impl<T: Serialize> WriteBody for v1::Json<T> {
    async fn write<R: Read, W: ResponseWriter<Error = R::Error>>(
        value: T,
        status: StatusCode,
        headers: impl HeadersIter,
        connection: Connection<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error> {
        let response = Json(value)
            .into_response()
            .with_status_code(status)
            .with_headers(HEADERS)
            .with_headers(headers);
        writer.write_response(connection, response).await
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

impl<B: Serialize> Reply<B> for B {
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
