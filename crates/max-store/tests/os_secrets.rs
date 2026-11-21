//! The real OS credential store (Credential Manager on Windows, Secret Service on Linux).
//! Ignored by default because it writes to the user's store; run with
//! `cargo test -p max-store --test os_secrets -- --ignored`. It deletes what it writes.

#![allow(clippy::unwrap_used)]

use max_store::{OsSecrets, SecretStore};

#[test]
#[ignore = "writes to the OS credential store"]
fn os_secrets_roundtrip_and_cleanup() {
    let store = OsSecrets::new("maxly-gpui-test");
    let name = format!("probe-{}", std::process::id());
    assert_eq!(store.get(&name).unwrap(), None);
    store.set(&name, &[1, 2, 3, 0, 255]).unwrap();
    assert_eq!(store.get(&name).unwrap(), Some(vec![1, 2, 3, 0, 255]));
    store.delete(&name).unwrap();
    assert_eq!(store.get(&name).unwrap(), None);
    store.delete(&name).unwrap();
}
