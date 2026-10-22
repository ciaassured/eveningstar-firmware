//! Request and response bodies of the Eveningstar HTTP API, serialised as JSON.
//!
//! The crate is `no_std` and allocation-free. The `schema` feature adds JSON
//! Schemas of the types and the OpenAPI document, which need `alloc`.

#![cfg_attr(not(test), no_std)]

#[cfg(feature = "schema")]
extern crate alloc;

mod device_id;
pub mod v1;

pub use device_id::{DEVICE_ID_LEN, DeviceId, ParseDeviceIdError};

/// Whether `T`'s JSON Schema accepts `value`.
#[cfg(all(test, feature = "schema"))]
fn schema_accepts<T: schemars::JsonSchema>(value: &serde_json::Value) -> bool {
    let schema = schemars::SchemaGenerator::default().into_root_schema_for::<T>();
    jsonschema::validator_for(schema.as_value())
        .unwrap()
        .is_valid(value)
}
