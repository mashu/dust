# Platform notes

The parts of building, shipping and storing that are specific to one target.
Everything else is in the [README](../README.md).

## Android

Dioxus's `mobile` feature is the same wry/tao webview stack as the desktop
build, so the whole UI, curriculum and scoring come across unchanged. `dx`
enables that feature itself for `--platform android`.

Audio has full parity with the desktop app: `src/audio/native.rs` is compiled
as-is, with cpal routing it to AAudio through
[oboe](https://github.com/google/oboe) instead of ALSA. Building it compiles
oboe's C++ with the NDK toolchain, which is why an NDK is needed and not just
the SDK.

One Android-specific fallback: the receiver background runs as a second,
continuously open output stream, and some devices refuse to open two at once.
If that happens the Morse, its envelope and QSB still play — only the
background hiss drops out — rather than the session refusing to start.

If a toolchain bring-up ever blocks on oboe, `--features mobile-silent` builds
the same app with a player that keeps a session's timing but makes no sound,
and the practice screen says so.

### Building an APK

```bash
rustup target add aarch64-linux-android
export ANDROID_HOME=~/Android/Sdk          # or /usr/lib/android-sdk
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.2.12479018"
dx bundle --platform android --target aarch64-linux-android --package-types apk --release
```

Pass `--target` explicitly: without it `dx` builds for the host architecture,
which on an x86_64 machine means an emulator APK that a phone will not run.

On Debian the `google-android-*-installer` packages in contrib fetch Google's
SDK bits; the official `commandlinetools` zip plus `sdkmanager` gives tighter
control over NDK versions. Either way you need JDK 17 and
`ANDROID_HOME`/`ANDROID_NDK_HOME` exported.

Without cutting a release: **Actions → Android APK → Run workflow**, on any
branch. The APK is attached to that run as an artifact.

### Play Store

Play wants an **Android App Bundle** (`.aab`) signed with an upload key,
targeting API 36. The keystore lives outside the repo
(`~/.android/dust-play-upload.jks`) so it is never committed.

One-time, create the upload key — keep the env file private, since losing the
key means a Play Console upload-key reset:

```bash
install -m 700 -d ~/.android
pw="$(openssl rand -base64 32)"
printf 'KEYSTORE_PASSWORD=%s\nKEY_PASSWORD=%s\n' "$pw" "$pw" > ~/.android/dust-play-upload.env
chmod 600 ~/.android/dust-play-upload.env
# shellcheck disable=SC1090
source ~/.android/dust-play-upload.env
keytool -genkeypair -v \
  -keystore ~/.android/dust-play-upload.jks \
  -storetype JKS \
  -alias upload \
  -keyalg RSA -keysize 4096 -validity 10000 \
  -dname "CN=Dust Morse Trainer, O=Dust, C=US" \
  -storepass "$KEYSTORE_PASSWORD" \
  -keypass "$KEY_PASSWORD"
chmod 600 ~/.android/dust-play-upload.jks
unset pw KEYSTORE_PASSWORD KEY_PASSWORD
```

Then, with a JDK that includes `javac` (17 or 21) and a writable SDK:

```bash
scripts/android-play-bundle.sh
```

That writes `dist/dust-<version>-android-arm64.aab` (`dev.dust.morse`). In Play
Console create the app, let Google generate the app signing key, and upload that
AAB. Back up `~/.android/dust-play-upload.jks` and the env file beside it. The
public upload certificate is `dist/dust-upload-certificate.pem`.

## iOS

Same story as Android — `mobile` is the wry/tao webview stack and cpal reaches
CoreAudio. One thing is iOS-only: an app that has not claimed an
`AVAudioSession` is silent, follows the ringer switch and gets interrupted by
anything else on the device, and cpal does not claim one. The player takes the
`playback` category before opening its first stream.

What you can build, and what Apple lets you install, are different questions:

| | Signing | Runs on |
| --- | --- | --- |
| Simulator build | none | a Mac |
| **Unsigned IPA** (this repo) | none | your iPhone, after a sideloader re-signs it |
| Signed IPA / TestFlight | Apple Developer Program, $99/yr | any device, no re-signing |

Releases carry the unsigned `.ipa`, and **Actions → iOS IPA → Run workflow**
builds one from any branch in between. Install it with
[AltStore](https://altstore.io), [SideStore](https://sidestore.io) or
[Sideloadly](https://sideloadly.io): they re-sign the app with your own Apple
ID. A free Apple ID gives a signature that lasts **7 days** and allows **three**
sideloaded apps at a time — the tools re-sign in place before it expires. A paid
membership removes both limits and is the only route to TestFlight or the App
Store.

CI type-checks the simulator target on every PR, which needs no Apple account
and covers the audio-session code and cpal's CoreAudio backend.

## Desktop bundles

`dx bundle --platform desktop --release` writes to
`target/dx/dust/release/linux/` and friends. macOS builds are unsigned
(right-click → Open the first time). Windows needs WebView2, which is already
present on typical Windows 10/11 systems.

## Where your progress lives

Nothing syncs. Each install keeps its own history, in the place that platform
gives it:

| Build | Location |
| --- | --- |
| Web | `localStorage` for that exact origin (`https://mashu.github.io`, or `localhost:8080` while developing — the two are separate stores) |
| Linux | `~/.local/share/dust/` (`$XDG_DATA_HOME`) |
| Windows / macOS | the platform data dir `dirs` reports |
| Android | `/data/data/<package>/files/dust/` — `dirs` has no `HOME` to work from, so the store resolves the package name from `/proc/self/cmdline` |
| iOS | `Library/Application Support/dust/` inside the app container |

`DUST_DATA_DIR` overrides the directory on every desktop and mobile build,
which is what a portable install uses.

Updating the app does not clear any of it. The storage keys (`dust_settings`,
`dust_sessions`, `dust_auto_adjust_*`) and file names are not versioned, and
settings files from older builds still load — new fields fall back to their
defaults, and one unreadable field costs only that field.

Stats are computed across your whole history. Changing course — Koch to Mixed,
a new sequence, a custom alphabet — does not hide what you already did:
accuracy over time, letter mastery, mistakes, the calendar and the streak all
count every session, because a letter is the same letter whichever set it was
sent under. The one exception is the **Sampling** tab, which shows what the
trainer will draw next and so narrows to the current character set, the same way
the sampler itself does.
