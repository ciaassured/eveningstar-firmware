//! The generated trust sets: each key is the one its committed `.pub` file
//! holds, passes `check_key`, and has a keynum of its own.

use eveningstar_signify::trust::{DEVELOPMENT, DEVELOPMENT_KEY};
use eveningstar_signify::{Generation, Purpose, check_key, public_key};

#[test]
fn the_development_key_is_the_committed_one() {
    let file = include_bytes!("../../../keys/eveningstar-dev-fw.pub");
    let (keynum, key) = public_key(file).unwrap();
    assert_eq!(DEVELOPMENT_KEY.keynum(), &keynum);
    assert_eq!(DEVELOPMENT_KEY.public_key(), &key);
    assert_eq!(DEVELOPMENT_KEY.generation(), Generation::Development);
    assert!(DEVELOPMENT.trusts_development_key());
}

#[test]
fn every_trusted_key_may_be_trusted() {
    assert_eq!(DEVELOPMENT.purpose(), Purpose::Firmware);
    for (index, key) in DEVELOPMENT.keys().iter().enumerate() {
        assert_eq!(key.purpose(), Purpose::Firmware);
        assert_eq!(check_key(key.public_key()), Ok(()));
        assert!(
            DEVELOPMENT.keys()[index + 1..]
                .iter()
                .all(|other| other.keynum() != key.keynum())
        );
    }
}
