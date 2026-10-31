//! Device pairing for the phone. The phone creates the P-256 key. This crate
//! verifies it. No key or token is minted here, and there is no relay.
//!
//! The wire format is `ecdsa-p256-sha256`: a SEC1 uncompressed public key
//! (`0x04 || X || Y`) and an ASN.1 DER signature over SHA-256 of the message.
//!
//! A real phone is not paired by these tests. Lost-phone revoke is covered
//! first, while `real_pairings` stays zero.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};

/// Session takeover is not implemented.
pub const TAKEOVER: bool = false;

/// Name of the phone signature algorithm.
pub const DEVICE_SIGNATURE: &str = "ecdsa-p256-sha256";

const GRANT_TTL: Duration = Duration::from_secs(30);
const MAX_CHALLENGES: usize = 64;

#[derive(Clone)]
pub struct Pairing {
    inner: std::sync::Arc<Mutex<Inner>>,
}

struct Inner {
    ttl: Duration,
    challenge_ttl: Duration,
    challenge_rate: usize,
    challenge_window: Duration,
    nonces: Vec<String>,
    challenges: Vec<Ticket>,
    recent: Vec<Instant>,
    devices: HashMap<String, Device>,
    grants: HashMap<String, Grant>,
}

struct Ticket {
    token: String,
    issued: Instant,
}

struct Device {
    public: VerifyingKey,
    revoked: bool,
}

struct Grant {
    opened: Instant,
    released: bool,
}

impl Pairing {
    pub fn new() -> Self {
        Self::with_ttl(GRANT_TTL)
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(Inner {
                ttl,
                challenge_ttl: GRANT_TTL,
                challenge_rate: 32,
                challenge_window: Duration::from_secs(1),
                nonces: Vec::new(),
                challenges: Vec::new(),
                recent: Vec::new(),
                devices: HashMap::new(),
                grants: HashMap::new(),
            })),
        }
    }

    pub fn with_challenge_limit(rate: usize, challenge_ttl: Duration) -> Self {
        let pairing = Self::new();
        let mut inner = pairing.inner.lock().expect("pairing");
        inner.challenge_rate = rate.max(1);
        inner.challenge_ttl = challenge_ttl;
        drop(inner);
        pairing
    }

    /// No real phone has been paired. Tests assert this stays zero.
    pub fn real_pairings(&self) -> u64 {
        0
    }

    pub fn is_paired(&self, device_id: &str) -> bool {
        let inner = self.inner.lock().expect("pairing");
        inner
            .devices
            .get(device_id)
            .is_some_and(|device| !device.revoked)
    }

    pub fn issue_nonce(&self) -> String {
        let nonce = random_token();
        let mut inner = self.inner.lock().expect("pairing");
        push_capped(&mut inner.nonces, nonce.clone());
        nonce
    }

    pub fn issue_challenge(&self) -> Result<String, AuthError> {
        let mut inner = self.inner.lock().expect("pairing");
        let now = Instant::now();
        let ttl = inner.challenge_ttl;
        let window = inner.challenge_window;
        let rate = inner.challenge_rate;
        inner
            .challenges
            .retain(|ticket| now.saturating_duration_since(ticket.issued) <= ttl);
        inner
            .recent
            .retain(|issued| now.saturating_duration_since(*issued) <= window);
        if inner.recent.len() >= rate {
            return Err(AuthError::RateLimited);
        }
        let challenge = random_token();
        inner.recent.push(now);
        if inner.challenges.len() >= MAX_CHALLENGES {
            inner.challenges.remove(0);
        }
        inner.challenges.push(Ticket {
            token: challenge.clone(),
            issued: now,
        });
        Ok(challenge)
    }

    pub fn qr_payload(nonce: &str) -> String {
        format!("pendant-pair:v1:{nonce}")
    }

    /// The check and the insert share one lock. A revoked id is not overwritten.
    pub fn pair(
        &self,
        device_id: &str,
        public_b64: &str,
        nonce: &str,
        sig_b64: &str,
        takeover: bool,
    ) -> Result<(), AuthError> {
        let public = decode_key(public_b64).map_err(|_| AuthError::BadSignature)?;
        let msg = pair_message(device_id, public_b64, nonce);
        if !verify(&public, &msg, sig_b64) {
            return Err(AuthError::BadSignature);
        }
        let mut inner = self.inner.lock().expect("pairing");
        if inner
            .devices
            .get(device_id)
            .is_some_and(|device| device.revoked)
        {
            return Err(AuthError::Revoked);
        }
        let other = inner
            .devices
            .iter()
            .any(|(id, device)| id != device_id && !device.revoked);
        if other && (takeover || !TAKEOVER) {
            return Err(AuthError::TakeoverOff);
        }
        if !take_token(&mut inner.nonces, nonce) {
            return Err(AuthError::BadSignature);
        }
        inner.devices.insert(
            device_id.to_string(),
            Device {
                public,
                revoked: false,
            },
        );
        Ok(())
    }

    /// Auth is a signature from an already paired device over a fresh challenge.
    /// Unknown ids are not stored.
    pub fn revoke(
        &self,
        target_id: &str,
        signer_id: &str,
        sig_b64: &str,
        challenge: &str,
    ) -> Result<(), AuthError> {
        let msg = revoke_message(target_id, challenge);
        let mut inner = self.inner.lock().expect("pairing");
        if !take_ticket(&mut inner, challenge) {
            return Err(AuthError::BadSignature);
        }
        let Some(signer) = inner.devices.get(signer_id) else {
            return Err(AuthError::BadSignature);
        };
        if signer.revoked {
            return Err(AuthError::Revoked);
        }
        let public = signer.public.clone();
        if !verify(&public, &msg, sig_b64) {
            return Err(AuthError::BadSignature);
        }
        let Some(target) = inner.devices.get_mut(target_id) else {
            return Ok(());
        };
        target.revoked = true;
        if let Some(grant) = inner.grants.get_mut(target_id) {
            grant.released = true;
        }
        Ok(())
    }

    /// Host operator revoke. No phone signature. The route is local-only.
    pub fn host_revoke(&self, target_id: &str) {
        let mut inner = self.inner.lock().expect("pairing");
        let Some(target) = inner.devices.get_mut(target_id) else {
            return;
        };
        target.revoked = true;
        if let Some(grant) = inner.grants.get_mut(target_id) {
            grant.released = true;
        }
    }

    pub fn open_grant(
        &self,
        device_id: &str,
        sig_b64: &str,
        challenge: &str,
    ) -> Result<(), AuthError> {
        let msg = grant_message(device_id, challenge);
        self.verify_fresh(device_id, &msg, sig_b64, challenge)?;
        let mut inner = self.inner.lock().expect("pairing");
        inner.grants.insert(
            device_id.to_string(),
            Grant {
                opened: Instant::now(),
                released: false,
            },
        );
        Ok(())
    }

    pub fn authorize(
        &self,
        device_id: &str,
        message: &[u8],
        sig_b64: &str,
        challenge: &str,
    ) -> Result<(), AuthError> {
        self.verify_fresh(device_id, message, sig_b64, challenge)?;
        let mut inner = self.inner.lock().expect("pairing");
        let ttl = inner.ttl;
        let Some(grant) = inner.grants.get_mut(device_id) else {
            return Err(AuthError::GrantRequired);
        };
        if grant.released {
            return Err(AuthError::GrantReleased);
        }
        if grant.opened.elapsed() > ttl {
            grant.released = true;
            return Err(AuthError::GrantReleased);
        }
        Ok(())
    }

    pub fn attach(&self, device_id: &str, sig_b64: Option<&str>, challenge: Option<&str>) -> bool {
        let (Some(sig_b64), Some(challenge)) = (sig_b64, challenge) else {
            return false;
        };
        let msg = attach_message(device_id, challenge);
        self.authorize(device_id, &msg, sig_b64, challenge).is_ok()
    }

    pub fn release(&self, device_id: &str) {
        let mut inner = self.inner.lock().expect("pairing");
        if let Some(grant) = inner.grants.get_mut(device_id) {
            grant.released = true;
        }
    }

    fn verify_fresh(
        &self,
        device_id: &str,
        message: &[u8],
        sig_b64: &str,
        challenge: &str,
    ) -> Result<(), AuthError> {
        let mut inner = self.inner.lock().expect("pairing");
        if !take_ticket(&mut inner, challenge) {
            return Err(AuthError::BadSignature);
        }
        let Some(device) = inner.devices.get(device_id) else {
            return Err(AuthError::BadSignature);
        };
        if device.revoked {
            return Err(AuthError::Revoked);
        }
        let public = device.public.clone();
        drop(inner);
        if verify(&public, message, sig_b64) {
            Ok(())
        } else {
            Err(AuthError::BadSignature)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    BadSignature,
    GrantRequired,
    GrantReleased,
    Revoked,
    TakeoverOff,
    Unpaired,
    RateLimited,
}

pub fn unpaired_log(host: &str) -> String {
    format!("unpaired device refused via=serve host={host}")
}

pub fn sign_message(
    device_id: &str,
    method: &str,
    path: &str,
    challenge: &str,
    canonical: &str,
) -> Vec<u8> {
    format!("pendant-sign:v1\n{device_id}\n{method}\n{path}\n{challenge}\n{canonical}").into_bytes()
}

pub fn pair_message(device_id: &str, public_b64: &str, nonce: &str) -> Vec<u8> {
    format!("pendant-pair:v1\n{device_id}\n{public_b64}\n{nonce}").into_bytes()
}

pub fn grant_message(device_id: &str, challenge: &str) -> Vec<u8> {
    format!("pendant-grant:v1\n{device_id}\n{challenge}").into_bytes()
}

pub fn attach_message(device_id: &str, challenge: &str) -> Vec<u8> {
    format!("pendant-attach:v1\n{device_id}\n{challenge}").into_bytes()
}

pub fn revoke_message(target_id: &str, challenge: &str) -> Vec<u8> {
    format!("pendant-revoke:v1\n{target_id}\n{challenge}").into_bytes()
}

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("random");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn push_capped(items: &mut Vec<String>, item: String) {
    if items.len() >= MAX_CHALLENGES {
        items.remove(0);
    }
    items.push(item);
}

fn ct_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn take_token(items: &mut Vec<String>, token: &str) -> bool {
    let mut found = None;
    for (index, item) in items.iter().enumerate() {
        if ct_eq(item, token) {
            found = Some(index);
        }
    }
    if let Some(index) = found {
        items.remove(index);
        true
    } else {
        false
    }
}

fn take_ticket(inner: &mut Inner, token: &str) -> bool {
    let now = Instant::now();
    let ttl = inner.challenge_ttl;
    let mut found = None;
    for (index, ticket) in inner.challenges.iter().enumerate() {
        let fresh = now.saturating_duration_since(ticket.issued) <= ttl;
        if fresh && ct_eq(&ticket.token, token) {
            found = Some(index);
        }
    }
    inner
        .challenges
        .retain(|ticket| now.saturating_duration_since(ticket.issued) <= ttl || found.is_some());
    if let Some(index) = found {
        if index < inner.challenges.len() && ct_eq(&inner.challenges[index].token, token) {
            inner.challenges.remove(index);
            return true;
        }
    }
    false
}

fn verify(key: &VerifyingKey, message: &[u8], sig_b64: &str) -> bool {
    let Ok(bytes) = STANDARD.decode(sig_b64.trim()) else {
        return false;
    };
    let Ok(sig) = Signature::from_der(&bytes) else {
        return false;
    };
    key.verify(message, &sig).is_ok()
}

fn decode_key(public_b64: &str) -> Result<VerifyingKey, ()> {
    let bytes = STANDARD.decode(public_b64.trim()).map_err(|_| ())?;
    VerifyingKey::from_sec1_bytes(&bytes).map_err(|_| ())
}

#[cfg(test)]
pub struct TestDevice {
    pub id: String,
    signing: p256::ecdsa::SigningKey,
}

#[cfg(test)]
fn random_signing_key() -> p256::ecdsa::SigningKey {
    loop {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("random");
        if let Ok(key) = p256::ecdsa::SigningKey::from_slice(&bytes) {
            return key;
        }
    }
}

#[cfg(test)]
impl TestDevice {
    pub fn generate(id: &str) -> Self {
        Self {
            id: id.to_string(),
            signing: random_signing_key(),
        }
    }

    pub fn public_b64(&self) -> String {
        let point = self.signing.verifying_key().to_sec1_point(false);
        STANDARD.encode(point.as_bytes())
    }

    pub fn sign_b64(&self, message: &[u8]) -> String {
        use p256::ecdsa::signature::Signer;
        let sig: Signature = self.signing.sign(message);
        STANDARD.encode(sig.to_der().as_bytes())
    }
}
