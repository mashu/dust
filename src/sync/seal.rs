//! What sync puts in the drive, sealed: encrypted and authenticated on the
//! device, with a key the drive never sees.
//!
//! A data key, 256 random bits made as sync is first turned on, seals
//! everything (XChaCha20-Poly1305, a fresh random nonce each time, the
//! file's name bound in so that no file can stand in for another). The
//! drive holds the data key only wrapped: sealed under a key stretched from
//! the user's passphrase with Argon2id, in the keyring ([`KEYRING`]), which
//! every device opens with the same passphrase. The passphrase never leaves
//! the device it is typed on, nor is it kept there: only the data key is.
//!
//! The names of the files are no clue either: a pack is named at random,
//! and a device's settings by a keyed hash of the device ([`Keys::name`]).

use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use super::text::{hex, random, unhex};

/// The keyring's name in the drive: the one file whose name says what it
/// is, as every device has to find it before it can open anything else.
pub const KEYRING: &str = "dust-keyring";
/// What the keyring's wrapped key is bound to.
const KEYRING_BOUND: &[u8] = b"dust-keyring";
/// The layout of a sealed file: this byte, the nonce, then the ciphertext
/// with its tag.
const SEALED_LAYOUT: u8 = 1;
const NONCE_LEN: usize = 24;
/// The fewest characters a new passphrase may have.
pub const MIN_PASSPHRASE: usize = 8;

/// The key everything in the drive is sealed with.
#[derive(Clone, PartialEq, Eq)]
pub struct DataKey([u8; 32]);

impl DataKey {
    /// A new key, at random.
    fn fresh() -> Option<Self> {
        random::<32>().map(Self)
    }

    /// The key in hex, as this device keeps it.
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    /// The key a device kept in hex.
    pub fn from_hex(text: &str) -> Option<Self> {
        let bytes: [u8; 32] = unhex(text)?.try_into().ok()?;
        Some(Self(bytes))
    }
}

/// Never shown, not even in a test's failure.
impl fmt::Debug for DataKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DataKey(..)")
    }
}

/// How hard a passphrase is stretched into a key: Argon2id's memory in
/// kibibytes, its passes, and its lanes. Written into the keyring, so a
/// later build can stretch harder without stranding what an earlier one
/// sealed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stretch {
    pub m: u32,
    pub t: u32,
    pub p: u32,
}

impl Stretch {
    /// OWASP's first choice for Argon2id: 19 MiB, two passes, one lane. A
    /// moment on a phone or in a browser, once for each device that joins.
    pub const DEFAULT: Self = Self {
        m: 19_456,
        t: 2,
        p: 1,
    };

    /// As little as Argon2 takes: for tests, which open keyrings by the
    /// dozen.
    #[cfg(test)]
    pub const QUICK: Self = Self { m: 8, t: 1, p: 1 };

    /// These, if they are any a device can stretch with in reason: a
    /// keyring asking for more memory than a phone has, or passes enough
    /// to hang it, is not opened at all.
    fn checked(self) -> Option<Self> {
        let sane = (8..=1_048_576).contains(&self.m)
            && (1..=64).contains(&self.t)
            && (1..=16).contains(&self.p)
            && self.m >= 8 * self.p;
        sane.then_some(self)
    }
}

/// The keyring as the drive holds it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyringFile {
    /// The layout of the keyring.
    dust_sync: u32,
    /// How the passphrase is stretched: Argon2id, always, for now.
    alg: String,
    #[serde(flatten)]
    stretch: Stretch,
    /// The salt it is stretched with, in hex.
    salt: String,
    /// The data key, sealed under the stretched passphrase, in hex.
    key: String,
}

const KEYRING_LAYOUT: u32 = 1;
const ARGON2ID: &str = "argon2id";

/// Why a keyring was not opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unlock {
    /// The passphrase is not the one it was made with.
    Wrong,
    /// It is no keyring this build can open: damaged, or written by a later
    /// one.
    Unreadable,
}

/// A passphrase as it is stretched: as typed, but for the spaces around it,
/// which a phone's keyboard puts in unasked.
fn as_stretched(passphrase: &str) -> &[u8] {
    passphrase.trim().as_bytes()
}

/// `passphrase` stretched with `salt`.
fn stretched(passphrase: &str, salt: &[u8], stretch: Stretch) -> Option<[u8; 32]> {
    let stretch = stretch.checked()?;
    let params = argon2::Params::new(stretch.m, stretch.t, stretch.p, Some(32)).ok()?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(as_stretched(passphrase), salt, &mut key)
        .ok()?;
    Some(key)
}

/// A new keyring for `passphrase`, around a new data key: the file to put
/// in the drive, and the key. None only where the device has no randomness
/// to make them with.
pub fn new_keyring(passphrase: &str, stretch: Stretch) -> Option<(Vec<u8>, DataKey)> {
    let key = DataKey::fresh()?;
    let salt = random::<16>()?;
    let wrapping = stretched(passphrase, &salt, stretch)?;
    let wrapped = seal_with(&wrapping, KEYRING_BOUND, &key.0)?;
    let file = KeyringFile {
        dust_sync: KEYRING_LAYOUT,
        alg: ARGON2ID.into(),
        stretch,
        salt: hex(&salt),
        key: hex(&wrapped),
    };
    let raw = serde_json::to_vec(&file).ok()?;
    Some((raw, key))
}

/// The data key the keyring `raw` holds, opened with `passphrase`.
pub fn open_keyring(raw: &[u8], passphrase: &str) -> Result<DataKey, Unlock> {
    let file: KeyringFile = serde_json::from_slice(raw).map_err(|_| Unlock::Unreadable)?;
    if file.dust_sync != KEYRING_LAYOUT || file.alg != ARGON2ID {
        return Err(Unlock::Unreadable);
    }
    let salt = unhex(&file.salt).ok_or(Unlock::Unreadable)?;
    let wrapped = unhex(&file.key).ok_or(Unlock::Unreadable)?;
    let wrapping = stretched(passphrase, &salt, file.stretch).ok_or(Unlock::Unreadable)?;
    let key = open_with(&wrapping, KEYRING_BOUND, &wrapped).ok_or(Unlock::Wrong)?;
    let key: [u8; 32] = key.try_into().map_err(|_| Unlock::Unreadable)?;
    Ok(DataKey(key))
}

/// `plain` sealed under `key`, bound to `bound`.
fn seal_with(key: &[u8; 32], bound: &[u8], plain: &[u8]) -> Option<Vec<u8>> {
    let nonce = random::<NONCE_LEN>()?;
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    let sealed = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plain,
                aad: bound,
            },
        )
        .ok()?;
    let mut out = Vec::with_capacity(1 + NONCE_LEN + sealed.len());
    out.push(SEALED_LAYOUT);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Some(out)
}

/// What `sealed` holds, sealed under `key` and bound to `bound`: none where
/// it was sealed otherwise, or has been changed since.
fn open_with(key: &[u8; 32], bound: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let (&layout, rest) = sealed.split_first()?;
    if layout != SEALED_LAYOUT || rest.len() < NONCE_LEN {
        return None;
    }
    let (nonce, body) = rest.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().ok()?;
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: body,
                aad: bound,
            },
        )
        .ok()
}

/// The keyed hash of `message` under `key`.
fn keyed(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key)
        .expect("HMAC takes a key of any length");
    mac.update(message);
    mac.finalize().into_bytes().into()
}

/// What a data key seals and names with: a key of each, made from it, so
/// that neither use gives anything of the other away.
#[derive(Clone)]
pub struct Keys {
    seal: [u8; 32],
    names: [u8; 32],
}

impl Keys {
    pub fn of(key: &DataKey) -> Self {
        Self {
            seal: keyed(&key.0, b"dust sync: seal"),
            names: keyed(&key.0, b"dust sync: names"),
        }
    }

    /// `plain` sealed to be put in the drive as `name`.
    pub fn seal(&self, name: &str, plain: &[u8]) -> Option<Vec<u8>> {
        seal_with(&self.seal, name.as_bytes(), plain)
    }

    /// What `sealed`, found in the drive as `name`, holds: none where this
    /// key did not seal it, under that name, or it has been changed since.
    pub fn open(&self, name: &str, sealed: &[u8]) -> Option<Vec<u8>> {
        open_with(&self.seal, name.as_bytes(), sealed)
    }

    /// The name of the file of `kind` that stands for `what`, as `prefix`
    /// and a keyed hash of both: the same on every device with this key,
    /// and no clue to anyone without it.
    pub fn name(&self, prefix: &str, kind: &str, what: &str) -> String {
        let mut message = Vec::with_capacity(kind.len() + 1 + what.len());
        message.extend_from_slice(kind.as_bytes());
        message.push(0);
        message.extend_from_slice(what.as_bytes());
        format!("{prefix}{}", hex(&keyed(&self.names, &message)[..16]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A keyring opens with its passphrase, around spaces typed with it,
    /// and with nothing else.
    #[test]
    fn a_keyring_opens_with_its_passphrase_and_no_other() {
        let (raw, key) = new_keyring("correct horse", Stretch::QUICK).expect("a keyring");
        assert_eq!(open_keyring(&raw, "correct horse"), Ok(key.clone()));
        assert_eq!(open_keyring(&raw, " correct horse \n"), Ok(key.clone()));
        assert_eq!(open_keyring(&raw, "correct horsE"), Err(Unlock::Wrong));
        assert_eq!(open_keyring(&raw, ""), Err(Unlock::Wrong));
        // What it holds says nothing of the key.
        let text = String::from_utf8(raw.clone()).expect("JSON");
        assert!(!text.contains(&key.to_hex()), "{text}");
        assert!(text.contains("\"alg\":\"argon2id\""), "{text}");
        // Another keyring of the same passphrase holds another key.
        let (other, other_key) = new_keyring("correct horse", Stretch::QUICK).expect("a keyring");
        assert_ne!(other, raw);
        assert_ne!(other_key, key);
        assert_eq!(format!("{key:?}"), "DataKey(..)");
        assert_eq!(DataKey::from_hex(&key.to_hex()), Some(key));
        assert_eq!(DataKey::from_hex("00"), None);
    }

    /// The default stretch is the one a keyring is made with in the app,
    /// and opens as the quick one does.
    #[test]
    fn a_keyring_stretched_as_the_app_stretches_it_opens() {
        let (raw, key) = new_keyring("a passphrase", Stretch::DEFAULT).expect("a keyring");
        let file: KeyringFile = serde_json::from_slice(&raw).expect("JSON");
        assert_eq!(file.stretch, Stretch::DEFAULT);
        assert_eq!(open_keyring(&raw, "a passphrase"), Ok(key));
    }

    /// A keyring damaged, from a later build, or asking for more than any
    /// device should stretch with, is not opened: nor taken for a wrong
    /// passphrase.
    #[test]
    fn a_keyring_that_is_not_one_this_build_opens_says_so() {
        let (raw, _) = new_keyring("passphrase", Stretch::QUICK).expect("a keyring");
        let mut file: serde_json::Value = serde_json::from_slice(&raw).expect("JSON");
        assert_eq!(
            open_keyring(b"not json", "passphrase"),
            Err(Unlock::Unreadable)
        );
        let edited = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut copy = file.clone();
            edit(&mut copy);
            open_keyring(copy.to_string().as_bytes(), "passphrase")
        };
        assert_eq!(
            edited(&|f| f["dustSync"] = 2.into()),
            Err(Unlock::Unreadable)
        );
        assert_eq!(
            edited(&|f| f["alg"] = "scrypt".into()),
            Err(Unlock::Unreadable)
        );
        assert_eq!(
            edited(&|f| f["m"] = 4_000_000.into()),
            Err(Unlock::Unreadable)
        );
        assert_eq!(edited(&|f| f["t"] = 0.into()), Err(Unlock::Unreadable));
        assert_eq!(
            edited(&|f| f["salt"] = "zz".into()),
            Err(Unlock::Unreadable)
        );
        // A key changed is a key that does not open, as a wrong passphrase.
        let key = file["key"].as_str().expect("hex").to_string();
        let flipped = format!(
            "{}{}",
            &key[..key.len() - 1],
            if key.ends_with('0') { "1" } else { "0" }
        );
        file["key"] = flipped.into();
        assert_eq!(
            open_keyring(file.to_string().as_bytes(), "passphrase"),
            Err(Unlock::Wrong)
        );
    }

    /// What is sealed opens under its own name with its own key, and under
    /// no other name, nor key, nor once a byte of it has changed.
    #[test]
    fn a_sealed_file_opens_only_as_itself() {
        let (_, key) = new_keyring("passphrase", Stretch::QUICK).expect("a keyring");
        let keys = Keys::of(&key);
        let sealed = keys.seal("dust-p-1", b"what it holds").expect("sealed");
        assert_eq!(
            keys.open("dust-p-1", &sealed).as_deref(),
            Some(&b"what it holds"[..])
        );
        assert_eq!(keys.open("dust-p-2", &sealed), None, "another name");
        let (_, other) = new_keyring("passphrase", Stretch::QUICK).expect("a keyring");
        assert_eq!(
            Keys::of(&other).open("dust-p-1", &sealed),
            None,
            "another key"
        );
        for at in [0, 1, 10, sealed.len() - 1] {
            let mut changed = sealed.clone();
            changed[at] ^= 1;
            assert_eq!(keys.open("dust-p-1", &changed), None, "byte {at}");
        }
        assert_eq!(keys.open("dust-p-1", &sealed[..20]), None, "cut short");
        assert_eq!(keys.open("dust-p-1", &[]), None);
        // Sealed twice, the same bytes are two files that look nothing alike.
        let again = keys.seal("dust-p-1", b"what it holds").expect("sealed");
        assert_ne!(again, sealed);
        assert!(
            !sealed.windows(4).any(|w| w == b"what"),
            "nothing in the clear"
        );
    }

    /// A name is the same for every device with the key, and another for
    /// another key, kind or thing named.
    #[test]
    fn names_are_keyed() {
        let (_, key) = new_keyring("passphrase", Stretch::QUICK).expect("a keyring");
        let keys = Keys::of(&key);
        let name = keys.name("dust-s-", "settings", "device one");
        assert!(
            name.starts_with("dust-s-") && name.len() == "dust-s-".len() + 32,
            "{name}"
        );
        assert_eq!(
            Keys::of(&key).name("dust-s-", "settings", "device one"),
            name
        );
        assert_ne!(keys.name("dust-s-", "settings", "device two"), name);
        assert_ne!(keys.name("dust-s-", "other", "device one"), name);
        let (_, other) = new_keyring("passphrase", Stretch::QUICK).expect("a keyring");
        assert_ne!(
            Keys::of(&other).name("dust-s-", "settings", "device one"),
            name
        );
        assert!(!name.contains("device"));
    }
}
