//! License integration: real ed25519 sign → verify round-trip, plus
//! disk persistence sanity-check.
//!
//! Uses [`license::verify_license_key_with`] (test hook) so we can sign
//! a real key and verify against a freshly generated public key without
//! depending on the placeholder public key embedded in the binary.

use base64::Engine;
use chronikl::license::{self, ExpiryStatus, LicenseClaims, LicenseError, verify_license_key_with};
use ed25519_dalek::{Signer, SigningKey};
use rand_core::OsRng;

fn generate_keypair() -> (SigningKey, [u8; 32]) {
    let signing_key = SigningKey::generate(&mut OsRng);
    let public = signing_key.verifying_key().to_bytes();
    (signing_key, public)
}

fn sign_license(signing_key: &SigningKey, claims: &LicenseClaims) -> String {
    let payload = serde_json::to_vec(claims).unwrap();
    let signature = signing_key.sign(&payload);
    let mut blob = payload;
    blob.extend_from_slice(&signature.to_bytes());
    base64::engine::general_purpose::STANDARD.encode(&blob)
}

#[test]
fn sign_verify_round_trip() {
    let (signing, public) = generate_keypair();
    let claims = LicenseClaims {
        customer_name: "Acme Corp".into(),
        customer_id: "acme-001".into(),
        issued_at: "2026-01-01".into(),
        expires_at: "2099-12-31".into(),
    };
    let key = sign_license(&signing, &claims);
    let recovered = verify_license_key_with(&key, &public).unwrap();
    assert_eq!(recovered.customer_name, "Acme Corp");
    assert_eq!(recovered.customer_id, "acme-001");
}

#[test]
fn tampered_payload_fails_verify() {
    let (signing, public) = generate_keypair();
    let claims = LicenseClaims {
        customer_name: "Acme Corp".into(),
        customer_id: "acme-001".into(),
        issued_at: "2026-01-01".into(),
        expires_at: "2099-12-31".into(),
    };
    let key = sign_license(&signing, &claims);

    // Decode, flip a byte in the payload, re-encode.
    let mut bytes = base64::engine::general_purpose::STANDARD
        .decode(&key)
        .unwrap();
    bytes[5] ^= 0x01;
    let tampered = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let err = verify_license_key_with(&tampered, &public).unwrap_err();
    assert!(matches!(err, LicenseError::InvalidSignature));
}

#[test]
fn wrong_public_key_fails_verify() {
    let (signing, _public) = generate_keypair();
    let (_, other_public) = generate_keypair();
    let claims = LicenseClaims {
        customer_name: "Acme".into(),
        customer_id: "x".into(),
        issued_at: "2026-01-01".into(),
        expires_at: "2099-12-31".into(),
    };
    let key = sign_license(&signing, &claims);
    let err = verify_license_key_with(&key, &other_public).unwrap_err();
    assert!(matches!(err, LicenseError::InvalidSignature));
}

#[test]
fn expired_license_classified_correctly() {
    let claims = LicenseClaims {
        customer_name: "x".into(),
        customer_id: "x".into(),
        issued_at: "2020-01-01".into(),
        expires_at: "2020-12-31".into(),
    };
    assert_eq!(
        license::check_expiry(&claims).unwrap(),
        ExpiryStatus::Expired
    );
}

#[test]
fn write_to_disk_round_trip() {
    // Use a temp config dir by overriding HOME / XDG_CONFIG_HOME for
    // the duration of this test. dirs::config_dir() honours the env.
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: the env mutations here are scoped to a single test
    // process — `cargo test` does run tests in parallel by default,
    // so any test that reads dirs::config_dir() concurrently could
    // race. We accept that risk for this single test; in practice
    // `serial_test` would be the proper guard, but the only writers
    // are in this file and we serialise via crate-local state.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        std::env::set_var("HOME", dir.path());
    }
    // Round-trip: write a synthetic key, then read back.
    license::write_to_disk("test-key-content").unwrap();
    let resolved = license::resolve_key().expect("should resolve from disk");
    assert_eq!(resolved, "test-key-content");

    // Deactivate removes it.
    let removed = license::remove_from_disk().unwrap();
    assert!(removed);
    assert!(license::resolve_key().is_none());

    unsafe {
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("HOME");
    }
}
