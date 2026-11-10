# Sync

Sync keeps the history and the settings the same on every device a user
turns it on, through a folder of Dust's own in the user's own Google Drive.
It is off until the user turns it on, on the History tab of Stats, and it
is end-to-end encrypted: everything is sealed on the device, with a key
only the user's passphrase opens, before it leaves. Google keeps the files
and cannot read them. The developer has no server and receives nothing.

The code is `src/sync/`; the privacy policy (`docs/privacy.html`) says all
of this to the user, in the words a test holds it to.

## What goes where

Google Drive's **app data folder** (`drive.appdata`) is a folder each app
has in each user's Drive: hidden among their files, reachable by that app
alone (every OAuth client of the one Google Cloud project), and counted
against the user's storage. Drive's settings show it under *Manage apps*,
where the user can delete it. It is the only thing sync asks Google for:
none of the user's own files, nor anything of any other app's. Google counts
the scope non-sensitive, so the app needs only Google's brand verification,
with no security assessment.

The folder holds three kinds of file, each sealed (below):

| File | What it holds | Written |
| --- | --- | --- |
| `dust-keyring` | the data key, wrapped under the passphrase | once, by the first device |
| `dust-p-<random>` | a pack: records sent, deletions, sessions put back | once, never changed |
| `dust-s-<keyed hash>` | one device's settings, level counters, theme and marks, each stamped with when it changed | by that device alone |

## The history

The history is already kept by record (`src/persist/record.rs`): each
session is written once and never over, named by when it finished and a
hash of what it holds, and a deletion is a tombstone kept for good. That is
a set that only grows, beside a set of deletions that only grows: two
devices that each take up everything the other wrote come to the same
history, in whatever order, with nothing to merge. Sync carries exactly
that.

Each sync (`src/sync/run.rs`):

1. lists the folder, and reads the packs it has not read before
   (`DriveState` puts every pack together: of each record, whether some pack
   holds it, and the latest time it was deleted and put back);
2. asks the store what it lists, and which of the drive's records it does
   not list it deleted by hand, and when (`Store::deletions`): none where
   the store cannot say, as missing is never deleted;
3. works out the plan (`src/sync/plan.rs`): records to take, deletions to
   make here, records and deletions to send;
4. makes the local change in one `HistoryChange`, before anything is sent;
5. sends one or more packs (a few megabytes apiece, so a long history is a
   few files, not one per session);
6. merges the settings, the level counters, the theme and the marks.

A record the drive says is deleted (its latest deletion later than its
latest putting back) is deleted here. A session put back here by Undo, or
by a restore, after a deletion the drive holds is sent as *put back*, so
that the deletion does not take it again; a device that let it go takes it
back, and so does a device that deleted it by hand, whose deletion the
drive holds then, sent from it or made on another device as well, rather
than send it again. One a device deleted by hand after the drive's
deletion, by its own clock, is the later word, and is sent all the same:
a putting back made between the two loses to it on every device. Deleted
by hand again after it is put back here, it is a deletion by hand as any
other, whatever was noted of the one before. A putting back as late as a
deletion stands, here as in the drive. Missing is never deleted: a record
is let go only for a deletion. A device updated from a build that noted no
deletion by hand notes those it made that the drive holds, put back since
or not, at its first sync at which the store can say which they are, and
looks again at each sync until then.
A deletion or a putting back is stamped when it is made (a deletion by
hand when its tombstone says, though it is sent later, or as the sync that
sends it began, where a clock set ahead said later), or just after the
latest of the record the device knew the drive to hold by then, where that
is later (`SyncState::stamp_change`): one made after another is the later,
however far apart the devices' clocks are. But where a sync read the drive
and did not send a deletion by hand, as the store could not say which were
deleted, as the sync listed the history before the deletion was made, or
as it was cut short as it sent, the deletion is stamped after what that
sync read as well: a putting back made elsewhere after it by the clock,
read there, loses to it. A putting back made while a sync is under way is
stamped again as it ends, after what that sync read and sent, and a session
put back by hand meanwhile, in that tab or window or in another on the
device, is not let go by that sync, which planned with no note of it, even
where sync is turned off before it ends: the next sends it as put back,
where sync is on still. A putting back noted while sync knows nothing of
the drive, as just after it is turned on anew, when what it knew before
went with it, is stamped again after what the drive holds once its first
sync reads it.

One session can be two records: a backup restored on one device names each
session where it finished (`RecordId::placed`), while the device it came
from named each as it kept it. A record to take that is alike in everything
to one this device holds under another name is its twin: the smaller name
is kept on every device, and the other let go everywhere, as a deletion
sent only once the one kept is in the drive. A twin let go is no deletion
by hand: should another device put its name back, restoring the same
backup, it is let go again, never taken back beside the one kept. Two
records of a session that differ, as one a restore filled in, are both
kept.

What a device keeps of the drive (`SyncState`: the packs read and what they
said) is only so as not to read it all again. Lost, the next sync reads
every pack again and comes to the same. A sync cut short leaves the store
as it was, or with all it took, and the next one sends what this one did
not. A pack the drive made though its answer was lost on the way back, or
though the app stopped before the sync ended, is found there by the next
sync, and the deletions by hand it holds are the drive's from then on, as
they would have been had the answer come (`SyncState::unanswered`, kept
before the pack goes): not sent again, so a putting back made elsewhere
since still stands. One the drive never made is sent again.

## Everything else a device uses

A synced device is set up as every other: the history is one, and so is
everything else a device uses (`src/sync/settings.rs`, `Using`):

- every setting, the volume and the paddle keys among them;
- every run of auto-level counters, the counts towards the next level,
  by its run's key (`level:<key>`): a run moved by a session on one device
  goes on from there on the next, and one a level reset clears is cleared
  everywhere;
- the theme (`mark:theme`), and whether the guided setup (`mark:tour`) and
  the word on the paddle keys (`mark:paddleKeys`) have been shown. A mark is
  only ever set.

Each device writes only its own file of them, every key with when it was
last changed. A device merging takes the newest of each key from every file
and its own, so a change made on one device and another made on another
both come through, and no device writes over another's file. A change is
stamped after the newest stamp of its key the device has taken up, so one
made after another is the newer, however far apart the devices' clocks
are. A setting's value a device's build cannot read, a later build's
choice, is taken up and sent as it was stored, at its own stamp, never as
that build reads it: it stays the later build's on every device until a
device changes that setting; and in a setting of named entries, such as
the callers' odds, what a later build wrote in an entry stays though a
device changes another entry. A device joining a sync takes what the drive
holds, as a restore takes a backup's settings, and adds nothing of its own
until it changes something.

All that stays each device's own: when it last saved a backup file, which
is a file on that device, and a session or run it has under way, which is
stored, and synced, once it ends.

## Encryption

`src/sync/seal.rs`:

- **Data key**: 256 random bits, made as the first device turns sync on.
- **Keyring**: the data key sealed under a key stretched from the
  passphrase with **Argon2id** (19 MiB, 2 passes, 1 lane: OWASP's first
  choice), with a random salt. The stretch is written in the keyring, so a
  later build can stretch harder, up to 256 MiB and 16 passes: a keyring
  asking for more is not opened, as the stretch runs on the app's thread.
- **Sealing**: **XChaCha20-Poly1305**, a fresh random 192-bit nonce for
  every file, the file's name bound in as associated data, so no file can
  stand in for another. Packs are deflated first.
- **Names**: a pack's name is random; a device's settings file is named by
  an HMAC of the device's random id; neither says anything of what it
  holds.

The passphrase never leaves the device it is typed on, and is not kept.
Each device keeps the data key, in what sync keeps
(`Store::save_sync`): `sync.json` in the store's folder on a computer, for
the user alone to read (mode 0600 on Linux and macOS; on Windows an access
list that lets in the user alone and inherits nothing from the folder,
wherever the volume keeps one); `dust/sync.json` in the app's no-backup
folder on Android, which Android's Auto Backup leaves out; `dust_sync` in a
browser's `localStorage`.

What Google can see: how many files there are, how large, and when each
device reads or writes them, with the account and the IP address of each
request. What it cannot: anything of what they hold. A lost passphrase is
unrecoverable by design: the synced copy can then not be read, though each
device keeps what it holds.

On the web, the site's storage is shared with every other Pages site of the
account (`docs/platforms.md`), and so is the data key kept there, as the
history itself is. The access token is kept in memory only.

## Signing in

`src/sync/auth.rs`, one way for each platform, as Google asks each to:

- **Web**: Google's sign-in in a window of its own (OAuth's token flow, for
  a page with no server), sent back to `oauth.html` beside the app
  (`docs/oauth.html`, copied into the site by `web-site.yml`), which hands
  the token to the app over a `BroadcastChannel` and closes. The token
  lasts an hour, in memory only; the next visit asks for a click to sign in
  as it syncs. No Google script is loaded.
- **Computer**: Google's sign-in in the user's own browser, sent back to
  `http://127.0.0.1:<port>` (RFC 8252), with PKCE. The code is traded for a
  token and a refresh token, which is kept to sign in again with no one
  asked.
- **Android**: Google Play services' authorization
  (`Identity.getAuthorizationClient`), asked for by the activity
  (`android/MainActivity.kt`) over JNI. The Gradle hook adds
  `play-services-auth` (`scripts/android-gradle-hook.sh`).
- **iOS**: not yet; the card is not shown.

Requests go straight from the device to `accounts.google.com`,
`oauth2.googleapis.com` and `www.googleapis.com`: the browser's own fetch on
the web, and `ureq` (rustls, with its roots built in) on a thread of its own
elsewhere (`src/sync/http.rs`).

## When it syncs

`src/state/sync.rs`: as the app opens; three seconds after a session is kept,
a deletion or a restore made, or the settings stored; as the app comes back
into sight, if it has not synced for two minutes; and on *Sync now*. Never
while a session or a run is under way: once it is over. A browser whose
sign-in has run out asks for a click instead.

Each sync shows in the corner of the screen, above the nav
(`src/ui/sync_pip.rs`): a small cloud breathing while it runs, a check for
a moment once it is done, then gone; a mark for a few seconds where it went
wrong, the Sync card saying what. Where a browser's sign-in ran out, the
cloud stays, and a tap on it signs in again from any screen, as a browser
opens Google's window only for a tap. The corner is never shown over a
session or a run, and takes no tap but that one.

Two tabs or windows of the app on one device keep what sync keeps in the
same place (`SyncState`), and each lays what it did over what the store
holds as it writes, note by note, rather than writing its own whole over
the other's (`SyncState::merged`): a session one of them puts back by Undo,
a deletion the other's sync sent, a sign-in made in either, and the
settings as the later sync left them all stay. Each sync begins from what
the store holds, and lays what it did over what the store holds as it
ends, so a putting back made in another tab or window meanwhile goes with
the next sync. It begins from the settings the store holds as well: a tab or
window that has not looked since another changed a setting and synced it
takes that change up, rather than send its own older value back as a change
made there. A note another tab or window left out goes only once the drive
shows it sent, or no more to send, as a browser can hand one tab another's
write late. Every such write reads what the store keeps for sync first, on
the app's thread, which is one read of `sync.json`, or of `localStorage`,
more than writing it whole was. They sync one at a time: a sync asked for
while another tab or window syncs waits for it, the corner's cloud breathing
meanwhile, ten minutes at most, then begins from what that one kept. Turning
sync off with the copy waits for it too, so nothing is written into the
folder after the copy is deleted. Where nothing locks, as in a browser
without Web Locks, tabs sync as they come, and the merge alone keeps each
one's notes (`docs/platforms.md`).

Turning sync off on one device forgets the key and the sign-in there; the
copy stays for the other devices. Another tab or window of the app on that
device, which keeps what sync keeps in the same place, finds it gone and
turns sync off too, saying so: a browser's other tabs at once, as the
browser tells each of the write (a `storage` event), and a window as it
comes back into sight, syncs, or keeps what it keeps. One that finds there
instead what another tab or window kept as it turned sync on anew turns off
as well, saying so, rather than write its own over it: opened again, it
syncs as that one does. A session it put back by hand that sync had yet to
send is not forgotten with it: it goes with the sync turned on anew in the
other, or, where sync was turned off there, waits with this tab or window
for the next sync turned on in it; closed first, the tab or window loses it.
Either sync sends it as put back after every deletion of it that sync knows
of, or finds at its first read of the drive: a deletion made on another
device after the putting back among them, which it then undoes. One left off
notes no putting back at all. So, too, a tab or window takes up what another
synced: when this device last synced, which its card shows, so that coming
back into sight it does not sync again just after the other did, and a
sign-in made there, which it signs in with from then on. Neither clears
anything from the store, nor does a tab or window that finds the store so as
sync is turned off in it, by hand or as the copy is gone from the drive.
Turning it off with the copy deletes every file of Dust's from the folder,
the keyring first, and every other device then finds it gone, says so, and
turns sync off too.

## Setting up

A build offers sync only where it is built with an OAuth client for where
it runs (`src/sync/config.rs`). Without one, the card is not shown, and the
app reaches no more than it did before.

1. **Google Cloud project.** Create one at
   <https://console.cloud.google.com>, and enable the **Google Drive API**.
2. **Branding and audience** (Google Auth Platform). App name *Dust*, a
   support email, the home page `https://mashu.github.io/dust/`, the privacy
   policy `https://mashu.github.io/dust/privacy/`, and the authorized
   domain `mashu.github.io`. Google asks for the domain to be verified in
   Search Console, which for `mashu.github.io` is done from the user site
   (`mashu/mashu.github.io`). Audience: external.
3. **Data access.** Add the one scope,
   `https://www.googleapis.com/auth/drive.appdata`. It is non-sensitive:
   publishing to production then needs brand verification only.
4. **Clients.** In the one project, so that they share the one app folder:
   - *Web application*: authorized JavaScript origin
     `https://mashu.github.io`; authorized redirect URI
     `https://mashu.github.io/dust/oauth.html` (and, to try it locally,
     `http://localhost:8080` with `http://localhost:8080/oauth.html`).
   - *Desktop app*: nothing to fill in; it gives an ID and a secret, which
     Google says is no secret in an installed app.
   - *Android*: the package `dev.dust.morse`, and the SHA-1 of each key the
     app is signed with: the release key CI signs the APK with, and Play's
     app signing key (Play Console, *App integrity*) for the store's build.
5. **Repository variables** (Settings, Secrets and variables, Actions,
   *Variables*; none is a secret):
   - `DUST_GOOGLE_WEB_CLIENT_ID`
   - `DUST_GOOGLE_DESKTOP_CLIENT_ID`, `DUST_GOOGLE_DESKTOP_CLIENT_SECRET`
   - `DUST_GOOGLE_ANDROID_SYNC`: `1`, once the Android client exists.

   The workflows hand them to every build. Locally, set the same variables
   in the environment before `dx serve` or `cargo build`.
6. **Play's Data safety form.** Sync sends the user's practice to their own
   Google Drive end-to-end encrypted, unreadable to anyone but the user,
   and to no one else: Google's guidance does not count such data as
   collected. Check the form's current wording before answering.

For local web testing, `dx serve` does not serve `docs/oauth.html`: copy it
into the folder `dx` serves the app from, or try sync on the Pages site.

## Checking it

The Rust tests sync devices through an in-memory folder, through a fake of
Drive's REST API (`src/sync/drive/fake.rs`), and through the app itself
(`src/state/sync/tests.rs`); the computer's sign-in is tested through a real
loopback socket. `scripts/browser-sync-check.cjs` syncs two browsers in
Chromium, on the site as `web-site.yml` builds it, against a fake Google:
the sign-in window, `oauth.html`, the passphrase, a history and settings
crossing over, a sign-in run out, and the copy deleted. It checks too that
nothing reaches Google before sync is on, and that nothing in the drive is
in the clear. A build made without a web client ID offers no sync, and the
check passes saying so.
