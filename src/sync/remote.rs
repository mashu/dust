//! The folder sync keeps its files in, as sync sees it ([`Remote`]): a flat
//! list of named files, each read, written and deleted whole. Google Drive's
//! app data folder is one ([`super::drive`]); a test has its own, in memory
//! ([`MemoryRemote`]).

use crate::persist::record::Pending;

/// A file in the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    /// What the folder knows it by.
    pub id: String,
    pub name: String,
    /// When it was made, as the folder writes the time: only ever compared
    /// with another of the folder's own.
    pub created: String,
}

/// Why the folder did not do what it was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteError {
    /// It needs the user to sign in again: never signed in on this device,
    /// signed out, or the access given to Dust taken back.
    SignIn,
    /// The drive has no room left.
    Full,
    /// The file is not there, or not any more.
    Gone,
    /// Anything else, in words: offline, say, or the server failing.
    Failed(String),
}

impl RemoteError {
    /// In words that follow "Sync stopped:".
    pub fn why(&self) -> String {
        match self {
            Self::SignIn => "sign in to Google again to sync".into(),
            Self::Full => "your Google Drive has no room left".into(),
            Self::Gone => "a file in your Google Drive went as Dust read it".into(),
            Self::Failed(why) => why.trim_end_matches('.').to_string(),
        }
    }
}

/// The folder.
pub trait Remote {
    /// Every file in it.
    fn list(&self) -> Pending<Result<Vec<RemoteFile>, RemoteError>>;
    /// What the file `id` holds.
    fn get(&self, id: &str) -> Pending<Result<Vec<u8>, RemoteError>>;
    /// Make a file called `name` holding `bytes`, and say what it is known
    /// by.
    fn create(&self, name: &str, bytes: Vec<u8>) -> Pending<Result<String, RemoteError>>;
    /// Put `bytes` in the file `id` in place of what it held.
    fn update(&self, id: &str, bytes: Vec<u8>) -> Pending<Result<(), RemoteError>>;
    fn delete(&self, id: &str) -> Pending<Result<(), RemoteError>>;
}

#[cfg(test)]
pub use memory::MemoryRemote;

#[cfg(test)]
mod memory {
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;

    use super::*;
    use crate::persist::record::done;

    /// Each file by its id: its name, when it was made, and its bytes.
    type Files = RefCell<BTreeMap<String, (String, String, Vec<u8>)>>;

    /// A folder in memory, shared by every device of a test that holds it,
    /// with what a test needs to make it misbehave as a drive can.
    #[derive(Default)]
    pub struct MemoryRemote {
        files: Files,
        made: Cell<u64>,
        /// Every request fails, as one does with the user signed out.
        pub signed_out: Cell<bool>,
        /// Every request fails, for the reason given.
        pub failing: RefCell<Option<RemoteError>>,
        /// Every file made or changed from now on fails: the drive is full.
        pub full: Cell<bool>,
        /// How many times each was asked.
        pub lists: Cell<usize>,
        pub gets: Cell<usize>,
        pub writes: Cell<usize>,
    }

    impl MemoryRemote {
        fn refused(&self) -> Option<RemoteError> {
            if self.signed_out.get() {
                return Some(RemoteError::SignIn);
            }
            self.failing.borrow().clone()
        }

        /// The names of every file, in the order they were made.
        pub fn names(&self) -> Vec<String> {
            let files = self.files.borrow();
            let mut all: Vec<(&String, &String)> =
                files.values().map(|(name, made, _)| (made, name)).collect();
            all.sort();
            all.into_iter().map(|(_, name)| name.clone()).collect()
        }

        /// Every byte of every file, put together: what a test looks for
        /// anything in the clear in.
        pub fn every_byte(&self) -> Vec<u8> {
            self.files
                .borrow()
                .values()
                .flat_map(|(name, _, bytes)| name.bytes().chain(bytes.iter().copied()))
                .collect()
        }

        /// Change what the file called `name` holds, as damage would.
        pub fn spoil(&self, name: &str) {
            for (held, _, bytes) in self.files.borrow_mut().values_mut() {
                if held == name
                    && let Some(last) = bytes.last_mut()
                {
                    *last ^= 1;
                }
            }
        }
    }

    impl Remote for MemoryRemote {
        fn list(&self) -> Pending<Result<Vec<RemoteFile>, RemoteError>> {
            self.lists.set(self.lists.get() + 1);
            if let Some(refused) = self.refused() {
                return done(Err(refused));
            }
            let files = self
                .files
                .borrow()
                .iter()
                .map(|(id, (name, made, _))| RemoteFile {
                    id: id.clone(),
                    name: name.clone(),
                    created: made.clone(),
                })
                .collect();
            done(Ok(files))
        }

        fn get(&self, id: &str) -> Pending<Result<Vec<u8>, RemoteError>> {
            self.gets.set(self.gets.get() + 1);
            if let Some(refused) = self.refused() {
                return done(Err(refused));
            }
            done(
                self.files
                    .borrow()
                    .get(id)
                    .map(|(_, _, bytes)| bytes.clone())
                    .ok_or(RemoteError::Gone),
            )
        }

        fn create(&self, name: &str, bytes: Vec<u8>) -> Pending<Result<String, RemoteError>> {
            if let Some(refused) = self.refused() {
                return done(Err(refused));
            }
            if self.full.get() {
                return done(Err(RemoteError::Full));
            }
            self.writes.set(self.writes.get() + 1);
            let made = self.made.get() + 1;
            self.made.set(made);
            let id = format!("file{made}");
            // Made later than any other, as the drive's own clock says.
            let created = format!("2026-11-04T00:00:{made:06}Z");
            self.files
                .borrow_mut()
                .insert(id.clone(), (name.into(), created, bytes));
            done(Ok(id))
        }

        fn update(&self, id: &str, bytes: Vec<u8>) -> Pending<Result<(), RemoteError>> {
            if let Some(refused) = self.refused() {
                return done(Err(refused));
            }
            if self.full.get() {
                return done(Err(RemoteError::Full));
            }
            self.writes.set(self.writes.get() + 1);
            done(match self.files.borrow_mut().get_mut(id) {
                Some(file) => {
                    file.2 = bytes;
                    Ok(())
                }
                None => Err(RemoteError::Gone),
            })
        }

        fn delete(&self, id: &str) -> Pending<Result<(), RemoteError>> {
            if let Some(refused) = self.refused() {
                return done(Err(refused));
            }
            done(
                self.files
                    .borrow_mut()
                    .remove(id)
                    .map(|_| ())
                    .ok_or(RemoteError::Gone),
            )
        }
    }
}
