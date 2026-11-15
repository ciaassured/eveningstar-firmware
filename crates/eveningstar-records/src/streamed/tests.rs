#![allow(
    clippy::cast_possible_truncation,
    reason = "the generator's numbers are cut to the small ranges the cases use"
)]

use super::*;
use crate::{Container, Writer};

/// [`find`] over `bytes`, read from memory.
fn streamed(bytes: &[u8], magic: u32, kind: u16) -> Result<Option<Place>, Error> {
    let read = |at: usize, buf: &mut [u8]| -> Result<(), ()> {
        buf.copy_from_slice(bytes.get(at..at + buf.len()).ok_or(())?);
        Ok(())
    };
    find(bytes.len(), magic, kind, read).expect("reads stay inside the limit")
}

/// [`find`] over the first `limit` bytes of `bytes`, read from memory.
fn streamed_within(
    bytes: &[u8],
    limit: usize,
    magic: u32,
    kind: u16,
) -> Result<Option<Place>, Error> {
    let read = |at: usize, buf: &mut [u8]| -> Result<(), ()> {
        assert!(at + buf.len() <= limit, "a read past the limit");
        buf.copy_from_slice(bytes.get(at..at + buf.len()).ok_or(())?);
        Ok(())
    };
    find(limit, magic, kind, read).expect("reads stay inside the limit")
}

/// What the held container gives for `bytes`, as a place in it.
fn held(bytes: &[u8], magic: u32, kind: u16) -> Result<Option<Place>, Error> {
    let container = Container::parse(bytes, magic)?;
    let entry = container.find(kind)?;
    Ok(entry.map(|entry| Place {
        offset: entry.value.as_ptr() as usize - bytes.as_ptr() as usize,
        len: entry.value.len(),
    }))
}

const MAGIC: u32 = 0x5453_4554;

/// A small pseudo-random generator, for the cases.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Containers of a few entries of types 1 to 4, of lengths 0 to 12.
fn cases() -> Vec<Vec<u8>> {
    let mut rng = Rng(0x5EED);
    let mut cases = Vec::new();
    for _ in 0..500 {
        let mut buf = [0; 128];
        let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
        for _ in 0..rng.next() % 5 {
            let kind = (rng.next() % 4 + 1) as u16;
            let len = (rng.next() % 13) as usize;
            writer.push(kind, &vec![0xA5; len]).unwrap();
        }
        cases.push(writer.finish().to_vec());
    }
    cases
}

#[test]
fn streaming_gives_the_held_container_s_verdict() {
    let mut rng = Rng(0xF11B);
    for case in cases() {
        // Each case as written, with one byte changed, cut short, and with
        // bytes after it.
        let mut variants = vec![case.clone()];
        for _ in 0..8 {
            let mut changed = case.clone();
            let at = (rng.next() as usize) % changed.len();
            changed[at] = rng.next() as u8;
            variants.push(changed);
        }
        variants.push(case[..(rng.next() as usize) % case.len()].to_vec());
        let mut longer = case.clone();
        longer.extend_from_slice(&[0x5A; 12]);
        variants.push(longer);
        for bytes in variants {
            for kind in 0..6 {
                assert_eq!(
                    streamed(&bytes, MAGIC, kind),
                    held(&bytes, MAGIC, kind),
                    "{bytes:02x?} {kind}"
                );
            }
        }
    }
}

#[test]
fn a_limit_gives_the_verdict_of_the_bytes_before_it() {
    let mut rng = Rng(0x11A1);
    for case in cases() {
        let mut longer = case.clone();
        longer.extend_from_slice(&[0x5A; 12]);
        for _ in 0..4 {
            let limit = (rng.next() as usize) % (longer.len() + 1);
            for kind in 0..6 {
                assert_eq!(
                    streamed_within(&longer, limit, MAGIC, kind),
                    held(&longer[..limit], MAGIC, kind),
                    "{longer:02x?} {limit} {kind}"
                );
            }
        }
    }
}

#[test]
fn streaming_reads_only_inside_the_limit() {
    let mut buf = [0; 64];
    let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
    writer.push(1, &[1; 8]).unwrap();
    let bytes = writer.finish().to_vec();
    // A limit shorter than the header reads nothing; one shorter than the
    // total refuses the length.
    let none = |_: usize, _: &mut [u8]| -> Result<(), ()> { panic!("read") };
    assert_eq!(find(5, MAGIC, 1, none), Ok(Err(Error::Truncated)));
    let reads = |at: usize, buf: &mut [u8]| -> Result<(), ()> {
        assert!(at + buf.len() <= 6, "a read past the header");
        buf.copy_from_slice(&bytes[at..at + buf.len()]);
        Ok(())
    };
    assert_eq!(
        find(bytes.len() - 1, MAGIC, 1, reads),
        Ok(Err(Error::Length {
            total: bytes.len() as u16
        }))
    );
}
