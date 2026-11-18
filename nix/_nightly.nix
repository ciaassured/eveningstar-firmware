# The pinned nightly toolchain: cargo-fuzz needs it, and only a nightly rustdoc
# checks the error code a `compile_fail` doctest names. This dated one is what
# rust-overlay's locked revision provides.
{ inputs, pkgs }:
(inputs.rust-overlay.lib.mkRustBin { } pkgs).nightly."2026-10-12".minimal
