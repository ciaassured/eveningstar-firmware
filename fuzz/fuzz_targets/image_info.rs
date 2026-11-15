//! The image information block: any bytes, read as a block, never panic, and
//! a block that reads gives the facts its rules allow.

#![no_main]

use eveningstar_image_info::{Block, MAX_LEN, NAME_LEN};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(block) = Block::parse(data) else {
        return;
    };
    let board = block.board();
    assert!(!board.name().is_empty() && board.name().len() <= NAME_LEN);
    assert_ne!(board.revisions(), 0);
    let compatibility = block.compatibility();
    assert!(compatibility.oldest_schema() <= compatibility.schema());
    // Each key is a 44-byte slot inside the block.
    let keys = block.keys().count();
    assert!(keys >= 1 && keys * 44 <= MAX_LEN);
    let _ = block.signing();
    assert_eq!(
        block.trusts_development_key(),
        block.keys().any(|key| key.is_development())
    );
});
