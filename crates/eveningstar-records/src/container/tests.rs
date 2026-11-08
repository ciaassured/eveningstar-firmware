use crate::{Container, Error, HEADER_LEN, Value, Writer};

const MAGIC: u32 = 0x4F46_4E49;

/// A type released with an 8-byte value: a `u32` and a `u32`.
const OLD: u16 = 1;
const OLD_MIN: usize = 8;

/// A type a newer firmware added.
const NEWER: u16 = 7;

/// A container with one `OLD` entry, as the type's first release writes it.
fn released(buf: &mut [u8]) -> &[u8] {
    let mut writer = Writer::new(buf, MAGIC).unwrap();
    let mut value = [0; OLD_MIN];
    value[..4].copy_from_slice(&0x1122_3344u32.to_le_bytes());
    value[4..].copy_from_slice(&5u32.to_le_bytes());
    writer.push(OLD, &value).unwrap();
    writer.finish()
}

/// The same `OLD` entry as a later release writes it, with an extra field,
/// after a type this firmware doesn't know.
fn future(buf: &mut [u8]) -> &[u8] {
    let mut writer = Writer::new(buf, MAGIC).unwrap();
    writer.push(NEWER, b"something new").unwrap();
    let mut value = [0; OLD_MIN + 4];
    value[..4].copy_from_slice(&0x1122_3344u32.to_le_bytes());
    value[4..8].copy_from_slice(&5u32.to_le_bytes());
    value[8..].copy_from_slice(&9u32.to_le_bytes());
    writer.push(OLD, &value).unwrap();
    writer.finish()
}

/// What a reader that knows `OLD`'s first release reads.
fn read_old(bytes: &[u8]) -> Result<(u32, u32, Option<u32>), Error> {
    let container = Container::parse(bytes, MAGIC)?;
    let entry = container.require(OLD)?;
    let value = Value::new(entry.value, OLD_MIN).unwrap();
    Ok((value.u32(0).unwrap(), value.u32(4).unwrap(), value.u32(8)))
}

#[test]
fn a_written_container_reads_back() {
    let mut buf = [0xAA; 64];
    let bytes = released(&mut buf);
    assert_eq!(bytes.len(), HEADER_LEN + 4 + OLD_MIN);
    let container = Container::parse(bytes, MAGIC).unwrap();
    assert_eq!(container.magic(), MAGIC);
    assert_eq!(container.total_len(), bytes.len());
    assert_eq!(read_old(bytes), Ok((0x1122_3344, 5, None)));
}

#[test]
fn values_are_padded_to_four_bytes_with_zeros() {
    let mut buf = [0xAA; 64];
    let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
    writer.push(3, b"abcde").unwrap();
    let bytes = writer.finish();
    let entry = Container::parse(bytes, MAGIC).unwrap().require(3).unwrap();
    assert_eq!(entry.value, b"abcde\0\0\0");
}

#[test]
fn an_older_reader_reads_a_newer_container_as_without_the_additions() {
    let mut old = [0; 64];
    let mut new = [0; 64];
    let (old, new) = (released(&mut old), future(&mut new));
    let (a, b, extra) = read_old(new).unwrap();
    assert_eq!((a, b), (0x1122_3344, 5));
    assert_eq!(extra, Some(9));
    let (a, b, _) = read_old(old).unwrap();
    assert_eq!((a, b), (0x1122_3344, 5));
}

#[test]
fn a_value_shorter_than_its_minimum_is_refused() {
    assert!(Value::new(&[0; 7], OLD_MIN).is_err());
    let value = Value::new(&[0; 8], OLD_MIN).unwrap();
    assert_eq!(value.u32(8), None);
    assert_eq!(value.u64(1), None);
    assert_eq!(value.slice(4, 4), Some(&[0u8; 4][..]));
    assert_eq!(value.u8(8), None);
}

#[test]
fn the_magic_and_the_length_are_checked() {
    let mut buf = [0; 64];
    let bytes = released(&mut buf).to_vec();
    assert_eq!(
        Container::parse(&bytes, MAGIC + 1),
        Err(Error::Magic { found: MAGIC })
    );
    assert_eq!(Container::parse(&bytes[..5], MAGIC), Err(Error::Truncated));
    assert_eq!(
        Container::parse(&bytes[..bytes.len() - 1], MAGIC),
        Err(Error::Length {
            total: u16::try_from(bytes.len()).unwrap()
        })
    );
    let mut short = bytes.clone();
    short[4..6].copy_from_slice(&5u16.to_le_bytes());
    assert_eq!(
        Container::parse(&short, MAGIC),
        Err(Error::Length { total: 5 })
    );
    // Bytes past the total length are ignored.
    let mut longer = bytes.clone();
    longer.extend_from_slice(&[0xFF; 9]);
    assert_eq!(read_old(&longer), read_old(&bytes));
}

#[test]
fn a_bad_entry_ends_the_entries_with_its_error() {
    let mut buf = [0; 64];
    let mut bytes = released(&mut buf).to_vec();
    // Length 6: not a multiple of 4.
    bytes[HEADER_LEN + 2..HEADER_LEN + 4].copy_from_slice(&6u16.to_le_bytes());
    let container = Container::parse(&bytes, MAGIC).unwrap();
    let mut entries = container.entries();
    assert_eq!(
        entries.next(),
        Some(Err(Error::EntryPadding { at: HEADER_LEN }))
    );
    assert_eq!(entries.next(), None);
    // Length 12: past the total length.
    bytes[HEADER_LEN + 2..HEADER_LEN + 4].copy_from_slice(&12u16.to_le_bytes());
    let container = Container::parse(&bytes, MAGIC).unwrap();
    assert_eq!(
        container.find(OLD),
        Err(Error::EntryTruncated { at: HEADER_LEN })
    );
}

#[test]
fn a_type_found_twice_or_missing_is_refused() {
    let mut buf = [0; 64];
    let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
    writer.push(2, &[1; 4]).unwrap();
    writer.push(2, &[2; 4]).unwrap();
    let bytes = writer.finish();
    let container = Container::parse(bytes, MAGIC).unwrap();
    assert_eq!(container.find(2), Err(Error::Duplicate { kind: 2 }));
    assert_eq!(container.require(5), Err(Error::Missing { kind: 5 }));
    assert_eq!(container.find(5), Ok(None));
    assert_eq!(container.entries().count(), 2);
}

#[test]
fn the_writer_refuses_what_doesnt_fit_and_writes_nothing() {
    let mut buf = [0xAA; 16];
    assert!(Writer::new(&mut buf[..5], MAGIC).is_err());
    let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
    writer.push(1, &[1; 4]).unwrap();
    assert!(writer.push(1, &[1; 4]).is_err());
    let bytes = writer.finish();
    assert_eq!(bytes.len(), HEADER_LEN + 8);
    assert_eq!(Container::parse(bytes, MAGIC).unwrap().entries().count(), 1);
}

#[test]
fn the_writer_keeps_the_total_length_within_a_u16() {
    let mut buf = vec![0; 70_000];
    let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
    let value = vec![0; 60_000];
    writer.push(1, &value).unwrap();
    assert!(writer.push(2, &value[..8_000]).is_err());
    let bytes = writer.finish();
    assert!(Container::parse(bytes, MAGIC).is_ok());
}

#[test]
fn every_truncation_and_bit_flip_reads_without_panicking() {
    let mut buf = [0; 64];
    let bytes = future(&mut buf).to_vec();
    for len in 0..=bytes.len() {
        read_all(&bytes[..len]);
    }
    for bit in 0..bytes.len() * 8 {
        let mut flipped = bytes.clone();
        flipped[bit / 8] ^= 1 << (bit % 8);
        read_all(&flipped);
    }
}

/// Reads every entry of `bytes` as each accessor allows.
fn read_all(bytes: &[u8]) {
    let Ok(container) = Container::parse(bytes, MAGIC) else {
        return;
    };
    for entry in container.entries().flatten() {
        if let Ok(value) = Value::new(entry.value, 4) {
            let _ = (value.u8(3), value.u16(2), value.u32(0), value.u64(4));
        }
    }
    let _ = container.find(OLD);
}

#[test]
fn an_empty_container_and_an_empty_value_read() {
    let mut buf = [0; 16];
    let bytes = Writer::new(&mut buf, MAGIC).unwrap().finish().to_vec();
    let container = Container::parse(&bytes, MAGIC).unwrap();
    assert_eq!(container.total_len(), HEADER_LEN);
    assert_eq!(container.entries().next(), None);
    assert_eq!(container.require(OLD), Err(Error::Missing { kind: OLD }));
    let mut buf = [0; 16];
    let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
    writer.push(OLD, &[]).unwrap();
    let bytes = writer.finish().to_vec();
    let container = Container::parse(&bytes, MAGIC).unwrap();
    let entry = container.require(OLD).unwrap();
    assert_eq!(entry.value, &[] as &[u8]);
}

#[test]
fn entry_bytes_that_arent_a_multiple_of_four_are_refused() {
    let mut buf = [0; 64];
    let mut bytes = released(&mut buf).to_vec();
    for extra in 1..4u16 {
        let total = u16::try_from(bytes.len()).unwrap() + extra;
        bytes.push(0);
        bytes[4..6].copy_from_slice(&total.to_le_bytes());
        assert_eq!(
            Container::parse(&bytes, MAGIC),
            Err(Error::Length { total })
        );
    }
}
