//! Logging, through `defmt` with the `defmt` feature and to nowhere without
//! it.
//!
//! The macros take a format string and arguments as `defmt`'s do. Without the
//! feature they evaluate a reference to each argument and log nothing.

macro_rules! info {
    ($s:literal $(, $x:expr)* $(,)?) => {{
        #[cfg(feature = "defmt")]
        ::defmt::info!($s $(, $x)*);
        #[cfg(not(feature = "defmt"))]
        let _ = ($(&$x,)*);
    }};
}

macro_rules! warn {
    ($s:literal $(, $x:expr)* $(,)?) => {{
        #[cfg(feature = "defmt")]
        ::defmt::warn!($s $(, $x)*);
        #[cfg(not(feature = "defmt"))]
        let _ = ($(&$x,)*);
    }};
}

macro_rules! error {
    ($s:literal $(, $x:expr)* $(,)?) => {{
        #[cfg(feature = "defmt")]
        ::defmt::error!($s $(, $x)*);
        #[cfg(not(feature = "defmt"))]
        let _ = ($(&$x,)*);
    }};
}

/// `value`, logged through its `Debug` implementation.
#[cfg(feature = "defmt")]
pub(crate) fn debug<T: core::fmt::Debug + ?Sized>(value: &T) -> defmt::Debug2Format<'_, T> {
    defmt::Debug2Format(value)
}

/// `value`, logged through its `Debug` implementation.
#[cfg(not(feature = "defmt"))]
pub(crate) fn debug<T: core::fmt::Debug + ?Sized>(value: &T) -> &T {
    value
}
