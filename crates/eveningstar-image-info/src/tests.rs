use eveningstar_esp_formats::test_support::{
    APP, app_image_with_layout, board_layout_record, with_layout,
};
use eveningstar_records::Writer;
use eveningstar_records::registry;
use eveningstar_signify::{Purpose, TrustedKey};

use super::*;

/// A release key of generation `generation`, with made-up bytes.
const fn release(generation: u16, byte: u8) -> TrustedKey {
    TrustedKey::for_tests(
        Purpose::Firmware,
        Generation::Release(generation),
        [byte; 8],
        [byte; 32],
    )
}

/// What the fixture holds: a board, schemas 0 to 0, and a development
/// build that also trusts three release generations.
const KEYS: &[TrustedKey] = &[
    trust::DEVELOPMENT_KEY,
    release(1, 0x11),
    release(2, 0x22),
    release(3, 0x33),
];

fn sample() -> ImageInfo {
    ImageInfo {
        board: Board::new("eveningstar", &[2]).unwrap(),
        compatibility: Compatibility::new(0, 0).unwrap(),
        trust: Trust::new(
            &TrustSet::for_tests(Purpose::Firmware, KEYS),
            Generation::Development,
        )
        .unwrap(),
    }
}

/// The facts `bytes` hold, read through a [`Block`].
fn parse(bytes: &[u8]) -> Result<ImageInfo, Error> {
    let block = Block::parse(bytes)?;
    let mut keys = [NO_KEY; MAX_KEYS];
    let mut count = 0;
    for (slot, key) in keys.iter_mut().zip(block.keys()) {
        *slot = key;
        count += 1;
    }
    Ok(ImageInfo {
        board: block.board(),
        compatibility: block.compatibility(),
        trust: Trust {
            signing: block.signing(),
            keys,
            count,
        },
    })
}

/// The block of [`sample`], as this format first encoded it.
const FIXTURE: &[u8] = include_bytes!("../testdata/block-1.bin");

#[test]
fn the_block_round_trips() {
    let info = sample();
    let block = encode(&info);
    assert_eq!(block.len(), LEN);
    assert_eq!(parse(&block), Ok(info));
    assert_eq!(info.board.name(), "eveningstar");
    assert!(info.board.supports(2));
    assert!(!info.board.supports(1));
    assert_eq!(info.trust.keys().len(), 4);
    assert!(info.trust.trusts_development_key());
    assert_eq!(info.trust.keys()[3].generation(), Generation::Release(3));
}

#[test]
fn the_released_fixture_reads_and_is_what_encode_writes() {
    assert_eq!(parse(FIXTURE), Ok(sample()));
    assert_eq!(&encode(&sample())[..], FIXTURE);
}

/// A newer image's block: an unknown type first, and every value longer
/// than this reader knows, in another order.
fn future() -> Vec<u8> {
    let block = encode(&sample());
    let container = Container::parse(&block, MAGIC).unwrap();
    let mut buf = [0; MAX_LEN];
    let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
    writer.push(0x7777, &[0xAB; 12]).unwrap();
    for kind in [TRUST, BOARD, COMPATIBILITY] {
        let mut longer = container.require(kind).unwrap().value.to_vec();
        longer.extend_from_slice(&[0xCD; 8]);
        writer.push(kind, &longer).unwrap();
    }
    writer.finish().to_vec()
}

/// [`future`], committed, so a change of the encoder can't change it.
const FUTURE: &[u8] = include_bytes!("../testdata/block-future.bin");

#[test]
fn a_future_block_reads_as_the_same_facts() {
    assert_eq!(future(), FUTURE);
    assert_eq!(parse(FUTURE), Ok(sample()));
}

#[test]
fn a_block_needs_each_type_once() {
    let block = encode(&sample());
    let container = Container::parse(&block, MAGIC).unwrap();
    for missing in [BOARD, COMPATIBILITY, TRUST] {
        let mut buf = [0; MAX_LEN];
        let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
        for kind in [BOARD, COMPATIBILITY, TRUST]
            .into_iter()
            .filter(|kind| *kind != missing)
        {
            writer
                .push(kind, container.require(kind).unwrap().value)
                .unwrap();
        }
        assert!(matches!(parse(writer.finish()), Err(Error::Records(_))));
    }
    let mut buf = [0; MAX_LEN];
    let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
    for kind in [BOARD, COMPATIBILITY, TRUST, BOARD] {
        writer
            .push(kind, container.require(kind).unwrap().value)
            .unwrap();
    }
    assert!(matches!(parse(writer.finish()), Err(Error::Records(_))));
}

/// `block` with the value of `kind` replaced by what `change` makes of it.
fn with_value(kind: u16, change: impl Fn(&mut Vec<u8>)) -> Vec<u8> {
    let block = encode(&sample());
    let container = Container::parse(&block, MAGIC).unwrap();
    let mut buf = [0; MAX_LEN];
    let mut writer = Writer::new(&mut buf, MAGIC).unwrap();
    for entry in container.entries() {
        let entry = entry.unwrap();
        let mut value = entry.value.to_vec();
        if entry.kind == kind {
            change(&mut value);
        }
        writer.push(entry.kind, &value).unwrap();
    }
    writer.finish().to_vec()
}

#[test]
fn values_outside_their_rules_are_refused() {
    let short = with_value(TRUST, |value| value.truncate(TRUST_LEN - 4));
    assert_eq!(parse(&short), Err(Error::TooShort { kind: TRUST }));
    let upper = with_value(BOARD, |value| value[0] = b'E');
    assert_eq!(parse(&upper), Err(Error::Board));
    let gap = with_value(BOARD, |value| value[0] = 0);
    assert_eq!(parse(&gap), Err(Error::Board));
    let no_revision = with_value(BOARD, |value| value[NAME_LEN..BOARD_LEN].fill(0));
    assert_eq!(parse(&no_revision), Err(Error::Board));
    let backwards = with_value(COMPATIBILITY, |value| value[0] = 1);
    assert_eq!(parse(&backwards), Err(Error::Compatibility));
    let big = with_value(TRUST, |value| {
        value[8..12].copy_from_slice(&0x1_0000_u32.to_le_bytes());
    });
    assert_eq!(parse(&big), Err(Error::Trust));
}

#[test]
fn slots_appended_after_the_fourth_are_read() {
    // A later image with a fifth slot, appended after the ones `encode`
    // writes: the development key, under generation 0.
    let five = with_value(TRUST, |value| {
        value[4] = 5;
        value.extend_from_slice(&0_u32.to_le_bytes());
        value.extend_from_slice(trust::DEVELOPMENT_KEY.keynum());
        value.extend_from_slice(trust::DEVELOPMENT_KEY.public_key());
    });
    let block = Block::parse(&five).unwrap();
    assert_eq!(block.keys().count(), 5);
    assert!(block.keys().nth(4).unwrap().is_development());
    // A count past the slots the value holds, and a count of 0.
    let past = with_value(TRUST, |value| value[4] = 5);
    assert_eq!(parse(&past), Err(Error::Trust));
    let none = with_value(TRUST, |value| value[4] = 0);
    assert_eq!(parse(&none), Err(Error::Trust));
}

#[test]
fn the_development_key_is_known_by_its_bytes() {
    // The development key's bytes under generation 7, and a release key's
    // under generation 0.
    let relabelled = with_value(TRUST, |value| {
        value[8] = 7;
        value[8 + KEY_LEN] = 0;
    });
    let block = Block::parse(&relabelled).unwrap();
    let keys: Vec<Key> = block.keys().collect();
    assert_eq!(keys[0].generation(), Generation::Release(7));
    assert!(keys[0].is_development());
    assert_eq!(keys[1].generation(), Generation::Development);
    assert!(!keys[1].is_development());
    assert!(block.trusts_development_key());
    // Without the development key's bytes, no key is the development key.
    let without = with_value(TRUST, |value| value[20..52].fill(0x99));
    assert!(!Block::parse(&without).unwrap().trusts_development_key());
}

#[test]
fn slots_past_the_count_are_ignored() {
    let fewer = with_value(TRUST, |value| {
        value[4] = 1;
        value[8 + KEY_LEN..].fill(0xEE);
    });
    let info = parse(&fewer).unwrap();
    assert_eq!(info.trust.keys().len(), 1);
    assert!(info.trust.trusts_development_key());
}

#[test]
fn every_truncation_and_bit_flip_reads_without_a_panic() {
    let block = encode(&sample());
    for len in 0..block.len() {
        assert!(parse(&block[..len]).is_err());
    }
    for bit in 0..block.len() * 8 {
        let mut flipped = block;
        flipped[bit / 8] ^= 1 << (bit % 8);
        let _ = parse(&flipped);
    }
}

#[test]
fn constructors_refuse_what_the_block_cant_hold() {
    assert_eq!(Board::new("", &[2]), None);
    assert_eq!(Board::new("seventeen-bytes-a", &[2]), None);
    assert_eq!(Board::new("Eveningstar", &[2]), None);
    assert_eq!(Board::new("eveningstar", &[]), None);
    assert_eq!(Board::new("eveningstar", &[32]), None);
    assert_eq!(Compatibility::new(1, 0), None);
    let five = [KEYS[0], KEYS[1], KEYS[2], KEYS[3], release(4, 0x44)];
    let five: &'static [TrustedKey] = Box::leak(Box::new(five));
    let set = TrustSet::for_tests(Purpose::Firmware, five);
    assert_eq!(Trust::new(&set, Generation::Development), None);
    let set = TrustSet::for_tests(Purpose::Firmware, KEYS);
    assert_eq!(Trust::new(&set, Generation::Release(0)), None);
    let zero: &'static [TrustedKey] = Box::leak(Box::new([release(0, 0x55)]));
    let set = TrustSet::for_tests(Purpose::Firmware, zero);
    assert_eq!(Trust::new(&set, Generation::Development), None);
}

#[test]
fn the_types_keep_the_released_registry() {
    registry::validate(TYPES).unwrap();
    registry::check(include_str!("../registry.txt"), TYPES).unwrap();
}

#[test]
fn an_image_s_block_is_read_after_its_layout_record() {
    let record = board_layout_record();
    let mut buf = [0; MAX_LEN];
    let block = encode(&sample());
    let image = with_layout(APP, &[&record[..], &block[..]].concat());
    assert_eq!(read(&image[..], &mut buf), Ok(Found::Block(&block[..])));
    assert_eq!(parse(&block), Ok(sample()));

    // An image with a record and no block after it, and one with no record.
    let image = with_layout(APP, &record);
    assert_eq!(read(&image[..], &mut buf), Ok(Found::Absent));
    assert_eq!(read(APP, &mut buf), Ok(Found::Absent));

    // A header longer than the room a reader allows.
    let mut long = block;
    long[4..6].copy_from_slice(&u16::try_from(MAX_LEN + 4).unwrap().to_le_bytes());
    let image = with_layout(APP, &[&record[..], &long[..]].concat());
    assert_eq!(read(&image[..], &mut buf), Ok(Found::BadLength));

    // A header whose length is shorter than the header itself.
    let mut short = block;
    short[4..6].copy_from_slice(&3_u16.to_le_bytes());
    let image = with_layout(APP, &[&record[..], &short[..]].concat());
    assert_eq!(read(&image[..], &mut buf), Ok(Found::BadLength));
}

#[test]
fn a_block_is_read_only_inside_the_first_segment() {
    // A first segment that ends 100 bytes after the layout record.
    let record = board_layout_record();
    let at = 256 + record.len();
    let header = |len: u16| {
        let mut builder = app_image_with_layout();
        let first = &mut builder.segments[0].1;
        first.truncate(at + 100);
        first[at..at + 4].copy_from_slice(&MAGIC.to_le_bytes());
        first[at + 4..at + 6].copy_from_slice(&len.to_le_bytes());
        builder.build()
    };
    let mut buf = [0; MAX_LEN];
    assert_eq!(read(&header(101)[..], &mut buf), Ok(Found::BadLength));
    let image = header(100);
    let Ok(Found::Block(block)) = read(&image[..], &mut buf) else {
        panic!("a block that fills the rest of the segment is read");
    };
    assert_eq!(block.len(), 100);
}
