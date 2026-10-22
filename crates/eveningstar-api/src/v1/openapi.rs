//! The OpenAPI document of version 1 of the API.

#![allow(
    clippy::large_stack_frames,
    reason = "building the document is a one-off that needs more stack than a firmware task has"
)]

use alloc::borrow::ToOwned;
use alloc::string::{String, ToString};

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, SchemaGenerator};
use serde_json::{Map, Value, json};

use super::{Error, ErrorCode, PATH, RebootRequest, RebootStatus, System, paths};

/// The document's description of the API as a whole.
const DESCRIPTION: &str = "The HTTP API of an Eveningstar, an interface board for \
    Morningstar solar charge controllers.\n\nA request to a path with no endpoint \
    is answered with `404` and the error `not_found`, and a request with a method \
    the endpoint does not support with `405` and `method_not_allowed`.";

/// The OpenAPI 3.1 document of every version 1 endpoint.
///
/// # Panics
///
/// If a type's request and response schemas differ.
#[must_use]
pub fn openapi() -> Value {
    let mut api = Document::new();

    let system = api.response::<System>();
    api.operation(
        paths::SYSTEM,
        "get",
        json!({
            "operationId": "get_system",
            "summary": "Get the device's identity and uptime",
            "responses": {
                "200": json_response("The device's identity and uptime.", &system),
            },
        }),
        &[],
    );

    let status = api.response::<RebootStatus>();
    api.operation(
        paths::REBOOT,
        "get",
        json!({
            "operationId": "get_reboot_status",
            "summary": "Get the pending reboot and its holds",
            "responses": {
                "200": json_response(
                    "The pending graceful reboot, if any, and what holds off graceful reboots.",
                    &status,
                ),
            },
        }),
        &[],
    );

    let request = api.request::<RebootRequest>();
    api.operation(
        paths::REBOOT,
        "post",
        json!({
            "operationId": "reboot",
            "summary": "Reboot the device",
            "description": "A graceful reboot waits for the holds listed by `GET /system/reboot` \
                to be released, up to a deadline.",
            "requestBody": json_request(&request),
            "responses": {
                "202": {
                    "description": "The device reboots after answering, dropping the connection.",
                },
            },
        }),
        &[ErrorCode::BadRequest, ErrorCode::PayloadTooLarge],
    );

    api.finish()
}

/// An OpenAPI document being built.
struct Document {
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

    /// Adds `operation` as `method` on `path`, answering with `errors` as well
    /// as its own responses.
    fn operation(&mut self, path: &str, method: &str, mut operation: Value, errors: &[ErrorCode]) {
        for &code in errors {
            operation["responses"][status(code).to_string()] =
                json!({ "$ref": format_args!("#/components/responses/{}", name(code)) });
        }
        let item = self.paths.entry(path).or_insert_with(|| json!({}));
        assert!(
            item.get(method).is_none(),
            "`{method} {path}` is already added"
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

        json!({
            "openapi": "3.1.1",
            "info": {
                "title": "Eveningstar API",
                "version": env!("CARGO_PKG_VERSION"),
                "description": DESCRIPTION,
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
        assert_eq!(responses.len(), 4);
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
