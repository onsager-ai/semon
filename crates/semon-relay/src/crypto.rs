use std::io::{self, Write};

use age::{Decryptor, Encryptor, x25519};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, AeadCore, OsRng, Payload, rand_core::RngCore},
};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use thiserror::Error;

use crate::FrameKey;

pub const DATA_KEY_BYTES: usize = 32;
pub const FRAME_NONCE_BYTES: usize = 24;
pub const CONTENT_TAG_BYTES: usize = 32;

pub type DataKey = [u8; DATA_KEY_BYTES];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedPayload {
    pub nonce: [u8; FRAME_NONCE_BYTES],
    pub ciphertext: Vec<u8>,
    pub tag: [u8; CONTENT_TAG_BYTES],
}

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("frame encryption failed")]
    Encrypt,
    #[error("frame authentication failed")]
    Decrypt,
    #[error("decrypted frame is shorter than its chain value")]
    PlaintextLength,
    #[error("decrypted frame content tag does not match")]
    Tag,
    #[error("cannot derive the frame content-tag key")]
    Kdf,
    #[error("cannot create an age envelope: {0}")]
    AgeEncrypt(#[from] age::EncryptError),
    #[error("cannot write an age envelope: {0}")]
    AgeIo(#[from] io::Error),
    #[error("cannot decrypt an age envelope: {0}")]
    AgeDecrypt(#[from] age::DecryptError),
    #[error("decrypted envelope contains {0} bytes, expected 32")]
    EnvelopeLength(usize),
}

pub fn generate_data_key() -> DataKey {
    let mut key = [0_u8; DATA_KEY_BYTES];
    OsRng.fill_bytes(&mut key);
    key
}

pub fn encrypt_frame(
    key: &DataKey,
    frame_key: &FrameKey,
    machine: &str,
    chain: &[u8; 32],
    line: &[u8],
) -> Result<EncryptedPayload, CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let mut plaintext = Vec::with_capacity(chain.len() + line.len());
    plaintext.extend_from_slice(line);
    plaintext.extend_from_slice(chain);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &plaintext,
                aad: &frame_aad(frame_key, machine),
            },
        )
        .map_err(|_| CryptoError::Encrypt)?;
    Ok(EncryptedPayload {
        nonce: nonce.into(),
        ciphertext,
        tag: content_tag(key, line)?,
    })
}

pub fn decrypt_frame(
    key: &DataKey,
    frame_key: &FrameKey,
    machine: &str,
    payload: &EncryptedPayload,
) -> Result<([u8; 32], Vec<u8>), CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&payload.nonce),
            Payload {
                msg: &payload.ciphertext,
                aad: &frame_aad(frame_key, machine),
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    if plaintext.len() < 32 {
        return Err(CryptoError::PlaintextLength);
    }
    let (line, chain) = plaintext.split_at(plaintext.len() - 32);
    let chain: [u8; 32] = chain
        .try_into()
        .expect("the decrypted frame chain length was checked");
    verify_content_tag(key, line, &payload.tag)?;
    Ok((chain, line.to_vec()))
}

pub fn content_tag(key: &DataKey, line: &[u8]) -> Result<[u8; 32], CryptoError> {
    let mut mac = tag_mac(key)?;
    mac.update(line);
    Ok(mac.finalize().into_bytes().into())
}

fn verify_content_tag(key: &DataKey, line: &[u8], tag: &[u8; 32]) -> Result<(), CryptoError> {
    let mut mac = tag_mac(key)?;
    mac.update(line);
    mac.verify_slice(tag).map_err(|_| CryptoError::Tag)
}

fn tag_mac(key: &DataKey) -> Result<Hmac<Sha256>, CryptoError> {
    let mut tag_key = [0_u8; 32];
    Hkdf::<Sha256>::new(None, key)
        .expand(b"semon tag", &mut tag_key)
        .map_err(|_| CryptoError::Kdf)?;
    Ok(<Hmac<Sha256> as Mac>::new_from_slice(&tag_key).expect("HMAC-SHA256 accepts a 32-byte key"))
}

pub fn encrypt_envelope(
    key: &DataKey,
    recipients: &[x25519::Recipient],
) -> Result<Vec<u8>, CryptoError> {
    let encryptor = Encryptor::with_recipients(
        recipients
            .iter()
            .map(|recipient| recipient as &dyn age::Recipient),
    )?;
    let mut envelope = Vec::new();
    let mut writer = encryptor.wrap_output(&mut envelope)?;
    writer.write_all(key)?;
    writer.finish()?;
    Ok(envelope)
}

pub fn decrypt_envelope(
    envelope: &[u8],
    identity: &x25519::Identity,
) -> Result<DataKey, CryptoError> {
    let decryptor = Decryptor::new_buffered(envelope)?;
    let mut reader = decryptor.decrypt(std::iter::once(identity as &dyn age::Identity))?;
    let mut key = Vec::new();
    io::Read::read_to_end(&mut reader, &mut key)?;
    let length = key.len();
    key.try_into()
        .map_err(|_| CryptoError::EnvelopeLength(length))
}

fn frame_aad(key: &FrameKey, machine: &str) -> Vec<u8> {
    let mut aad = b"semon frame v1\0".to_vec();
    append_string(&mut aad, &key.session);
    append_string(&mut aad, &key.stream);
    aad.extend_from_slice(&key.generation.to_be_bytes());
    aad.extend_from_slice(&key.epoch.to_be_bytes());
    aad.extend_from_slice(&key.seq.to_be_bytes());
    append_string(&mut aad, machine);
    aad
}

fn append_string(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}
