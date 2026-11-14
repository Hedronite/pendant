// The Android device key: an Android Keystore P-256 key that signs only
// after a BiometricPrompt match, for every signature.
//
// build.rs compiles this file to dex for Android builds only. The library
// embeds that dex and loads it with InMemoryDexClassLoader
// (platform/keystore.rs); nothing is fetched at run time. nsh 0.5.0 has no
// hook for an app's own Java, and BiometricPrompt needs a subclass of its
// AuthenticationCallback, which JNI alone cannot make.
//
// Every method is static and called from a worker thread, never the UI
// thread: sign() blocks until the prompt ends.

package com.hedronite.pendant;

import android.content.Context;
import android.content.DialogInterface;
import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.os.CancellationSignal;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.Signature;
import java.security.cert.Certificate;
import java.security.spec.ECGenParameterSpec;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Executor;

public final class PendantKey {
    /** The error of the last failed call, read by the caller. 0: none. */
    public static volatile int lastError;

    /** A Keystore or Java failure, not a BiometricPrompt code. */
    public static final int ERROR_KEYSTORE = -1;
    /** No Application to show the prompt from. */
    public static final int ERROR_NO_CONTEXT = -2;
    /** The prompt ended without a result. */
    public static final int ERROR_INTERRUPTED = -3;
    /** The prompt's Cancel button: the framework's code 13, not public in its API. */
    public static final int ERROR_NEGATIVE_BUTTON = 13;

    private static final String STORE = "AndroidKeyStore";

    private PendantKey() {}

    private static KeyStore store() throws Exception {
        KeyStore store = KeyStore.getInstance(STORE);
        store.load(null);
        return store;
    }

    public static boolean exists(String alias) {
        lastError = 0;
        try {
            return store().containsAlias(alias);
        } catch (Exception e) {
            lastError = ERROR_KEYSTORE;
            return false;
        }
    }

    /** Makes the key. Each use needs a strong biometric (timeout 0). */
    public static boolean create(String alias) {
        lastError = 0;
        try {
            KeyGenParameterSpec spec = new KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN)
                    .setAlgorithmParameterSpec(new ECGenParameterSpec("secp256r1"))
                    .setDigests(KeyProperties.DIGEST_SHA256)
                    .setUserAuthenticationRequired(true)
                    .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
                    .setInvalidatedByBiometricEnrollment(true)
                    .build();
            KeyPairGenerator generator = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, STORE);
            generator.initialize(spec);
            generator.generateKeyPair();
            return true;
        } catch (Exception e) {
            lastError = ERROR_KEYSTORE;
            return false;
        }
    }

    public static boolean delete(String alias) {
        lastError = 0;
        try {
            store().deleteEntry(alias);
            return true;
        } catch (Exception e) {
            lastError = ERROR_KEYSTORE;
            return false;
        }
    }

    /** The public key as X.509 SubjectPublicKeyInfo DER, or null. */
    public static byte[] publicKey(String alias) {
        lastError = 0;
        try {
            Certificate certificate = store().getCertificate(alias);
            if (certificate == null) {
                lastError = ERROR_KEYSTORE;
                return null;
            }
            return certificate.getPublicKey().getEncoded();
        } catch (Exception e) {
            lastError = ERROR_KEYSTORE;
            return null;
        }
    }

    /**
     * Signs message (ECDSA over SHA-256, DER) after a BiometricPrompt
     * match, or returns null with lastError set: a BiometricPrompt error
     * code (user cancel 10, negative button 13, lockout 7 and 9, ...) or
     * one of this class's negative codes.
     */
    public static byte[] sign(String alias, byte[] message, String title) {
        lastError = 0;
        Context context = application();
        if (context == null) {
            lastError = ERROR_NO_CONTEXT;
            return null;
        }
        Signature signature;
        try {
            PrivateKey key = (PrivateKey) store().getKey(alias, null);
            signature = Signature.getInstance("SHA256withECDSA");
            signature.initSign(key);
        } catch (Exception e) {
            lastError = ERROR_KEYSTORE;
            return null;
        }

        final Outcome outcome = new Outcome();
        final Executor main = context.getMainExecutor();
        final CancellationSignal cancel = new CancellationSignal();
        final BiometricPrompt prompt = new BiometricPrompt.Builder(context)
                .setTitle(title)
                .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                .setNegativeButton("Cancel", main, new DialogInterface.OnClickListener() {
                    @Override
                    public void onClick(DialogInterface dialog, int which) {
                        outcome.finish(ERROR_NEGATIVE_BUTTON, null);
                    }
                })
                .build();
        final BiometricPrompt.CryptoObject crypto = new BiometricPrompt.CryptoObject(signature);
        main.execute(new Runnable() {
            @Override
            public void run() {
                prompt.authenticate(crypto, cancel, main, new Callback(outcome));
            }
        });

        try {
            outcome.done.await();
        } catch (InterruptedException e) {
            cancel.cancel();
            lastError = ERROR_INTERRUPTED;
            return null;
        }
        if (outcome.error != 0 || outcome.signature == null) {
            lastError = outcome.error != 0 ? outcome.error : ERROR_INTERRUPTED;
            return null;
        }
        try {
            outcome.signature.update(message);
            return outcome.signature.sign();
        } catch (Exception e) {
            lastError = ERROR_KEYSTORE;
            return null;
        }
    }

    /** The app's Application, found through the framework's ActivityThread. */
    private static Context application() {
        try {
            Object app = Class.forName("android.app.ActivityThread")
                    .getMethod("currentApplication")
                    .invoke(null);
            return (Context) app;
        } catch (Exception e) {
            return null;
        }
    }

    /** What the prompt answered. */
    private static final class Outcome {
        final CountDownLatch done = new CountDownLatch(1);
        volatile int error;
        volatile Signature signature;

        void finish(int error, Signature signature) {
            if (done.getCount() == 0) {
                return;
            }
            this.error = error;
            this.signature = signature;
            done.countDown();
        }
    }

    private static final class Callback extends BiometricPrompt.AuthenticationCallback {
        private final Outcome outcome;

        Callback(Outcome outcome) {
            this.outcome = outcome;
        }

        @Override
        public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult result) {
            BiometricPrompt.CryptoObject crypto = result.getCryptoObject();
            outcome.finish(0, crypto == null ? null : crypto.getSignature());
        }

        @Override
        public void onAuthenticationError(int code, CharSequence message) {
            outcome.finish(code == 0 ? ERROR_INTERRUPTED : code, null);
        }

        // onAuthenticationFailed: one non-matching try. The prompt stays
        // open for another, so nothing ends here.
    }
}
