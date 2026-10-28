//! The OpenAPI document of version 1 of the API.

#![allow(
    clippy::large_stack_frames,
    reason = "building the document is a one-off that needs more stack than a firmware task has"
)]

use alloc::borrow::ToOwned;
use alloc::string::{String, ToString};
use core::fmt::Write;

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, SchemaGenerator};
use serde_json::{Map, Value, json};

use super::{
    Empty, Endpoint, Error, ErrorCode, Errors, Json, Operation, PATH, RequestBody, RouteError,
};

/// The document's description of the API as a whole, before the errors any
/// request can be answered with.
const DESCRIPTION: &str = "The HTTP API of an Eveningstar, an interface board for \
    Morningstar solar charge controllers.\n\nA request with a body must have the \
    `Content-Type` `application/json`, and is answered with `415` \
    `unsupported_media_type` otherwise.";

/// The OpenAPI 3.1 document of every version 1 operation.
///
/// # Panics
///
/// If a type's request and response schemas differ, an operation has two
/// responses with the same status code, or a JSON response's type has no
/// description.
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
        E::Request: DocumentRequest,
        E::Response: DocumentResponse,
    {
        let mut operation = json!({
            "operationId": id,
            "summary": E::SUMMARY,
            "responses": { E::STATUS.to_string(): E::Response::document(self) },
        });
        if let Some(description) = E::DESCRIPTION {
            operation["description"] = description.into();
        }
        if let Some(body) = E::Request::document(self) {
            operation["requestBody"] = body;
        }

        let errors = <E::Request as RequestBody>::Error::CODES
            .iter()
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

        let method = E::METHOD.as_str().to_ascii_lowercase();
        let item = self.paths.entry(E::PATH).or_insert_with(|| json!({}));
        assert!(
            item.get(&method).is_none(),
            "`{method} {}` has two operations",
            E::PATH
        );
        item[method] = operation;
    }

    fn finish(mut self) -> Value {
        let error = self.response::<Error>();

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
        let responses: Map<String, Value> = schemas["ErrorCode"]["oneOf"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|code| {
                let name = code["const"].as_str().unwrap_or_default().to_owned();
                let response = json!({
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

/// A response body the document can describe.
pub(super) trait DocumentResponse {
    /// The successful response of an operation.
    fn document(document: &mut Document) -> Value;
}

impl DocumentResponse for Empty {
    fn document(_document: &mut Document) -> Value {
        json!({ "description": "The response has no body." })
    }
}

impl<T: JsonSchema> DocumentResponse for Json<T> {
    fn document(document: &mut Document) -> Value {
        let schema = document.response::<T>();
        json_response(&document.response_description(&schema), &schema)
    }
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
    fn error_responses_name_their_codes() {
        let document = openapi();
        let responses = document["components"]["responses"].as_object().unwrap();
        assert_eq!(responses.len(), 5);
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
