//! Receiver-only client issuance. Origin public keys come from the confirmed PoP.
use crate::enrollment::{CertificatePin, Pin};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, PublicKeyData,
};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use zeroize::{Zeroize, Zeroizing};

struct CaKey(KeyPair);
impl Drop for CaKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

type Result<T> = std::result::Result<T, ()>;
const TEN_YEARS: u64 = 365 * 10 * 86400;

struct OriginKey<'a>(&'a [u8]);
impl PublicKeyData for OriginKey<'_> {
    fn der_bytes(&self) -> &[u8] {
        self.0
    }
    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_ECDSA_P256_SHA256
    }
}
fn timestamp(now: u64) -> Result<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(i64::try_from(now).map_err(|_| ())?).map_err(|_| ())
}
fn ca_params(created: u64) -> Result<CertificateParams> {
    let mut p = CertificateParams::default();
    p.distinguished_name = rcgen::DistinguishedName::new();
    p.distinguished_name
        .push(DnType::CommonName, "Semon client issuer");
    p.not_before = timestamp(created.saturating_sub(300))?;
    p.not_after = timestamp(created.checked_add(TEN_YEARS).ok_or(())?)?;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    Ok(p)
}
pub(crate) fn validate_ca(private: &[u8], certificate: &[u8]) -> Result<()> {
    let key = KeyPair::from_pkcs8_der_and_sign_algo(
        &rustls::pki_types::PrivatePkcs8KeyDer::from(private),
        &rcgen::PKCS_ECDSA_P256_SHA256,
    )
    .map(CaKey)
    .map_err(|_| ())?;
    let cert = rustls::pki_types::CertificateDer::from(certificate);
    let parsed = rustls::server::ParsedCertificate::try_from(&cert).map_err(|_| ())?;
    if parsed.subject_public_key_info().as_ref() != key.0.subject_public_key_info() {
        return Err(());
    }
    Ok(())
}
pub(crate) fn generate(created: u64) -> Result<(Zeroizing<Vec<u8>>, Vec<u8>)> {
    let key = CaKey(KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(|_| ())?);
    let cert = ca_params(created)?.self_signed(&key.0).map_err(|_| ())?;
    Ok((Zeroizing::new(key.0.serialize_der()), cert.der().to_vec()))
}
pub(crate) fn pem(der: &[u8]) -> Vec<u8> {
    let mut base64 = crate::provision_wire::encode(der)
        .replace('-', "+")
        .replace('_', "/");
    while !base64.len().is_multiple_of(4) {
        base64.push('=');
    }
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in base64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).expect("base64 ASCII"));
        out.push('\n');
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out.into_bytes()
}
pub(crate) fn pin(public: &[u8]) -> Result<CertificatePin> {
    if public.len() != 65 || public[0] != 4 {
        return Err(());
    }
    Ok(CertificatePin {
        pin: Pin::Spki(Sha256::digest(OriginKey(public).subject_public_key_info()).into()),
        generation: 1,
        revoked_at: None,
    })
}
pub(crate) fn issue(
    private: &[u8],
    created: u64,
    public: &[u8],
    enrollment: &str,
    now: u64,
    until: u64,
) -> Result<Vec<u8>> {
    pin(public)?;
    if until <= now
        || now < created.saturating_sub(300)
        || until > now.saturating_add(crate::provision_wire::INITIAL_CREDENTIAL_VALIDITY_SECONDS)
        || until > created.checked_add(TEN_YEARS).ok_or(())?
    {
        return Err(());
    }
    let key = KeyPair::from_pkcs8_der_and_sign_algo(
        &rustls::pki_types::PrivatePkcs8KeyDer::from(private),
        &rcgen::PKCS_ECDSA_P256_SHA256,
    )
    .map(CaKey)
    .map_err(|_| ())?;
    let issuer = rcgen::Issuer::new(ca_params(created)?, &key.0);
    let mut p = CertificateParams::default();
    p.distinguished_name = rcgen::DistinguishedName::new();
    p.distinguished_name.push(DnType::CommonName, enrollment);
    p.not_before = timestamp(now.saturating_sub(300))?;
    p.not_after = timestamp(until)?;
    p.is_ca = IsCa::ExplicitNoCa;
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let mut serial = [0; 16];
    SystemRandom::new().fill(&mut serial).map_err(|_| ())?;
    p.serial_number = Some(rcgen::SerialNumber::from_slice(&serial));
    p.signed_by(&OriginKey(public), &issuer)
        .map(|c| c.der().to_vec())
        .map_err(|_| ())
}

/// Validate public delivery without inventing client-CA trust at the origin.
/// rustls parses the certificate for SPKI extraction; yasna reads exact validity
/// and EKU values. This does not establish issuer trust or check key usage.
pub(crate) fn validate(der: &[u8], public: &[u8], until: u64) -> Result<(u64, u64)> {
    let certificate = rustls::pki_types::CertificateDer::from(der);
    let parsed = rustls::server::ParsedCertificate::try_from(&certificate).map_err(|_| ())?;
    if parsed.subject_public_key_info().as_ref() != OriginKey(public).subject_public_key_info() {
        return Err(());
    }
    use yasna::{ASN1Error, ASN1ErrorKind, Tag, models::ObjectIdentifier};
    let invalid = || ASN1Error::new(ASN1ErrorKind::Invalid);
    let (validity, extensions) = yasna::parse_der(der, |r| {
        r.read_sequence(|r| {
            let values = r.next().read_sequence(|r| {
                r.next().read_tagged(Tag::context(0), |r| r.read_u64())?;
                r.next().read_der()?; // serial
                r.next().read_der()?; // signature algorithm
                r.next().read_der()?; // issuer (not a locally known trust anchor)
                let validity = r
                    .next()
                    .read_sequence(|r| Ok((r.next().read_der()?, r.next().read_der()?)))?;
                r.next().read_der()?; // subject
                r.next().read_der()?; // SPKI, checked by rustls above
                let extensions = r.next().read_tagged(Tag::context(3), |r| {
                    r.collect_sequence_of(|r| {
                        r.read_sequence(|r| {
                            let oid = r.next().read_oid()?;
                            r.read_optional(|r| r.read_bool())?;
                            Ok((oid, r.next().read_bytes()?))
                        })
                    })
                })?;
                Ok((validity, extensions))
            })?;
            r.next().read_der()?;
            r.next().read_bitvec_bytes()?;
            Ok(values)
        })
    })
    .map_err(|_| ())?;
    fn date(bytes: &[u8]) -> Result<OffsetDateTime> {
        yasna::parse_der(bytes, |r| r.read_utctime().map(|t| *t.datetime()))
            .or_else(|_| {
                yasna::parse_der(bytes, |r| r.read_generalized_time().map(|t| *t.datetime()))
            })
            .map_err(|_| ())
    }
    let from = date(&validity.0)?.unix_timestamp();
    let to = date(&validity.1)?.unix_timestamp();
    if to != i64::try_from(until).map_err(|_| ())?
        || from >= to
        || to - from
            > i64::try_from(crate::provision_wire::INITIAL_CREDENTIAL_VALIDITY_SECONDS + 300)
                .map_err(|_| ())?
    {
        return Err(());
    }
    let eku_oid = ObjectIdentifier::from_slice(&[2, 5, 29, 37]);
    let eku = extensions
        .iter()
        .find(|(oid, _)| *oid == eku_oid)
        .ok_or(())?;
    yasna::parse_der(&eku.1, |r| {
        let usages = r.collect_sequence_of(|r| r.read_oid())?;
        if usages != [ObjectIdentifier::from_slice(&[1, 3, 6, 1, 5, 5, 7, 3, 2])] {
            return Err(invalid());
        }
        Ok(())
    })
    .map_err(|_| ())?;
    Ok((u64::try_from(from).map_err(|_| ())?, until))
}

pub(crate) fn validate_at(
    der: &[u8],
    public: &[u8],
    until: u64,
    now: u64,
) -> std::result::Result<(), &'static str> {
    let (from, to) =
        validate(der, public, until).map_err(|_| "invalid origin client certificate")?;
    if now >= to {
        return Err("origin client certificate expired; reconnect with fresh enrollment");
    }
    if from > now.saturating_add(300) {
        return Err("origin client certificate is not yet valid");
    }
    Ok(())
}

pub(crate) fn validate_receipt(
    der: &[u8],
    origin_hash: &[u8; 32],
    until: u64,
    created: u64,
) -> Result<()> {
    let certificate = rustls::pki_types::CertificateDer::from(der);
    let parsed = rustls::server::ParsedCertificate::try_from(&certificate).map_err(|_| ())?;
    let spki = parsed.subject_public_key_info();
    let public = yasna::parse_der(spki.as_ref(), |r| {
        r.read_sequence(|r| {
            r.next().read_der()?;
            let (public, bits) = r.next().read_bitvec_bytes()?;
            if bits != 520 {
                return Err(yasna::ASN1Error::new(yasna::ASN1ErrorKind::Invalid));
            }
            Ok(public)
        })
    })
    .map_err(|_| ())?;
    if <[u8; 32]>::from(Sha256::digest(&public)) != *origin_hash {
        return Err(());
    }
    let (from, _) = validate(der, &public, until)?;
    if from != created.saturating_sub(300) {
        return Err(());
    }
    Ok(())
}
