//! Sync: the history and the settings kept in step between the user's own
//! devices, through a folder of Dust's own in the user's own Google Drive
//! (its app data folder, which only Dust can open: `drive.appdata`). Off
//! until the user turns it on. Everything it puts there is sealed on the
//! device first, with a key only a passphrase of the user's opens
//! ([`seal`]): Google keeps it and cannot read it, and the developer never
//! receives any of it, as Dust has no server of its own to receive it on.
//!
//! The history syncs as it is stored: by record, each written once and
//! never over ([`crate::persist::record`]), and each deletion a tombstone
//! kept for good. So what the drive holds is only ever added to, in packs
//! of records, deletions and sessions put back ([`pack`]); every device
//! takes up all of them, in whatever order they come, and comes to the same
//! history as every other ([`plan`]). There is nothing to merge, and no
//! copy any device writes over. Everything else a device uses goes too, so
//! that every synced device is set up as every other: the settings, every
//! one, the level counters, the theme, and the marks of what has been shown
//! once. They are each device's own file, each key stamped with when it was
//! last changed, the newest of each taken ([`settings`]).
//!
//! [`run`] is a sync, start to end, against any store and any folder; the
//! folder is Google Drive's ([`drive`]), reached over HTTPS ([`http`])
//! once the user has signed in ([`auth`]); and [`ctx`] is the app's side of
//! it: when it syncs, and what the card on the History tab shows of it.

pub mod auth;
pub mod config;
pub mod ctx;
pub mod drive;
pub mod http;
pub mod pack;
pub mod plan;
pub mod remote;
pub mod run;
pub mod seal;
pub mod settings;
pub mod state;
pub mod text;
