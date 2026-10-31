//! The history's files read and written on a thread of their own,
//! `dust-history` ([`THREAD`]), so that nothing the user does waits on a
//! disk: a session's end, a deletion, a restore, a look for what another
//! copy of the app stored. On a phone the thread the app's taps and
//! screens run on is the one that would wait.
//!
//! The thread takes one job at a time, in the order they were asked for
//! ([`HistoryJobs`]): a change asked for before a look at the history is
//! made before that look, as it would be were both made then and there.
//! Each answer comes back as the app's [`Pending`], which the app takes up
//! once it comes ([`crate::history::settle_or_spawn`]). It waits there and
//! then only for the history read as it opens, which starts as the app
//! does ([`super::preload`]), and, for a moment, for what it asked to be
//! written as it closes ([`HistoryJobs::flush`]).
//!
//! A test asks the same of the files then and there, on its own thread
//! ([`HistoryJobs::inline`]), so whatever it asked for is done once it
//! looks.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use tokio::sync::oneshot;

use super::HistorySaved;
use super::files::{HistoryFiles, Watch};
use super::record::{HistoryChange, HistoryLoad, Pending, Record, RecordId, done};

/// The thread the history's files are read and written on.
pub(super) const THREAD: &str = "dust-history";

/// How long a change the files would not take waits, at the head of the
/// queue, before it is tried again: twice, before the app is told. A file
/// another program holds open for a moment, as a virus scanner holds each
/// new one on Windows, is let go of by then. Whatever is still not stored
/// after that is kept by the app, and stored with its next change.
const RETRY_AFTER: [Duration; 2] = [Duration::from_millis(100), Duration::from_millis(400)];

/// How long the app, closing, waits at most for the history to be written
/// ([`HistoryJobs::flush`]): a window closed must go, and every write is
/// whole, so one cut short loses nothing that was stored before it.
pub(super) const FLUSH_WITHIN: Duration = Duration::from_secs(3);

/// What the app is told of a change when the files could not be reached at
/// all.
const UNREACHED: &str = "the history's files could not be reached.";

/// A job for the files, done on their thread.
type Job = Box<dyn FnOnce(&HistoryFiles) + Send>;

/// The history's files, and how the app reaches them: on a thread of their
/// own, or then and there.
pub(super) struct HistoryJobs {
    run: Run,
    /// Whether the files changed elsewhere, asked on the app's own thread.
    watch: Watch,
    /// How many bytes the sessions stored hold, as the files last knew.
    size: Arc<AtomicU64>,
}

enum Run {
    /// Each job done then and there, on the caller's thread.
    Inline(HistoryFiles),
    /// Each job sent to the worker's thread, which owns the files.
    Worker(mpsc::Sender<Job>),
}

/// The history, being read ([`HistoryJobs::start_load`]): read already, or
/// on its way from the worker.
pub(super) enum Loading {
    Read(HistoryLoad),
    Coming(mpsc::Receiver<HistoryLoad>),
}

impl Loading {
    /// The history as it was read, once it is: none, where the worker
    /// stopped short of reading it, which the next look at the files takes
    /// up ([`super::Store::history_listing`]).
    pub(super) fn wait(self) -> HistoryLoad {
        match self {
            Self::Read(load) => load,
            Self::Coming(coming) => coming.recv().unwrap_or_else(|_| {
                eprintln!("the history could not be read as the app opened");
                HistoryLoad::default()
            }),
        }
    }
}

impl HistoryJobs {
    /// The files reached then and there, on the caller's thread: how a test
    /// reaches them.
    pub(super) fn inline(files: HistoryFiles) -> Self {
        Self {
            watch: files.watch(),
            size: files.size(),
            run: Run::Inline(files),
        }
    }

    /// The files reached on a thread of their own, which takes them; or
    /// then and there, should no thread start.
    pub(super) fn on_worker(files: HistoryFiles) -> Self {
        let (watch, size) = (files.watch(), files.size());
        let root = files.root().to_path_buf();
        let (jobs, queue) = mpsc::channel::<Job>();
        let started = thread::Builder::new()
            .name(THREAD.into())
            .spawn(move || work(files, &queue));
        match started {
            Ok(_) => Self {
                run: Run::Worker(jobs),
                watch,
                size,
            },
            Err(err) => {
                eprintln!(
                    "the history is written on the app's own thread, as none other would start: {err}"
                );
                Self::inline(HistoryFiles::at(&root))
            }
        }
    }

    /// Start reading the whole history ([`HistoryFiles::load`]), behind
    /// whatever was asked for before.
    pub(super) fn start_load(&self) -> Loading {
        match &self.run {
            Run::Inline(files) => Loading::Read(files.load()),
            Run::Worker(jobs) => {
                let (answer, coming) = mpsc::channel();
                // Sent or not, the answer is waited for: one never sent is
                // a channel closed, and the wait ends.
                let _ = jobs.send(Box::new(move |files: &HistoryFiles| {
                    let _ = answer.send(files.load());
                }));
                Loading::Coming(coming)
            }
        }
    }

    /// Read the whole history, and wait for it.
    pub(super) fn load(&self) -> HistoryLoad {
        self.start_load().wait()
    }

    /// Make `change` ([`HistoryFiles::apply`]). On the worker, a change the
    /// files would not take is tried again before anything asked for after
    /// it ([`RETRY_AFTER`]), and only then is the app told.
    pub(super) fn apply(&self, change: HistoryChange) -> Pending<HistorySaved> {
        let again = matches!(self.run, Run::Worker(_));
        let made = self.ask(move |files| {
            let mut saved = files.apply(&change);
            if again {
                for wait in RETRY_AFTER {
                    if !matches!(saved, HistorySaved::Failed(_)) {
                        break;
                    }
                    thread::sleep(wait);
                    saved = files.apply(&change);
                }
            }
            saved
        });
        Box::pin(async move {
            made.await
                .unwrap_or_else(|| HistorySaved::Failed(UNREACHED.into()))
        })
    }

    /// Every record stored ([`HistoryFiles::listing`]).
    pub(super) fn listing(&self) -> Pending<Option<Vec<RecordId>>> {
        let listed = self.ask(HistoryFiles::listing);
        Box::pin(async move { listed.await.flatten() })
    }

    /// The records `ids`, as stored ([`HistoryFiles::read`]).
    pub(super) fn read(&self, ids: Vec<RecordId>) -> Pending<Vec<Result<Record, RecordId>>> {
        let asked = ids.clone();
        let read = self.ask(move |files| files.read(&asked));
        Box::pin(async move {
            read.await
                .unwrap_or_else(|| ids.into_iter().map(Err).collect())
        })
    }

    /// Which of `ids` were deleted by hand ([`HistoryFiles::forgotten`]):
    /// none, where the files cannot be reached, as missing is never
    /// deleted.
    pub(super) fn forgotten(&self, ids: Vec<RecordId>) -> Pending<Vec<RecordId>> {
        let found = self.ask(move |files| files.forgotten(&ids));
        Box::pin(async move { found.await.unwrap_or_default() })
    }

    /// Whether the history changed elsewhere since this copy last looked:
    /// asked here, of the folders, and of nothing in them.
    pub(super) fn changed(&self) -> bool {
        self.watch.changed()
    }

    /// How many bytes the sessions stored hold, as of the last read of the
    /// history and every change since: asked here, of nothing on the disk.
    pub(super) fn bytes(&self) -> u64 {
        self.size.load(Ordering::Relaxed)
    }

    /// Do `job` with the files, behind whatever was asked for before: none,
    /// where the files could not be reached.
    pub(super) fn ask<T: Send + 'static>(
        &self,
        job: impl FnOnce(&HistoryFiles) -> T + Send + 'static,
    ) -> Pending<Option<T>> {
        match &self.run {
            Run::Inline(files) => done(Some(job(files))),
            Run::Worker(jobs) => {
                let (answer, answered) = oneshot::channel();
                let sent = jobs.send(Box::new(move |files: &HistoryFiles| {
                    let _ = answer.send(job(files));
                }));
                if sent.is_err() {
                    return done(None);
                }
                Box::pin(async move { answered.await.ok() })
            }
        }
    }

    /// Wait until everything asked for so far is done, but no longer than
    /// `within`: whether it was.
    pub(super) fn flush(&self, within: Duration) -> bool {
        let Run::Worker(jobs) = &self.run else {
            return true;
        };
        let (answer, answered) = mpsc::channel();
        let sent = jobs.send(Box::new(move |_: &HistoryFiles| {
            let _ = answer.send(());
        }));
        // A worker gone has nothing left to do.
        sent.is_err() || answered.recv_timeout(within).is_ok()
    }
}

/// The worker: each job, in the order it came, until the store is gone.
fn work(mut files: HistoryFiles, queue: &mpsc::Receiver<Job>) {
    files.keep_to_worker();
    for job in queue {
        // A job that stops short, as none should, stops alone: its answer
        // is never sent, which the app takes as nothing done, and the next
        // job is done as ever.
        if catch_unwind(AssertUnwindSafe(|| job(&files))).is_err() {
            eprintln!("a job on the history's files stopped short");
        }
    }
}

/// What `pending` comes to, waited for on this thread: how a test, outside
/// the app, takes an answer from the worker.
#[cfg(test)]
pub(super) fn wait<T>(mut pending: Pending<T>) -> T {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake};

    struct Unpark(thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Arc::new(Unpark(thread::current())).into();
    let mut asked = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(value) = pending.as_mut().poll(&mut asked) {
            return value;
        }
        thread::park_timeout(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::desktop::tests::TempDir;
    use crate::persist::record::records_for_blob;
    use crate::persist::tests::session;
    use cw_core::SessionResult;
    use std::time::Instant;

    /// `count` sessions a minute apart, each a record, oldest first.
    fn records(count: u64) -> Vec<Record> {
        let sessions: Vec<SessionResult> = (1..=count)
            .map(|n| SessionResult {
                timestamp: n * 60_000,
                ..session(&format!("day {n}"))
            })
            .collect();
        records_for_blob(&sessions)
    }

    fn put(records: &[Record]) -> HistoryChange {
        HistoryChange {
            put: records.to_vec(),
            ..HistoryChange::default()
        }
    }

    /// Everything asked of the worker is done in the order it was asked,
    /// and the app, closing, waits until it is: what it stored last is on
    /// the disk by then, whether it looked for the answers or not.
    #[test]
    fn flush_waits_for_queued_changes() {
        let dir = TempDir::new("flush");
        let jobs = HistoryJobs::on_worker(HistoryFiles::at(&dir.0));
        jobs.load();
        let records = records(40);
        // Never looked at: the app may close before it takes them up.
        let asked: Vec<Pending<HistorySaved>> = records
            .chunks(4)
            .map(|four| jobs.apply(put(four)))
            .collect();
        assert!(jobs.flush(FLUSH_WITHIN), "done in time");
        let written = std::fs::read_dir(dir.0.join("history").join("sessions"))
            .expect("a folder of records")
            .count();
        assert_eq!(written, records.len());
        for saved in asked {
            assert_eq!(wait(saved), HistorySaved::Stored { kept: 4 });
        }
        // Asked after, and read after, what was written before it.
        let listed = wait(jobs.listing()).expect("listed");
        let ids: Vec<RecordId> = records.iter().map(|record| record.id).collect();
        assert_eq!(listed, ids);
    }

    /// The worker's answers come in the order the jobs were asked, each
    /// seeing what was asked before it: a listing asked after a change
    /// holds it, and one asked before a deletion still holds the session.
    #[test]
    fn the_worker_does_each_job_in_the_order_asked() {
        let dir = TempDir::new("in-order");
        let jobs = HistoryJobs::on_worker(HistoryFiles::at(&dir.0));
        jobs.load();
        let records = records(3);
        let stored = jobs.apply(put(&records));
        let before = jobs.listing();
        let gone = jobs.apply(HistoryChange {
            forget: vec![(
                records[0].id,
                crate::persist::Deleted {
                    timestamp: 60_000,
                    date: "day 1".into(),
                    at: 7,
                    unread: serde_json::Map::new(),
                },
            )],
            ..HistoryChange::default()
        });
        let after = jobs.listing();
        let forgotten = jobs.forgotten(vec![records[0].id, records[1].id]);
        assert_eq!(wait(stored), HistorySaved::Stored { kept: 3 });
        assert_eq!(wait(before).expect("listed").len(), 3);
        assert_eq!(wait(gone), HistorySaved::Stored { kept: 0 });
        assert_eq!(wait(after).expect("listed"), [records[1].id, records[2].id]);
        assert_eq!(wait(forgotten), [records[0].id]);
        let read = wait(jobs.read(vec![records[1].id, records[0].id]));
        assert_eq!(read, [Ok(records[1].clone()), Err(records[0].id)]);
        assert!(!jobs.changed(), "its own changes are none to take up");
    }

    /// A change the files would not take is tried again before anything
    /// asked after it, and is told of only once it still fails: a name in
    /// the way for a moment, as a file another program holds, is out of
    /// the way by then, and the change is made.
    #[cfg(unix)]
    #[test]
    fn a_change_that_fails_for_a_moment_is_made_before_anything_after_it() {
        let dir = TempDir::new("retried");
        let jobs = HistoryJobs::on_worker(HistoryFiles::at(&dir.0));
        jobs.load();
        let records = records(2);
        let sessions = dir.0.join("history").join("sessions");
        let in_the_way = sessions.join(format!("{}.json", records[1].id.name()));
        std::fs::create_dir_all(in_the_way.join("held")).unwrap();
        let started = Instant::now();
        let stored = jobs.apply(put(&records));
        let listed = jobs.listing();
        // Out of the way while the worker waits to try it again.
        std::thread::sleep(RETRY_AFTER[0] / 2);
        std::fs::remove_dir_all(&in_the_way).unwrap();
        assert_eq!(wait(stored), HistorySaved::Stored { kept: 2 });
        assert!(started.elapsed() >= RETRY_AFTER[0], "tried again");
        assert_eq!(wait(listed).expect("listed").len(), 2, "made first");

        // In the way for good: told of as failed only once it was tried
        // again as often as it is.
        let later = super::tests::records(3).remove(2);
        let blocked = sessions.join(format!("{}.json", later.id.name()));
        std::fs::create_dir_all(blocked.join("held")).unwrap();
        let started = Instant::now();
        let failed = wait(jobs.apply(put(std::slice::from_ref(&later))));
        assert!(matches!(failed, HistorySaved::Failed(_)), "{failed:?}");
        assert!(
            started.elapsed() >= RETRY_AFTER.iter().sum::<Duration>(),
            "tried again, twice"
        );
    }

    /// The files of a worker are reached on its thread alone: a test that
    /// reaches them from another, as no part of the app may, is stopped.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "the history's files were reached from")]
    fn the_workers_files_are_reached_on_its_thread_alone() {
        let dir = TempDir::new("off-thread");
        let mut files = HistoryFiles::at(&dir.0);
        files.keep_to_worker();
        files.load();
    }

    /// Ten thousand sessions read back as the app opens, on the worker,
    /// in a second or two: how long a long history keeps the app waiting.
    #[test]
    #[ignore = "a timing, on a disk of its own: run by hand"]
    fn ten_thousand_records_load() {
        let dir = TempDir::new("ten-thousand");
        let records = records(10_000);
        let jobs = HistoryJobs::on_worker(HistoryFiles::at(&dir.0));
        jobs.load();
        for part in records.chunks(500) {
            wait(jobs.apply(put(part)));
        }
        drop(jobs);
        let started = Instant::now();
        let jobs = HistoryJobs::on_worker(HistoryFiles::at(&dir.0));
        let load = jobs.load();
        let took = started.elapsed();
        assert_eq!(load.sessions.len(), records.len());
        eprintln!("10,000 records loaded in {took:?}");
        assert!(took < Duration::from_secs(5), "{took:?}");
    }
}
