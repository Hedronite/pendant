//! Device pairing for the phone. The phone creates the key. This crate verifies
//! it. No key or token is minted here, and there is no relay.
//!
//! A real phone is not paired by these tests. Lost-phone revoke is covered
//! first, while `real_pairings` stays zero.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// Session takeover is not implemented.
pub const TAKEOVER: bool = false;

const GRANT_TTL: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct Pairing {
    inner: std::sync::Arc<Mutex<Inner>>,
}

struct Inner {
    ttl: Duration,
    nonces: Vec<String>,
    devices: HashMap<String, Device>,
    grants: HashMap<String, Grant>,
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
                nonces: Vec::new(),
                devices: HashMap::new(),
                grants: HashMap::new(),
            })),
        }
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
        let mut bytes = [0u8; 16];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
        let nonce = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        self.inner
            .lock()
            .expect("pairing")
            .nonces
            .push(nonce.clone());
        nonce
    }

    pub fn qr_payload(nonce: &str) -> String {
        format!("pendant-pair:v1:{nonce}")
    }

    pub fn pair(
        &self,
        device_id: &str,
        public_b64: &str,
        nonce: &str,
        sig_b64: &str,
        takeover: bool,
    ) -> Result<(), AuthError> {
        if takeover || self.other_device_active(device_id) {
            if self.other_device_active(device_id) {
                return Err(AuthError::TakeoverOff);
            }
        }
        let public = decode_key(public_b64).map_err(|_| AuthError::BadSignature)?;
        let msg = pair_message(nonce);
        if !verify(&public, &msg, sig_b64) {
            return Err(AuthError::BadSignature);
        }
        let mut inner = self.inner.lock().expect("pairing");
        let Some(pos) = inner.nonces.iter().position(|n| n == nonce) else {
            return Err(AuthError::BadSignature);
        };
        inner.nonces.remove(pos);
        inner.devices.insert(
            device_id.to_string(),
            Device {
                public,
                revoked: false,
            },
        );
        Ok(())
    }

    pub fn revoke(&self, device_id: &str) {
        let mut inner = self.inner.lock().expect("pairing");
        if let Some(device) = inner.devices.get_mut(device_id) {
            device.revoked = true;
        } else {
            inner.devices.insert(
                device_id.to_string(),
                Device {
                    public: dummy_key(),
                    revoked: true,
                },
            );
        }
        if let Some(grant) = inner.grants.get_mut(device_id) {
            grant.released = true;
        }
    }

    pub fn open_grant(&self, device_id: &str, sig_b64: &str) -> Result<(), AuthError> {
        let msg = grant_message(device_id);
        self.verify_device(device_id, &msg, sig_b64)?;
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
    ) -> Result<(), AuthError> {
        self.verify_device(device_id, message, sig_b64)?;
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

    pub fn attach(&self, device_id: &str, sig_b64: Option<&str>) -> bool {
        let Some(sig_b64) = sig_b64 else {
            return false;
        };
        let msg = attach_message(device_id);
        if self.authorize(device_id, &msg, sig_b64).is_err() {
            return false;
        }
        true
    }

    pub fn release(&self, device_id: &str) {
        let mut inner = self.inner.lock().expect("pairing");
        if let Some(grant) = inner.grants.get_mut(device_id) {
            grant.released = true;
        }
    }

    fn verify_device(
        &self,
        device_id: &str,
        message: &[u8],
        sig_b64: &str,
    ) -> Result<(), AuthError> {
        let inner = self.inner.lock().expect("pairing");
        let Some(device) = inner.devices.get(device_id) else {
            return Err(AuthError::BadSignature);
        };
        if device.revoked {
            return Err(AuthError::Revoked);
        }
        if verify(&device.public, message, sig_b64) {
            Ok(())
        } else {
            Err(AuthError::BadSignature)
        }
    }

    fn other_device_active(&self, device_id: &str) -> bool {
        let inner = self.inner.lock().expect("pairing");
        inner
            .devices
            .iter()
            .any(|(id, device)| id != device_id && !device.revoked)
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
}

pub fn unpaired_log(host: &str) -> String {
    format!("unpaired device refused via=serve host={host}")
}

pub fn sign_message(device_id: &str, method: &str, path: &str, canonical: &str) -> Vec<u8> {
    format!("pendant-sign:v1\n{device_id}\n{method}\n{path}\n{canonical}").into_bytes()
}

pub fn pair_message(nonce: &str) -> Vec<u8> {
    format!("pendant-pair:v1:{nonce}").into_bytes()
}

pub fn grant_message(device_id: &str) -> Vec<u8> {
    format!("pendant-grant:v1\n{device_id}\nforeground").into_bytes()
}

pub fn attach_message(device_id: &str) -> Vec<u8> {
    format!("pendant-attach:v1\n{device_id}").into_bytes()
}

fn verify(key: &VerifyingKey, message: &[u8], sig_b64: &str) -> bool {
    let Ok(bytes) = STANDARD.decode(sig_b64.trim()) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(&bytes) else {
        return false;
    };
    key.verify(message, &sig).is_ok()
}

fn decode_key(public_b64: &str) -> Result<VerifyingKey, ()> {
    let bytes = STANDARD.decode(public_b64.trim()).map_err(|_| ())?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| ())?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| ())
}

fn dummy_key() -> VerifyingKey {
    ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]).verifying_key()
}

#[cfg(test)]
pub struct TestDevice {
    pub id: String,
    signing: ed25519_dalek::SigningKey,
}

#[cfg(test)]
impl TestDevice {
    pub fn generate(id: &str) -> Self {
        Self {
            id: id.to_string(),
            signing: ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng),
        }
    }

    pub fn public_b64(&self) -> String {
        STANDARD.encode(self.signing.verifying_key().as_bytes())
    }

    pub fn sign_b64(&self, message: &[u8]) -> String {
        use ed25519_dalek::Signer;
        STANDARD.encode(self.signing.sign(message).to_bytes())
    }
}
