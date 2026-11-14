# Pendant

Pendant is a phone-sized web app for the **herdr** session already running on your computer.

It is not a cloud product and not a remote terminal. Herdr stays the local brain. Pendant is a thin client: a loopback HTTP bridge plus a PWA you open on your phone.

## Run locally

You need a Rust toolchain (1.85+) and, for live data, herdr 0.9.x running on the same machine.

```bash
cargo run -p pendant
```

Then open [http://127.0.0.1:8780](http://127.0.0.1:8780).

| Variable | Default |
| --- | --- |
| `PENDANT_BIND` | `127.0.0.1:8780` |
| `PENDANT_HERDR_SOCK` | `~/.config/herdr/herdr.sock` |

The process refuses `PENDANT_BIND` unless the address is loopback. `0.0.0.0`, `::`, and any other non-loopback address are refused.

## Phone access (Tailscale Serve)

On the machine running herdr and Pendant, publish the loopback bridge on your tailnet:

```bash
tailscale serve --bg --https=8443 http://127.0.0.1:8780
```

Pendant HTTPS is port `8443`, proxying `http://127.0.0.1:8780`. Port `443` is not the Pendant route. Set `PENDANT_SERVE_ORIGIN` to the exact origin a browser sends on that route. Leaving it unset makes those requests `403`.

Only Serve's HTTP-proxy mode is supported, with exactly that command. The local-only gate on pairing trusts the forwarding header Serve adds, so a raw TCP forward, a TLS-terminated TCP forward, or any other proxy that does not add `X-Forwarded-For` defeats the gate. Do not use one.

## Phone app (built on a Mac)

The phone app lives in `app/`: an [nsh](https://github.com/longbridge/nsh) shell over the bridge, its own Cargo workspace. It is built and run on a Mac with Xcode, never on the machine serving herdr.

**Toolchain:**
- nsh CLI 0.5.0, installed under `~/.nsh` with `nsh` on `PATH`.
- Xcode with an iOS simulator runtime.
- Rust targets `aarch64-apple-ios`, `aarch64-apple-ios-sim` and `aarch64-linux-android`.
- Android SDK (platform-tools, build-tools, emulator), NDK r27 and JDK 17. `ANDROID_HOME` and `JAVA_HOME` must be set.

**Build.** The app learns its bridge at build time from your Serve origin:

```bash
cd app
export PENDANT_BRIDGE_URL=https://<your-host>.<tailnet>.ts.net:8443
export PENDANT_BUILD_SHA=$(git rev-parse --short HEAD)   # optional: logged as "build <sha>" at start
nsh package --target ios-simulator
nsh package --target android --abi arm64-v8a
```

**Android `USE_BIOMETRIC` step.** It is required before installing:

```bash
python3 android/add-biometric-permission.py .nsh/out/Pendant.apk
```

The device key asks for BiometricPrompt on every signature, and nsh 0.5.0 has no way to add a manifest permission. This step:
- adds exactly one `<uses-permission android:name="android.permission.USE_BIOMETRIC"/>` to the packaged manifest;
- realigns the APK and re-signs it with the debug key nsh used (`~/.android/debug.keystore`), using SDK build-tools only;
- refuses to finish unless `aapt2 dump xmltree` shows that permission as the only change.

The device key's Java (`app/android/*.java`) is compiled to dex by `app/build.rs` for Android builds only, from source; nothing is fetched or prebuilt.

**Install and pair:**

```bash
xcrun simctl install booted .nsh/out/Pendant.app
adb install -r .nsh/out/Pendant.apk
```

- Pair by opening the bridge's `pendant://pair?code=…` link on the device. The payload is a one-time secret: do not log it, paste it into chat or commit it.
- Each pairing makes a new device key.
- A simulator or emulator biometric is fine for development. It is never evidence of a real pairing.

**D1 folder probe.** `pendant://d1` opens a page that runs the phone's own folder picker (iOS `UIDocumentPickerViewController`, Android `ACTION_OPEN_DOCUMENT_TREE`). It counts the `.md` files the app can read and reports whether access survives a restart. `app/tools/d1-fixture.sh DIR` writes a made-up fixture to try it on (25 top-level notes, 3 other files, one nested folder of 5). The probe links nothing.

## Security

- Tailnet-only. Pendant listens on loopback. Reach it through Tailscale Serve, not the public internet.
- No WAN bind. Do not put Pendant behind a port-forward or bind it to `0.0.0.0`.
- Anyone who can open the PWA can prompt agents, send keys, and create worktrees. Treat an unlocked phone on your tailnet as operator access.

## Workspace

| Crate | Role |
| --- | --- |
| `pendant` | Binary: runs the bridge and serves the PWA |
| `pendant-bridge` | Loopback HTTP (`/v1/…`) |
| `pendant-herdr` | herdr Unix-socket client (CLI JSON fallback) |
| `pendant-web` | Static PWA assets |

This tree is version `0.0.1` to match the crates.io name reserve. Packages are `publish = false`. Bump past `0.0.1` when a real crate ships.

## License

MIT. Third-party notices, including nsh (Apache-2.0), are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
