//! A pack: what one sync of one device adds to the drive, in one file. The
//! records it stores there, the sessions it deleted, and those it put back
//! after a deletion the drive already holds ([`Pack`]); written as JSON,
//! packed with deflate, then sealed ([`super::seal`]).
//!
//! A pack is written once and never changed, and what the drive holds is
//! every pack there is, so packs are only ever added: two devices syncing at
//! once each add their own, and neither can undo what the other wrote. A
//! record two devices both send is in two packs, and is one record still,
//! by its name ([`RecordId`]). Packs are many sessions apiece, so that a
//! long history goes to the drive, and comes from it, in a few files rather
//! than one for each session: each a request of its own.

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use serde::{Deserialize, Serialize};

use super::seal::Keys;
use super::text::random_hex;
use crate::persist::record::RecordId;

/// What every pack's name starts with: the rest is at random.
pub const PACK_PREFIX: &str = "dust-p-";
/// The layout of a pack.
pub const PACK_LAYOUT: u32 = 1;
/// The most a pack holds of sessions as they are written, before it is
/// packed: four megabytes, under the five Drive takes in one upload
/// however little it packs.
const PACK_BYTES: usize = 4_000_000;
/// The most a pack is read to, unpacked: far more than any pack holds, and
/// no more, whatever the file says.
const UNPACKED_MAX: u64 = 64_000_000;

/// What a pack holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    pub dust_pack: u32,
    /// The device that wrote it ([`super::state::SyncState::device`]).
    pub device: String,
    /// When, in milliseconds since the epoch.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub put: Vec<Put>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forget: Vec<Forget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revive: Vec<Revive>,
}

/// A record, as the device that sent it stored it: its name, and the bytes
/// written for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Put {
    pub id: String,
    pub s: String,
}

/// A session deleted by hand, and when.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Forget {
    pub id: String,
    pub at: u64,
}

/// A session put back, by Undo or a restore, after a deletion the drive
/// holds, and when: with the record, for a device that let it go to take
/// it again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Revive {
    pub id: String,
    pub at: u64,
    pub s: String,
}

impl Put {
    pub fn record_id(&self) -> Option<RecordId> {
        RecordId::parse(&self.id)
    }
}

impl Forget {
    pub fn record_id(&self) -> Option<RecordId> {
        RecordId::parse(&self.id)
    }
}

impl Revive {
    pub fn record_id(&self) -> Option<RecordId> {
        RecordId::parse(&self.id)
    }
}

/// A new pack's name.
pub fn pack_name() -> Option<String> {
    random_hex::<16>().map(|name| format!("{PACK_PREFIX}{name}"))
}

/// `pack`, packed and sealed to go in the drive as `name`.
pub fn seal_pack(keys: &Keys, name: &str, pack: &Pack) -> Option<Vec<u8>> {
    let json = serde_json::to_vec(pack).ok()?;
    let mut packer = DeflateEncoder::new(Vec::new(), Compression::default());
    packer.write_all(&json).ok()?;
    keys.seal(name, &packer.finish().ok()?)
}

/// Why a pack found in the drive was not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unopened {
    /// It is no pack this key sealed under its name, or it was changed
    /// since: damaged, say. It opens no better the next time.
    Damaged,
    /// It opened, but holds no pack this build reads: one a later build
    /// wrote, which this one reads once updated.
    Later,
}

/// The pack `sealed`, found in the drive as `name`, or why not.
pub fn open_pack(keys: &Keys, name: &str, sealed: &[u8]) -> Result<Pack, Unopened> {
    let packed = keys.open(name, sealed).ok_or(Unopened::Damaged)?;
    let mut json = Vec::new();
    DeflateDecoder::new(packed.as_slice())
        .take(UNPACKED_MAX)
        .read_to_end(&mut json)
        .map_err(|_| Unopened::Later)?;
    let pack: Pack = serde_json::from_slice(&json).map_err(|_| Unopened::Later)?;
    if pack.dust_pack == PACK_LAYOUT {
        Ok(pack)
    } else {
        Err(Unopened::Later)
    }
}

/// What one sync sends, in as many packs as it takes to keep each to its
/// size ([`PACK_BYTES`]): the deletions go in the first, being small, and
/// the sessions put back and the records fill it and those after it. None
/// where there is nothing to send.
pub fn packs_of(
    device: &str,
    at: u64,
    put: Vec<Put>,
    forget: Vec<Forget>,
    revive: Vec<Revive>,
) -> Vec<Pack> {
    let fresh = || Pack {
        dust_pack: PACK_LAYOUT,
        device: device.to_string(),
        at,
        ..Pack::default()
    };
    // A pack holding a session already is full where `size` more would
    // take it past its size: one larger than a pack still goes, alone.
    let full = |pack: &Pack, bytes: usize, size: usize| {
        !(pack.put.is_empty() && pack.revive.is_empty()) && bytes + size > PACK_BYTES
    };
    let mut packs = Vec::new();
    let mut pack = Pack { forget, ..fresh() };
    let mut bytes: usize = 0;
    for one in revive {
        let size = one.id.len() + one.s.len();
        if full(&pack, bytes, size) {
            packs.push(std::mem::replace(&mut pack, fresh()));
            bytes = 0;
        }
        bytes += size;
        pack.revive.push(one);
    }
    for one in put {
        let size = one.id.len() + one.s.len();
        if full(&pack, bytes, size) {
            packs.push(std::mem::replace(&mut pack, fresh()));
            bytes = 0;
        }
        bytes += size;
        pack.put.push(one);
    }
    if !(pack.put.is_empty() && pack.forget.is_empty() && pack.revive.is_empty()) {
        packs.push(pack);
    }
    packs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::seal::{Stretch, new_keyring};

    fn keys() -> Keys {
        Keys::of(
            &new_keyring("passphrase", Stretch::QUICK)
                .expect("a keyring")
                .1,
        )
    }

    fn put(n: usize, size: usize) -> Put {
        Put {
            id: RecordId {
                order: n as u64,
                nonce: 7,
            }
            .name(),
            s: "x".repeat(size),
        }
    }

    /// A pack comes back as it went, under its own name and key only, and
    /// what it holds is in no way readable in the drive.
    #[test]
    fn a_pack_comes_back_as_it_went() {
        let keys = keys();
        let name = pack_name().expect("a name");
        assert!(name.starts_with(PACK_PREFIX) && name.len() == PACK_PREFIX.len() + 32);
        let pack = Pack {
            dust_pack: PACK_LAYOUT,
            device: "phone".into(),
            at: 12,
            put: vec![Put {
                id: RecordId { order: 5, nonce: 9 }.name(),
                s: r#"{"date":"2026-11-01","groups":["KMURE"]}"#.into(),
            }],
            forget: vec![Forget {
                id: RecordId { order: 3, nonce: 1 }.name(),
                at: 11,
            }],
            revive: vec![Revive {
                id: RecordId { order: 2, nonce: 1 }.name(),
                at: 12,
                s: "{}".into(),
            }],
        };
        let sealed = seal_pack(&keys, &name, &pack).expect("sealed");
        assert_eq!(open_pack(&keys, &name, &sealed), Ok(pack.clone()));
        assert!(!sealed.windows(5).any(|w| w == b"KMURE"));
        assert_eq!(
            open_pack(&keys, "dust-p-other", &sealed),
            Err(Unopened::Damaged)
        );
        assert_eq!(
            open_pack(&self::keys(), &name, &sealed),
            Err(Unopened::Damaged)
        );
        assert_eq!(
            pack.put[0].record_id(),
            Some(RecordId { order: 5, nonce: 9 })
        );
        assert_eq!(
            Put {
                id: "nonsense".into(),
                s: String::new()
            }
            .record_id(),
            None
        );
        // A pack of another layout, sealed as well as any, is not read, and
        // is no damaged one.
        let later = Pack {
            dust_pack: 2,
            ..pack
        };
        let sealed = seal_pack(&keys, &name, &later).expect("sealed");
        assert_eq!(open_pack(&keys, &name, &sealed), Err(Unopened::Later));
        let unpacked = keys.seal(&name, b"not deflated").expect("sealed");
        assert_eq!(open_pack(&keys, &name, &unpacked), Err(Unopened::Later));
    }

    /// What a later build adds to a pack is passed over, and the rest read.
    #[test]
    fn a_pack_with_more_than_this_build_knows_still_reads() {
        let keys = keys();
        let json = br#"{"dustPack":1,"device":"d","at":1,"later":[1],"put":[{"id":"x","s":"{}","also":2}]}"#;
        let mut packer = DeflateEncoder::new(Vec::new(), Compression::default());
        packer.write_all(json).expect("packed");
        let sealed = keys
            .seal("dust-p-1", &packer.finish().expect("packed"))
            .expect("sealed");
        let pack = open_pack(&keys, "dust-p-1", &sealed).expect("a pack");
        assert_eq!(pack.put.len(), 1);
        assert!(pack.forget.is_empty() && pack.revive.is_empty());
    }

    /// A long history goes in packs of no more than their size, every
    /// session once, in order; and nothing to send is no pack.
    #[test]
    fn a_long_history_goes_in_packs_of_their_size() {
        assert!(packs_of("d", 1, Vec::new(), Vec::new(), Vec::new()).is_empty());
        let puts: Vec<Put> = (0..25).map(|n| put(n, 400_000)).collect();
        let forget = vec![Forget {
            id: put(99, 0).id,
            at: 3,
        }];
        let packs = packs_of("d", 1, puts.clone(), forget.clone(), Vec::new());
        assert!(packs.len() >= 3, "{}", packs.len());
        assert_eq!(packs[0].forget, forget);
        assert!(packs[1..].iter().all(|pack| pack.forget.is_empty()));
        for pack in &packs {
            let bytes: usize = pack.put.iter().map(|p| p.id.len() + p.s.len()).sum();
            assert!(bytes <= PACK_BYTES, "{bytes}");
            assert_eq!((pack.device.as_str(), pack.at), ("d", 1));
        }
        let all: Vec<Put> = packs.into_iter().flat_map(|pack| pack.put).collect();
        assert_eq!(all, puts);
        // One session larger than a pack still goes, in a pack of its own.
        let packs = packs_of(
            "d",
            1,
            vec![put(1, PACK_BYTES + 10), put(2, 10)],
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(packs.len(), 2);
        // Deletions alone are a pack.
        assert_eq!(packs_of("d", 1, Vec::new(), forget, Vec::new()).len(), 1);
    }

    /// Sessions put back go in packs of no more than their size too, every
    /// one once, in order: a long history deleted and put back by Undo is
    /// sent, and not in one pack past what Drive takes.
    #[test]
    fn sessions_put_back_go_in_packs_of_their_size() {
        let revive: Vec<Revive> = (0..25)
            .map(|n| {
                let one = put(n, 400_000);
                Revive {
                    id: one.id,
                    at: 9,
                    s: one.s,
                }
            })
            .collect();
        let forget = vec![Forget {
            id: put(99, 0).id,
            at: 3,
        }];
        let packs = packs_of("d", 1, vec![put(50, 10)], forget.clone(), revive.clone());
        assert!(packs.len() >= 3, "{}", packs.len());
        assert_eq!(packs[0].forget, forget);
        for pack in &packs {
            let bytes: usize = pack
                .put
                .iter()
                .map(|p| p.id.len() + p.s.len())
                .chain(pack.revive.iter().map(|r| r.id.len() + r.s.len()))
                .sum();
            assert!(bytes <= PACK_BYTES, "{bytes}");
        }
        let all: Vec<Revive> = packs.iter().flat_map(|pack| pack.revive.clone()).collect();
        assert_eq!(all, revive);
        assert_eq!(packs.iter().map(|pack| pack.put.len()).sum::<usize>(), 1);
    }

    /// A sealed pack is a fraction of what it holds: sessions are JSON, and
    /// pack well.
    #[test]
    fn sessions_pack_small() {
        let keys = keys();
        let session = serde_json::to_string(&crate::testing::stored_session(
            "2026-11-01",
            &[("KMURE", "KMURE"), ("PAXTO", "PAXTE"), ("QZLRV", "QZLRV")],
        ))
        .expect("JSON");
        let pack = Pack {
            dust_pack: PACK_LAYOUT,
            put: (0..200)
                .map(|n| Put {
                    id: RecordId { order: n, nonce: n }.name(),
                    s: session.clone(),
                })
                .collect(),
            ..Pack::default()
        };
        let sealed = seal_pack(&keys, "dust-p-1", &pack).expect("sealed");
        let raw = serde_json::to_vec(&pack).expect("JSON").len();
        assert!(sealed.len() * 4 < raw, "{} of {raw}", sealed.len());
    }
}
