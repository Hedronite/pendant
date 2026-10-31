//! The phone's device key for pairing.
//!
//! The key is made on the phone and never leaves it. On iOS it is a P-256 key
//! in the Secure Enclave, stored in the Keychain under [`TAG`], usable only
//! while the device is unlocked, never synced or backed up, and each signature
//! needs a biometric match from the currently enrolled set. Nothing here
//! writes the key to a file, and nothing ships one: the repo holds no key.
//!
//! The wire format the bridge verifies is [`ALGORITHM`]:
//! - public key: the SEC1 uncompressed point, 65 bytes, `0x04 || X || Y`;
//! - signature: ECDSA over SHA-256 of the message, ASN.1 DER (X9.62).
//!
//! Android Keystore is not reachable from here. Its key generation and
//! `BiometricPrompt` are Java APIs that need the app's `Context` and a
//! foreground `Activity`, and there is no app shell yet to own them, so every
//! call on Android returns [`KeyError::Unsupported`].

use std::fmt;

/// The algorithm a device signature uses, as the bridge names it.
pub const ALGORITHM: &str = "ecdsa-p256-sha256";

/// The Keychain application tag the device key is stored under.
pub const TAG: &str = "com.hedronite.pendant.device-key";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// A device key already exists. It is not replaced: delete it first.
    Exists,
    /// No device key is stored.
    NotFound,
    /// The person dismissed the biometric prompt.
    Cancelled,
    /// The biometric did not match, or none is enrolled.
    AuthFailed,
    /// The platform refused, with its status code.
    Os(i64),
    /// This platform has no device key here.
    Unsupported(&'static str),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::Exists => write!(f, "a device key already exists"),
            KeyError::NotFound => write!(f, "no device key"),
            KeyError::Cancelled => write!(f, "biometric cancelled"),
            KeyError::AuthFailed => write!(f, "biometric failed"),
            KeyError::Os(code) => write!(f, "keystore status {code}"),
            KeyError::Unsupported(why) => write!(f, "no device key on this platform: {why}"),
        }
    }
}

impl std::error::Error for KeyError {}

/// The device key, by reference. The private half stays in the platform.
pub struct DeviceKey {
    public_key: Vec<u8>,
    key: imp::Key,
}

impl DeviceKey {
    /// Make the device key and store it. Refuses with [`KeyError::Exists`]
    /// when one is already stored, so a paired key is never swapped silently.
    pub fn create() -> Result<DeviceKey, KeyError> {
        if imp::Key::stored(None)?.is_some() {
            return Err(KeyError::Exists);
        }
        DeviceKey::wrap(imp::Key::generate(imp::Protection::Device)?)
    }

    /// The stored device key, if there is one. Reading it needs no biometric.
    pub fn load() -> Result<Option<DeviceKey>, KeyError> {
        imp::Key::stored(None)?.map(DeviceKey::wrap).transpose()
    }

    /// Remove the stored device key. A key that is not there is not an error.
    pub fn delete() -> Result<(), KeyError> {
        imp::Key::delete()
    }

    /// The public key to pair with, in the SEC1 uncompressed form.
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// Sign `message`. The platform shows the biometric prompt with `reason`
    /// and signs only on a match, on the phone, while the app is in front.
    pub fn sign(&self, message: &[u8], reason: &str) -> Result<Vec<u8>, KeyError> {
        self.key.sign(message, reason)
    }

    fn wrap(key: imp::Key) -> Result<DeviceKey, KeyError> {
        Ok(DeviceKey {
            public_key: key.public_key()?,
            key,
        })
    }
}

#[cfg(target_vendor = "apple")]
mod imp {
    //! Security.framework through its C API. Every CoreFoundation object
    //! that a `Create` or `Copy` call returns is owned by a [`Cf`], which
    //! releases it once.

    use super::{KeyError, TAG};
    use std::ffi::c_void;
    use std::ptr;

    type CFTypeRef = *const c_void;
    type CFIndex = isize;
    type OSStatus = i32;

    #[repr(C)]
    struct Opaque {
        _private: [u8; 0],
    }

    const UTF8: u32 = 0x0800_0100;
    const SINT32: CFIndex = 3;
    const BIOMETRY_CURRENT_SET: usize = 1 << 3;
    const PRIVATE_KEY_USAGE: usize = 1 << 30;
    const ERR_ITEM_NOT_FOUND: OSStatus = -25300;
    const ERR_USER_CANCELED: i64 = -128;
    const ERR_AUTH_FAILED: i64 = -25293;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFTypeDictionaryKeyCallBacks: Opaque;
        static kCFTypeDictionaryValueCallBacks: Opaque;
        static kCFBooleanTrue: CFTypeRef;
        fn CFDictionaryCreateMutable(
            alloc: CFTypeRef,
            capacity: CFIndex,
            keys: *const Opaque,
            values: *const Opaque,
        ) -> CFTypeRef;
        fn CFDictionarySetValue(dict: CFTypeRef, key: CFTypeRef, value: CFTypeRef);
        fn CFDataCreate(alloc: CFTypeRef, bytes: *const u8, len: CFIndex) -> CFTypeRef;
        fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
        fn CFDataGetLength(data: CFTypeRef) -> CFIndex;
        fn CFStringCreateWithBytes(
            alloc: CFTypeRef,
            bytes: *const u8,
            len: CFIndex,
            encoding: u32,
            external: u8,
        ) -> CFTypeRef;
        fn CFNumberCreate(alloc: CFTypeRef, kind: CFIndex, value: *const c_void) -> CFTypeRef;
        fn CFErrorGetCode(err: CFTypeRef) -> CFIndex;
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecClass: CFTypeRef;
        static kSecClassKey: CFTypeRef;
        static kSecAttrKeyType: CFTypeRef;
        static kSecAttrKeyTypeECSECPrimeRandom: CFTypeRef;
        static kSecAttrKeySizeInBits: CFTypeRef;
        static kSecAttrKeyClass: CFTypeRef;
        static kSecAttrKeyClassPrivate: CFTypeRef;
        static kSecAttrTokenID: CFTypeRef;
        static kSecAttrTokenIDSecureEnclave: CFTypeRef;
        static kSecAttrIsPermanent: CFTypeRef;
        static kSecAttrApplicationTag: CFTypeRef;
        static kSecAttrAccessControl: CFTypeRef;
        static kSecAttrAccessibleWhenUnlockedThisDeviceOnly: CFTypeRef;
        static kSecPrivateKeyAttrs: CFTypeRef;
        static kSecReturnRef: CFTypeRef;
        static kSecUseOperationPrompt: CFTypeRef;
        static kSecUseDataProtectionKeychain: CFTypeRef;
        static kSecKeyAlgorithmECDSASignatureMessageX962SHA256: CFTypeRef;
        fn SecAccessControlCreateWithFlags(
            alloc: CFTypeRef,
            protection: CFTypeRef,
            flags: usize,
            error: *mut CFTypeRef,
        ) -> CFTypeRef;
        fn SecKeyCreateRandomKey(params: CFTypeRef, error: *mut CFTypeRef) -> CFTypeRef;
        fn SecKeyCopyPublicKey(key: CFTypeRef) -> CFTypeRef;
        fn SecKeyCopyExternalRepresentation(key: CFTypeRef, error: *mut CFTypeRef) -> CFTypeRef;
        fn SecKeyCreateSignature(
            key: CFTypeRef,
            algorithm: CFTypeRef,
            data: CFTypeRef,
            error: *mut CFTypeRef,
        ) -> CFTypeRef;
        fn SecItemCopyMatching(query: CFTypeRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemDelete(query: CFTypeRef) -> OSStatus;
    }

    #[cfg(test)]
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanFalse: CFTypeRef;
    }

    #[cfg(test)]
    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecAttrKeyClassPublic: CFTypeRef;
        fn SecKeyCreateWithData(
            data: CFTypeRef,
            attrs: CFTypeRef,
            error: *mut CFTypeRef,
        ) -> CFTypeRef;
        fn SecKeyVerifySignature(
            key: CFTypeRef,
            algorithm: CFTypeRef,
            data: CFTypeRef,
            signature: CFTypeRef,
            error: *mut CFTypeRef,
        ) -> u8;
    }

    /// An owned CoreFoundation object.
    pub(super) struct Cf(CFTypeRef);

    impl Cf {
        /// Owns `p`, the result of a `Create` or `Copy` call; null is `None`.
        fn owned(p: CFTypeRef) -> Option<Cf> {
            (!p.is_null()).then_some(Cf(p))
        }

        fn as_ptr(&self) -> CFTypeRef {
            self.0
        }

        fn bytes(&self) -> Vec<u8> {
            // SAFETY: `self` owns a CFData (every caller builds it from a
            // `Copy`/`Create` call that returns CFData), so the pointer and
            // length describe one live buffer for as long as `self` lives.
            unsafe {
                let len = CFDataGetLength(self.0);
                std::slice::from_raw_parts(CFDataGetBytePtr(self.0), len as usize).to_vec()
            }
        }
    }

    impl Drop for Cf {
        fn drop(&mut self) {
            // SAFETY: `Cf` is only built from a non-null owned reference, and
            // it is released exactly once, here.
            unsafe { CFRelease(self.0) }
        }
    }

    // SAFETY: CoreFoundation and Security objects are reference counted with
    // atomic counts and are immutable once made, so a reference can move to
    // another thread.
    unsafe impl Send for Cf {}

    fn data(bytes: &[u8]) -> Cf {
        // SAFETY: `bytes` is a live slice; CFDataCreate copies it.
        Cf::owned(unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as CFIndex) })
            .expect("CFDataCreate")
    }

    fn string(s: &str) -> Cf {
        // SAFETY: `s` is live UTF-8 and is copied, not borrowed (external = 0).
        Cf::owned(unsafe {
            CFStringCreateWithBytes(ptr::null(), s.as_ptr(), s.len() as CFIndex, UTF8, 0)
        })
        .expect("CFStringCreateWithBytes")
    }

    fn number(n: i32) -> Cf {
        // SAFETY: `n` is a live i32, read once as kCFNumberSInt32Type.
        Cf::owned(unsafe { CFNumberCreate(ptr::null(), SINT32, (&n as *const i32).cast()) })
            .expect("CFNumberCreate")
    }

    /// A CFDictionary that retains its keys and values.
    fn dict(pairs: &[(CFTypeRef, CFTypeRef)]) -> Cf {
        // SAFETY: the callbacks are CoreFoundation's own CFType callbacks, so
        // the dictionary retains what it is given. Every key and value is a
        // live CF object: a Security constant or one owned by the caller.
        unsafe {
            let d = Cf::owned(CFDictionaryCreateMutable(
                ptr::null(),
                pairs.len() as CFIndex,
                &raw const kCFTypeDictionaryKeyCallBacks,
                &raw const kCFTypeDictionaryValueCallBacks,
            ))
            .expect("CFDictionaryCreateMutable");
            for (k, v) in pairs {
                CFDictionarySetValue(d.as_ptr(), *k, *v);
            }
            d
        }
    }

    /// The code of an owned CFError, which is then released.
    fn error(err: CFTypeRef) -> KeyError {
        let Some(err) = Cf::owned(err) else {
            return KeyError::Os(0);
        };
        // SAFETY: `err` is a live CFError that `err` owns.
        match unsafe { CFErrorGetCode(err.as_ptr()) } as i64 {
            ERR_USER_CANCELED => KeyError::Cancelled,
            ERR_AUTH_FAILED => KeyError::AuthFailed,
            code => KeyError::Os(code),
        }
    }

    pub(super) enum Protection {
        /// Secure Enclave, stored in the Keychain, biometric to sign.
        Device,
        /// A software key held in memory only, so the FFI can run on a host
        /// without a Secure Enclave entitlement. Never shipped.
        #[cfg(test)]
        Memory,
    }

    impl Protection {
        fn held(&self) -> bool {
            match self {
                Protection::Device => false,
                #[cfg(test)]
                Protection::Memory => true,
            }
        }
    }

    pub(super) struct Key {
        /// Held for a memory key. A stored key is fetched again on each
        /// signature, so the prompt carries that signature's reason.
        held: Option<Cf>,
    }

    impl Key {
        pub(super) fn generate(protection: Protection) -> Result<Key, KeyError> {
            let tag = data(TAG.as_bytes());
            let bits = number(256);
            let mut err: CFTypeRef = ptr::null();
            // SAFETY: the Security constants are read-only CFStrings that live
            // for the process. Every dictionary entry is a live CF object;
            // `err` is a valid out slot and, when set, is owned by `error`.
            unsafe {
                let params = match protection {
                    Protection::Device => {
                        let access = Cf::owned(SecAccessControlCreateWithFlags(
                            ptr::null(),
                            kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
                            PRIVATE_KEY_USAGE | BIOMETRY_CURRENT_SET,
                            &mut err,
                        ))
                        .ok_or_else(|| error(err))?;
                        let private = dict(&[
                            (kSecAttrIsPermanent, kCFBooleanTrue),
                            (kSecAttrApplicationTag, tag.as_ptr()),
                            (kSecAttrAccessControl, access.as_ptr()),
                        ]);
                        dict(&[
                            (kSecAttrKeyType, kSecAttrKeyTypeECSECPrimeRandom),
                            (kSecAttrKeySizeInBits, bits.as_ptr()),
                            (kSecAttrTokenID, kSecAttrTokenIDSecureEnclave),
                            (kSecUseDataProtectionKeychain, kCFBooleanTrue),
                            (kSecPrivateKeyAttrs, private.as_ptr()),
                        ])
                    }
                    #[cfg(test)]
                    Protection::Memory => {
                        let private = dict(&[(kSecAttrIsPermanent, kCFBooleanFalse)]);
                        dict(&[
                            (kSecAttrKeyType, kSecAttrKeyTypeECSECPrimeRandom),
                            (kSecAttrKeySizeInBits, bits.as_ptr()),
                            (kSecPrivateKeyAttrs, private.as_ptr()),
                        ])
                    }
                };
                let key = Cf::owned(SecKeyCreateRandomKey(params.as_ptr(), &mut err))
                    .ok_or_else(|| error(err))?;
                Ok(Key {
                    held: protection.held().then_some(key),
                })
            }
        }

        /// The stored private key. With a `reason`, a signature made with the
        /// returned reference prompts with it.
        fn copy_stored(reason: Option<&str>) -> Result<Option<Cf>, KeyError> {
            let tag = data(TAG.as_bytes());
            let reason = reason.map(string);
            // SAFETY: as in `generate`; `found` is a valid out slot that
            // SecItemCopyMatching fills with an owned SecKey on success.
            unsafe {
                let mut pairs = vec![
                    (kSecClass, kSecClassKey),
                    (kSecAttrApplicationTag, tag.as_ptr()),
                    (kSecAttrKeyType, kSecAttrKeyTypeECSECPrimeRandom),
                    (kSecAttrKeyClass, kSecAttrKeyClassPrivate),
                    (kSecUseDataProtectionKeychain, kCFBooleanTrue),
                    (kSecReturnRef, kCFBooleanTrue),
                ];
                if let Some(r) = &reason {
                    pairs.push((kSecUseOperationPrompt, r.as_ptr()));
                }
                let query = dict(&pairs);
                let mut found: CFTypeRef = ptr::null();
                match SecItemCopyMatching(query.as_ptr(), &mut found) {
                    0 => Ok(Cf::owned(found)),
                    ERR_ITEM_NOT_FOUND => Ok(None),
                    status => Err(KeyError::Os(status.into())),
                }
            }
        }

        pub(super) fn stored(reason: Option<&str>) -> Result<Option<Key>, KeyError> {
            Ok(Key::copy_stored(reason)?.map(|_| Key { held: None }))
        }

        pub(super) fn delete() -> Result<(), KeyError> {
            let tag = data(TAG.as_bytes());
            // SAFETY: as in `generate`.
            let status = unsafe {
                let query = dict(&[
                    (kSecClass, kSecClassKey),
                    (kSecAttrApplicationTag, tag.as_ptr()),
                    (kSecUseDataProtectionKeychain, kCFBooleanTrue),
                ]);
                SecItemDelete(query.as_ptr())
            };
            match status {
                0 | ERR_ITEM_NOT_FOUND => Ok(()),
                status => Err(KeyError::Os(status.into())),
            }
        }

        fn private(&self, reason: Option<&str>) -> Result<Cf, KeyError> {
            match &self.held {
                // SAFETY: retaining a live SecKey before a second owner
                // takes it; the copy below is a fresh owned reference.
                Some(key) => Ok(Cf(unsafe { CFRetain(key.as_ptr()) })),
                None => Key::copy_stored(reason)?.ok_or(KeyError::NotFound),
            }
        }

        pub(super) fn public_key(&self) -> Result<Vec<u8>, KeyError> {
            let private = self.private(None)?;
            let mut err: CFTypeRef = ptr::null();
            // SAFETY: `private` is a live SecKey. Copying its public half
            // needs no biometric; both results are owned here.
            unsafe {
                let public =
                    Cf::owned(SecKeyCopyPublicKey(private.as_ptr())).ok_or(KeyError::Os(0))?;
                let bytes = Cf::owned(SecKeyCopyExternalRepresentation(public.as_ptr(), &mut err))
                    .ok_or_else(|| error(err))?;
                Ok(bytes.bytes())
            }
        }

        pub(super) fn sign(&self, message: &[u8], reason: &str) -> Result<Vec<u8>, KeyError> {
            let private = self.private(Some(reason))?;
            let message = data(message);
            let mut err: CFTypeRef = ptr::null();
            // SAFETY: a live SecKey, a Security algorithm constant and a live
            // CFData. For a stored key the platform shows the biometric prompt
            // and returns null with an owned CFError when it is not met.
            unsafe {
                let signature = Cf::owned(SecKeyCreateSignature(
                    private.as_ptr(),
                    kSecKeyAlgorithmECDSASignatureMessageX962SHA256,
                    message.as_ptr(),
                    &mut err,
                ))
                .ok_or_else(|| error(err))?;
                Ok(signature.bytes())
            }
        }
    }

    /// Whether `signature` over `message` verifies under the SEC1
    /// `public_key`, checked by Security itself.
    #[cfg(test)]
    pub(super) fn verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
        let (key_data, message, signature) = (data(public_key), data(message), data(signature));
        let bits = number(256);
        let mut err: CFTypeRef = ptr::null();
        // SAFETY: as in `generate`; every argument is a live CF object.
        unsafe {
            let attrs = dict(&[
                (kSecAttrKeyType, kSecAttrKeyTypeECSECPrimeRandom),
                (kSecAttrKeyClass, kSecAttrKeyClassPublic),
                (kSecAttrKeySizeInBits, bits.as_ptr()),
            ]);
            let Some(public) = Cf::owned(SecKeyCreateWithData(
                key_data.as_ptr(),
                attrs.as_ptr(),
                &mut err,
            )) else {
                let _ = error(err);
                return false;
            };
            let ok = SecKeyVerifySignature(
                public.as_ptr(),
                kSecKeyAlgorithmECDSASignatureMessageX962SHA256,
                message.as_ptr(),
                signature.as_ptr(),
                &mut err,
            ) != 0;
            if !ok {
                let _ = error(err);
            }
            ok
        }
    }
}

#[cfg(not(target_vendor = "apple"))]
mod imp {
    use super::KeyError;

    #[cfg(target_os = "android")]
    const WHY: &str = "Android Keystore and BiometricPrompt need the app shell";
    #[cfg(not(target_os = "android"))]
    const WHY: &str = "not a phone";

    pub(super) enum Protection {
        Device,
    }

    pub(super) struct Key;

    impl Key {
        pub(super) fn generate(_: Protection) -> Result<Key, KeyError> {
            Err(KeyError::Unsupported(WHY))
        }

        pub(super) fn stored(_: Option<&str>) -> Result<Option<Key>, KeyError> {
            Err(KeyError::Unsupported(WHY))
        }

        pub(super) fn delete() -> Result<(), KeyError> {
            Err(KeyError::Unsupported(WHY))
        }

        pub(super) fn public_key(&self) -> Result<Vec<u8>, KeyError> {
            Err(KeyError::Unsupported(WHY))
        }

        pub(super) fn sign(&self, _: &[u8], _: &str) -> Result<Vec<u8>, KeyError> {
            Err(KeyError::Unsupported(WHY))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire format the bridge verifies: a 65-byte SEC1 point and a DER
    /// ECDSA signature that Security accepts for the message and refuses for
    /// any other. A memory key, since a host test has no Secure Enclave.
    #[cfg(target_vendor = "apple")]
    #[test]
    fn a_signature_verifies_under_the_exported_public_key() {
        let key = DeviceKey::wrap(imp::Key::generate(imp::Protection::Memory).unwrap()).unwrap();
        let public = key.public_key();
        assert_eq!(public.len(), 65);
        assert_eq!(public[0], 0x04, "uncompressed point");

        let message = b"POST /v1/agents/a1/prompt\n2026-10-30T12:00:00Z\nnonce";
        let signature = key.sign(message, "Approve the prompt").unwrap();
        assert_eq!(signature[0], 0x30, "DER SEQUENCE");
        assert!(imp::verify(public, message, &signature));
        assert!(!imp::verify(
            public,
            b"POST /v1/agents/a2/prompt",
            &signature
        ));

        let other = DeviceKey::wrap(imp::Key::generate(imp::Protection::Memory).unwrap()).unwrap();
        assert_ne!(other.public_key(), public, "each key is new");
        assert!(!imp::verify(other.public_key(), message, &signature));
    }

    /// The device protection reaches the Secure Enclave on a phone. Run on
    /// the device with `--ignored`; a host without the Keychain entitlement
    /// gets a status code back, not a key and not a crash.
    #[test]
    #[ignore = "needs a phone with a Secure Enclave and an enrolled biometric"]
    fn a_device_key_is_made_in_the_keychain() {
        DeviceKey::delete().unwrap();
        let key = DeviceKey::create().unwrap();
        assert_eq!(key.public_key().len(), 65);
        assert!(matches!(DeviceKey::create(), Err(KeyError::Exists)));
        let loaded = DeviceKey::load().unwrap().expect("stored");
        assert_eq!(loaded.public_key(), key.public_key());
        DeviceKey::delete().unwrap();
        assert!(DeviceKey::load().unwrap().is_none());
    }

    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn other_platforms_say_unsupported() {
        assert!(matches!(DeviceKey::create(), Err(KeyError::Unsupported(_))));
        assert!(matches!(DeviceKey::load(), Err(KeyError::Unsupported(_))));
    }
}
