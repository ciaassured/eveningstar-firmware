//! API error responses: a status code and a JSON [`v1::Error`] body.
//!
//! [`NotFound`] answers requests no route matches. [`JsonErrors`] replaces the
//! plain text error responses picoserve's routing writes itself, and
//! [`ApiJson`] those of its JSON body extractor.

use eveningstar_api::v1;
use picoserve::ResponseSent;
use picoserve::extract::{FromRequest, JsonRejection};
use picoserve::io::Read;
use picoserve::request::{Path, ReadAllBodyError, Request, RequestBody, RequestParts};
use picoserve::response::{
    Body, Connection, HeadersIter, IntoResponse, Json, Response, ResponseWriter, StatusCode,
};
use picoserve::routing::{Layer, MethodHandlerService, Next, PathRouterService};

/// An error an API handler responds with.
#[derive(Clone, Copy, Debug, defmt::Format)]
pub(super) enum ApiError {
    BadRequest,
    NotFound,
    MethodNotAllowed,
    PayloadTooLarge,
}

impl ApiError {
    fn status_code(self) -> StatusCode {
        match self {
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        }
    }

    fn code(self) -> v1::ErrorCode {
        match self {
            Self::BadRequest => v1::ErrorCode::BadRequest,
            Self::NotFound => v1::ErrorCode::NotFound,
            Self::MethodNotAllowed => v1::ErrorCode::MethodNotAllowed,
            Self::PayloadTooLarge => v1::ErrorCode::PayloadTooLarge,
        }
    }
}

impl IntoResponse for ApiError {
    async fn write_to<R: Read, W: ResponseWriter<Error = R::Error>>(
        self,
        connection: Connection<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        Json(v1::Error { error: self.code() })
            .into_response()
            .with_status_code(self.status_code())
            .write_to(connection, response_writer)
            .await
    }
}

/// Answers every request with [`ApiError::NotFound`], whatever its path or
/// method.
pub(super) struct NotFound;

impl<State, PathParameters> PathRouterService<State, PathParameters> for NotFound {
    async fn call_path_router_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        _state: &State,
        _path_parameters: PathParameters,
        _path: Path<'_>,
        request: Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        ApiError::NotFound
            .write_to(request.body_connection.finalize().await?, response_writer)
            .await
    }
}

impl<State, PathParameters> MethodHandlerService<State, PathParameters> for NotFound {
    async fn call_method_handler_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        _state: &State,
        _path_parameters: PathParameters,
        _method: &str,
        request: Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        ApiError::NotFound
            .write_to(request.body_connection.finalize().await?, response_writer)
            .await
    }
}

/// Replaces the 405 responses picoserve's method routing writes, which are
/// plain text, with [`ApiError::MethodNotAllowed`].
pub(super) struct JsonErrors;

impl<State, PathParameters> Layer<State, PathParameters> for JsonErrors {
    type NextState = State;
    type NextPathParameters = PathParameters;

    async fn call_layer<
        'a,
        R: Read + 'a,
        NextLayer: Next<'a, R, State, PathParameters>,
        W: ResponseWriter<Error = R::Error>,
    >(
        &self,
        next: NextLayer,
        state: &State,
        path_parameters: PathParameters,
        _request_parts: RequestParts<'_>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        next.run(state, path_parameters, JsonErrorWriter(response_writer))
            .await
    }
}

/// Writes responses through to the wrapped writer, except 405s, which it
/// replaces with [`ApiError::MethodNotAllowed`].
struct JsonErrorWriter<W>(W);

impl<W: ResponseWriter> ResponseWriter for JsonErrorWriter<W> {
    type Error = W::Error;

    async fn write_response<R: Read<Error = Self::Error>, H: HeadersIter, B: Body>(
        self,
        connection: Connection<'_, R>,
        response: Response<H, B>,
    ) -> Result<ResponseSent, Self::Error> {
        if response.status_code() == StatusCode::METHOD_NOT_ALLOWED {
            ApiError::MethodNotAllowed
                .write_to(connection, self.0)
                .await
        } else {
            self.0.write_response(connection, response).await
        }
    }
}

/// A JSON request body, like picoserve's [`Json`] extractor, but rejected with
/// an [`ApiError`] instead of plain text.
pub(super) struct ApiJson<T>(pub(super) T);

impl<'r, State, T: serde::Deserialize<'r>> FromRequest<'r, State> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request<R: Read>(
        state: &'r State,
        request_parts: RequestParts<'r>,
        request_body: RequestBody<'r, R>,
    ) -> Result<Self, Self::Rejection> {
        match Json::from_request(state, request_parts, request_body).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(JsonRejection::IoError(ReadAllBodyError::BufferIsTooSmall { .. })) => {
                Err(ApiError::PayloadTooLarge)
            }
            Err(_) => Err(ApiError::BadRequest),
        }
    }
}
