//! The generated trust sets: each key is the one its committed `.pub` file
//! holds, passes `check_key`, and has a keynum of its own.

use eveningstar_signify::trust::{DEVELOPMENT, DEVELOPMENT_KEY};
use eveningstar_signify::{Generation, Purpose, check_key, public_key};

/// The committed public key file of the key of `generation`.
fn pub_file(generation: Generation) -> Vec<u8> {
    let name = match generation {
        Generation::Development => "eveningstar-dev-fw.pub".to_owned(),
        Generation::Release(n) => format!("eveningstar-{n}-fw.pub"),
    };
    let path = format!("{}/../../keys/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

#[test]
fn the_development_set_holds_the_development_key() {
    assert_eq!(DEVELOPMENT_KEY.generation(), Generation::Development);
    assert!(DEVELOPMENT.keys().contains(&DEVELOPMENT_KEY));
    assert!(DEVELOPMENT.trusts_development_key());
}

// A release build trusts a subset of these keys: the generator puts every
// release key in the development set too.
#[test]
fn every_trusted_key_is_its_committed_file_and_may_be_trusted() {
    assert_eq!(DEVELOPMENT.purpose(), Purpose::Firmware);
    for (index, key) in DEVELOPMENT.keys().iter().enumerate() {
        let (keynum, public) = public_key(&pub_file(key.generation())).unwrap();
        assert_eq!(key.keynum(), &keynum);
        assert_eq!(key.public_key(), &public);
        assert_eq!(key.purpose(), Purpose::Firmware);
        assert_eq!(check_key(key.public_key()), Ok(()));
        assert!(
            DEVELOPMENT.keys()[index + 1..]
                .iter()
                .all(|other| other.keynum() != key.keynum())
        );
    }
}
