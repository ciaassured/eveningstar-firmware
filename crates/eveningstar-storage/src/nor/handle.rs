//! Readers, writers and write sessions of the internal flash's regions.

use core::fmt;
use core::marker::PhantomData;

use eveningstar_esp_formats::pace::{Pace as _, Yield};

use super::{NorDevice, SECTOR_SIZE, State, WORD_SIZE};
use crate::region::{CHUNK_LEN, is_protected};
use crate::roles::{Internal, Programmable, Role, Writable};
use crate::span::Span;
use crate::state::{Spans, WriteState};
use crate::{AccessError, ReadError, Slot, Source};

/// The absolute address of the `len` bytes at `offset` in `span`.
fn address(span: Span, offset: u32, len: usize) -> Result<u32, ReadError> {
    let len = u32::try_from(len).map_err(|_| ReadError::OutOfRange)?;
    span.inner(Span::new(offset, len))
        .map(|inner| inner.offset)
        .ok_or(ReadError::OutOfRange)
}

/// Fills `buf` with the bytes at `offset` in `span` of `device`, in one call.
fn read_direct(
    device: &dyn NorDevice,
    span: Span,
    offset: u32,
    buf: &mut [u8],
) -> Result<(), ReadError> {
    let address = address(span, offset, buf.len())?;
    device.read(address, buf).map_err(|_| ReadError::Device)
}

/// Fills `buf` with the bytes at `offset` of a span `len` bytes long through
/// `read`, in steps of at most [`CHUNK_LEN`] bytes, yielding after each full
/// step. Nothing is read when the range doesn't lie inside the span. On any
/// error `buf` is zeroed, so no bytes of a failed read are kept.
pub(super) async fn read_chunked(
    len: u32,
    offset: u32,
    buf: &mut [u8],
    read: impl Fn(u32, &mut [u8]) -> Result<(), ReadError>,
) -> Result<(), ReadError> {
    if let Err(error) = address(Span::new(0, len), offset, buf.len()) {
        buf.fill(0);
        return Err(error);
    }
    let mut at = offset;
    let mut result = Ok(());
    for chunk in buf.chunks_mut(CHUNK_LEN as usize) {
        // At most CHUNK_LEN.
        let Ok(chunk_len) = u32::try_from(chunk.len()) else {
            result = Err(ReadError::OutOfRange);
            break;
        };
        if let Err(error) = read(at, chunk) {
            result = Err(error);
            break;
        }
        at = at.saturating_add(chunk_len);
        if chunk_len == CHUNK_LEN {
            Yield.pause().await;
        }
    }
    if result.is_err() {
        buf.fill(0);
    }
    result
}

/// What every handle holds: the device, its state table, and the region's
/// absolute span and the physical spans it covers.
#[derive(Clone, Copy)]
pub(super) struct Extent<'f> {
    pub(super) device: &'f dyn NorDevice,
    pub(super) state: &'f State,
    pub(super) span: Span,
    pub(super) spans: Spans,
}

impl<'f> Extent<'f> {
    /// The extent of `span`, absolute, on `device`, or `None` when it
    /// doesn't lie inside the device's capacity.
    fn new(device: &'f dyn NorDevice, state: &'f State, span: Span) -> Option<Self> {
        if !Span::new(0, device.capacity()).contains(span) {
            return None;
        }
        Some(Self {
            device,
            state,
            span,
            spans: State::spans_of(span),
        })
    }

    /// The region's write state.
    pub(super) fn write_state(&self) -> WriteState {
        self.state.load(self.spans)
    }
}

impl fmt::Debug for Extent<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extent")
            .field("span", &self.span)
            .field("state", &self.write_state())
            .finish_non_exhaustive()
    }
}

/// The reader of a region with role `R`.
///
/// A reader reads only through snapshots, which fail once the region is
/// written. There is at most one reader per role and boot: it is neither
/// `Clone` nor `Copy`, and only the crate makes one.
pub struct Reader<'f, R: Role<Device = Internal>> {
    pub(super) extent: Extent<'f>,
    role: PhantomData<fn() -> R>,
}

impl<'f, R: Role<Device = Internal>> Reader<'f, R> {
    /// The reader of `span`, absolute, on `device`, or `None` when the span
    /// doesn't lie inside the device's capacity.
    #[cfg_attr(
        not(any(test, feature = "test-support")),
        allow(dead_code, reason = "only the test support makes readers so far")
    )]
    pub(crate) fn new(device: &'f dyn NorDevice, state: &'f State, span: Span) -> Option<Self> {
        Some(Self {
            extent: Extent::new(device, state, span)?,
            role: PhantomData,
        })
    }

    /// The region's span, from the start of the device.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.extent.span
    }

    /// Length of the region in bytes.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.extent.span.len
    }

    /// Whether the region has no bytes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.extent.span.len == 0
    }

    /// The region's write state now.
    #[must_use]
    pub fn write_state(&self) -> WriteState {
        self.extent.write_state()
    }
}

impl<R: Role<Device = Internal>> fmt::Debug for Reader<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reader")
            .field("role", &R::NAME)
            .field("extent", &self.extent)
            .finish()
    }
}

/// The writer of a region with role `R`: the region's only writer.
///
/// A writer reads the region directly at any time, as nothing else writes
/// it. It erases and programs only inside a [`Session`]
/// ([`begin`](Self::begin)). There is at most one writer per role and boot:
/// it is neither `Clone` nor `Copy`, and only the crate makes one.
pub struct Writer<'f, R: Writable<Device = Internal>> {
    extent: Extent<'f>,
    /// The slot that booted, whose app the guard protects.
    running: Slot,
    role: PhantomData<fn() -> R>,
}

impl<'f, R: Writable<Device = Internal>> Writer<'f, R> {
    /// The writer of `span`, absolute, on `device`, while the app runs from
    /// `running`; `None` when the span doesn't lie inside the device's
    /// capacity or inside one physical span of the compiled layout, so every
    /// byte it writes has a write state.
    #[cfg_attr(
        not(any(test, feature = "test-support")),
        allow(dead_code, reason = "only the test support makes writers so far")
    )]
    pub(crate) fn new(
        device: &'f dyn NorDevice,
        state: &'f State,
        span: Span,
        running: Slot,
    ) -> Option<Self> {
        if !State::inside_one(span) {
            return None;
        }
        Some(Self {
            extent: Extent::new(device, state, span)?,
            running,
            role: PhantomData,
        })
    }

    /// The region's span, from the start of the device.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.extent.span
    }

    /// Length of the region in bytes.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.extent.span.len
    }

    /// Whether the region has no bytes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.extent.span.len == 0
    }

    /// The region's write state now: always `Stable`, as a session borrows
    /// the writer.
    #[must_use]
    pub fn write_state(&self) -> WriteState {
        self.extent.write_state()
    }

    /// Opens a write session, which marks the region written to until it is
    /// dropped. No snapshot of the region can be taken meanwhile, and those
    /// taken before fail.
    pub fn begin(&mut self) -> Session<'_, R> {
        Session::new(self.extent, self.running)
    }

    /// Fills `buf` with the bytes at `offset` in the region, in steps of at
    /// most 4 KiB with a yield after each.
    ///
    /// # Errors
    ///
    /// [`ReadError::OutOfRange`] when the range doesn't lie inside the
    /// region, before anything is read, and [`ReadError::Device`] when the
    /// flash fails a read; `buf` is zeroed on either.
    pub async fn read_async(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        read_chunked(self.len(), offset, buf, |at, chunk| {
            Source::read(self, at, chunk)
        })
        .await
    }
}

impl<R: Writable<Device = Internal>> Source for Writer<'_, R> {
    fn size(&self) -> u32 {
        self.extent.span.len
    }

    /// Reads the region directly, in one device call.
    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        read_direct(self.extent.device, self.extent.span, offset, buf)
    }
}

impl<R: Writable<Device = Internal>> fmt::Debug for Writer<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writer")
            .field("role", &R::NAME)
            .field("extent", &self.extent)
            .field("running", &self.running)
            .finish()
    }
}

/// A write session on a region with role `R`, from [`Writer::begin`].
///
/// While it lives the region is written to: snapshots of it can't be taken,
/// and those taken before fail. Its erases and programs take offsets relative
/// to the region, use checked arithmetic, and pass the guard of the protected
/// spans (the bootloader's span and the table's sector, `factory`,
/// `reserved` and the slot that booted) before they reach the device.
///
/// The session counts as written from just before its first device call: a
/// call refused before it reaches the device marks nothing, and a call the
/// device fails counts, as it may have changed bits. Dropping the session, on
/// any path, ends it: at a new epoch when it issued a device call, else at
/// the epoch it started from. A session leaked rather than dropped leaves the
/// region written to for the rest of the boot.
pub struct Session<'w, R: Writable<Device = Internal>> {
    extent: Extent<'w>,
    running: Slot,
    /// Whether a device call was issued.
    written: bool,
    role: PhantomData<fn() -> R>,
}

impl<'w, R: Writable<Device = Internal>> Session<'w, R> {
    /// Opens a session on `extent`.
    ///
    /// The open state is stored before the session issues any device call:
    /// the store is `SeqCst`, and each device call is a compiler and hardware
    /// barrier on the one core that reaches the flash, so no call of the
    /// session moves before it.
    fn new(extent: Extent<'w>, running: Slot) -> Self {
        extent.state.begin(extent.spans);
        Self {
            extent,
            running,
            written: false,
            role: PhantomData,
        }
    }

    /// The absolute range `from..to` of the region, checked against its
    /// length; `None` when it isn't inside the region.
    fn range(&self, from: u32, to: u32) -> Option<(u32, u32)> {
        if from > to || to > self.extent.span.len {
            return None;
        }
        let start = self.extent.span.offset.checked_add(from)?;
        let end = self.extent.span.offset.checked_add(to)?;
        Some((start, end))
    }

    /// Refuses `start..end`, absolute, when it touches a protected span.
    fn guard(&self, start: u32, end: u32) -> Result<(), AccessError> {
        if is_protected(start, end, Some(self.running)) {
            Err(AccessError::Protected)
        } else {
            Ok(())
        }
    }

    /// Erases the sector at `address`, marking the session written first.
    fn erase_sector(&mut self, address: u32) -> Result<(), AccessError> {
        self.written = true;
        self.extent
            .device
            .erase_sector(address)
            .map_err(|_| AccessError::Device)
    }

    /// Programs `bytes` at `address`, marking the session written first.
    fn program_chunk(&mut self, address: u32, bytes: &[u8]) -> Result<(), AccessError> {
        self.written = true;
        self.extent
            .device
            .program(address, bytes)
            .map_err(|_| AccessError::Device)
    }

    /// Erases the sectors from `from` up to `to`, offsets in the region on
    /// sector boundaries, setting their bytes to `0xFF`. Erases one sector
    /// per device call and yields after each.
    ///
    /// # Errors
    ///
    /// [`AccessError::OutOfRange`] when `from` is past `to` or `to` past the
    /// end of the region, [`AccessError::NotAligned`] when either isn't on a
    /// sector boundary of the device, [`AccessError::Protected`] when the
    /// range touches a protected span, all before anything is erased, and
    /// [`AccessError::Device`] when the flash fails an erase, after the
    /// sectors before it were erased.
    pub async fn erase(&mut self, from: u32, to: u32) -> Result<(), AccessError> {
        let (start, end) = self.range(from, to).ok_or(AccessError::OutOfRange)?;
        if !start.is_multiple_of(SECTOR_SIZE) || !end.is_multiple_of(SECTOR_SIZE) {
            return Err(AccessError::NotAligned);
        }
        self.guard(start, end)?;
        let mut address = start;
        while address < end {
            let next = address
                .checked_add(SECTOR_SIZE)
                .ok_or(AccessError::OutOfRange)?;
            self.erase_sector(address)?;
            address = next;
            Yield.pause().await;
        }
        Ok(())
    }

    /// The region's span, from the start of the device.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.extent.span
    }

    /// Length of the region in bytes.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.extent.span.len
    }

    /// Whether the region has no bytes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.extent.span.len == 0
    }

    /// Fills `buf` with the bytes at `offset` in the region, in steps of at
    /// most 4 KiB with a yield after each.
    ///
    /// # Errors
    ///
    /// [`ReadError::OutOfRange`] when the range doesn't lie inside the
    /// region, before anything is read, and [`ReadError::Device`] when the
    /// flash fails a read; `buf` is zeroed on either.
    pub async fn read_async(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        read_chunked(self.len(), offset, buf, |at, chunk| {
            Source::read(self, at, chunk)
        })
        .await
    }
}

impl<R: Programmable<Device = Internal>> Session<'_, R> {
    /// Programs `bytes` at `offset` in the region, both the offset and the
    /// length multiples of [`WORD_SIZE`]: each byte becomes the AND of what
    /// it held and what is programmed. Programs at most 4 KiB per device call
    /// and yields after each.
    ///
    /// # Errors
    ///
    /// [`AccessError::NotAligned`] when the offset or the length isn't a
    /// multiple of the word size, [`AccessError::OutOfRange`] when the range
    /// doesn't lie inside the region, [`AccessError::Protected`] when it
    /// touches a protected span, all before anything is programmed, and
    /// [`AccessError::Device`] when the flash fails a program, after the
    /// bytes before it were programmed.
    pub async fn program(&mut self, offset: u32, bytes: &[u8]) -> Result<(), AccessError> {
        let len = u32::try_from(bytes.len()).map_err(|_| AccessError::OutOfRange)?;
        if !offset.is_multiple_of(WORD_SIZE) || !len.is_multiple_of(WORD_SIZE) {
            return Err(AccessError::NotAligned);
        }
        let to = offset.checked_add(len).ok_or(AccessError::OutOfRange)?;
        let (start, end) = self.range(offset, to).ok_or(AccessError::OutOfRange)?;
        if !start.is_multiple_of(WORD_SIZE) {
            return Err(AccessError::NotAligned);
        }
        self.guard(start, end)?;
        let mut address = start;
        for chunk in bytes.chunks(CHUNK_LEN as usize) {
            // At most CHUNK_LEN.
            let chunk_len = u32::try_from(chunk.len()).map_err(|_| AccessError::OutOfRange)?;
            let next = address
                .checked_add(chunk_len)
                .ok_or(AccessError::OutOfRange)?;
            self.program_chunk(address, chunk)?;
            address = next;
            Yield.pause().await;
        }
        Ok(())
    }
}

impl<R: Writable<Device = Internal>> Source for Session<'_, R> {
    fn size(&self) -> u32 {
        self.extent.span.len
    }

    /// Reads the region directly, in one device call.
    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        read_direct(self.extent.device, self.extent.span, offset, buf)
    }
}

impl<R: Writable<Device = Internal>> Drop for Session<'_, R> {
    /// Ends the session: at a new epoch when it issued a device call, else at
    /// the epoch it started from. The store is `SeqCst` and follows the
    /// session's last device call, a barrier, so no call of the session moves
    /// after it.
    fn drop(&mut self) {
        self.extent.state.end(self.extent.spans, self.written);
    }
}

impl<R: Writable<Device = Internal>> fmt::Debug for Session<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("role", &R::NAME)
            .field("extent", &self.extent)
            .field("running", &self.running)
            .field("written", &self.written)
            .finish()
    }
}
