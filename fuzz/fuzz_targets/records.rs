//! The record encoding: any bytes, read as a container, its entries and their
//! fields, never panic; and any entries written read back as written.

#![no_main]

use eveningstar_records::{Container, Error, Value, Writer, streamed};
use libfuzzer_sys::fuzz_target;

const MAGIC: u32 = 0x4F46_4E49;

fuzz_target!(|data: &[u8]| {
    read(data);
    round_trip(data);
    streamed_agrees(data);
});

/// Finding each type with the container read through a callback gives what
/// the held container gives.
fn streamed_agrees(data: &[u8]) {
    for kind in 0..8 {
        let read = |at: usize, buf: &mut [u8]| -> Result<(), ()> {
            buf.copy_from_slice(data.get(at..at + buf.len()).ok_or(())?);
            Ok(())
        };
        let found =
            streamed::find(data.len(), MAGIC, kind, read).expect("reads stay inside the limit");
        let held = Container::parse(data, MAGIC).and_then(|container| container.find(kind));
        let held = held.map(|entry| {
            entry.map(|entry| streamed::Place {
                offset: entry.value.as_ptr() as usize - data.as_ptr() as usize,
                len: entry.value.len(),
            })
        });
        assert_eq!(found, held);
    }
}

/// Reads `data` as a container, as every accessor allows.
fn read(data: &[u8]) {
    let Ok(container) = Container::parse(data, MAGIC) else {
        return;
    };
    assert!(container.total_len() <= data.len());
    for (index, entry) in container.entries().enumerate() {
        let Ok(entry) = entry else { break };
        assert_eq!(entry.value.len() % 4, 0);
        if let Ok(value) = Value::new(entry.value, 4) {
            fields_read_as_bytes(&value, entry.value);
        }
        // The first entry of its type, a later one of its type, or a later
        // entry that doesn't read.
        match container.find(entry.kind) {
            Ok(Some(found)) => {
                assert_eq!(found.kind, entry.kind);
                assert!(
                    container
                        .entries()
                        .take(index + 1)
                        .flatten()
                        .any(|e| e == found)
                );
            }
            Err(Error::Duplicate { kind }) => assert_eq!(kind, entry.kind),
            Err(_) => {}
            Ok(None) => panic!("an entry read but its type isn't found"),
        }
    }
}

/// Checks that `value`'s accessors give the little-endian bytes of `bytes`.
fn fields_read_as_bytes(value: &Value<'_>, bytes: &[u8]) {
    for offset in 0..bytes.len() + 1 {
        let at = |n: usize| bytes.get(offset..offset + n);
        assert_eq!(value.slice(offset, 3), at(3));
        assert_eq!(value.u8(offset), at(1).map(|b| b[0]));
        assert_eq!(
            value.u16(offset),
            at(2).map(|b| u16::from_le_bytes(b.try_into().unwrap()))
        );
        assert_eq!(
            value.u32(offset),
            at(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        );
        assert_eq!(
            value.u64(offset),
            at(8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        );
    }
}

/// Splits `data` into entries (a type byte, a length byte, that many value
/// bytes), writes them, and checks they read back in order.
fn round_trip(data: &[u8]) {
    let mut buf = [0u8; 4096];
    let Ok(mut writer) = Writer::new(&mut buf, MAGIC) else {
        return;
    };
    let mut written = Vec::new();
    let mut rest = data;
    while let [kind, len, tail @ ..] = rest {
        let len = usize::from(*len).min(tail.len());
        let (value, tail) = tail.split_at(len);
        if writer.push(u16::from(*kind), value).is_err() {
            break;
        }
        written.push((u16::from(*kind), value));
        rest = tail;
    }
    let bytes = writer.finish();
    let container = Container::parse(bytes, MAGIC).expect("a written container reads");
    let read: Vec<_> = container
        .entries()
        .map(|entry| entry.expect("a written entry reads"))
        .collect();
    assert_eq!(read.len(), written.len());
    for (entry, (kind, value)) in read.iter().zip(&written) {
        assert_eq!(entry.kind, *kind);
        assert_eq!(&entry.value[..value.len()], *value);
        assert!(entry.value[value.len()..].iter().all(|byte| *byte == 0));
    }
}
