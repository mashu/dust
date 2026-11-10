# Platform notes

The parts of building, shipping and storing that are specific to one target.
Building and testing in general is in the [README](../README.md).

## Android

Dioxus's `mobile` feature is the same wry/tao webview stack as the desktop
build, so the whole UI, curriculum and scoring come across unchanged. `dx`
enables that feature itself for `--platform android`.

Audio has full parity with the desktop app: `src/audio/native.rs` is compiled
as-is, with cpal routing it to AAudio through
[oboe](https://github.com/google/oboe) instead of ALSA. Building it compiles
oboe's C++ with the NDK toolchain, which is why an NDK is needed and not just
the SDK.

The project `dx` generates needs six things put right, and `dx` 0.7 has a
setting for none of them. Every bundle command below, in CI as well, first
installs a Gradle init hook, [`scripts/android-gradle-hook.sh`](../scripts/android-gradle-hook.sh),
that does all six:

- **Android Gradle plugin 9, and no Material Components.** `dx` writes AGP
  8.7 with the separate `kotlin-android` plugin, and Material Components,
  which nothing in the app uses. Play asks for AGP 9.0 or later, for R8's
  newer optimizations, and flagged Material's date picker for calling
  `Window.setStatusBarColor` and `setNavigationBarColor`, which Android 15
  deprecates. Once Gradle has read the settings, before either build script
  is compiled, the hook moves the generated project to AGP
  `ANDROID_GRADLE_PLUGIN_VERSION` (9.0.1: AGP 9.0 runs on the Gradle 9.1
  wrapper `dx` writes; a later 9.x needs a later Gradle) with its built-in
  Kotlin and the Kotlin Gradle plugin it is built with (2.2.10): it takes
  out the `kotlin-android` plugin, the `kotlinOptions` block (Kotlin
  targets the Java version `compileOptions` sets) and the `sourceSets` block
  (built-in Kotlin finds `src/main/kotlin` itself, and refuses Kotlin
  sources added to the Java set), and drops Material Components. It also
  drops `android.defaults.buildfeatures.buildconfig` from the generated
  `gradle.properties` and loads the build's properties again: Gradle has
  already read the file by the time an init script runs, and AGP 9 removed
  that option and will not apply while it is set. The generated app script
  already sets `buildFeatures.buildConfig`. CI reads
  the plugin version back out of the build's metadata and looks for
  Material's classes in its dex files (`check_android_build`), and fails if
  the version is older or any class is there.

- **Resource shrinking.** `dx` has R8 shrink the release build's code but
  not its resources, so every resource of AppCompat and the webview library
  shipped whether the app reached it or not, and Play flagged it among its
  R8 findings ("Resource shrinking isn't enabled"). The hook sets
  `shrinkResources` on the release build type, and R8 drops what nothing
  reaches. AGP 9's optimized shrinking requires non-final resource IDs, so
  the hook sets `android.nonFinalResIds=true` in the generated
  `gradle.properties` (`dx` writes `false`). The app reaches its own resources only from the manifest (the
  launcher icon, the label, the theme and the network security config),
  which the shrinker always keeps, and nothing in the app, `wry` or `tao`
  looks a resource up by name.
- **Launcher icon.** The icon is [`assets/icon.png`](../assets/icon.png).
  Android 8 and later show the adaptive icon packed in the app, and `dx`
  puts the stock robot there. The hook copies
  [`assets/android/`](../assets/android) over those resources before the
  package is built. Regenerate the densities with
  `scripts/render-android-icons.py` after changing the master.
- **Version code.** `dx` writes `versionCode = 1` into every build, and Play
  turns away an upload whose version code is not above every earlier one.
  The hook sets it from the workspace version in `Cargo.toml`: MAJOR × 1,000,000
  \+ MINOR × 1,000 + PATCH, so 0.14.4 is 14004. Every new version is then a
  higher code. The version name is the version itself. CI reads both back out
  of the APK it built and fails if either is wrong.
- **Native debug symbols.** Play warns about an app bundle with native code
  but no debug symbols, and shows its native crashes as bare addresses. The
  release profile keeps the symbol table (`strip = "debuginfo"` in
  `Cargo.toml` drops only the debug info). The Android plugin packs that table
  only after its own strip shrinks the library, which does not happen once
  rustc has already removed the debug info, so setting
  `debugSymbolLevel = "SYMBOL_TABLE"` leaves the bundle without a symbol file.
  `scripts/android-play-bundle.sh` writes it with `llvm-objcopy --strip-debug`
  to `BUNDLE-METADATA/com.android.tools.build.debugsymbols/` and strips it out
  of the library the app ships. The script stops if that file is missing.
  Play reads it from the bundle. The hook still points the plugin at
  `ANDROID_NDK_HOME` (`ndkPath` and `ndkVersion`) as the plugin is applied.
- **Configuration changes.** `dx`'s manifest has the activity handle only a
  turn of the screen itself. For any other change Android recreates the
  activity, and the webview with it, and the app ends: Dioxus has no window
  left, and tao exits the process. Dark mode turned on, a keyboard or a
  paddle adapter plugged in, the font or display size changed, a new
  language, a foldable opened: each closed it, mid-session or not. The hook
  rewrites the activity's `android:configChanges` in the generated manifest
  to every change there is (`ANDROID_CONFIG_CHANGES`), and fails the build if
  it finds none to rewrite. It also removes `android:extractNativeLibs`,
  which `dx` sets to true: AGP 9 rejects the attribute, and that legacy
  packaging is what Play refuses of an app targeting Android 15. Without it
  the plugin stores the native libraries uncompressed and aligned. The webview lays itself out again for a new size
  or density. The Auto theme follows dark mode switched on meanwhile only as
  far as the webview passes the change on to the page; Light and Dark pin
  it either way.

**Edge to edge.** The app targets SDK 36, so from Android 15 the system
draws it edge to edge: the window reaches under the status bar, the
navigation bar and a display cutout, and the keyboard no longer shrinks it.
`wry` puts the webview in the activity's content as it is. The app's own
activity, [`android/MainActivity.kt`](../android/MainActivity.kt), which
`Dioxus.toml` points `dx` at (`application.android_main_activity`), pads the
content by those insets from Android 15 and consumes them, so the page is laid
out between the bars and above the keyboard, whatever version of the webview
the phone has (the webview reports system bars to the page's
`env(safe-area-inset-*)` only in its newer versions). Before Android 15 the
system keeps the content clear itself, as it always has. The bars show the
window behind them, coloured by the app's theme, which the hook copies in with
the icon ([`assets/android/values*/styles.xml`](../assets/android/values)): the
paper of the light theme with dark icons, or the dark theme's background with
light ones in dark mode, the status bar through `colorPrimaryDark` before
Android 15. Nothing calls the window's bar colours, which Android 15
deprecates. The bars follow the system's dark mode, not the app's own Light
or Dark choice.

The screen stays on through a session — the count down, every send and
every answer — through the window's own keep-screen-on flag
(`FLAG_KEEP_SCREEN_ON`), set and cleared over JNI on the activity's thread
([`src/keep_awake.rs`](../src/keep_awake.rs)). It needs no permission, and the
system lets go of it with the window. The answer box asks for no on-screen
keyboard (`inputmode="none"`): the training screen has a keypad of its own, so
nothing slides in over the groups. A keyboard plugged in or paired still types
into the box.

Out of sight — another app in front, the screen locked — a session or a
contest run pauses, as it does in a browser tab, and Resume carries on. The
webview gives a hidden page no animation frames, and Dioxus waits for each
render to be drawn on one before it runs anything else, the pause the page
asks for among it. The page's head stands a timer in for a frame that does
not come ([`src/keep_running.js`](../src/keep_running.js), the desktop
build's too), so the pause lands at once. A process the system freezes
whole runs nothing until it thaws, and then its timers all come due at once:
a session and a run each keep a heartbeat, and a gap in it of more than two
seconds is taken as a pause from where it began, before anything that waited
out the gap is heard. No answer closes empty for it, and the time away is
not answering time. The same holds on iOS.

Back goes back through the app's screens: from Stats, Settings, Listen or
a session's results to Practice. In a session or a contest run it pauses
first, as putting the app behind does, and Back again leaves the way Exit
leaves it, keeping what was answered. At Practice it leaves the app. The app moves between its screens by itself, so while it is away from
Practice it keeps one entry of its own on the webview's history, which
wry's activity goes back through before it hands Back to Android
([`src/system_back.rs`](../src/system_back.rs)). Without it Back left the
app from any screen, and Android 7 to 11, which close an app on Back, took
a session under way with it. A browser's Back does the same in the web
app. On Android 16 Back never reaches an app built for it, as Dust is: the
system puts the app behind instead, which pauses a session as any time out
of sight does.

Whatever else plays stops out of sight, on every build: the band preview,
the letters, a sample, Hear a QSO. The stream of groups stops too on a
phone, where the system may freeze an app out of sight whole, and holds the
screen on while it plays instead, as Hear a QSO does. On the desktop and in
a browser it plays on, as it is there to be copied while you work in another
program or tab ([below](#copying-while-you-work-in-other-programs)).

The Morse, the band noise and static, and the paddle sidetone are mixed into
one output stream, so there is no second stream for a device to refuse — some
Android devices will not open two at once.

The paddle's sidetone keeps the keyer's time, not the audio callback's. An
output renders a whole buffer at once, 10 ms on Windows and in most browsers
and about 23 ms on iOS unless asked, so a gate it only read would open at the
start of whichever buffer came next, and a 40 ms dit would play as 30 or 50.
Instead the keyer stamps each edge with the instant it meant it for, and the
player sounds every edge a constant lead later: the longest buffer the device
has lately asked for, and 2 ms for the keyer's own timer to be late. Natively
the callback places each edge on the frame it falls on
([`src/audio/mixer/sidetone.rs`](../src/audio/mixer/sidetone.rs)); the browser
is told each edge ahead of time on its own clock
([`SidetoneGate`](../src/audio/shape.rs)). Every element and space then sounds
as long as it was keyed, on every platform, at the cost of about half a buffer
of lag on average. A burst of keying that reaches the output late — the
squeeze that wakes a sleeping stream — keeps its own time, that much later,
until the next quiet spell. iOS is asked for a 5 ms buffer: cpal cannot set
one there, and the session's default would leave the sidetone some 25 ms
behind the paddle. The sidetone opens and closes along the keying
envelope's edge, as a rig shapes its sidetone with its transmitter's, at a
level of its own, half the limiter's knee.

That stream is closed once nothing has sounded for a second, away from
Settings, so the phone (or a desktop's sound card and sound server) can rest
with the app open on a screen or in the background. The next send, band or
paddle squeeze opens it again. Settings keeps it open for the paddle's first
squeeze, and a session, or a run while it is not paused, holds it open. The
volume keys turn the media volume meanwhile, which Dust plays at: before
Android 9 they would turn the ringer with no stream playing. The browser
build suspends its audio the same way, once nothing is left to hear.

The browser plays the native player's own mixer: the band, the stations,
the receiver's filter and AGC, the mute while you send, the fades and the
fading, all in Rust ([`src/audio/stream.rs`](../src/audio/stream.rs)). A
browser plays only what it is handed ahead of time, so the mixer is run
half a second ahead of its clock, a tenth of a second at a time, each
stretch scheduled as a buffer, with a copy of the mixer kept from the start
of each. A send, a stop, a new band or an RF gain moved is made past the
browser's clock and its base latency by a margin: 30 ms, or the base
latency and 20 ms more where that is longer (at most 150 ms), as a phone's
or a tablet's browser renders a device buffer of 40 ms and more in one go,
and its clock steps on by as much at once; made short of it, a send's start
could fall behind the clock before it reached the browser, and be skipped.
The stretch that moment falls in is played again from its copy up to
there, sample for sample as it was, and the rest made anew. The graph left is the volume, the soft limiter and
the paddle's sidetone. On iPhone Safari the page takes the playback audio
session the first time something is about to be heard, so the Ring/Silent
switch does not mute it.

If a toolchain bring-up ever blocks on oboe, `--features mobile-silent` builds
the same app with a player that keeps a session's timing but makes no sound,
and the practice screen says so.

### tao, patched

Every task the app has — a session's timers and heartbeat, each send, every
render — runs when Dioxus is told to poll it, and on Android that word goes
through tao's event loop: `EventLoopProxy::send_event` queues it and wakes
the thread's looper. tao 0.34 took the queue only when `ALooper_pollAll`
returned a wake. With a lifecycle event from the activity (a resume, a
pause, a change of focus) ready at the same moment, the looper takes the
wake itself and returns the lifecycle event instead, so the queued word
waited, and Dioxus, which sends one per wake, sent no other. Nothing ran
until a message from the page woke the loop some other way. That is the
band playing on after the phone is unlocked, on a screen that answers
nothing, and a tap on Stats that goes through only seconds later.

tao 0.37.0 takes the queue on every return of the looper, as the NDK says
to. Dioxus 0.7.10 asks for tao `^0.34`, so the fix cannot come by
upgrading: `Cargo.toml` patches crates.io's tao with
[`vendor/tao`](../vendor/tao), tao 0.34.8 as published (its own commit, so
the change reads as a diff against it) with that one change to
`run_return` in `src/platform_impl/android/mod.rs`, marked `Dust:`. Its
`Cargo.toml` also turns its warnings off, which Cargo would otherwise show
on every build of a path dependency. It is outside the workspace, so
`cargo fmt`, Clippy and the tests leave it alone, and `codecov.yml` leaves
it out of the coverage. The desktop builds use the same copy; nothing
changed for them.

CI runs [`scripts/check-tao-patch.sh`](../scripts/check-tao-patch.sh)
first: it fails if the copy has lost the fix, if `Cargo.toml` no longer
patches tao, or if `Cargo.lock` builds any tao but the copy.

When Dioxus moves to tao 0.37 or later, Cargo warns that the patch was not
used, `Cargo.lock` lists it under `[[patch.unused]]`, and that check fails.
Then:

1. Check the new tao's `run_return` drains `self.receiver` after the match
   on what the looper returned, not only for `Poll::Wake` (0.37.0 does).
2. Remove the `[patch.crates-io]` table and the `exclude` line from
   `Cargo.toml`, and delete `vendor/tao`.
3. Run `cargo update -p tao`: tao's entry in `Cargo.lock` gets its
   crates.io `source` and `checksum` back.
4. Delete `scripts/check-tao-patch.sh` and its step in
   `.github/workflows/ci.yml`.

A Dioxus that asks for a later 0.34, 0.35 or 0.36 leaves the patch unused
in the same way. Those still lose the wake: vendor that release in place of
this one, as published, and make the same change.

As a backstop, a thread of the app's own wakes the event loop every quarter
second whatever else does ([`src/main.rs`](../src/main.rs)), and each wake
has tao take what is waiting. A wake lost some other way then holds the
app's tasks back a quarter second at most, well inside the two seconds a
session's heartbeat takes for time away. A wake with nothing waiting costs
the loop one empty turn. It does not depend on the patch, and can stay
when the patch goes.

Should the app stand still anyway, with the audio held, it says so in the
system log ([`src/time/stall_log.rs`](../src/time/stall_log.rs)). Every wait the
app's tasks make goes through `sleep_ms`, which notes when the task is due
again, and a claim of the audio is due at once. A thread looks every half
second, and the first time it finds the app more than 1.5 seconds past due
while a lease holds the audio, it writes one line for that stall:

```text
dust: stalled: a task due 2140 ms ago has not run, audio held
```

`adb logcat -s RustStdoutStderr` shows it. A long wait asked for, such as
the stream's pause between groups, is not due until it is over, so none
should ever appear: one is worth a report, with what was on the screen and
what had just been tapped. The desktop and web builds have none of this.

### Building an APK

```bash
rustup target add aarch64-linux-android
export ANDROID_HOME=~/Android/Sdk          # or /usr/lib/android-sdk
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.2.12479018"
# shellcheck disable=SC1091
source scripts/android-gradle-hook.sh
install_android_gradle_hook "$PWD"
trap 'rm -f "$ANDROID_GRADLE_HOOK"' EXIT
dx bundle --platform android --target aarch64-linux-android --package-types apk --release
```

Pass `--target` explicitly: without it `dx` builds for the host architecture,
which on an x86_64 machine means an emulator APK that a phone will not run.

That APK is the debug build type, signed with your own debug key, so it
installs only where Dust is not installed yet. The native library is
linked for 16 KB memory pages, which Play requires of an app targeting
Android 15 or later, whatever the NDK (`.cargo/config.toml`).

On Debian the `google-android-*-installer` packages in contrib fetch Google's
SDK bits; the official `commandlinetools` zip plus `sdkmanager` gives tighter
control over NDK versions. Either way you need JDK 17 and
`ANDROID_HOME`/`ANDROID_NDK_HOME` exported.

Without cutting a release: **Actions → Android APK → Run workflow**, on any
branch. The APK is attached to that run as an artifact. With the release
profile it is signed with the release key, as a release's is, and installs
over one; with the debug profile it is a debug build like the one above.

### Release signing

An APK installs over the one before it only if both carry the same key. A
release's APK is Android's release build type, minified and not
debuggable, signed with the project's own release key, so each release
installs over the last and keeps its history. `dx` builds that type only
when `Dioxus.toml` has a `[bundle.android]` table, so CI writes one at build
time from four repository secrets
([`scripts/android-release-signing.sh`](../scripts/android-release-signing.sh)),
then checks that the APK is not debuggable and carries that key. Without
the secrets the APK job fails and the release goes out with no APK.

One-time, make the key and add the secrets. The passwords and the alias
may hold only letters, digits, `.`, `_` and `-`: `dx` passes them to Gradle
through a template that would change anything else.

```bash
install -m 700 -d ~/.android
pw="$(openssl rand -hex 24)"
printf 'ANDROID_KEYSTORE_PASSWORD=%s\nANDROID_KEY_PASSWORD=%s\nANDROID_KEY_ALIAS=dust\n' \
  "$pw" "$pw" > ~/.android/dust-release.env
chmod 600 ~/.android/dust-release.env
keytool -genkeypair -v \
  -keystore ~/.android/dust-release.p12 \
  -storetype PKCS12 \
  -alias dust \
  -keyalg RSA -keysize 4096 -validity 10000 \
  -dname "CN=Dust Morse Trainer, O=Dust, C=US" \
  -storepass "$pw" \
  -keypass "$pw"
chmod 600 ~/.android/dust-release.p12
base64 < ~/.android/dust-release.p12 | tr -d '\n' | gh secret set ANDROID_KEYSTORE_BASE64
printf '%s' "$pw" | gh secret set ANDROID_KEYSTORE_PASSWORD
printf '%s' "$pw" | gh secret set ANDROID_KEY_PASSWORD
printf 'dust' | gh secret set ANDROID_KEY_ALIAS
unset pw
```

Without `gh`, add the same four under Settings → Secrets and variables →
Actions. Back up `~/.android/dust-release.p12` and the env file beside it:
GitHub never shows a secret again, and an APK signed with any other key can
never update an install.

The Play build is signed with Play's own app signing key, so the Play app
and a GitHub APK, both `dev.dust.morse`, cannot update each other. Nor can
any later release update an APK from 0.16.1 or before, which carried a
throwaway debug key. Android refuses such an install. To move across, save
a backup (Stats → History → Save a backup), uninstall, install the other
build and restore the backup. Uninstalling deletes the app's data, and on
Android 9 and older the backup with it, as it lies in the app's own
`Android/data/dev.dust.morse/files/Download`: copy it elsewhere first.

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

That writes `dist/dust-<version>-android-arm64.aab` (`dev.dust.morse`), prints
its version code, and lists the native debug symbols packed inside it. It
stops if that symbol file is missing. In Play Console create the app, let Google generate the
app signing key, and upload that AAB. Each later upload must come from a higher
version in `Cargo.toml`. Rebuilding the same version gives the same code, which
Play refuses. Back up `~/.android/dust-play-upload.jks` and the env file beside it. The
public upload certificate is `dist/dust-upload-certificate.pem`.

The store listing privacy-policy URL is
[https://mashu.github.io/dust/privacy/](https://mashu.github.io/dust/privacy/).
That page is [`docs/privacy.html`](privacy.html), copied into the Pages site
on every deploy. Settings in the app links to the same URL. The page has to
be live on `main` before Play review: a listing that points at it before the
Pages workflow has published it will fail the check. The Pages workflow
deploys a push to `main` once its whole CI run has passed, so a change to the
page goes live a CI run after it is merged.

## iOS

Same story as Android — `mobile` is the wry/tao webview stack and cpal reaches
CoreAudio. One thing is iOS-only: an app that has not claimed an
`AVAudioSession` is silent, follows the ringer switch and gets interrupted by
anything else on the device, and cpal does not claim one. The player takes the
`playback` category before opening its first stream.

The screen is kept on through a session by the webview's Screen Wake Lock,
which WebKit has on iOS 18.4 and later; an older iOS dims and locks on its
usual schedule.

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

CI lints the simulator target with Clippy on every PR, which needs no Apple
account and covers the audio-session code and cpal's CoreAudio backend. The
Android target is linted the same way, the `mobile` build with oboe's C++,
and the desktop build on Windows and macOS as well as Linux, where it is
also tested. The web app is built and its site checked as Pages would serve
it.

What carries the keys on macOS, Windows and the phones is tested on Linux:
the page scripts that hear the paddles and a run's keys are run in Node, on
a page made up for them, and held to what the app does with the same keys.
What no test runs: the web build's audio (`src/audio/web`), which needs a
browser's Web Audio, and the platform audio code, which needs the phone —
the iOS audio session, Android's volume keys and output latency. The
mixer, the receiver stream and the timing of a send that they hand their
work to are tested natively. Nor does any test run a real input method:
the keys of a composition, a dead key's among them, are made up in Node
as Safari, Chrome and the macOS build are known to hand them over, so a
paddle key that is dead, and a `[` typed in Pinyin or Kana, are checked
by hand on a Mac, an iPhone and in Safari. In WebKit — Safari, every
browser on an iPhone and the macOS build — a `[` or a `]` that ends a
composition, as Pinyin's pick of the first or last character of a word
does, still keys its paddle: it arrives after `compositionend` and, like
a dead key's press, not as composing but with the input method's key
code, 229. On the Linux desktop every bracket keys its paddle, composing
or not, as the window that keys them tells of no composition.

## Desktop bundles

`dx bundle --platform desktop --release` writes to
`target/dx/dust/release/linux/` and friends. Desktop is the system webview
(WebKitGTK on Linux, WebView2 on Windows, WKWebView on macOS). The Linux
`.deb` names the packages it needs (WebKitGTK 4.1, GTK 3, ALSA and the
rest), so `apt install ./dust-….deb` brings them in. Windows needs
WebView2, already present on typical Windows 10/11.

macOS builds are unsigned, so macOS will not open Dust the first time. On
macOS 15 and later, go to System Settings → Privacy & Security, choose
**Open Anyway** beside the message about Dust, and confirm. On macOS 14
and earlier, Control-click the app and choose **Open**. If macOS says
instead that Dust is damaged, run
`xattr -dr com.apple.quarantine /Applications/Dust.app` and open it again.

On macOS Dust has the usual menu bar: the app's menu (About, Hide, Quit),
Edit and Window ([`src/menus.rs`](../src/menus.rs)). WKWebView turns
Cmd+V, Cmd+C, Cmd+X, Cmd+A and Cmd+Z into editing only through Edit's
items, so without them nothing could be pasted into a field. Linux and
Windows show no menu bar: their webviews take those keys themselves. On
every desktop a right-click in a field opens the field's own menu, with
Cut, Copy and Paste; anywhere else in the main window it opens nothing.

On Linux, **Restore a backup** and **Load a text file** open the desktop's
file chooser portal (`xdg-desktop-portal`, with a backend that chooses
files: `-gtk`, `-gnome` or `-kde`), or `zenity` where there is none. Every
desktop install has one or the other; a bare window manager may have
neither, and then no chooser opens. Dioxus hands the app no file then, as
for a chooser cancelled, so the button looks for either where the desktop
keeps them (the XDG data folders, the `PATH`) and, finding neither, says
what to install.

**Launcher icon.** GNOME looks the icon up in the hicolor theme, and only in
the sizes that theme lists. [`assets/icon.png`](../assets/icon.png) is scaled
into those sizes under [`assets/linux/`](../assets/linux) by
`scripts/render-linux-icons.py`, and `dx bundle` installs them as `dust` next
to `dust.desktop`. Regenerate them after changing the master. The same master
is the window icon.

### Drawn on the graphics card

On Linux, WebKitGTK draws the page with the graphics card through its DMA-BUF
renderer. Left to itself, Dioxus turns that renderer off on every Wayland
session with a GPU (`WEBKIT_DISABLE_DMABUF_RENDERER`), a workaround for
NVIDIA's driver. A recent WebKitGTK (2.52 among them) then sets its hardware
acceleration to never and paints every page by the processor, so scrolling
lags. Dust keeps it on ([`src/linux_webview.rs`](../src/linux_webview.rs)).
The one exception is NVIDIA's own driver on a Wayland session, whose windows
can come up blank with it on. The app still runs under XWayland, as Dioxus
has it, for the window manager's title bar.

Settings → Display → Check says what draws the page: the graphics card by
name, or the processor ("llvmpipe" and the like), and why compositing is off
if it is. On Linux that comes from WebKitGTK's own `webkit://gpu` page, which
GNOME Web shows as well. A page's WebGL is only told a made-up name there.
That page also says when the card draws but the processor copies each frame
to the window (under X11 or XWayland, or with
`WEBKIT_DMABUF_RENDERER_FORCE_SHM`), and when Skia paints on the processor
(`WEBKIT_SKIA_ENABLE_CPU_RENDERING`). The check says both, with the cause.

To try the graphics card under NVIDIA, start Dust with
`WEBKIT_DISABLE_DMABUF_RENDERER=0`. Set to `1`, it paints by the processor
anywhere, should the graphics driver misbehave. `GDK_BACKEND=wayland` runs
it as a native Wayland window. Any of these set before Dust starts is left
as it is.

### Copying while you work in other programs

Listen → **Stream groups** keeps sending groups with the window minimised or
behind others. **Floating window** opens a small window that shows each group
after it is sent, with its speed and the few before it. Closing the main
window closes it too.

It has no title bar. Drag it to move it, and right-click it for the
desktop's own window menu, the one a title bar would open: on GNOME choose
**Always on Top**, on KDE **Keep Above Others**. Placed there, it stays above
everything wherever the main window goes. On Windows and macOS, which have no
such menu, the right-click puts it on top or takes it off. Then **Lock
floating window** makes it click-through: every click goes to the program
underneath, so it can sit over your work. A locked window cannot be clicked,
so unlock or close it from the main window.

The see-through background needs a compositor, which every Wayland desktop
has; without one it is drawn solid.

## The first frame

Every build has the stylesheet before it paints anything, so the app is
never drawn unstyled, however long it takes to start. Desktop and the phones
inline it in the webview's head (`src/main.rs`), which the webview reads
before its first paint; the web links it from the head of `index.html`, a
static-head `asset!` (`src/app.rs`), and shows its boot screen until the app
mounts. The phones used to put the sheet in from the app once it had
mounted, and Android drew the first frames raw before it arrived.

The app waits for the whole history before anything of it runs, so nothing
acts on a history half read; it never waits on it to draw a frame. A
history still being read, or moved to this version's records, as the first
frame is drawn holds the app on a boot screen of its own (`src/app.rs`):
the paper alone for the first second, as most reads are over by then, then
the app's mark and "Reading your history… n of N", or "Moving your
history… n of N" during a move, until the history is in, with no time
after which it gives up. The app used to read it as it drew its first
frame: a Linux window stood blank meanwhile, a Mac's or a Windows PC's
showed nothing at all, and Android's event loop waited on it. In a browser
the page's own boot screen (`index.html`'s `#boot`) shows from the first
frame, as the history is read before the app starts, and gains the same
words after about a second, written in by the app's code, so the page runs
no script of its own for it.

The rest of what a frame needs is in the head too: a phone's viewport, edge
to edge from the start, and every build's link to the type faces, which are
asked for as the page loads. The faces never hold up the first paint: the
sheet loads as print media and is switched on once it has arrived, and its
faces are `display=optional`, so a face that is late, or a phone offline,
leaves the system's for that page rather than swapping it in under the
reader. Before
the page has loaded, the desktop window and Android's webview are the light
theme's paper, not white; iOS keeps its webview's own colour, which wry
would make translucent to change.

## Where your progress lives

Each install keeps its own history, in the place that platform gives it.
Sync, if the user turns it on, keeps installs in step through an encrypted
copy in their own Google Drive ([sync.md](sync.md)); without it, each
install's history is its own:

| Build | Location |
| --- | --- |
| Web | the browser's storage for that exact origin (`https://mashu.github.io`, shared with every other Pages site of the account, below; or `localhost:8080` while developing — the two are separate stores): the history in the IndexedDB database `dust_history`, the rest in `localStorage` |
| Linux | `~/.local/share/dust/` (`$XDG_DATA_HOME`) |
| Windows | `%APPDATA%\dust\`, the roaming application data `dirs` reports |
| macOS | `~/Library/Application Support/dust/` |
| Android | `dust/` in the app's own files folder, which Android names for the user the app runs as: `/data/data/<package>/files/` for the phone's owner, `/data/user/<user>/<package>/files/` for a work profile, another user, Private Space or a cloned app. `dirs` has no `HOME` to work from, so the store asks Android (`Context.getFilesDir()`, over JNI), or failing that works it out from `/proc/self/cmdline` and the process's uid |
| iOS | `Library/Application Support/dust/` inside the app container |

`DUST_DATA_DIR` overrides the directory on every desktop and mobile build,
which is what a portable install uses. A build with no data folder to be
had keeps `dust/` in the temporary folder, never in whatever folder it was
started in. A desktop uninstaller leaves the folder where it is: it is the
user's, and named `dust`, not after the bundle identifier.

The phones back it up themselves. The manifest `dx` writes sets no
`android:allowBackup`, so Android's Auto Backup is on: with backup on in the
phone's settings, the whole files folder, `dust/` included, goes to the
user's Google account, and comes back on a new phone or a reinstall; setting
up a new phone straight from the old one copies it too. That is kept on
purpose, as for most users it is the only copy of the history they have.
Auto Backup takes at most 25 MB of an app's data, though, and past that
backs up none of it. The history is nearly all of Dust's, a few kilobytes a
session, so once the sessions stored pass 20 MB the Backup card says so,
and to save a backup file now and then (`Store::outgrows_device_backup`).
On iOS, iCloud Backup and a backup on a computer take the app container the
same way. The privacy policy and the Privacy card say all of this, platform
by platform (`Platform::kept` in `src/ui/privacy.rs`, which a test holds the
policy to).

A browser keys `localStorage`, IndexedDB, the room it gives them,
`navigator.storage.persist()` and Web Locks by origin, not by path, and a
Pages project site is a path under `https://<owner>.github.io`. So every
other project the account publishes on Pages shares Dust's store: one can
read it, clear it (`localStorage.clear()`, deleting the database, or a user
clearing that site's data), or fill the room, and then a session finished
waits, in the app and in its mark, to be stored until there is room again,
and the app says so. Dust keeps its side clean: every key, database and
lock name it uses starts with `dust_`, and it removes only keys of its own,
by exact name or by its own prefix, never the rest. Giving
the web app an origin of its own ends the sharing: a custom domain set on
the Dust repository's own Pages site (a subdomain such as
`dust.example.org`; one set on the account's user site would carry every
project site along with it), or a user or organisation site
(`<name>.github.io`) that holds only Dust. A new origin starts with an empty
store, so users move with a backup, and the policy and
`PRIVACY_POLICY_URL` change with it.

The web app's page sets a content security policy, in a `<meta>` of
`index.html`, as Pages sets no headers. Scripts come only from the site:
the page's one inline script by its hash, and the code the app builds as
it runs (Dioxus's eval, the history's, the marks' and the tabs' page
scripts) and its WebAssembly, by `'unsafe-eval'` and
`'wasm-unsafe-eval'`. Styles come from the site, inline (Dioxus writes
style attributes) and from Google's sheet of type faces; the faces from
Google, or inline in the stylesheet; images only inline, the icons. The
page connects to the site and to Google Drive alone, and has no frames,
workers, plugins or form posts, and loads nothing else from anywhere, the
site included (`default-src 'none'`). Sign-in is a window of Google's own,
which the page's policy does not govern; `oauth.html`, where it comes back,
runs its own script alone, by its hash, and the privacy page runs none.
Tests in `src/app.rs` hold each page's policy to that. The Pages step adds
the hash of any inline script `dx bundle` puts in the page, and fails on
anything inline a page's policy leaves uncovered
(`scripts/check-site-policy.cjs`), and both browser checks fail on any
breach. A meta cannot set `frame-ancestors`, so another site can still
frame the app. The policy limits what the page loads and where it
connects; it keeps no other page of the same origin from Dust's store.

A session or contest run is marked open while it runs, and the mark goes
once it ends and the store has taken it, or once it is discarded. Each copy of the app marks its own, a
window or a tab of its own, and holds a lock on it for as long as it runs. A
copy that stops lets go of its lock, however it stops: closed, crashed,
killed, reloaded, or rebuilt under `dx serve`. So a mark nobody holds was cut
off, and the app says so, once, on the screen it opens on, or once the
session it starts next is over, if the window that was running it stopped
meanwhile. A mark another window still holds is left to it. The copy that
finds a mark cut off holds its lock in turn, and leaves it where it is
until its own mark holds what the other carried, written and, on desktop
and mobile, flushed to the disk then and there, a wait the app makes only
after it has found a mark cut off; only then does it go. While its own
mark will not take a write, the disk or the page storage full say, it
waits for one that does, or for that mark to go. A copy that stops
before that lets go of it as it stops, and the next copy to look finds it
again: what a mark carried is in one mark or another all along, told of
twice at worst and kept once. Each group
answered, and each QSO logged, puts in the mark what Exit would keep were
it pressed then; the history itself is written only once the session ends.
A session the store has yet to take, for want of room say, rides along in
every mark after it until it is stored (`OpenSession::unstored`), so a copy
that stops first leaves it to the next launch all the same.
A session cut off is kept from its mark as Exit would have kept it, in the
history and towards its level's run, and the app says that it was. It is
kept once the marks that carried it have gone, so its level counts it once;
only while this copy's mark will not take a write is it kept before then,
and a copy that stops in between may count it again. One the store holds
already in a record that cannot be read for now, as its copy stored it
just before it stopped, waits with this copy's mark until that record
reads, and is then the record's; it is kept beside it only once the record
is no more, or is told of as one that could not be read. A record is named
when its session finished or after, just after a later one where the clock
was set back, so every session cut off that finished no later than such a
record is named waits so. Closing
the desktop window, or leaving or reloading the page, ends a session there
and then as Exit does, so nothing is left to find, and stores the settings
with it, a level it earned included. On desktop and
mobile the marks are `open/<id>.json`, each beside the `open/<id>.lock` its
copy holds, and each written whole, under another name first; one a power
cut left empty still says a session was cut off. A
lock file is locked under another name first and only then renamed into
place, so a copy looking for marks never finds one unheld that is about to
be held. In a browser they are `dust_open_session_<id>` in `localStorage`,
each held by a Web Lock of the same name. The browser grants a lock a moment
after it is asked for, so the mark goes in only once its lock is held, and
under the lock a tab looking for marks holds while it looks; it goes again
before its lock is let go of. A tab that finds a mark cut off asks for its
lock under that same look, and holds it until the mark can go. A browser
without Web Locks marks nothing, rather than take a tab still running for
one cut off.

Copies of the app on one device sync one at a time, and lay what each did
over what sync keeps one at a time ([sync.md](sync.md)). On desktop and
mobile each holds a lock file beside `sync.json` for it: `sync-run.lock`
for the whole of a sync, and while the synced copy is deleted, and
`sync.lock` only while it reads what sync keeps, lays what it did over it
and writes it back. They are files of their own rather than `sync.json`
locked, as on Windows a file locked cannot be read through another handle;
they hold nothing, are never deleted, and are let go of however the copy
holding one stops. A browser's tabs hold the Web Lock `dust_sync_run` for a
sync, and none for what sync keeps: a tab reads and writes `dust_sync` in
`localStorage` there and then, while a Web Lock is granted only a moment
after it is asked for, so a change kept under one would wait, and be lost
with a tab closed in that moment. A sync that finds another copy's under
way waits for it, ten minutes at most, then goes on without it, as copies
do where the store takes no lock at all: a browser without Web Locks, or a
folder that takes none, on some network shares. Two copies syncing at once
each plan from what sync kept as they began, which the merge as each ends
does not change: one can send again the deletion of a session the other
let go here, undoing everywhere a putting back made on another device, or
take back a session deleted here by hand meanwhile. Of the settings each
took, the later word on each, by its stamp, is the one kept here. A restore of a backup
holds `restore.lock` beside them (in a browser the Web Lock
`dust_restore`) from its start until neither it nor its Undo is under way
or offered, so a restore in another copy waits for its Undo rather than
take what both bring back for sessions already here, which the Undo would
then delete; where the store takes no lock, restores are made as they come.
The Undo is offered until its message is taken down, after eight seconds,
or another takes its place. A browser that suspends a tab out of sight, as
Safari on iOS does, or freezes it to save power, runs no timer there, and
the tab keeps `dust_restore` meanwhile: a restore in another tab waits, its
card saying so, until that tab is shown again, its message taken down, or
it is closed. No test suspends a page.

Updating the app does not clear any of it. The `localStorage` keys
(`dust_settings`, `dust_auto_adjust_*`, `dust_theme`, `dust_tutorial_done`,
`dust_told_of_paddle_keys`, `dust_asked_about_lock`, `dust_last_backup`,
`dust_open_session_*`, `dust_history_stamp`, and the history older builds
keep whole, `dust_sessions` and `dust_deleted`), the database `dust_history`
and the file names are not versioned; the history says how it is kept, in
`layout.json` or under `layout` in the database (below). Settings files from
older builds still load — new fields fall back to their defaults, and one
unreadable field costs only that field. A settings file from before the AGC threshold went comes back
with a threshold raised under the AGC as the RF gain backed off as far, which
is how the AGC heard it. A setting a later build added, which this one does
not read, is kept and written back with the rest, as are the keys a later
build added to a session, a group, its timing or a counter: a copy of this
build left open beside a newer one never strips them. So are the keys a later
build added to what sync keeps: this build lays what it changes there over
what the store holds as it writes, so they stay as a later build last left
them, and one it let go stays gone. So is a value this
build cannot read under a key it reads, a later build's choice of a filter
shape or a contest, say (`TrainingSettings::unread_values`): read as this
build can, it goes back as it was stored, to the file, a backup and sync
alike, until that setting is changed here, and from then on as chosen here.
In a setting of named entries, the callers' odds or their styles in each
contest, an entry this build does not read, a later build's share or the
styles of a contest it has no name for, stays as it was stored though the
rest of that setting is changed here, a card's reset included; so does one it
reads only in part, a contest's styles with a knob a later build added, until
that entry is changed here. A number past this build's range is not one: it
is read, held to the range, as it always was. A session is stored
once, as the build that stored it wrote it, and never written over, so what
a later build adds to it stays; and one this build writes again, as a restore
adds or fills it in, or an Undo puts it back, keeps every key a later build
added to it: on the session, its groups and their timings, inside a group's
times (`"charTimes"`), and inside a letter's entry of the tally
(`"letterAccuracy"`). The tally's maps themselves, `"letterAccuracy"` and
`"inserted"`, are keyed by one character each: a later build's key there that
is no one character, a prosign's say, makes the session one this build cannot
read, so a change that puts one there raises the format, and a later build
puts what it adds of a letter inside the letter's entry, and anything else
of the tally on the session. Each says the format it is stored in,
`"format"`: the oldest format a build must know to read it
(`SESSION_FORMAT`, `crates/cw-core/src/session.rs`), 1 so far, raised only
by a change an earlier build would misread; one that says none is from
before sessions said it. A build reads every format up to its own. A session
stored in a later one is a later build's: left where it is, and the app says
so once, as saved by a newer Dust; a build that reads it shows it, and a
restore leaves out a backup's copy of the session it says it holds. One that
says no later format and does not read is damaged, as is one whose bytes are
no text at all: left where it is too, and told of as damaged; a restore
brings a backup's copy of it back beside it. One that cannot be read at all
for now, as a file another program holds open for a moment cannot be, is
neither: it is read again at every look at the history, and on its own two,
ten and sixty seconds after it was found so; one that still cannot be read
at the last, or as the app opens again, is told of once, as one that could
not be read, and a restore asked for meanwhile is refused once, as the
backup could store its session a second time. One that reads after it was
told of, then fails once more, is told of again: a failure of its own.
Settings that do not read at all, a `settings.json` edited by hand
and left with a stray comma say, are set aside before the defaults the app
opens with are written over them: `settings.unreadable-*.json` beside it, or
`dust_settings_unreadable` in a browser. So are the sessions of a history
kept whole, by a build from before records, that do not read as it is moved
in (below), and such a history that does not read at all; and if one cannot
even be moved, nothing is saved over it until a later read of it reads, or
sets it aside: a file another program had open for a moment holds back no
more than the saves made meanwhile. The app says what it set aside, once,
and how to have it back. A set-aside history is a list of
sessions, which **Restore a backup** reads too: mended, or in a later version
that reads what this one could not, it puts its sessions back and leaves the
settings alone. On a computer the notice names the file; on a phone or in a
browser the copy stays in the app's own store, out of reach, and a backup is
the way back. A browser keeps the newest five copies of a history kept whole
that it set aside, `dust_sessions_unreadable`, `_2`, `_3`…, and lets the
oldest go first when a new one needs the room; a copy with no room beside
what it is set aside from is moved into that room instead, so a large
history that cannot be read never holds back every save after it.

From contest mode on, a mode added later is written under the name of the
older mode closest to it, plus a mark only newer builds read, so older builds
can still read its sessions: a contest run is stored as a callsign session
whose alphabet fingerprint starts `contest:`, with its log beside the groups.
An older build keeps such a session as practice in the older mode, and drops
what it does not know, the log, the next time it saves; after an upgrade the
mark makes it a contest run again, so it leaves the callsign copy speed and
board, which can shift them a little. A new mode name instead makes every
older build skip the session as unreadable and lose it on its next save, as
builds from before Words do with a Words session. A build that keeps the
history by record loses nothing so: it leaves a session it cannot read where
it is.

### How the history is kept

The history is kept a record a session, and holds every session until it is
deleted: there is no count, no budget, and nothing is let go to make room,
but in a browser that will not open its database (below).
A record is never written over. Its name says where the session goes in the
history and what it holds, `<order>-<hash>`: twenty digits from when it
finished, in milliseconds, raised past the newest where a clock set back
says earlier, so such a session goes after the newest rather than among
older ones; then the FNV-1a hash of the record's bytes (`RecordId`,
`src/persist/record.rs`). Two copies of one build storing one session name
it alike, and store it once; a restore names each session it adds by when it
finished, so two copies restoring one backup store each once too. Builds
from before sessions said their format wrote none, so name a session
otherwise; a move made again over their records finds each session there
all the same, by what it holds less its format.

On a computer or a phone the history is the folder `history/`, beside the
settings (`src/persist/files.rs`):

- `sessions/<id>.json`: a session, as the build that stored it wrote it.
- `deleted/<id>.json`: a tombstone. The session of that name was deleted by
  hand; it holds when and on what day the session finished, and when it was
  deleted, and nothing else of it.
- `layout.json`: how the history is kept, `{"layout": 1, "compatible": 1,
  …}`, and what the move from `sessions.json` took of it, written last.
- `told.json`: the records this build cannot read and has said so of,
  each as a newer Dust's, as damaged, or as one that could not be read;
  one of the last that reads again is noted no more. A build before
  records' formats notes there the records it cannot read, and those stay.
- `failing.json`: the records that could not be read at all as the history
  was last read whole, or at the app's last try, and have not read since,
  so that one that still cannot be read at the next launch is told of
  then.
- `restoring-<n>.json`: held locked while a restore of many sessions stores
  them, so one Dust stopped under is told of as cut short.
- `packs/*.jsonl.gz`: older records packed together, gzip, a record a line.
  No build writes them yet; this one reads them, so it will not miss
  sessions the build that does packs away.

Every file is written whole or not at all: under a temporary name, flushed
to the disk, renamed into place, and the folder flushed too. The files are
read and written on a thread of their own, `dust-history`, one job at a time
in the order asked (`src/persist/worker.rs`), so a long history never holds
the window up. It starts reading the history as the app starts, before the
window or webview is up, counting what it reads, and what it moves, for the
boot screen to say ([The first frame](#the-first-frame)); the app asks
without waiting, and opens once the read is in. The app waits for it as it
closes, three seconds at most, so a session kept as the window closes is
stored before it goes. A change that fails is tried twice more there,
within half a second, before the app is told. One that still fails, or that
the disk has no room for, leaves the session in the app and in its mark,
and it is tried again with the next change, as the app comes back into
sight, and every minute until it is stored.

In a browser the history is the IndexedDB database `dust_history`
(`src/persist/idb.rs`, `src/persist/idb.js`), whose keys and values are all
text, so what a build writes comes back byte for byte: the stores `sessions`
and `deleted`, by record name, as the folders above, and `meta`, which holds
`layout` and `told`. Each change is one transaction, whole or not at all; a
browser that has no room for one keeps none of it, and the app says the
browser is full. The database is opened, and the history read 2,000 records
at a time, while the page's boot screen shows, before the app starts; it
counts them as it goes, against how many the database says it holds, for
the boot screen to say. A move from the page storage is one change, made
whole, and the boot screen says only that it is being made. A
newer Dust that opens it in another tab closes it under this one, which then
asks to be reloaded before it stores anything; a database a newer Dust keeps
is only read.

Two windows or tabs on one history take no lock between them, and need none:
neither ever writes over what the other stored. Each takes up what the other
stored or deleted by listing the records and tombstones there are, as a
screen that shows the history opens and as the window comes back into
sight. A computer or a phone sees whether to by when the folders last
changed, and a browser tab is told by the others as each change is made,
over a `BroadcastChannel` named `dust_history`, and by `dust_history_stamp`,
which a tab that was asleep finds as it wakes. A record missing with no
tombstone was lost in passing, not deleted, and a copy that holds it stores
it again: only a deletion by hand deletes. Tombstones are kept for good,
with no count and no age past which they go, so a copy left open on the
history from before, or an older build's copy of it, never brings a deleted
session back: only Undo, or restoring a backup that holds it, does.

The first time a build that keeps records opens, it moves the history kept
whole, `sessions.json` or `dust_sessions`, in: each session a record, named
from its place in that history, so two copies moving one history name each
session alike. On a computer or a phone the records go in first and
`layout.json` last, so a move cut short is made again, writing no session
twice; in a browser the whole move is one transaction, which two tabs opening
at once make only once. Sessions that do not read are set aside, as above,
and not moved. Where the history kept whole cannot be read just then,
nothing is moved, the app says so, and the move is made at the next launch.

The history kept whole is left byte for byte as it was, and so are
`deleted.json`, `dust_deleted` and the copies set aside: they are where a
build from before records still keeps its history. Such a build never looks
in `history/` or the database. It opens the history as it was at the move,
stores its sessions there, up to its own 500, and remembers its deletions
beside it, but it cannot delete or write over a record. A build that keeps
records takes up, once, each session an older build stores there that
finished no earlier than a day before the newest session the move took
(`takenThrough` in the layout: the newest no later than the clock said as
the move was made, so a clock set ahead holds nothing back; where it took
none, the newest it remembers deleting, and where there was no history kept
whole at all, none, so nothing is cut off) and is neither
here already nor deleted here, and makes here each deletion the older build
made since, once, by what it is, whatever its clock said (`deletionsTaken`,
beside `deletedSeen`, which builds before them go by). A change there that
a build before those rules took up first, by its own, is taken up again by
them: such a build moves `legacy` on, and leaves `takenLegacy`, this
build's note of what it took, as it was.
A record here that a later build stored counts as the session it says it
holds; a damaged one does not, though it may still say one, and the older
build's copy is taken up beside it, as a restore brings a backup's back
beside it, and is left as it is when the older build deletes that session.
While a record here cannot be read at all for now, or a pack of them opened,
nothing is taken up: it may be the session, and the take-up waits for the
next look at which every record reads.
Going back to an older build shows the history as it was at the update,
plus what it stores, and loses nothing kept since: the records stay for the
next update. A history kept by a newer Dust in a way this build cannot
write, one whose `compatible` is past its own, is only read, and the app
says to update Dust.

Where a browser will not open its database, or answers nothing for five
seconds, or will not take the move, the history is kept for that visit as
builds before records kept it: whole, in `dust_sessions`, to a budget of
3,000,000 characters (`HISTORY_BUDGET`, `src/persist/legacy.rs`). Should it
outgrow that, the newest sessions that fit are kept and the oldest let go
(`store_newest_that_fit`), as before. Only what a change stores is written as
this build writes a session; every other session goes back as the history
held it, byte for byte, so a tab of an older build that shares the history
finds each as it left it. The app says all this at every launch
that falls back, and to save a backup; the database is left as it is, and
tried again at the next visit, which takes up what the visit stored as it
takes up an older build's sessions.

### Backups

A browser clears `localStorage` and IndexedDB along with cookies when
someone clears a site's data, and Safari clears both by itself after seven
days without a visit, unless the site is on the home screen. So **Stats → History** has a Backup
card:

- **Save a backup** writes `dust-backup-<date>.json`, the settings and the
  history in one file, somewhere the user can reach it:
  - the web app hands it over as a download;
  - desktop builds put it in Downloads (or Documents, or home), never over
    an earlier one;
  - Android adds it to the shared Downloads folder through the media store,
    which takes no permission from Android 10 on, writing it to the app's
    cache first and copying that in; before that it goes to the app's own
    folder on the shared storage, `Android/data/<package>/files/Download`;
  - iOS puts it in the app's Documents folder, which the Info.plist keys in
    `Dioxus.toml` (`UIFileSharingEnabled`, `LSSupportsOpeningDocumentsInPlace`)
    show in the Files app under On My iPhone, Dust.

  A computer or a phone writes it a session at a time, on the history's own
  thread, each as it is stored, so a history of any length is never held
  whole to be backed up; a browser writes it from its database the same
  way, but whole, to hand it over. Where the store cannot read its own
  history, the file holds every session the app shows instead. A session
  deleted never goes in one.

  The app's own folders, on iOS and on Android before 10, are deleted with
  the app, and the backup in them with the store it backs up: such a file is
  no backup until it is moved out. There the message after saving says so,
  and how (with the Files app, to iCloud Drive or another app; with a file
  manager), the card tells of the last one as saved in Dust's folder, and
  its note says to move each one out to keep it safe.
- **Restore a backup** adds the sessions in the file that are not here yet
  and takes its settings, level included, all but this device's own: its
  volume, and how its keyboard and paddles key (**Swap [ and ] paddles**,
  **Paddles on the keys right of P**), which a backup from a phone has no
  say in on a computer; the card says so before, and the message after, when they
  differed. Undo, in the message that follows,
  puts both back as they were: what the restore added taken out, and what it
  filled in as it was, while whatever another window kept or deleted
  meanwhile stands. A restore asked for while a session or run is under
  way is made once it is over, and so is an Undo clicked then, as the
  message says: either changes the settings, the level among them. An
  Undo clicked just before one began, which the store made only once it
  had, as a browser's database takes a moment to, takes out what the
  restore added then, and puts the settings back once the session or run
  is over. Either waits with the app: closed before the session or run is
  over, the app leaves the restore standing, or, for the second, its
  settings. One window restores at a time: a restore asked for in another
  window while this one's, or its Undo, is under way or still offered
  waits for it, its card saying so, and is made over what the Undo left.
  A failed Undo is offered again. A file that is not a backup, or larger
  than 1 GB, is refused, and the card says why; with no card drawn by
  then, a message does, once no session or run is under way. One past
  50 MB takes a while to read, and the card says so as it reads it; in a
  browser that says it has less room left for the site than the file
  holds, the card says so first, as a restore that does not fit stores
  none of it. A session or a setting the file can no longer read is left
  out, as when the store is read; so is a session past the memory the
  sessions read are held to, in all: about three times the file's length,
  or 16 MB for a short one, for what this build reads of them, and beside
  it 256 MB for what they hold that it does not read, a later build's
  keys, and 300 MB for contest runs' logs, each counted as the memory it
  takes. No history Dust stored reaches them, only a file made up to swell
  far past its length. The message says how many sessions were left out.
  A backup saved by any build restores on any other: the web app's on a
  phone, a phone's on a computer. A build from before records takes one of
  up to 50 MB, and keeps the newest 500 sessions of it.
- After ten sessions without a backup the card says so, and its Save button
  stands out. When the last one was saved is kept under `dust_last_backup`,
  or in `last_backup.txt`. A restored backup counts as the last one only when
  it holds every session here that had finished by the time it was saved: one
  from another device leaves this one's own sessions in no backup. A mark
  later than now, left by a clock set ahead, counts as made now, so the
  sessions after it are counted once the clock is right. A browser never says whether it kept a download,
  so the web app's card says the last backup was handed to the browser, not
  saved.

On a phone a file is chosen through the webview's own picker
(`src/ui/file_pick.rs`), for backups and word lists alike. The renderer asks
its native side for whatever a file input chose, and on Android and iOS the
native side gives nothing back; so there the button opens a file input made
outside the app's page, where the renderer does not reach, and the file is
read in the page and handed back.

After every finished session the web app also asks the browser to keep its
storage (`navigator.storage.persist()`) rather than clear it to make room.
Chrome decides by itself; Firefox asks once. That guards against the browser,
not against someone clearing the site's data: only a backup does.

The history keeps every session you finish, in the order they finished,
whatever the clock said: a session stamped by a clock set back goes after
the newest, not among older ones. A browser gives a site only so much room,
which other Pages sites of the account share; where it has none left, a
session finished is kept in the app and in its mark until there is, and the
app says so.

Sessions can be deleted under **Stats → History**: one from its own row, or
several with **Select**, all of them included. Deleting everything asks once
more. Every deletion can be undone from the message that follows it, for a
few seconds. Deleting a session takes it out of every stat, the calendar,
the streak and the score board, and out of what the sampler learns from; the
level you have reached stays where it is.

Two windows or tabs on one store each store what changed in them, and take
up what the other stored or deleted (above). A session is taken for deleted
only when it was deleted by hand: each deletion is remembered for good, by a
tombstone, and so is taken out again wherever another window still holds the
session. Undo, or a restore that brings the session back, forgets it. A
deletion made by a build from before deletions were remembered at all can
come back from a window that still held the session.

**Stats → Scores** is the score board: the sessions kept on this device,
ranked by what they earned a group, with gold, silver and bronze for the top
three and the personal best on its own card. A session's score is its groups'
points added up, so a longer session scores more for the same copy; points a
group compares a 15-group session with a 50-group one fairly. A longer group
earns more too, and a miss, which costs its whole group, costs it more, so on
the board every group counts as five characters: points a group are what a
session would earn in groups of five, at its speed and the share of characters
it missed. Groups of two compare with groups of eight, and a Koch session with
a callsign one, by their copy, at any accuracy. Every miss counts, and only
sessions of 10 groups or more go on the board, since a handful of lucky groups
says little. Beside the best is your form:
the median of your last 10 sessions, which one bad day hardly moves. A chart
of points a group over time draws the record as it stepped up and the form
as it went, and pointing at a row (or tapping it) marks when that session
was. There is one player, so a row is a place, a score and when it was set.
With more than one mode in the history, the board can show one at a time; a
finished session's results say where it came among the sessions of its own
mode.

Stats are computed across your whole history, and no setting hides any of
it. Changing course — Koch to Mixed, a new unlock order, a level, callsigns —
does not hide what you already did: accuracy over time, letter mastery,
mistakes, the calendar and the streak all count every session, because a
letter is the same letter whichever set it was sent under. Past 1,000
sessions, accuracy over time and the score timeline still draw the newest
100 one by one, and the older ones a week to a point, or a month, so the
whole history fits on the chart.

- The sampler learns what it knows of each character from every session,
  whatever set, order or level sent it. The **Sampling** tab's snapshot lists
  every character you have practised: the ones the current set draws with
  their chance of coming next, and after them, greyed, the ones it does not,
  with what your history says of them.
- **Copy span** and **Copy speed** are each kept per character set, since a
  callsign at 30 WPM is not a two-letter Koch group at 30 WPM. With history
  in more than one set, each card has a pill for every set, the one practised
  now chosen first. A set with no history yet — callsigns for the span, which
  has none — shows the first that has, rather than nothing.
- The Sampling tab's **Group length** and **Speed** cards are forecasts of
  the next session, so they follow the settings it will run with.
