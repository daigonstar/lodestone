# Lodestone Beacon for Android (first version)

Your phone holds your Beacon identity and approves new PCs by scanning the QR code they show. It is a Tauri 2 app:
Rust (`src-tauri/`, using the same `beacon/client` crate as the desktop app) and a plain HTML/JS interface (`ui/`, no
build step). Plan and reasons: [`../MOBILE-PLAN.md`](../MOBILE-PLAN.md).

**Status: first version, tested only by compiling and by the server and client tests. It has not been run on a
phone yet.** Chat, voice and notifications are not in it.

## What it does

- Make a new identity (24 recovery words) or restore one from its words.
- Add a server: register with an invite or setup code, or get into the account you already have there (your
  identity proves it, no approval needed).
- Approve a PC: on the PC choose **Sign in with my phone** (first-run screen, or your name, Devices, Sign in with
  another device), enter the server's domain, scan the QR code with the phone, compare the fingerprint on both
  screens, tap Approve. The PC is added to your account and receives your identity, sealed so the server can't read
  it (see "Adding a PC by scanning its QR code with your phone" in `../PROTOCOL.md`).

## What it needs and stores

- **Camera**, only on the scan screen, to read the QR code. Nothing is recorded or sent.
- **Internet**, only to the Beacon servers you add. No analytics, no crash reports, no Google or other services.
- The identity key and each server's device key are in the app's private storage. **Not yet in the Android
  Keystore** (planned). Android backups of the app are turned off, so the key isn't copied to the cloud.
- Servers must have a certificate from a public authority (Mozilla's list is built in). Certificates you installed on
  the phone are not used.

## Build

Needs Rust, the Android SDK and NDK (r27), JDK 17 or newer and `cargo install tauri-cli --version "^2"`.

    cd beacon/mobile/src-tauri
    export ANDROID_HOME=… NDK_HOME=… JAVA_HOME=…
    cargo tauri android build --apk --target aarch64     # arm64 phones

The result is `gen/android/app/build/outputs/apk/universal/release/*.apk`. Release builds are unsigned: sign them with
your own key (`apksigner`). The Gradle build downloads Android libraries from Google's and Maven Central's servers
(build time only, not part of the app).

## For F-Droid

`fdroid/io.github.daigonstar.lodestone.beacon.yml` is a draft of the metadata F-Droid needs. It has not been
submitted or checked by F-Droid. Open points: whether F-Droid's build servers can build the Tauri Android project
from source, and the `rustls`/Gradle dependency review.

`ui/jsQR.js` is the unmodified `jsQR` 1.4.0 QR reader (Apache-2.0, licence in `ui/jsQR.LICENSE`), a readable
(not minified) build.
