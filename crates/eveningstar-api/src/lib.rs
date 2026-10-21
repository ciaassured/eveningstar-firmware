//! Request and response bodies of the Eveningstar HTTP API, serialised as JSON.
//!
//! The crate is `no_std` and allocation-free.

#![cfg_attr(not(test), no_std)]

mod device_id;

pub use device_id::{DEVICE_ID_LEN, DeviceId, ParseDeviceIdError};
