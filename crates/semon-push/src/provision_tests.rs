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
#[test]
fn wrong_receiver_identity_refused_before_approval_without_side_effects() {
    let fixture = Fixture::new();
    let mut controller = fixture.controller();
    let (opened, code) = opened(&mut controller, [1; 32], 100);
    let key = key();
    let mut input = input(&controller, 100);
    input.receiver_id = "substituted-receiver".into();
    let t = transcript(&opened, &key, [1; 32], input.commitment());
    let keys = pairing::derive_keys(&code, &t).unwrap();
    controller
        .handle(
            confirmation(&t, &keys.origin_confirmation(&t)),
            [1; 32],
            100,
            |_, _| panic!(),
        )
        .unwrap();
    let before = controller.core.registry().unwrap();
    let proof = key
        .sign(&SystemRandom::new(), &wire::possession_message(&t))
        .unwrap();
    assert!(
        controller
            .handle(
                Request::Approve(wire::Approve {
                    version: 1,
                    session_id: t.session_id.clone(),
                    proof: wire::encode(proof.as_ref()),
                    input
                }),
                [1; 32],
                100,
                |_, _| panic!()
            )
            .is_err()
    );
    assert_eq!(
        controller.core.state(&t.session_id, 100).unwrap(),
        pairing::State::Confirmed
    );
    assert_eq!(controller.core.registry().unwrap(), before);
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
fn input(controller: &Controller, now: u64) -> pairing::ActivationInput {
    pairing::ActivationInput {
        receiver_id: controller.custody.receiver_id().into(),
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
    let input = input(&controller, 100);
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
    let mut substituted: pairing::ActivationInput =
        serde_json::from_slice(&serde_json::to_vec(&input).unwrap()).unwrap();
    substituted.valid_until -= 1;
    assert!(
        controller
            .handle(
                Request::Approve(wire::Approve {
                    version: 1,
                    session_id: t.session_id.clone(),
                    proof: wire::encode(proof.as_ref()),
                    input: substituted,
                }),
                exporter,
                100,
                |_, _| panic!("code shown twice")
            )
            .is_err()
    );
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
    let recovery_exporter = [9; 32];
    let mut signed = b"semon-asi-recovery-pop-v1\0".to_vec();
    signed.extend_from_slice(&t.digest());
    signed.extend_from_slice(t.session_id.as_bytes());
    signed.extend_from_slice(&recovery_exporter);
    let recovery_proof = key.sign(&SystemRandom::new(), &signed).unwrap();
    let recover = wire::Recover {
        version: 1,
        transcript: wire::RecoveryTranscript {
            session_id: t.session_id.clone(),
            endpoint: t.endpoint.clone(),
            exporter: wire::encode(&t.exporter),
            origin_key: wire::encode(&t.origin_key),
            scope_commitment: wire::encode(&t.scope),
            origin_nonce: wire::encode(&t.origin_nonce),
            receiver_nonce: wire::encode(&t.receiver_nonce),
        },
        application_key: wire::encode(confirmed.recovery_key_for_test()),
        proof: wire::encode(recovery_proof.as_ref()),
    };
    let recover_bytes = wire::encode_message(&recover).unwrap();
    assert!(
        controller
            .handle(
                wire::Request::parse(wire::Route::Recover, &recover_bytes).unwrap(),
                [8; 32],
                101,
                |_, _| panic!("recovery code")
            )
            .is_err()
    );
    let recovered = controller
        .handle(
            wire::Request::parse(wire::Route::Recover, &recover_bytes).unwrap(),
            recovery_exporter,
            101,
            |_, _| panic!("recovery code"),
        )
        .unwrap();
    let recovered: wire::Activated = serde_json::from_slice(&recovered).unwrap();
    assert_eq!(recovered.enrollment_id, activated.enrollment_id);
    assert_eq!(recovered.credentials[0].1, activated.credentials[0].1);
    assert!(
        controller
            .handle(
                wire::Request::parse(wire::Route::Recover, &recover_bytes).unwrap(),
                recovery_exporter,
                102,
                |_, _| panic!("recovery code")
            )
            .is_err()
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
fn wrong_code_charges_attempt_and_connection_replay_cannot_confirm() {
    let fixture = Fixture::new();
    let mut controller = fixture.controller();
    let (created, code) = opened(&mut controller, [1; 32], 100);
    let key = key();
    let t = transcript(
        &created,
        &key,
        [1; 32],
        input(&controller, 100).commitment(),
    );
    let keys = pairing::derive_keys(&code, &t).unwrap();
    let correct = keys.origin_confirmation(&t);
    assert!(
        controller
            .handle(confirmation(&t, &correct), [2; 32], 100, |_, _| panic!(
                "code"
            ))
            .is_err()
    );
    for _ in 0..3 {
        assert!(
            controller
                .handle(
                    confirmation(&t, &pairing::Mac::from_bytes([0; 32])),
                    [1; 32],
                    100,
                    |_, _| panic!("code")
                )
                .is_err()
        );
    }
    assert!(
        controller
            .handle(confirmation(&t, &correct), [1; 32], 100, |_, _| panic!(
                "code"
            ))
            .is_err()
    );
    assert_eq!(
        controller.core.state(&created.session_id, 100).unwrap(),
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
    let mut input = input(&controller, 700);
    assert!(controller.validate_input(&input, 700).is_ok());
    input.valid_until += 1;
    assert!(controller.validate_input(&input, 700).is_err());
    input.valid_until = 700;
    assert!(controller.validate_input(&input, 700).is_err());
    input.valid_until = 701;
    input.kinds = vec![(Kind::BodyChannel, true)];
    assert!(controller.validate_input(&input, 700).is_err());
    input.kinds = vec![(Kind::AnalyticsPublish, false)];
    let before = controller.core.registry().unwrap();
    assert!(controller.validate_input(&input, 700).is_err());
    assert_eq!(controller.core.registry().unwrap(), before);
    input.kinds = vec![(Kind::MirrorPush, false), (Kind::MirrorPush, false)];
    assert!(controller.validate_input(&input, 700).is_err());
    input.kinds = vec![(Kind::MirrorPush, false)];
    input.machine = "foreign".into();
    assert!(controller.validate_input(&input, 700).is_err());
    assert!(controller.core.registry().unwrap().enrollments().is_empty());
}

#[test]
fn confirmation_finishing_at_expiry_spends_attempt_but_refuses() {
    let fixture = Fixture::new();
    let mut controller = fixture.controller();
    let (created, code) = opened(&mut controller, [1; 32], 100);
    let key = key();
    let t = transcript(
        &created,
        &key,
        [1; 32],
        input(&controller, 100).commitment(),
    );
    let keys = pairing::derive_keys(&code, &t).unwrap();
    let clock = std::cell::Cell::new(699);
    let now = || {
        let current = clock.get();
        clock.set(700);
        current
    };
    assert!(
        controller
            .handle_with_clock(
                confirmation(&t, &keys.origin_confirmation(&t)),
                [1; 32],
                &now,
                |_, _| panic!("code")
            )
            .is_err()
    );
    assert_eq!(
        controller.core.state(&created.session_id, 700).unwrap(),
        pairing::State::Expired
    );
    assert!(controller.core.registry().unwrap().enrollments().is_empty());
}

#[test]
fn analytics_refusal_changes_no_authority_and_never_refunds_attempts() {
    let fixture = Fixture::new();
    let mut controller = fixture.controller();
    let (created, code) = opened(&mut controller, [1; 32], 100);
    let key = key();
    let mut input = input(&controller, 100);
    input.kinds = vec![(Kind::AnalyticsPublish, false)];
    let t = transcript(&created, &key, [1; 32], input.commitment());
    let keys = pairing::derive_keys(&code, &t).unwrap();
    assert!(
        controller
            .handle(
                confirmation(&t, &pairing::Mac::from_bytes([0; 32])),
                [1; 32],
                100,
                |_, _| panic!("code")
            )
            .is_err()
    );
    let response = controller
        .handle(
            confirmation(&t, &keys.origin_confirmation(&t)),
            [1; 32],
            100,
            |_, _| panic!("code"),
        )
        .unwrap();
    let response: wire::Confirmed = serde_json::from_slice(&response).unwrap();
    let _confirmed = keys
        .confirm_receiver(
            &t,
            &pairing::Mac::from_bytes(wire::decode_fixed(&response.receiver_mac).unwrap()),
        )
        .unwrap();
    let proof = key
        .sign(&SystemRandom::new(), &wire::possession_message(&t))
        .unwrap();
    let registry = controller.core.registry().unwrap();
    let acl = std::fs::read(
        controller
            .custody
            .acl_dir()
            .join(crate::receiver_acl::ACL_FILE),
    )
    .unwrap();
    assert!(
        controller
            .handle(
                Request::Approve(wire::Approve {
                    version: 1,
                    session_id: t.session_id.clone(),
                    proof: wire::encode(proof.as_ref()),
                    input
                }),
                [1; 32],
                100,
                |_, _| panic!("code")
            )
            .is_err()
    );
    assert_eq!(
        controller.take_unavailable(),
        Some("analytics_publish available after origin connect / producer admission lands")
    );
    assert_eq!(controller.core.registry().unwrap(), registry);
    assert_eq!(
        std::fs::read(
            controller
                .custody
                .acl_dir()
                .join(crate::receiver_acl::ACL_FILE)
        )
        .unwrap(),
        acl
    );
    assert_eq!(
        controller.core.state(&t.session_id, 100).unwrap(),
        pairing::State::Confirmed
    );
    assert!(
        controller
            .handle(
                confirmation(&t, &keys_for_replay(&code, &t)),
                [1; 32],
                100,
                |_, _| panic!("code")
            )
            .is_err()
    );
}
fn keys_for_replay(code: &Secret, t: &Transcript) -> pairing::Mac {
    pairing::derive_keys(code, t)
        .unwrap()
        .origin_confirmation(t)
}
