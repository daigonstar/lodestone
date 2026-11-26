# Lodestone Beacon on phones: plan

Status: **proposal, nothing built.** Written 2026-11-26 for Robb to choose from. It was written by reading the
`comms` branch (client, server, media, app, `PROTOCOL.md`) and PR #10 (identity and QR pairing). Nothing here
was run on a phone, and the store rules are from my knowledge of them plus a quick search: **check each
store's current page when you sign up**, they change. Where I'm unsure I say so.

## The goal

A Beacon app for Android (Google Play) and iOS (App Store). The phone is:

1. A full Beacon device: text chat, files, reactions, later voice, cameras and screen sharing.
2. An authenticator: it holds your identity and approves a new PC by scanning the QR code that PC shows.

## What exists today, and how much a phone can reuse

| Part | Today | On a phone |
| --- | --- | --- |
| `beacon/client` (MLS, device keys, identity, server calls, SQLite store, AES-GCM files) | Rust, `openmls`, `rusqlite` (bundled), `reqwest` + `rustls`, `rustls-platform-verifier` | **Reuse as is.** All of it targets Android and iOS (bundled SQLite means no system library). Needs a build check, not a rewrite. This is the valuable 60% of the app. |
| `beacon/server` | Axum server | Not on the phone. Phones are clients only. |
| `beacon/app/src/web.rs`, `servers.rs` (the layer between UI and client, ~60 routes on 127.0.0.1 with a per-launch token) | Axum, HTTP on localhost | Reuse the logic, **change the transport**. Move the handlers into a shared library crate and call them from Tauri commands on mobile (a localhost web server inside a phone app works but fights Android's cleartext rules and iOS background limits, and is a worse security story). |
| `beacon/app/ui` (plain HTML/JS/CSS, ~1,800 lines of JS) | Desktop layout "A" | Reusable only with Tauri/web approaches. Needs a **phone layout** (bottom tabs, one pane at a time, touch targets) whichever way we go. |
| `beacon/media` (call engine: `str0m`, frame encryption, relay protocol) | Rust, reusable | Engine and frame encryption reuse. **Audio and video capture/playback do not**: they are GStreamer, PulseAudio/PipeWire, WASAPI, xdg-portals. Phones need native audio and camera code. |
| Protocol | `lodestone-beacon/v1` | No change needed for phase 1. Small additions later (below). |

So "how much is shared" depends on the approach: the Rust core is shared by all of them; the UI is shared only
if we keep web tech.

## Approaches compared

### A. Tauri 2 mobile (recommended)

Same Rust core, same web UI (made responsive), wrapped as an Android and an iOS app. Tauri 2 supports
Android and iOS and lets us write small Kotlin and Swift plugins for the things a web view can't do.

- **Shared:** all Rust, nearly all UI. One codebase for chat and settings. Lodestone and Beacon desktop keep
  working from the same repository.
- **Native pieces still needed (Kotlin/Swift plugins):** secure key storage, QR scanning (Tauri has an
  official barcode-scanner plugin; check what it pulls in), push, background service, audio capture and
  playback for calls, camera, screen capture.
- **Risks:** mobile support is newer than desktop, so expect rough edges (web view camera permissions on
  Android are a known pain point), a thinner plugin ecosystem, and iOS builds that need a Mac or a macOS CI
  runner. Web view UI will feel slightly less native than SwiftUI or Compose.
- **Cost:** lowest. Roughly 1x.

### B. Native Swift + Kotlin, Rust core through UniFFI

Two native apps, the best feel and best access to background audio, CallKit, foreground services.

- **Shared:** the Rust core only. **Both UIs rewritten**, plus the glue (Rust to Swift and Kotlin bindings).
- **Cost:** about 3x A, and it needs someone fluent in Swift and Kotlin. Every feature gets built twice.
  The desktop UI and the phone UIs then drift apart.
- Makes sense later if Beacon gets popular and the web view feel becomes the complaint.

### C. Another cross-platform toolkit (Flutter, React Native, Kotlin Multiplatform) with the Rust core

One new UI instead of two, native-looking widgets.

- **Shared:** Rust core through a bridge (flutter_rust_bridge, UniFFI). **UI rewritten once**, and not shared
  with desktop either way.
- **Costs:** a new language and a large new dependency tree (AGENTS.md rule 7: a heavy framework from a big
  company, plus its plugins, each to be vetted). About 1.7x A. The Beacon design work (layout A, mystic
  icons) has to be redone.

### D. Not an app: a PWA

Doesn't meet "on the app stores", can't hold keys safely, can't receive push without a relay. Ruled out.

### Recommendation

**A, with one rule that keeps B open:** put everything the phone does into a plain Rust library crate with a
small, UI-neutral command API (the refactor of `web.rs` above). The Tauri shell is then one thin front end
on that library. If the web UI ever isn't good enough, a native UI can sit on the same library without
touching the crypto or protocol. This also makes the desktop app simpler.

Before committing, a **1 to 2 week spike**: build `beacon/client` for `aarch64-linux-android` and
`aarch64-apple-ios`, run an MLS group between a phone and a PC, load the current UI in the Tauri mobile
shell, scan a QR. If that works, A is confirmed.

## Voice, cameras and screen sharing on a phone

The call engine encrypts frames itself and talks to Beacon's own relay, so we **can't use the web view's
built-in WebRTC**. It has to be the Rust engine fed by native capture.

| Feature | Android | iOS |
| --- | --- | --- |
| Listen and talk | Native audio (Oboe or AAudio) plus an Opus codec in Rust, replacing the GStreamer part of `media/src/audio.rs`. A foreground service of type microphone keeps the call alive when the screen is off. | `AVAudioEngine`/`AVAudioSession` plus the same Opus codec. Background mode "audio" keeps the call going. No CallKit needed because Beacon voice channels are rooms you join, not rings. |
| Camera | CameraX frames to the VP8 path. Hardware VP8 encode is hit and miss, software encode (libvpx) is fine for small pictures. | AVFoundation frames. **iOS has no hardware VP8 encoder**, software VP8 only. If that's too slow, the protocol could allow H.264 (needs a protocol change, both ends, decision for later). |
| Watch someone's screen or camera | Decode VP8 and draw it. Easy. | Same. Easy. |
| Share your screen | MediaProjection plus a foreground service (declared on Google Play). Moderate. | Needs a ReplayKit **broadcast extension**, a separate process with a tight memory limit that must hand frames to the app. **Hardest single feature.** |

So: listen, talk, watch and send camera are realistic in phase 3. Phone screen **sending** comes last, and
iOS may ship without it first.

## History for a new device

Messages are MLS-encrypted with forward secrecy, so a new device **cannot decrypt old ciphertext**; the only
source of history is another device's own decrypted copy. Recommendation (same as the earlier discussion):
**copy it at pairing time**, from the approving device, as an end-to-end encrypted bundle.

- The approving device exports its stored messages (and attachment keys) for the chosen channels, encrypts
  them with the key that already travels in the pairing QR, and uploads the result as an ordinary opaque blob
  through the server. The new device downloads and imports it. The server learns only a size and a time.
- Let the person choose how much (default the last 30 days, or "everything"), because phones have little
  space and bundles can be large. Show progress.
- It is a one-time copy. The two devices are not kept in sync afterwards, and a device removed later keeps
  whatever it already copied (state that plainly in the UI).
- It needs a small protocol addition (a bundle blob attached to a pairing) and is **not in PR #10**, which
  lists it as "not in this PR".

## Phone as authenticator

PR #10's pairing has the approving device show the QR (`beacon-pair:1|<server url>|<code>|<key>`) and the
new device scan it, with the identity sealed to the key in the QR. For "approve a new PC by scanning" the
direction is the **reverse**: the new PC shows a QR holding its device key, a fresh one-time public key and a
server address; the phone scans, shows the PC's fingerprint to compare, and on approval seals the identity to
that one-time key and sends it via the existing approve call. That is both a smaller change (the QR no longer
carries a secret, only a public key) and safer, but it **changes PR #10's flow**, so it should be agreed
before PR #10 merges. A phone that scans a QR also needs the camera permission only for that moment.

## Notifications without leaking content

This is the one hard problem, and the one that touches AGENTS.md.

- **What leaks:** with a push service, Apple or Google sees that "a push arrived for this device at this
  time", and the sender's service sees which Beacon server and device. They never see message text (nothing
  readable is sent).
- **The catch with self-hosting:** an iOS push can only be sent with the **app publisher's** Apple key, and an
  Android FCM push with the publisher's Firebase key. Self-hosted Beacon servers run by other people don't
  have those keys, and we can't hand the keys out. So push needs **a small relay run by the app publisher**
  (Robb): the server tells the relay "wake device token X", the relay sends a content-free push, the phone
  wakes, connects to its server, fetches and decrypts, then shows the notification itself (iOS notification
  service extension; Android service).
- **Options:**
  1. **No push at first (recommended for phase 1 and 2).** Android: optional foreground service keeping the
     live connection (battery cost, persistent notification). iOS: notifications only while the app is open
     or recently used. Honest and simple, but iOS users will notice.
  2. **A publisher push relay** (content-free, stores only device token and server address, open source,
     documented). Best experience, but it is **a server of our own** and an extra network destination, which
     rules 1 and 2 currently forbid. It would need Robb to amend AGENTS.md on purpose, with the exact limits
     written there (no content, no analytics, nothing stored beyond the token, off by default).
  3. **UnifiedPush / ntfy on Android** (self-hostable, no Google). Fine for Android and F-Droid, nothing for
     iOS (Apple allows only APNs).
- **Recommendation:** ship phase 1 and 2 with option 1 plus 3 on Android, decide on option 2 as its own step
  once there are real users who ask. Don't use the Firebase SDK in the app (it brings analytics and
  Google Play services dependencies).

## Store requirements

### Accounts (Robb must create these himself)

| | Apple Developer Program | Google Play Console |
| --- | --- | --- |
| Cost | US$99 a year, renews yearly or the app is removed | US$25 once |
| Who can enrol | Individual (your legal name becomes the public seller name) or organisation (needs a legal entity and a free D-U-N-S number, which can take days to weeks) | Personal or organisation (organisation needs a D-U-N-S number) |
| Timing | Individual: usually a day or two after identity verification, sometimes longer | Identity verification first. **Personal accounts made recently must run a closed test with at least 12 testers for 14 days before they can publish to production** (organisation accounts are exempt). Verify the current number on Google's page; it has changed before. |
| Needs | A Mac (or macOS CI) to build iOS. TestFlight for beta testing | An Android phone is nice to have, an emulator is enough to start |
| First review | Often 1 to 3 days; first submissions are often rejected once with questions | Several days for a new account; can be longer |

Start the account sign-ups **now**, they're on the critical path, and the 14-day Google test can only start
once an installable build exists.

### Review rules that matter for an E2EE chat app

- **User-generated content (Apple 1.2, Google's UGC policy).** Both want: a way to filter or remove
  objectionable content, a way to **report** content or a user, a way to **block** abusive users, and
  published contact information, with timely action. Beacon is end-to-end encrypted so the publisher and the
  server can't read anything, and that is accepted (Signal, WhatsApp are on both stores), but the *tools
  must exist in the app*. What to build:
  - **Block** (hide a person's messages on this device, client-only).
  - **Report**: the reporter chooses messages, the app sends the decrypted excerpt plus the sender to the
    community's owner and admins as an end-to-end encrypted message. The reporter chooses what to reveal.
  - Existing admin tools (remove, ban, delete channel) already exist on the server; surface them in the
    phone UI.
  - A **contact address** in the app and in the listing for the publisher (and a way to reach a server's
    operator).
  - A short **community guidelines** page in the app, and wording that the publisher doesn't run servers and
    can't remove content from them. Reviewers push back on apps with "no moderation"; the honest answer is
    "moderation belongs to each community's admins, here are their tools".
- **Account deletion (Apple 5.1.1(v), Google's account-deletion rule).** The app lets people create accounts
  on a server, so it must let them delete the account **in the app**. Today the server has leave/remove
  member and remove device but **no "delete my account" endpoint**. Needs a server route and UI, and Google
  asks for a web page describing deletion for the listing.
- **Demo access for reviewers (Apple 2.1).** The app is empty without a server. Robb must run a **review
  server** (a small public Beacon instance, possibly always on) with an invite code or a demo account, and
  give it in the review notes. This has a running cost and must stay up while reviews happen.
- **Minimum functionality (Apple 4.2).** A web wrapper gets rejected; ours has native key storage, QR
  scanning and so on, which helps. The first-run screen must clearly explain how to join or set up a
  server.
- **Privacy labels and Data safety form.** Both stores ask what is collected. Beacon's own code collects
  nothing and has no analytics, so answers are likely "data not collected" by the publisher, but data does
  go to servers the user chooses. Fill it in so it matches what the code does (rule 8) and re-check it each
  release. Both stores need a privacy policy URL (a page in this repository, hosted on GitHub Pages, is
  enough).
- **Encryption export.** Both stores ask. The app uses its own encryption beyond what the OS provides, so on
  Apple it can't be answered as "only exempt HTTPS". In the US, apps like this are normally self-classified
  under export rules for mass-market encryption (ECCN 5D002) and need an annual self-classification report
  to the US Commerce Department (BIS), filed by early February. Apple also asks about France's declaration.
  The source being public changes some of the filing details. This is paperwork, not a blocker, but get it
  right: use Apple's export-compliance page and BIS's guidance, and consider a short paid consult with an
  export-compliance advisor. I am not a lawyer.
- **Platform plumbing.** Google requires a recent target API level, a data safety form, and declarations for
  foreground service types (microphone, screen capture) and for full-screen intents if used. Apple needs
  usage strings for camera and microphone, and TestFlight builds before release.
- **Other store terms.** Bundle/package ID (suggest `io.github.daigonstar.lodestone.beacon` unless Robb owns
  a domain), age rating (chat with strangers means a 12+/17+ type rating questionnaire), and a name that
  isn't taken.

### Licence note (GPL-3.0-or-later on the App Store)

Beacon is GPL-3.0. Apple's store terms are widely held to conflict with the GPL for code the distributor
doesn't own (VLC was pulled in 2011 over this). The distributor here is Robb, who owns the code and can
publish his own work under store terms; Signal's iOS app is an open-source app on the App Store the same
way. The risks are **outside contributors' code** and **third-party libraries** under licences that don't
allow it: GStreamer is LGPL (avoid it on mobile anyway), `openmls`, `rusqlite`, `str0m` are permissive. If
Robb wants outside contributions to Beacon mobile, add a contributor agreement or an explicit store
distribution exception to the licence first. Worth a short legal opinion before the first iOS submission.

## What conflicts with AGENTS.md

| Rule | Conflict or work needed |
| --- | --- |
| 1. No telemetry or servers of our own | **Real conflict with a push relay** (option 2 above) and with the review server only if it were run by us on the user's behalf (it isn't; it's a normal Beacon server). Don't add Firebase, Crashlytics or any analytics SDK. The stores' own crash and usage reports are platform features, not ours; say so in the privacy text. |
| 2. Every address listed | Push hosts (`api.push.apple.com`, FCM) if relay ever ships. Build tooling (Gradle, Maven, CocoaPods, Swift packages) mentions hosts: `scripts/check-network.py` scans only `src-tauri`, `beacon/*/src` and UI files today, so it must be extended to the mobile project's code and decided case by case. |
| 3. Sign-in data never leaves the PC | The identity seed and device key go in **Android Keystore / iOS Keychain**, not a 600 file. **Turn off cloud backup** of app data (Android `allowBackup=false` and no auto-backup; iOS Keychain "this device only", exclude the data folder from iCloud) so the seed isn't copied to Google or Apple. Block screenshots on the recovery-phrase screen, avoid putting it on the clipboard, hide the app-switcher preview. Mention in the README. |
| 4. No obfuscated or minified code | Android release builds minify with R8 by default: **turn minification and obfuscation off** (or publish the mapping files). Apple's strip and bitcode settings are fine but document them. |
| 5. Official sources only | Updates come from the stores. The Android APK on GitHub releases (for people without Google Play) must be the same signed build. No downloading code at run time (Apple forbids it anyway). |
| 6. Say what changes on the system | Phones are sandboxed; list the permissions (camera, microphone, notifications, foreground service) in the README with the reason each is requested. |
| 7. No surprise dependencies | Each Kotlin/Swift/Rust library for audio, Opus, VP8, QR needs a stated reason. Prefer ZXing-style scanning or the platform's own over libraries that call Google Play services. Keep a list. |
| 8. Be honest | Store listing, privacy labels and README say what the code does and what's untested (for example "iOS screen sharing not available yet"). |

## Phased path

Estimates are rough and assume one developer working with Claude on a sensible weekly rhythm, with Robb
testing on real phones; stores' review time is on top.

**Phase 0: decisions and groundwork (week 1 to 2)**
- Robb: choose approach (below), start Apple and Google sign-ups, say yes or no to the push relay question
  and the licence note.
- Spike: client crate on Android and iOS, MLS with a PC, UI in the Tauri shell.
- Split the app layer into a shared library crate plus the desktop adapter (needed for everything).
- Settle the reverse-QR pairing before PR #10 merges.

**Phase 1: pairing and chat (week 3 to 8)** (what you asked for first)
- Create or restore an identity (24 words), approve a PC by scanning its QR, link a phone to a PC.
- Join communities, read and send text, files, reactions, profiles. Phone layout.
- History copy at pairing.
- Keystore/Keychain, backups off.
- Block, report, delete account, community guidelines, contact info.
- Android first (APK in GitHub releases, Google closed test starts: the 14 days clock), iOS to TestFlight.

**Phase 2: store release of chat (week 8 to 12)**
- Review server for Apple and Google reviewers, privacy policy, privacy labels, data safety, export
  paperwork, listing text and screenshots (the Beacon icon and artwork already exist).
- Submit to both stores. Expect at least one rejection round on Apple.
- Notifications option 1 and Android UnifiedPush.

**Phase 3: voice (week 12 to 18)**
- Listen and talk in voice channels, with background audio. Watch cameras and screen shares.

**Phase 4: sending video (week 18 onwards)**
- Camera on, Android screen share, then iOS screen share (ReplayKit) if it proves worth it.

**Later, separately decided:** push relay, H.264, Wear/tablet layouts.

## What I need from Robb

1. **Approach:** A (Tauri 2 mobile, recommended), B (native), or C (other toolkit).
2. **Start the developer accounts** (Apple US$99 a year, Google US$25 once), as individual or organisation.
   I can't create them. Tell me the seller name you want shown publicly.
3. **Push:** accept "no push first, UnifiedPush on Android" for now, or want to amend AGENTS.md for a
   publisher push relay?
4. **A Mac or macOS CI:** do you have a Mac, or should the iOS build run in GitHub Actions (free on public
   repositories; signing keys would live in GitHub secrets)?
5. **Licence:** ask for a short legal opinion on App Store distribution, or accept the note above and
   revisit if outside contributors appear.
6. Which phones can you test on (Android version, iPhone model)?

Nothing will be built until you choose.
