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

The project `dx` generates needs three things put right, and `dx` 0.7 has a
setting for none of them. Every bundle command below, in CI as well, first
installs a Gradle init hook, [`scripts/android-gradle-hook.sh`](../scripts/android-gradle-hook.sh),
that does all three:

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

The screen stays on through a session — the count down, every send and
every answer — through the window's own keep-screen-on flag
(`FLAG_KEEP_SCREEN_ON`), set and cleared over JNI on the activity's thread
([`src/keep_awake.rs`](../src/keep_awake.rs)). It needs no permission, and the
system lets go of it with the window. The answer box asks for no on-screen
keyboard (`inputmode="none"`): the training screen has a keypad of its own, so
nothing slides in over the groups. A keyboard plugged in or paired still types
into the box.

The Morse, the band noise and static, and the paddle sidetone are mixed into
one output stream, so there is no second stream for a device to refuse — some
Android devices will not open two at once.

If a toolchain bring-up ever blocks on oboe, `--features mobile-silent` builds
the same app with a player that keeps a session's timing but makes no sound,
and the practice screen says so.

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
Pages workflow has published it will fail the check.

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

CI type-checks the simulator target on every PR, which needs no Apple account
and covers the audio-session code and cpal's CoreAudio backend.

## Desktop bundles

`dx bundle --platform desktop --release` writes to
`target/dx/dust/release/linux/` and friends. Desktop is the system webview
(WebKitGTK on Linux, WebView2 on Windows, WKWebView on macOS). macOS builds
are unsigned (right-click → Open the first time). Windows needs WebView2,
already present on typical Windows 10/11.

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

A session or contest run is marked open while it runs, and the mark goes
once it ends, stored or discarded. Each copy of the app marks its own, a
window or a tab of its own, and holds a lock on it for as long as it runs. A
copy that stops lets go of its lock, however it stops: closed, crashed,
killed, reloaded, or rebuilt under `dx serve`. So a mark nobody holds was cut
off, and the app says so, once, on the screen it opens on, or once the
session it starts next is over, if the window that was running it stopped
meanwhile. A mark another window still holds is left to it. On desktop and
mobile the marks are `open/<id>.json`, each beside the `open/<id>.lock` its
copy holds; one a power cut left empty still says a session was cut off. In
a browser they are `dust_open_session_<id>` in `localStorage`, each held by
a Web Lock of the same name. A browser without Web Locks marks nothing,
rather than take a tab still running for one cut off.

Updating the app does not clear any of it. The storage keys (`dust_settings`,
`dust_sessions`, `dust_auto_adjust_*`, `dust_theme`, `dust_tutorial_done`,
`dust_last_backup`, `dust_open_session_*`) and
file names are not versioned, and settings files from older builds still load
— new fields fall back to their defaults, and one unreadable field costs only
that field. A history that
cannot be read at all is set aside beside the new one rather than written over,
and if it cannot even be moved, nothing is saved over it.

From contest mode on, a mode added later is written under the name of the
older mode closest to it, plus a mark only newer builds read, so older builds
can still read its sessions: a contest run is stored as a callsign session
whose alphabet fingerprint starts `contest:`, with its log beside the groups.
An older build keeps such a session as practice in the older mode, and drops
what it does not know, the log, the next time it saves; after an upgrade the
mark makes it a contest run again, so it leaves the callsign copy speed and
board, which can shift them a little. A new mode name instead makes every
older build skip the session as unreadable and lose it on its next save, as
builds from before Words do with a Words session.

### Backups

A browser clears `localStorage` along with cookies when someone clears a
site's data, and Safari clears it by itself after seven days without a visit,
unless the site is on the home screen. So **Stats → History** has a Backup
card:

- **Save a backup** writes `dust-backup-<date>.json`, the settings and the
  history in one file, somewhere the user can reach it:
  - the web app hands it over as a download;
  - desktop builds put it in Downloads (or Documents, or home), never over
    an earlier one;
  - Android adds it to the shared Downloads folder through the media store,
    which takes no permission from Android 10 on; before that it goes to the
    app's own folder on the shared storage, `Android/data/<package>/files/Download`;
  - iOS puts it in the app's Documents folder, which the Info.plist keys in
    `Dioxus.toml` (`UIFileSharingEnabled`, `LSSupportsOpeningDocumentsInPlace`)
    show in the Files app under On My iPhone, Dust.
- **Restore a backup** adds the sessions in the file that are not here yet
  and takes its settings, level included. Undo, in the message that follows,
  puts both back as they were. A file that is not a backup, or far too large
  to be one, is refused, and the card says why. A session or a setting the
  file can no longer read is left out, as when the store is read. A backup
  saved by any build restores on any other: the web app's on a phone, a
  phone's on a computer.
- After ten sessions without a backup the card says so, and its Save button
  stands out. When the last one was saved is kept under `dust_last_backup`,
  or in `last_backup.txt`.

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

The history keeps your newest 500 sessions. A browser also gives each site only
a few megabytes; should the history outgrow that — hundreds of sessions of
hundreds of groups, or long contest runs, which take more room — the newest
that fit are kept, and the app says so.

Sessions can be deleted under **Stats → History**: one from its own row, or
several with **Select**, all of them included. Deleting everything asks once
more. Every deletion can be undone from the message that follows it, for a
few seconds. Deleting a session takes it out of every stat, the calendar,
the streak and the score board, and out of what the sampler learns from; the
level you have reached stays where it is.

**Stats → Scores** is the score board: the sessions kept on this device,
ranked by what they earned a group, with gold, silver and bronze for the top
three and the personal best on its own card. A session's score is its groups'
points added up, so a longer session scores more for the same copy; points a
group compares a 15-group session with a 50-group one fairly. It is the mean,
so every miss counts, and only sessions of 10 groups or more go on the board,
since a handful of lucky groups says little. Beside the best is your form:
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
letter is the same letter whichever set it was sent under.

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
