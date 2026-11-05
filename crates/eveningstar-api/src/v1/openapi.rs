//! The OpenAPI document of version 1 of the API.

#![allow(
    clippy::large_stack_frames,
    reason = "building the document is a one-off that needs more stack than a firmware task has"
)]

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, SchemaGenerator};
use serde_json::{Map, Value, json};

use super::endpoint::param_name;
use super::{
    Bytes, Empty, Endpoint, Error, ErrorCode, Errors, Json, Method, Operation, PATH, PathParam,
    RequestBody, ResponseBody, RouteError, Scope,
};

/// The document's description of the API as a whole, before the errors any
/// request can be answered with.
const DESCRIPTION: &str = "The HTTP API of an Eveningstar, an interface board for \
    Morningstar solar charge controllers.\n\nA request with a body must have the \
    `Content-Type` `application/json`, and is answered with `415` \
    `unsupported_media_type` otherwise.\n\nEach operation's `security` \
    requirement names the scope a bearer token needs to call it. The device \
    has no authentication yet, so it enforces none of them.";

/// The name of the security scheme operations require scopes of.
const BEARER_SCHEME: &str = "bearerAuth";

/// What the `bearerAuth` security scheme says about itself.
const BEARER_DESCRIPTION: &str = "A token in the `Authorization` header: \
    `Authorization: Bearer <token>`. An operation's requirement lists the \
    scopes the token needs; the schema `Scope` defines them. The device \
    doesn't check tokens yet.";

/// The OpenAPI 3.1 document of every version 1 operation.
///
/// # Panics
///
/// If a type's request and response schemas differ, an operation has two
/// responses with the same status code, a JSON response's type has no
/// description, or an operation's path names a different number of
/// parameters than its `Params` type holds.
#[must_use]
pub fn openapi() -> Value {
    let mut document = Document::new();
    for &operation in Operation::ALL {
        operation.document(&mut document);
    }
    document.finish()
}

/// An OpenAPI document being built.
pub(super) struct Document {
    /// Schemas of request bodies, as the device reads them.
    requests: SchemaGenerator,
    /// Schemas of response bodies, as the device writes them.
    responses: SchemaGenerator,
    paths: Map<String, Value>,
}

impl Document {
    fn new() -> Self {
        let settings = SchemaSettings::draft2020_12().with(|settings| {
            settings.definitions_path = "/components/schemas".into();
            settings.meta_schema = None;
        });
        Self {
            requests: settings.clone().for_deserialize().into_generator(),
            responses: settings.for_serialize().into_generator(),
            paths: Map::new(),
        }
    }

    /// The schema of a `T` request body.
    fn request<T: JsonSchema>(&mut self) -> Value {
        self.requests.subschema_for::<T>().to_value()
    }

    /// The schema of a `T` response body.
    fn response<T: JsonSchema>(&mut self) -> Value {
        self.responses.subschema_for::<T>().to_value()
    }

    /// The description of the response body `schema`, or of the schema it
    /// refers to.
    fn response_description(&self, schema: &Value) -> String {
        let definition = schema["$ref"]
            .as_str()
            .and_then(|reference| reference.strip_prefix("#/components/schemas/"))
            .and_then(|name| self.responses.definitions().get(name))
            .unwrap_or(schema);
        let Some(description) = definition["description"].as_str() else {
            panic!("the response schema {schema} has no description");
        };
        description.to_owned()
    }

    /// Adds the operation with ID `id`, at `E`'s method and path.
    pub(super) fn operation<E>(&mut self, id: &str)
    where
        E: Endpoint,
        E::Params: DocumentParams,
        E::Request: DocumentRequest,
        E::Response: DocumentResponse,
    {
        let mut operation = json!({
            "operationId": id,
            "summary": E::SUMMARY,
            "security": [{ BEARER_SCHEME: [E::SCOPE] }],
            "responses": E::Response::document(self, E::STATUS),
        });
        if let Some(description) = E::DESCRIPTION {
            operation["description"] = description.into();
        }

        let names: Vec<&str> = E::PATH.split('/').filter_map(param_name).collect();
        let schemas = E::Params::document(self);
        assert_eq!(
            names.len(),
            schemas.len(),
            "the path of `{id}` names {} parameters, and its `Params` holds {}",
            names.len(),
            schemas.len()
        );
        let mut parameters: Vec<Value> = names
            .into_iter()
            .zip(schemas)
            .map(|(name, schema)| {
                json!({ "name": name, "in": "path", "required": true, "schema": schema })
            })
            .collect();
        parameters.extend(E::Response::parameters());
        if !parameters.is_empty() {
            operation["parameters"] = parameters.into();
        }

        if let Some(body) = E::Request::document(self) {
            operation["requestBody"] = body;
        }

        let errors = <E::Request as RequestBody>::Error::CODES
            .iter()
            .chain(<E::Response as ResponseBody>::Error::CODES)
            .chain(E::Error::CODES);
        for &code in errors {
            let status = status(code).to_string();
            assert!(
                operation["responses"].get(&status).is_none(),
                "`{id}` has two responses with status code {status}"
            );
            operation["responses"][status] =
                json!({ "$ref": format_args!("#/components/responses/{}", name(code)) });
        }

        let head = E::Response::head(E::STATUS).filter(|_| E::METHOD == Method::Get);
        let head = head.map(|responses| {
            let mut head = operation.clone();
            head["operationId"] = format!("{id}Head").into();
            head["summary"] = format!("{} (headers only)", E::SUMMARY).into();
            head["responses"] = responses;
            let path_parameters = operation["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|parameter| parameter["in"] == "path")
                .cloned();
            let parameters: Vec<Value> = path_parameters
                .chain(E::Response::head_parameters())
                .collect();
            head["parameters"] = parameters.into();
            let errors = <E::Response as ResponseBody>::Error::CODES
                .iter()
                .filter(|code| !E::Response::HEAD_SKIPS.contains(code))
                .chain(E::Error::CODES);
            for &code in errors {
                head["responses"][status(code).to_string()] =
                    json!({ "$ref": format_args!("#/components/responses/{}", name(code)) });
            }
            head
        });

        let method = E::METHOD.as_str().to_ascii_lowercase();
        let item = self.paths.entry(E::PATH).or_insert_with(|| json!({}));
        assert!(
            item.get(&method).is_none(),
            "`{method} {}` has two operations",
            E::PATH
        );
        item[method] = operation;
        if let Some(head) = head {
            item["head"] = head;
        }
    }

    fn finish(mut self) -> Value {
        let error = self.response::<Error>();
        // Operations name scopes by value, so no schema refers to them.
        self.response::<Scope>();

        let mut schemas = self.responses.take_definitions(true);
        for (name, schema) in self.requests.take_definitions(true) {
            if let Some(response) = schemas.get(&name) {
                assert_eq!(
                    response, &schema,
                    "the request and response schemas of `{name}` differ"
                );
            } else {
                schemas.insert(name, schema);
            }
        }

        // One response per error code, from the codes' schemas.
        let unsatisfiable = name(ErrorCode::RangeNotSatisfiable);
        let responses: Map<String, Value> = schemas["ErrorCode"]["oneOf"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|code| {
                let name = code["const"].as_str().unwrap_or_default().to_owned();
                let mut response = json!({
                    "description": code["description"],
                    "content": {
                        "application/json": {
                            "schema": {
                                "allOf": [error],
                                "properties": { "error": { "const": name } },
                            },
                        },
                    },
                });
                if name == unsatisfiable {
                    response["headers"] = json!({
                        "Content-Range": content_range("`bytes */<length>`: the body's length."),
                    });
                }
                let retry_after = serde_json::from_value::<ErrorCode>(json!(name))
                    .ok()
                    .and_then(ErrorCode::retry_after_s);
                if let Some(seconds) = retry_after {
                    response["headers"] = json!({
                        "Retry-After": {
                            "description": "The seconds to wait before asking again.",
                            "required": true,
                            "schema": { "const": seconds.to_string() },
                        },
                    });
                }
                (name, response)
            })
            .collect();

        let mut description = String::from(DESCRIPTION);
        description.push_str("\n\nAny request can also be answered with:\n");
        for &code in RouteError::CODES {
            let name = name(code);
            let meaning = responses[&name]["description"].as_str().unwrap_or_default();
            let _ = write!(description, "\n- `{}` `{name}`: {meaning}", status(code));
        }

        json!({
            "openapi": "3.1.1",
            "info": {
                "title": "Eveningstar API",
                "version": env!("CARGO_PKG_VERSION"),
                "description": description,
                "license": {
                    "name": env!("CARGO_PKG_LICENSE"),
                    "identifier": env!("CARGO_PKG_LICENSE"),
                },
            },
            "servers": [{ "url": PATH }],
            "paths": self.paths,
            "components": {
                "securitySchemes": {
                    BEARER_SCHEME: {
                        "type": "http",
                        "scheme": "bearer",
                        "description": BEARER_DESCRIPTION,
                    },
                },
                "schemas": schemas,
                "responses": responses,
            },
        })
    }
}

/// A request body the document can describe.
pub(super) trait DocumentRequest {
    /// The request body of an operation, or `None` if it has none.
    fn document(document: &mut Document) -> Option<Value>;
}

impl DocumentRequest for Empty {
    fn document(_document: &mut Document) -> Option<Value> {
        None
    }
}

impl<T: JsonSchema> DocumentRequest for Json<T> {
    fn document(document: &mut Document) -> Option<Value> {
        Some(json_request(&document.request::<T>()))
    }
}

/// The parameters of a path the document can describe.
pub(super) trait DocumentParams {
    /// The schema of each parameter, in the order of the path's segments.
    fn document(document: &mut Document) -> Vec<Value>;
}

impl DocumentParams for () {
    fn document(_document: &mut Document) -> Vec<Value> {
        Vec::new()
    }
}

impl<T: PathParam + JsonSchema> DocumentParams for T {
    fn document(document: &mut Document) -> Vec<Value> {
        alloc::vec![document.request::<T>()]
    }
}

/// A response body the document can describe.
pub(super) trait DocumentResponse {
    /// The successful responses of an operation whose status code is
    /// `status`, by status code.
    fn document(document: &mut Document, status: u16) -> Value;

    /// The request headers the responses depend on, as parameters.
    fn parameters() -> Vec<Value> {
        Vec::new()
    }

    /// The error codes of the body that a `HEAD` request is never answered
    /// with.
    const HEAD_SKIPS: &[ErrorCode] = &[];

    /// The successful responses to a `HEAD` request of a `GET` operation
    /// whose status code is `status`, or `None` if the body has no `HEAD`.
    fn head(_status: u16) -> Option<Value> {
        None
    }

    /// The request headers the responses to a `HEAD` request depend on, as
    /// parameters.
    fn head_parameters() -> Vec<Value> {
        Vec::new()
    }
}

impl DocumentResponse for Empty {
    fn document(_document: &mut Document, status: u16) -> Value {
        json!({ status.to_string(): { "description": "The response has no body." } })
    }
}

impl<T: JsonSchema> DocumentResponse for Json<T> {
    fn document(document: &mut Document, status: u16) -> Value {
        let schema = document.response::<T>();
        let response = json_response(&document.response_description(&schema), &schema);
        json!({ status.to_string(): response })
    }
}

impl DocumentResponse for Bytes {
    fn document(_document: &mut Document, status: u16) -> Value {
        let content = json!({
            "application/octet-stream": { "schema": { "type": "string", "format": "binary" } },
        });
        let mut headers = bytes_headers();
        headers["Content-Digest"] = json!({
            "description": "`sha-256=:<base64>:`: the SHA-256 of the bytes this \
                response carries (RFC 9530), when the request's \
                `Want-Content-Digest` asks for it.",
            "schema": { "type": "string" },
        });
        let mut partial_headers = headers.clone();
        partial_headers["Content-Range"] = content_range(
            "`bytes <first>-<last>/<length>`: the offsets of the first and last \
            bytes sent, and the body's length.",
        );
        json!({
            status.to_string(): {
                "description": "The whole body.",
                "headers": headers,
                "content": content,
            },
            Bytes::PARTIAL_STATUS.to_string(): {
                "description": "The part of the body the `Range` header asks for.",
                "headers": partial_headers,
                "content": content,
            },
        })
    }

    fn parameters() -> Vec<Value> {
        alloc::vec![
            json!({
                "name": "Range",
                "in": "header",
                "description": "One range of bytes of the body, such as \
                    `bytes=0-1023`, `bytes=1024-` or `bytes=-512`, to answer \
                    with that part alone. The last offset is clamped to the \
                    body's end. A header that isn't valid is ignored, and \
                    several ranges, or one starting past the body's end, are \
                    answered with `416`.",
                "schema": { "type": "string" },
            }),
            json!({
                "name": "If-Range",
                "in": "header",
                "description": "The `ETag` of the body the `Range` was taken \
                    from. If the body's tag differs, or the value isn't a \
                    strong entity tag, the answer is the whole body.",
                "schema": { "type": "string" },
            }),
            if_match_parameter(),
            json!({
                "name": "Want-Content-Digest",
                "in": "header",
                "description": "`sha-256=<weight>`, a weight from 1 to 10, to \
                    have the response carry a `Content-Digest` (RFC 9530). Any \
                    other algorithm, a weight of 0, and a value that isn't a \
                    valid dictionary are ignored.",
                "schema": { "type": "string" },
            }),
        ]
    }

    const HEAD_SKIPS: &[ErrorCode] = &[ErrorCode::RangeNotSatisfiable];

    fn head(status: u16) -> Option<Value> {
        Some(json!({
            status.to_string(): {
                "description": "The headers of the whole body, without the \
                    body. `Range` and `If-Range` are ignored.",
                "headers": bytes_headers(),
            },
        }))
    }

    fn head_parameters() -> Vec<Value> {
        alloc::vec![if_match_parameter()]
    }
}

/// The headers of every response with a binary body or a part of it, and of
/// the response to a `HEAD` request for one.
fn bytes_headers() -> Value {
    json!({
        "Content-Length": {
            "description": "The length of the bytes sent, or for a `HEAD` \
                request, of the whole body.",
            "required": true,
            "schema": { "type": "string" },
        },
        "Accept-Ranges": {
            "description": "`bytes`: the body can be asked for in part.",
            "required": true,
            "schema": { "const": "bytes" },
        },
        "ETag": {
            "description": "The body's strong entity tag. It differs whenever \
                the body may differ.",
            "required": true,
            "schema": { "type": "string" },
        },
        "Repr-Digest": {
            "description": "`sha-256=:<base64>:`: the SHA-256 of the whole \
                body (RFC 9530), also when a part of it is sent.",
            "required": true,
            "schema": { "type": "string" },
        },
        "Content-Disposition": {
            "description": "`attachment`, with the name to save the body under.",
            "required": true,
            "schema": { "type": "string" },
        },
    })
}

/// The `If-Match` request header, as a parameter.
fn if_match_parameter() -> Value {
    json!({
        "name": "If-Match",
        "in": "header",
        "description": "`*`, or a list of entity tags. If none of them is \
            the body's `ETag`, compared strongly, the answer is `412`.",
        "schema": { "type": "string" },
    })
}

/// A required `Content-Range` response header, described by `description`.
fn content_range(description: &str) -> Value {
    json!({ "description": description, "required": true, "schema": { "type": "string" } })
}

/// A response with a JSON body matching `schema`.
fn json_response(description: &str, schema: &Value) -> Value {
    json!({
        "description": description,
        "content": { "application/json": { "schema": schema } },
    })
}

/// A required JSON request body matching `schema`.
fn json_request(schema: &Value) -> Value {
    json!({
        "required": true,
        "content": { "application/json": { "schema": schema } },
    })
}

/// `code` as it is written in JSON.
fn name(code: ErrorCode) -> String {
    match serde_json::to_value(code) {
        Ok(Value::String(name)) => name,
        _ => unreachable!("error codes serialise as strings"),
    }
}

/// The status code of responses with `code`.
fn status(code: ErrorCode) -> u16 {
    let Some(status) = code.status() else {
        panic!("error code `{}` has no status code", name(code));
    };
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether the document's schema `name` accepts `value`.
    fn component_accepts(document: &Value, name: &str, value: &Value) -> bool {
        let mut schema = document.clone();
        schema["$ref"] = json!(format!("#/components/schemas/{name}"));
        jsonschema::validator_for(&schema).unwrap().is_valid(value)
    }

    /// Every `$ref` in `value`.
    fn references(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                if let Some(Value::String(reference)) = object.get("$ref") {
                    found.push(reference.clone());
                }
                object.values().for_each(|value| references(value, found));
            }
            Value::Array(array) => array.iter().for_each(|value| references(value, found)),
            _ => {}
        }
    }

    #[test]
    fn every_reference_resolves() {
        let document = openapi();
        let mut found = Vec::new();
        references(&document, &mut found);
        assert_ne!(found, Vec::<String>::new());
        for reference in found {
            let pointer = reference.strip_prefix('#').unwrap();
            assert!(
                document.pointer(pointer).is_some(),
                "{reference} is missing"
            );
        }
    }

    #[test]
    fn every_operation_is_at_its_method_and_path() {
        let document = openapi();
        for &operation in Operation::ALL {
            let method = operation.method().as_str().to_ascii_lowercase();
            let id = &document["paths"][operation.path()][method]["operationId"];
            assert_eq!(id, operation.name());
        }
    }

    #[test]
    fn responses_follow_the_endpoints() {
        let document = openapi();
        let statuses = |path: &str, method: &str| -> Vec<String> {
            let responses = document["paths"][path][method]["responses"].as_object();
            responses.unwrap().keys().cloned().collect()
        };
        assert_eq!(statuses("/system", "get"), ["200"]);
        // Reading the JSON request body adds its errors.
        assert_eq!(
            statuses("/system/reboot", "post"),
            ["202", "400", "413", "415"]
        );
        let reboot = &document["paths"]["/system/reboot"]["post"];
        assert!(reboot["requestBody"].is_object());
        assert!(reboot["responses"]["202"].get("content").is_none());
        assert!(reboot.get("parameters").is_none());
        assert!(
            document["paths"]["/system"]["get"]
                .get("parameters")
                .is_none()
        );
    }

    #[test]
    fn path_parameters_are_declared_with_their_schemas() {
        let document = openapi();
        let operation = &document["paths"]["/test/{item}/content"]["get"];
        let parameter = &operation["parameters"][0];
        assert_eq!(
            parameter,
            &json!({
                "name": "item",
                "in": "path",
                "required": true,
                "schema": { "$ref": "#/components/schemas/TestItem" },
            })
        );
        assert!(component_accepts(&document, "TestItem", &json!("first")));
        assert!(component_accepts(
            &document,
            "TestItem",
            &json!("second_item")
        ));
        assert!(!component_accepts(&document, "TestItem", &json!("third")));
    }

    #[test]
    fn binary_bodies_are_answered_whole_or_in_part() {
        let document = openapi();
        let operation = &document["paths"]["/test/{item}/content"]["get"];
        let responses = operation["responses"].as_object().unwrap();
        let statuses: Vec<_> = responses.keys().cloned().collect();
        assert_eq!(statuses, ["200", "206", "412", "416"]);

        let binary = json!({ "type": "string", "format": "binary" });
        for status in ["200", "206"] {
            let response = &responses[status];
            assert_eq!(
                response["content"]["application/octet-stream"]["schema"],
                binary
            );
            let headers = response["headers"].as_object().unwrap();
            for header in [
                "Content-Length",
                "Accept-Ranges",
                "ETag",
                "Repr-Digest",
                "Content-Disposition",
            ] {
                assert_eq!(headers[header]["required"], true, "{status} {header}");
            }
            assert!(headers["Content-Digest"].get("required").is_none());
            assert_eq!(headers.contains_key("Content-Range"), status == "206");
        }
        assert_eq!(
            responses["416"],
            json!({ "$ref": "#/components/responses/range_not_satisfiable" })
        );
        let unsatisfiable = &document["components"]["responses"]["range_not_satisfiable"];
        assert!(unsatisfiable["headers"]["Content-Range"].is_object());
        let not_found = &document["components"]["responses"]["not_found"];
        assert!(not_found.get("headers").is_none());

        let headers: Vec<_> = operation["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|parameter| parameter["in"] == "header")
            .map(|parameter| parameter["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            headers,
            ["Range", "If-Range", "If-Match", "Want-Content-Digest"]
        );
    }

    #[test]
    fn binary_bodies_answer_head_with_their_headers_alone() {
        let document = openapi();
        let item = &document["paths"]["/test/{item}/content"];
        let head = &item["head"];
        assert_eq!(head["operationId"], "GetTestContentHead");
        assert_eq!(head["security"], item["get"]["security"]);
        let responses = head["responses"].as_object().unwrap();
        let statuses: Vec<_> = responses.keys().cloned().collect();
        assert_eq!(statuses, ["200", "412"]);
        assert!(responses["200"].get("content").is_none());
        let headers = responses["200"]["headers"].as_object().unwrap();
        assert!(headers.contains_key("Repr-Digest"));
        assert!(!headers.contains_key("Content-Digest"));
        let parameters: Vec<_> = head["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|parameter| parameter["name"].as_str().unwrap())
            .collect();
        assert_eq!(parameters, ["item", "If-Match"]);
        // JSON bodies have no `HEAD`.
        assert!(document["paths"]["/system"].get("head").is_none());
    }

    #[test]
    fn busy_responses_say_when_to_ask_again() {
        let document = openapi();
        let responses = &document["components"]["responses"];
        let retry_after = &responses["busy"]["headers"]["Retry-After"];
        assert_eq!(retry_after["required"], true);
        assert_eq!(retry_after["schema"]["const"], "5");
        assert!(responses["unpublished"].get("headers").is_none());
    }

    #[test]
    fn route_errors_are_described_once() {
        let document = openapi();
        let description = document["info"]["description"].as_str().unwrap();
        assert!(description.contains("`404` `not_found`"), "{description}");
        assert!(
            description.contains("`405` `method_not_allowed`"),
            "{description}"
        );
    }

    #[test]
    fn operation_ids_are_unique() {
        let document = openapi();
        let mut ids: Vec<&str> = document["paths"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|item| item.as_object().unwrap().values())
            .map(|operation| operation["operationId"].as_str().unwrap())
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn responses_are_always_complete() {
        let document = openapi();
        let idle = json!({ "pending": null, "holds": [] });
        assert!(component_accepts(&document, "RebootStatus", &idle));
        // The device always writes `pending`, even with no pending reboot.
        let partial = json!({ "holds": [] });
        assert!(!component_accepts(&document, "RebootStatus", &partial));
    }

    #[test]
    fn requests_match_their_schemas() {
        let document = openapi();
        let request = json!({ "mode": "graceful" });
        assert!(component_accepts(&document, "RebootRequest", &request));
        let request = json!({ "mode": "later" });
        assert!(!component_accepts(&document, "RebootRequest", &request));
    }

    #[test]
    fn operations_name_their_scopes() {
        let document = openapi();
        for &operation in Operation::ALL {
            let item = &document["paths"][operation.path()];
            let method = operation.method().as_str().to_ascii_lowercase();
            let required = &item[method]["security"];
            assert_eq!(
                required,
                &json!([{ "bearerAuth": [operation.scope()] }]),
                "{}",
                operation.name()
            );
        }
        let scheme = &document["components"]["securitySchemes"]["bearerAuth"];
        assert_eq!(scheme["type"], "http");
        assert_eq!(scheme["scheme"], "bearer");
        let scopes = &document["components"]["schemas"]["Scope"]["oneOf"];
        let names: Vec<_> = scopes
            .as_array()
            .unwrap()
            .iter()
            .map(|scope| scope["const"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["read", "operate", "configure"]);
    }

    #[test]
    fn error_responses_name_their_codes() {
        let document = openapi();
        let responses = document["components"]["responses"].as_object().unwrap();
        assert_eq!(responses.len(), 10);
        for (name, response) in responses {
            let code: ErrorCode = serde_json::from_value(json!(name)).unwrap();
            let status = status(code);
            assert!(
                response["description"]
                    .as_str()
                    .is_some_and(|d| !d.is_empty())
            );

            let mut schema = document.clone();
            schema["$ref"] = json!(format!(
                "#/components/responses/{name}/content/application~1json/schema"
            ));
            let validator = jsonschema::validator_for(&schema).unwrap();
            assert!(validator.is_valid(&json!({ "error": name })), "{status}");
            assert!(
                !validator.is_valid(&json!({ "error": "teapot" })),
                "{status}"
            );
        }
    }
}
