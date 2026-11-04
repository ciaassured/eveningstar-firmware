//! Checks a bootloader, a partition table and, optionally, an app image with
//! the code the firmware runs at boot, before they are flashed or released:
//!
//! - the table judged as the bootloader judges it, with an MD5 entry and no
//!   entry after it, and its partitions compared with the generated layout
//!   by label;
//! - the bootloader verified and looked up in the known list, printing the
//!   entry to add to `known.rs` when it isn't there, and its entry's table
//!   offset and rollback checked;
//! - the app verified as an ESP32-C6 image of the `eveningstar` project that
//!   fits `ota_0`, the layout record right after its descriptor equal to the
//!   generated layout's, its size measured against [`SLOT_BUDGET_PERCENT`],
//!   and the three of them, flashed as `flash-all` writes them, reaching the
//!   boot tier through the verification chain.
//!
//! ```text
//! check-images --bootloader FILE --partition-table FILE [--app FILE]
//! ```
//!
//! Exits with 1 when a check fails and 2 on a usage error.

use core::fmt::Display;
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use eveningstar_storage::chain::{Tier, split};
use eveningstar_storage::image;
use eveningstar_storage::nor::{NorDevice, State};
use eveningstar_storage::table::{self, Match};
use eveningstar_storage::{DeviceError, LAYOUT_RECORD, known, layout};

/// The share of an app slot an image may fill, in percent. The slots can't
/// grow once boards ship, as the layout can't change over OTA, so the image
/// has to leave room for the releases that come after it.
const SLOT_BUDGET_PERCENT: u64 = 75;

/// The project every app image has to name.
const PROJECT: &str = "eveningstar";

/// The ESP-IDF bootloaders `nix/bootloader` builds always enable app
/// rollback and read the table at the layout's offset, which the entry
/// printed for a missing one states.
const ROLLBACK: &str = "app rollback";

const USAGE: &str = "usage: check-images --bootloader FILE --partition-table FILE [--app FILE]";

struct Args {
    bootloader: PathBuf,
    table: PathBuf,
    app: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut bootloader = None;
    let mut table = None;
    let mut app = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let slot = match arg.to_str() {
            Some("--bootloader") => &mut bootloader,
            Some("--partition-table") => &mut table,
            Some("--app") => &mut app,
            _ => return Err(format!("unexpected argument {}", arg.display())),
        };
        let value = args
            .next()
            .ok_or_else(|| format!("{} needs a file", arg.display()))?;
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(format!("{} given twice", arg.display()));
        }
    }
    Ok(Args {
        bootloader: bootloader.ok_or("--bootloader is missing")?,
        table: table.ok_or("--partition-table is missing")?,
        app,
    })
}

/// Polls `future` to completion. The crate's futures only yield, waking
/// themselves, so polling again is all they need.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
    }
}

/// Counts the failed checks while printing each result.
#[derive(Default)]
struct Checks {
    passed: usize,
    failed: usize,
}

impl Checks {
    fn pass(&mut self, what: impl Display) {
        println!("  ok    {what}");
        self.passed += 1;
    }

    fn fail(&mut self, what: impl Display) {
        println!("  FAIL  {what}");
        self.failed += 1;
    }

    fn check(&mut self, ok: bool, what: impl Display) {
        if ok {
            self.pass(what);
        } else {
            self.fail(what);
        }
    }
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn check_table(checks: &mut Checks, path: &Path, bytes: &[u8]) {
    println!("partition table {}", path.display());
    // The chip size the bootloader checks the table against: the layout's
    // flash size, which the Nix bootloader's header names.
    let verified = match table::verify(bytes, 0, layout::FLASH_SIZE) {
        Ok(verified) => verified,
        Err(error) => {
            return checks.fail(format!(
                "isn't valid as the bootloader judges it: {error:?}"
            ));
        }
    };
    checks.pass(format!(
        "valid as the bootloader judges it: {} partitions, end entry at {}",
        verified.partitions(),
        verified.end
    ));
    checks.check(
        verified.md5.is_some(),
        "has an MD5 entry, whose digest is the MD5 of the entries before it",
    );
    checks.check(
        !verified.md5_read_as_partition(),
        "no entry lies between the MD5 entry and the end entry",
    );
    let partitions: Result<Vec<_>, _> = table::partitions(bytes, 0, &verified)
        .map(|partition| partition.map(|(_, entry)| entry))
        .collect();
    let partitions = match partitions {
        Ok(partitions) => partitions,
        Err(error) => return checks.fail(format!("its partitions don't read: {error:?}")),
    };
    let comparison = table::compare_labels(partitions, &layout::PARTITIONS);
    for (expected, result) in layout::PARTITIONS.iter().zip(comparison.expected) {
        let what = format!(
            "{:<10} {:#09x} {:#09x}",
            expected.label, expected.offset, expected.size
        );
        match result {
            Match::Matches => checks.pass(format!("{what}  matches")),
            Match::Differs(fields) => checks.fail(format!("{what}  differs: {fields:?}")),
            Match::Missing => checks.fail(format!("{what}  missing")),
        }
    }
    if comparison.extra > 0 {
        checks.fail(format!(
            "{} partitions with no label of the layout, or a label repeated",
            comparison.extra
        ));
    }
}

fn check_bootloader(checks: &mut Checks, path: &Path, bytes: &[u8]) {
    println!("bootloader {}", path.display());
    let bootloader = match block_on(image::verify_bootloader(bytes)) {
        Ok(bootloader) => bootloader,
        Err(error) => return checks.fail(format!("doesn't verify: {error:?}")),
    };
    let idf = bootloader
        .descriptor()
        .and_then(|descriptor| descriptor.idf_version().to_str().map(str::to_owned));
    checks.pass(format!(
        "verified: {} bytes, ESP-IDF {}, file SHA-256 {}",
        bootloader.size(),
        idf.as_deref().unwrap_or("unknown"),
        known::Hex(bootloader.file_sha256())
    ));
    let digest = bootloader.digest();
    checks.check(
        bootloader.size() <= layout::TABLE_OFFSET,
        format!("ends before the table at {:#x}", layout::TABLE_OFFSET),
    );
    if let Some(known) = known::find(digest) {
        checks.pass(format!(
            "known: {} ({}), reads the table at {:#x}, rollback {}",
            known.name,
            known::Hex(digest),
            known.table_offset,
            known.rollback
        ));
        checks.check(
            known.table_offset == layout::TABLE_OFFSET,
            format!(
                "its table offset is the layout's, {:#x}",
                layout::TABLE_OFFSET
            ),
        );
        checks.check(known.rollback, "it supports app rollback");
    } else {
        checks.fail(format!("unknown digest {}", known::Hex(digest)));
        let name = format!("ESP-IDF {}, {ROLLBACK}", idf.as_deref().unwrap_or("?"));
        println!(
            "        add it to BOOTLOADERS in crates/eveningstar-storage/src/known.rs:\n{}",
            known::Entry {
                digest,
                name: &name,
                table_offset: layout::TABLE_OFFSET,
                rollback: true,
            }
        );
    }
}

fn check_app(checks: &mut Checks, path: &Path, bytes: &[u8]) {
    println!("app {}", path.display());
    let app = match block_on(image::verify_app(bytes)) {
        Ok(app) => app,
        Err(error) => return checks.fail(format!("doesn't verify: {error:?}")),
    };
    checks.pass(format!(
        "verified for chip {:#06x}: digest {}, file SHA-256 {}",
        app.header().chip_id,
        known::Hex(app.digest()),
        known::Hex(app.file_sha256())
    ));
    match image::app_descriptor(bytes) {
        Ok(descriptor) => {
            let project = descriptor.project_name();
            let version = descriptor.version();
            checks.check(
                project.as_bytes() == PROJECT.as_bytes(),
                format!("project {project:?}, version {version:?}"),
            );
        }
        Err(error) => checks.fail(format!("no descriptor: {error:?}")),
    }
    check_layout_record(checks, bytes);
    let slot = u64::from(layout::OTA_0.size);
    let size = u64::from(app.size());
    let budget = slot * SLOT_BUDGET_PERCENT / 100;
    checks.check(
        size <= slot,
        format!("{size} bytes fit the {slot}-byte ota_0 slot"),
    );
    // Tenths of a percent, rounded down.
    let per_mille = size * 1000 / slot;
    checks.check(
        size <= budget,
        format!(
            "{size} bytes, {}.{}% of the slot, against a budget of {SLOT_BUDGET_PERCENT}% ({budget} bytes)",
            per_mille / 10,
            per_mille % 10
        ),
    );
}

/// The app's layout record, right after its descriptor, compared with the
/// generated layout in every byte.
fn check_layout_record(checks: &mut Checks, bytes: &[u8]) {
    let embedded = match image::app_layout(bytes) {
        Ok(Some(embedded)) => embedded,
        Ok(None) => {
            return checks.fail(format!(
                "no layout record at offset {:#x} of the first segment, right after the descriptor",
                image::LAYOUT_AT
            ));
        }
        Err(error) => return checks.fail(format!("no layout record: {error:?}")),
    };
    let what = format!(
        "layout record: table at {:#x}, flash of {:#x} bytes, {} entries",
        embedded.table_offset(),
        embedded.flash_size(),
        embedded.entry_count()
    );
    match embedded.matches(bytes, &LAYOUT_RECORD) {
        Ok(true) => checks.pass(format!("{what}, the generated layout's")),
        Ok(false) => {
            checks.fail(format!("{what}, not the generated layout's"));
            let expected = LAYOUT_RECORD.entries();
            for index in 0..embedded.entry_count().max(len_u32(expected.len())) {
                let found = embedded.entry(bytes, index).ok().flatten();
                let wanted = usize::try_from(index).ok().and_then(|i| expected.get(i));
                if found.as_ref() != wanted {
                    println!("        entry {index} differs");
                }
            }
        }
        Err(error) => checks.fail(format!("{what}, unreadable: {error:?}")),
    }
}

fn len_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// A board's flash held in memory, read only: `split` never writes.
struct Board(Vec<u8>);

impl NorDevice for Board {
    fn capacity(&self) -> u32 {
        u32::try_from(self.0.len()).unwrap_or(u32::MAX)
    }

    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), DeviceError> {
        let start = usize::try_from(address).map_err(|_| DeviceError)?;
        let bytes = start
            .checked_add(buf.len())
            .and_then(|end| self.0.get(start..end))
            .ok_or(DeviceError)?;
        buf.copy_from_slice(bytes);
        Ok(())
    }

    fn erase_sector(&self, _: u32) -> Result<(), DeviceError> {
        Err(DeviceError)
    }

    fn program(&self, _: u32, _: &[u8]) -> Result<(), DeviceError> {
        Err(DeviceError)
    }
}

/// Flashes the images into an erased flash as `flash-all` lays them out, with
/// blank otadata, and runs the verification chain on it as the app in
/// `ota_0`, which has to reach the boot tier.
fn check_tier(checks: &mut Checks, bootloader: &[u8], table: &[u8], app: &[u8]) {
    println!("a board flashed with these, booting ota_0");
    let mut flash = vec![0xFF; usize::try_from(layout::FLASH_SIZE).unwrap_or(0)];
    let parts = [
        (0, bootloader, layout::TABLE_OFFSET),
        (layout::TABLE_OFFSET, table, table::MAX_LEN),
        (layout::OTA_0.offset, app, layout::OTA_0.size),
    ];
    for (offset, bytes, room) in parts {
        let at = usize::try_from(offset).unwrap_or(usize::MAX);
        let fits = u32::try_from(bytes.len()).is_ok_and(|len| len <= room);
        match flash.get_mut(at..at.saturating_add(bytes.len())) {
            Some(target) if fits => target.copy_from_slice(bytes),
            _ => return checks.fail("the images don't fit their places"),
        }
    }
    let mut board = Board(flash);
    let mut state = State::new();
    let storage = block_on(split(&mut board, &mut state, layout::OTA_0.offset));
    let report = storage.report;
    match report.failed {
        None => checks.check(report.tier == Tier::Boot, "reaches the boot tier"),
        Some(step) => checks.fail(format!(
            "reaches the {:?} tier only, as the {step:?} step failed",
            report.tier
        )),
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("check-images: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let files = (|| {
        let bootloader = read(&args.bootloader)?;
        let table = read(&args.table)?;
        let app = args.app.as_deref().map(read).transpose()?;
        Ok::<_, String>((bootloader, table, app))
    })();
    let (bootloader, table, app) = match files {
        Ok(files) => files,
        Err(error) => {
            eprintln!("check-images: {error}");
            return ExitCode::from(2);
        }
    };

    let mut checks = Checks::default();
    check_table(&mut checks, &args.table, &table);
    check_bootloader(&mut checks, &args.bootloader, &bootloader);
    if let (Some(path), Some(app)) = (&args.app, &app) {
        check_app(&mut checks, path, app);
        check_tier(&mut checks, &bootloader, &table, app);
    }

    if checks.failed == 0 {
        println!("all {} checks passed", checks.passed);
        ExitCode::SUCCESS
    } else {
        println!(
            "{} of {} checks failed",
            checks.failed,
            checks.passed + checks.failed
        );
        ExitCode::FAILURE
    }
}
