//! Fixtures shared by the crate's tests: an [`Ota`] over a board in RAM, split
//! by the storage crate, with the content modules of its regions, and a loop
//! that runs a future beside the slots' checks.

extern crate alloc;

use alloc::boxed::Box;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use eveningstar_boot::{App, AppSlot, Otadata};
use eveningstar_esp_formats::pace::Yield;
use eveningstar_storage::chain::{BootWriters, Handles};
use eveningstar_storage::nor::{NorDevice, Reader, State, Writer};
use eveningstar_storage::roles::{
    AlternateApp, AlternateConfig, CurrentApp, Otadata as OtadataRole,
};
use eveningstar_storage::test_support::{TestFlash, run};
use eveningstar_storage::{Slot, SlotPartitions as _, Span, Tier, layout, split};

use crate::{ConfirmGate, Ota, SelfChecks};

/// No self-checks: a confirmation waits for none.
pub(crate) static NO_CHECKS: SelfChecks<(), 0> = SelfChecks::new([]);

/// The most polls [`drive`] makes before it gives up.
const MAX_POLLS: usize = 10_000_000;

/// Polls `background` and `foreground` in turn, with a waker that does
/// nothing, until `foreground` completes, and returns its output. Panics
/// when `background` completes first or after [`MAX_POLLS`] polls, which
/// stand for a deadlock.
fn drive<B: Future, F: Future>(background: B, foreground: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    let mut background = pin!(background);
    let mut foreground = pin!(foreground);
    for _ in 0..MAX_POLLS {
        if let Poll::Ready(output) = foreground.as_mut().poll(&mut cx) {
            return output;
        }
        assert!(
            background.as_mut().poll(&mut cx).is_pending(),
            "the background future completed"
        );
    }
    panic!("no progress after {MAX_POLLS} polls");
}

/// The checks of a board's slots, which the futures of [`Ota`] wait for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Checks<'f> {
    app: App<'f, 'f>,
}

impl<'f> Checks<'f> {
    /// Runs `future` to completion with the slots' checks
    /// ([`App::run`]) polled beside it.
    pub(crate) fn run<F: Future>(self, future: F) -> F::Output {
        let app = self.app;
        drive(
            app.run(sha2::Sha256::default(), &mut Yield, &mut || {}),
            future,
        )
    }

    /// Runs each slot's check until it keeps the result of the slot's epoch
    /// now.
    pub(crate) fn settle(self) {
        let sha = sha2::Sha256::default();
        if let Some(current) = self.app.current() {
            run(current.check(&sha, &mut Yield, &mut || {}));
        }
        if let Some(alternate) = self.app.alternate() {
            run(alternate.check(&sha, &mut Yield, &mut || {}));
        }
    }

    /// The alternate slot's module.
    pub(crate) fn alternate(self) -> &'f AppSlot<'f, AlternateApp> {
        self.app.alternate().expect("the layout's slots")
    }
}

/// `value`, leaked, so it lives as long as the flash.
fn leak<'f, T: 'f>(value: T) -> &'f T {
    Box::leak(Box::new(value))
}

/// [`Ota`] over `writers` and `config`, the alternate slot's configuration,
/// with the modules of `current`, `alternate` and `otadata` and the
/// self-checks of `gate`, made once the running slot's check has kept its
/// result, and the slots' checks.
pub(crate) fn ota_of<'f>(
    writers: BootWriters<'f>,
    config: Writer<'f, AlternateConfig>,
    current: Reader<'f, CurrentApp>,
    alternate: Reader<'f, AlternateApp>,
    otadata: Reader<'f, OtadataRole>,
    gate: &'f dyn ConfirmGate,
) -> (Ota<'f>, Checks<'f>) {
    let current = leak(AppSlot::new(current));
    let alternate = leak(AppSlot::new(alternate));
    let otadata = leak(Otadata::compiled(otadata));
    let checks = Checks {
        app: App::layout(current, alternate),
    };
    let ota = checks.run(Ota::new(writers, config, current, alternate, otadata, gate));
    (ota, checks)
}

/// [`Ota`] over the boot writers `split` hands out on `device`, booted from
/// `running`, which reaches the boot tier, with no self-checks, and the
/// slots' checks.
pub(crate) fn ota_over(device: &mut dyn NorDevice, running: Slot) -> (Ota<'_>, Checks<'_>) {
    ota_gated(device, running, &NO_CHECKS)
}

/// [`ota_over`] with the self-checks of `gate`.
pub(crate) fn ota_gated<'f>(
    device: &'f mut dyn NorDevice,
    running: Slot,
    gate: &'f dyn ConfirmGate,
) -> (Ota<'f>, Checks<'f>) {
    let state = Box::leak(Box::new(State::new()));
    let storage = run(split(device, state, running.partition().offset));
    assert_eq!(storage.report.tier, Tier::Boot, "{:?}", storage.report);
    let Handles::Boot(handles, writers) = storage.handles else {
        panic!("{:?}", storage.handles);
    };
    ota_of(
        writers,
        handles.alternate_config,
        handles.current_app,
        handles.alternate_app,
        handles.otadata,
        gate,
    )
}

/// [`Ota`] over writers of `flash` made without `split`, booted from
/// `ota_0`: of `alternate` as the alternate slot and `otadata` as the
/// otadata, with modules of the layout's regions.
pub(crate) fn ota_on_spans(
    flash: &TestFlash,
    alternate: Span,
    otadata: Span,
) -> (Ota<'static>, Checks<'static>) {
    ota_of(
        flash.boot_writers(alternate, otadata, Slot::Ota0),
        flash.writer(Span::from(layout::CONFIG_1), Slot::Ota0),
        flash.reader(Span::from(layout::OTA_0)),
        flash.reader(Span::from(layout::OTA_1)),
        flash.reader(Span::from(layout::OTADATA)),
        &NO_CHECKS,
    )
}
