use super::*;
use crate::credentials::Secret;
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair},
};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "semon-provision-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn controller(&self) -> Controller {
        Controller::open(&self.0, "https://receiver.example:443", "laptop".into()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn key() -> EcdsaKeyPair {
    let random = SystemRandom::new();
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING, &random)
            .unwrap();
    EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8.as_ref(),
        &random,
    )
    .unwrap()
}
fn input(now: u64) -> pairing::ActivationInput {
    pairing::ActivationInput {
        machine: "laptop".into(),
        binding: Binding::Inapplicable,
        kinds: vec![(Kind::MirrorPush, false)],
        client_certificate_pins: vec![],
        valid_until: now + wire::INITIAL_CREDENTIAL_VALIDITY_SECONDS,
    }
}
fn opened(controller: &mut Controller, exporter: [u8; 32], now: u64) -> (wire::Opened, Secret) {
    let mut code = None;
    let response = controller
        .handle(
            Request::Open(wire::Open { version: 1 }),
            exporter,
            now,
            |_, secret| {
                code = Some(pairing::parse_code(secret.expose()).unwrap());
                Ok(())
            },
        )
        .unwrap();
    (serde_json::from_slice(&response).unwrap(), code.unwrap())
}
fn transcript(
    opened: &wire::Opened,
    key: &EcdsaKeyPair,
    exporter: [u8; 32],
    scope: [u8; 32],
) -> Transcript {
    Transcript {
        version: 1,
        session_id: opened.session_id.clone(),
        endpoint: opened.endpoint.clone(),
        exporter: exporter.to_vec(),
        origin_key: key.public_key().as_ref().to_vec(),
        scope,
        origin_nonce: [2; 32],
        receiver_nonce: wire::decode_fixed(&opened.receiver_nonce).unwrap(),
        origin_role: "origin".into(),
        receiver_role: "receiver".into(),
        profile: "public_tls".into(),
    }
}
fn confirmation(t: &Transcript, proof: &pairing::Mac) -> Request {
    Request::Confirm(wire::Confirm {
        version: 1,
        session_id: t.session_id.clone(),
        origin_key: wire::encode(&t.origin_key),
        origin_nonce: wire::encode(&t.origin_nonce),
        scope_commitment: wire::encode(&t.scope),
        origin_mac: wire::encode(proof.expose()),
    })
}
#[test]
fn confirmation_activation_and_recovery_preserve_one_registry_outcome() {
    let fixture = Fixture::new();
    let mut controller = fixture.controller();
    let exporter = [1; 32];
    let (opened, code) = opened(&mut controller, exporter, 100);
    let key = key();
    let input = input(100);
    let t = transcript(&opened, &key, exporter, input.commitment());
    let keys = pairing::derive_keys(&code, &t).unwrap();
    let response = controller
        .handle(
            confirmation(&t, &keys.origin_confirmation(&t)),
            exporter,
            100,
            |_, _| panic!("code shown twice"),
        )
        .unwrap();
    let response: wire::Confirmed = serde_json::from_slice(&response).unwrap();
    let confirmed = keys
        .confirm_receiver(
            &t,
            &pairing::Mac::from_bytes(wire::decode_fixed(&response.receiver_mac).unwrap()),
        )
        .unwrap();
    let proof = key
        .sign(&SystemRandom::new(), &wire::possession_message(&t))
        .unwrap();
    let response = controller
        .handle(
            Request::Approve(wire::Approve {
                version: 1,
                session_id: t.session_id.clone(),
                proof: wire::encode(proof.as_ref()),
                input,
            }),
            exporter,
            100,
            |_, _| panic!("code shown twice"),
        )
        .unwrap();
    let activated: wire::Activated = serde_json::from_slice(&response).unwrap();
    assert_eq!(activated.credentials.len(), 1);
    assert_eq!(activated.credentials[0].0, Kind::MirrorPush);
    let registry = controller.core.registry().unwrap();
    assert_eq!(registry.enrollments().len(), 1);
    assert_eq!(registry.credentials().len(), 1);
    // Recovery uses the origin-held confirmed application key, same semantic
    // path as the public wire recovery after verifying its fresh signature.
    let recovered = controller
        .core
        .deliver(
            &confirmed,
            wire::verify_possession(&t, &wire::encode(proof.as_ref())).unwrap(),
            101,
        )
        .unwrap();
    assert_eq!(recovered.enrollment_id, activated.enrollment_id);
    assert_eq!(
        recovered.credentials[0].1.expose(),
        activated.credentials[0].1
    );
    assert!(
        controller
            .core
            .deliver(
                &confirmed,
                wire::verify_possession(&t, &wire::encode(proof.as_ref())).unwrap(),
                102
            )
            .is_err()
    );
    assert_eq!(controller.core.registry().unwrap().enrollments().len(), 1);
}
#[test]
fn malformed_attempts_do_not_refund_and_foreign_connections_cannot_charge() {
    let fixture = Fixture::new();
    let mut controller = fixture.controller();
    let exporter = [1; 32];
    let (opened, _) = opened(&mut controller, exporter, 100);
    for _ in 0..5 {
        controller.malformed([2; 32], 100);
    }
    assert_eq!(
        controller.core.state(&opened.session_id, 100).unwrap(),
        pairing::State::Pending
    );
    for _ in 0..3 {
        controller.malformed(exporter, 100);
    }
    assert_eq!(
        controller.core.state(&opened.session_id, 100).unwrap(),
        pairing::State::Pending
    );
    controller.malformed(exporter, 100);
    assert_eq!(
        controller.core.state(&opened.session_id, 100).unwrap(),
        pairing::State::Rejected
    );
    assert!(controller.core.registry().unwrap().enrollments().is_empty());
}
#[test]
fn exact_expiry_cancellation_and_receiver_policy_refuse_without_activation() {
    let fixture = Fixture::new();
    let mut controller = fixture.controller();
    let (first, _) = opened(&mut controller, [1; 32], 100);
    controller.cancel(&first.session_id, 100).unwrap();
    assert_eq!(
        controller.core.state(&first.session_id, 100).unwrap(),
        pairing::State::Cancelled
    );
    let (second, _) = opened(&mut controller, [2; 32], 100);
    assert_eq!(
        controller.core.state(&second.session_id, 700).unwrap(),
        pairing::State::Expired
    );
    let mut input = input(700);
    assert!(controller.validate_input(&input, 700).is_ok());
    input.valid_until += 1;
    assert!(controller.validate_input(&input, 700).is_err());
    input.valid_until = 700;
    assert!(controller.validate_input(&input, 700).is_err());
    input.valid_until = 701;
    input.kinds = vec![(Kind::BodyChannel, true)];
    assert!(controller.validate_input(&input, 700).is_err());
    input.kinds = vec![(Kind::MirrorPush, false)];
    input.machine = "foreign".into();
    assert!(controller.validate_input(&input, 700).is_err());
    assert!(controller.core.registry().unwrap().enrollments().is_empty());
}
