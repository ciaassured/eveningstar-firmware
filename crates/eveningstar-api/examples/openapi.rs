//! Prints the OpenAPI document of version 1 of the API.

fn main() {
    println!("{:#}", eveningstar_api::v1::openapi());
}
