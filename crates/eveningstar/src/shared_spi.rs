//! One SPI device shared by several users.

use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embedded_hal_async::spi::{ErrorType, Operation, SpiDevice};

/// A handle to an SPI device that other handles also use.
///
/// Each transaction runs whole under the device's lock, so transactions from
/// different handles never interleave. Every transaction yields to the executor
/// once it has finished.
pub(crate) struct SharedSpi<S: 'static>(&'static Mutex<CriticalSectionRawMutex, S>);

impl<S> SharedSpi<S> {
    pub(crate) fn new(device: &'static Mutex<CriticalSectionRawMutex, S>) -> Self {
        Self(device)
    }
}

impl<S: ErrorType> ErrorType for SharedSpi<S> {
    type Error = S::Error;
}

impl<S: SpiDevice> SpiDevice for SharedSpi<S> {
    async fn transaction(
        &mut self,
        operations: &mut [Operation<'_, u8>],
    ) -> Result<(), Self::Error> {
        let result = self.0.lock().await.transaction(operations).await;
        // A short transfer often finishes before it is first polled, so a
        // transaction can complete without ever yielding, and a caller that
        // loops on transactions would keep every other task from running.
        // Yielding once, after the lock is released, prevents that.
        yield_now().await;
        result
    }
}
