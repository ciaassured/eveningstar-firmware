//! The image information block: any bytes, read as a block, never panic, and
//! a block that reads gives the facts its rules allow.

#![no_main]

use eveningstar_image_info::{Block, MAX_KEYS, NAME_LEN};
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
    assert!(block.keys().count() <= MAX_KEYS);
    let _ = block.signing();
    let _ = block.trusts_development_key();
});
