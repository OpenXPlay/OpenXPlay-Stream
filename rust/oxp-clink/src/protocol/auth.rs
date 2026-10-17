// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use super::{FieldValue as V, Fields, Frame, SecureRecord};
use crate::{require, Result};
use hmac::{Hmac, Mac};
use p256::{
    ecdsa::{
        signature::{RandomizedSigner, Verifier},
        Signature, SigningKey, VerifyingKey,
    },
    elliptic_curve::sec1::ToEncodedPoint,
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
    PublicKey, SecretKey,
};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
#[derive(Clone)]
pub struct AuthIdentity {
    secret: SecretKey,
}
impl AuthIdentity {
    pub fn new(pkcs8: &[u8], public: &[u8]) -> Result<Self> {
        let secret = SecretKey::from_pkcs8_der(pkcs8)?;
        require(raw(&secret.public_key()) == public, "identity_key_mismatch")?;
        Ok(Self { secret })
    }
    pub fn generate() -> Self {
        Self {
            secret: SecretKey::random(&mut OsRng),
        }
    }
    pub fn pkcs8(&self) -> Result<Vec<u8>> {
        Ok(self.secret.to_pkcs8_der()?.as_bytes().to_vec())
    }
    pub fn public_key(&self) -> Vec<u8> {
        raw(&self.secret.public_key())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthResult {
    Success,
    PinRequired,
    ConfirmationRequired,
    InternalError,
    Rejected,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthEvent {
    Result(AuthResult, u64),
    PeerVerified(Vec<u8>),
    Failed(String),
}
#[derive(Default, Debug)]
pub struct AuthOutput {
    pub frames: Vec<Frame>,
    pub session_key: Option<[u8; 16]>,
    pub events: Vec<AuthEvent>,
}
pub struct Auth {
    identity: AuthIdentity,
    device_id: String,
    name: String,
    local_ipv4: String,
    peer: Option<Vec<u8>>,
    agreement: Option<SecretKey>,
    nonce: [u8; 32],
    pin: Vec<u8>,
    quick: bool,
    sequence: u32,
    complete: bool,
}
impl Auth {
    pub fn new(
        identity: AuthIdentity,
        device_id: String,
        name: String,
        local_ipv4: String,
        peer: Option<Vec<u8>>,
    ) -> Result<Self> {
        require(
            !device_id.is_empty() && !name.is_empty(),
            "invalid_device_identity",
        )?;
        let parts: Vec<_> = local_ipv4.split('.').collect();
        require(
            parts.len() == 4
                && parts.iter().all(|p| {
                    !p.is_empty()
                        && p.bytes().all(|b| b.is_ascii_digit())
                        && p.parse::<u32>().is_ok_and(|v| v <= 255)
                }),
            "invalid_local_ipv4",
        )?;
        if let Some(p) = &peer {
            decode_public(p)?;
        }
        Ok(Self {
            identity,
            device_id,
            name,
            local_ipv4,
            peer,
            agreement: None,
            nonce: [0; 32],
            pin: Vec::new(),
            quick: false,
            sequence: 0,
            complete: false,
        })
    }
    pub fn start(
        &mut self,
        time: u32,
        sequence: u32,
        pin: Option<&str>,
        force_full: bool,
        confirmed: bool,
    ) -> Result<AuthOutput> {
        let mut nonce = [0; 32];
        OsRng.fill_bytes(&mut nonce);
        self.start_with_material(
            time,
            sequence,
            pin,
            force_full,
            confirmed,
            SecretKey::random(&mut OsRng),
            nonce,
        )
    }
    #[allow(clippy::too_many_arguments)] // One argument per protocol field.
    pub fn start_with_material(
        &mut self,
        time: u32,
        sequence: u32,
        pin: Option<&str>,
        force_full: bool,
        confirmed: bool,
        ephemeral: SecretKey,
        nonce: [u8; 32],
    ) -> Result<AuthOutput> {
        require(
            !self.complete && self.agreement.is_none(),
            "unexpected_auth_request",
        )?;
        self.quick = self.peer.is_some() && !force_full;
        self.pin = pin.unwrap_or("").as_bytes().to_vec();
        require(
            self.pin.len() <= 64 && (self.pin.is_empty() || self.pin.len() >= 6),
            "invalid_pin",
        )?;
        require(self.quick || self.pin.len() >= 6, "pin_required")?;
        let ep = raw(&ephemeral.public_key());
        let mut transcript = ep.clone();
        transcript.extend_from_slice(&nonce);
        let signing = SigningKey::from(&self.identity.secret);
        let signature: Signature = signing.sign_with_rng(&mut OsRng, &transcript);
        let mut values = std::collections::BTreeMap::from([
            (1, V::Varint(1)),
            (4, V::Bytes(ep)),
            (5, V::Bytes(signature.to_der().as_bytes().to_vec())),
            (6, V::Bytes(nonce.to_vec())),
            (7, V::Bytes(self.device_id.as_bytes().to_vec())),
            (8, V::Bytes(self.name.as_bytes().to_vec())),
            (9, V::Varint(confirmed as u64)),
        ]);
        if !self.quick {
            let public = self.identity.public_key();
            values.insert(3, V::Bytes(proof(&self.pin, &nonce, &public).to_vec()));
            values.insert(2, V::Bytes(public));
        }
        self.agreement = Some(ephemeral);
        self.nonce = nonce;
        self.sequence = sequence;
        Ok(AuthOutput {
            frames: vec![Frame {
                sequence,
                unix_seconds: time,
                source: 1,
                format: 1,
                kind: 1,
                category: 3,
                method: 1,
                body: Fields(values).encode()?,
            }],
            ..Default::default()
        })
    }
    pub fn receive(&mut self, frame: &Frame, time: u32) -> Result<AuthOutput> {
        require(
            !self.complete && self.agreement.is_some(),
            "unexpected_auth_response",
        )?;
        match self.receive_inner(frame, time) {
            Ok(v) => Ok(v),
            Err(e) => {
                self.agreement = None;
                self.complete = true;
                self.pin.fill(0);
                Ok(AuthOutput {
                    events: vec![AuthEvent::Failed(e.to_string())],
                    ..Default::default()
                })
            }
        }
    }
    fn receive_inner(&mut self, frame: &Frame, time: u32) -> Result<AuthOutput> {
        require(
            frame.category == 3
                && frame.method == 2
                && frame.kind == 2
                && frame.sequence == self.sequence
                && frame.source == 0
                && frame.format == 1,
            "invalid_auth_header",
        )?;
        let f = Fields::decode(&frame.body)?;
        require(
            [1, 0x0901].contains(&f.number(1)?),
            "auth_version_unsupported",
        )?;
        let result = f.number(7)?;
        if result != 0 {
            self.agreement = None;
            let r = match result {
                1 => AuthResult::PinRequired,
                2 => AuthResult::ConfirmationRequired,
                3 => AuthResult::InternalError,
                _ => AuthResult::Rejected,
            };
            return Ok(AuthOutput {
                events: vec![AuthEvent::Result(r, result)],
                ..Default::default()
            });
        }
        let car_nonce = f.sized_bytes(6, 32)?;
        let car_agreement = f.sized_bytes(4, 64)?;
        let signature = f.bytes(5)?;
        require(
            (1..=80).contains(&signature.len()),
            "invalid_auth_signature",
        )?;
        let car_identity = if self.quick {
            self.peer.clone().ok_or("invalid_peer_key")?
        } else {
            let p = f.sized_bytes(2, 64)?.to_vec();
            let mut nonces = car_nonce.to_vec();
            nonces.extend_from_slice(&self.nonce);
            let mut public = p.clone();
            public.extend_from_slice(&self.identity.public_key());
            let proof_key = Sha256::digest([nonces.as_slice(), &self.pin].concat());
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&proof_key).unwrap();
            mac.update(&public);
            require(
                mac.verify_slice(f.sized_bytes(3, 32)?).is_ok(),
                "auth_pin_proof_failed",
            )?;
            require(
                self.peer.as_ref().is_none_or(|peer| peer == &p),
                "peer_key_changed",
            )?;
            p
        };
        let ep = self.agreement.as_ref().unwrap();
        let mut transcript = car_agreement.to_vec();
        transcript.extend_from_slice(&raw(&ep.public_key()));
        transcript.extend_from_slice(car_nonce);
        transcript.extend_from_slice(&self.nonce);
        let verifying = VerifyingKey::from(decode_public(&car_identity)?);
        let signature = Signature::from_der(signature).map_err(|_| "auth_signature_failed")?;
        require(
            verifying.verify(&transcript, &signature).is_ok(),
            "auth_signature_failed",
        )?;
        let shared = p256::ecdh::diffie_hellman(
            ep.to_nonzero_scalar(),
            decode_public(car_agreement)?.as_affine(),
        );
        let salt = Sha256::digest([self.nonce.as_slice(), car_nonce].concat());
        let prk = hmac(&salt, shared.raw_secret_bytes().as_slice());
        let expanded = hmac(&prk, b"session_key\x01");
        let key: [u8; 16] = expanded[..16].try_into()?;
        let record = SecureRecord::new(&key)?.seal(self.local_ipv4.as_bytes())?;
        let confirmation = Frame {
            sequence: self.sequence.wrapping_add(1),
            unix_seconds: time,
            source: 1,
            format: 1,
            kind: 0,
            category: 3,
            method: 3,
            body: Fields(std::collections::BTreeMap::from([(1, V::Bytes(record))])).encode()?,
        };
        self.complete = true;
        self.agreement = None;
        self.pin.fill(0);
        Ok(AuthOutput {
            frames: vec![confirmation],
            session_key: Some(key),
            events: vec![
                AuthEvent::Result(AuthResult::Success, 0),
                AuthEvent::PeerVerified(car_identity),
            ],
        })
    }
}
pub fn decode_public(raw: &[u8]) -> Result<PublicKey> {
    require(raw.len() == 64, "invalid_peer_key")?;
    let mut sec1 = vec![4];
    sec1.extend_from_slice(raw);
    PublicKey::from_sec1_bytes(&sec1).map_err(|_| "invalid_peer_key".into())
}
fn raw(public: &PublicKey) -> Vec<u8> {
    public.to_encoded_point(false).as_bytes()[1..].to_vec()
}
pub fn hmac(key: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut h = <Hmac<Sha256> as Mac>::new_from_slice(key).unwrap();
    h.update(bytes);
    h.finalize().into_bytes().into()
}
pub fn proof(pin: &[u8], nonce: &[u8], bytes: &[u8]) -> [u8; 32] {
    hmac(&Sha256::digest([nonce, pin].concat()), bytes)
}
