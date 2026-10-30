//! Pendant's notes store: the Lapis vault on the phone, offline.
//!
//! One worker thread owns the [`Engine`]. The UI thread only hands it jobs, so
//! opening the index, refreshing it, searching and capturing never run on the
//! UI thread. The index lives in a caller-chosen directory in app-private
//! storage, never in the vault, and is excluded from backups. Search is
//! keyword (FTS5); the embedder is `None`. Capture goes through
//! [`lapis_notes::create_new`], which writes a new file and never overwrites.
//! Nothing here spawns a process or opens a network connection.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use lapis_lattice::{Engine, Hit, RefreshReport};
use lapis_notes::{CreateRequest, WrittenNote};

/// Name of the store's worker thread.
pub const WORKER: &str = "pendant-notes";

/// Where the store reads the vault and keeps its index.
#[derive(Debug, Clone)]
pub struct Config {
    /// The vault root. The store writes nothing in it except a new capture.
    pub vault: PathBuf,
    /// The index directory, in app-private storage. On iOS, a directory under
    /// Application Support; the store marks it excluded from backups. On
    /// Android, a directory under `Context.getNoBackupFilesDir()`, which the
    /// OS already leaves out of Auto Backup; anything else is refused.
    pub index_dir: PathBuf,
    /// The inbox bucket captures land in. Always passed to `create_new`, so
    /// the vault's `.lapis/vault.json` is never consulted.
    pub inbox: String,
}

/// Why a refresh ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Launch,
    Foreground,
}

/// Called on the worker thread when a refresh ends. A UI must hop to its own
/// thread before touching views.
pub type OnRefresh = Box<dyn Fn(Trigger, Result<RefreshReport, StoreError>) + Send>;

/// A capture written to the vault.
#[derive(Debug, Clone)]
pub struct Captured {
    pub note: WrittenNote,
    /// Why the new note is not searchable yet. The file is on disk either
    /// way, and the next refresh indexes it.
    pub index_error: Option<String>,
}

#[derive(Debug)]
pub enum StoreError {
    /// The engine refused or failed: confinement, SQLite, I/O.
    Index(lapis_lattice::Error),
    /// `create_new` refused or failed, an existing note included.
    Capture(lapis_notes::Error),
    /// The index directory could not be kept out of backups.
    Backup(String),
    /// `created` is not a `YYYY-MM-DD` date.
    Date(String),
    /// The worker thread could not start or has stopped.
    Worker(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Index(e) => write!(f, "index: {e}"),
            StoreError::Capture(e) => write!(f, "capture: {e}"),
            StoreError::Backup(m) => write!(f, "backup exclusion: {m}"),
            StoreError::Date(m) => write!(f, "date: {m}"),
            StoreError::Worker(m) => write!(f, "notes worker: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

enum Job {
    Refresh(Trigger),
    Search {
        query: String,
        limit: u32,
        reply: Sender<Result<Vec<Hit>, StoreError>>,
    },
    Capture {
        title: String,
        body: Option<String>,
        created: String,
        reply: Sender<Result<Captured, StoreError>>,
    },
}

/// The notes store. Dropping it stops the worker after its current job.
pub struct Store {
    jobs: Option<Sender<Job>>,
    worker: Option<JoinHandle<()>>,
    index_dir: PathBuf,
}

impl Store {
    /// Start the worker and open the index there. Returns once the index is
    /// open, or with the refusal: an index directory that is the vault, inside
    /// it, reached through `..`, or through a symlink into it is refused
    /// before anything is written.
    pub fn open(config: Config, on_refresh: OnRefresh) -> Result<Store, StoreError> {
        let (jobs, queue) = mpsc::channel();
        let (opened, open_result) = mpsc::channel();
        let worker = thread::Builder::new()
            .name(WORKER.into())
            .spawn(move || match open_engine(&config) {
                Ok(engine) => {
                    let _ = opened.send(Ok(engine.db_path()));
                    run(engine, &config, queue, on_refresh);
                }
                Err(err) => {
                    let _ = opened.send(Err(err));
                }
            })
            .map_err(|e| StoreError::Worker(e.to_string()))?;
        let db = open_result
            .recv()
            .map_err(|_| StoreError::Worker("stopped while opening".into()))??;
        let index_dir = db.parent().map(Path::to_path_buf).unwrap_or_default();
        Ok(Store {
            jobs: Some(jobs),
            worker: Some(worker),
            index_dir,
        })
    }

    /// The resolved index directory.
    pub fn index_dir(&self) -> &Path {
        &self.index_dir
    }

    /// Queue a refresh for app launch. Returns at once; the result goes to
    /// the `on_refresh` callback.
    pub fn launched(&self) -> Result<(), StoreError> {
        self.send(Job::Refresh(Trigger::Launch))
    }

    /// Queue a refresh for the app coming to the foreground.
    pub fn foregrounded(&self) -> Result<(), StoreError> {
        self.send(Job::Refresh(Trigger::Foreground))
    }

    /// Keyword search. Waits for the worker, so call it off the UI thread.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<Hit>, StoreError> {
        let (reply, answer) = mpsc::channel();
        self.send(Job::Search {
            query: query.to_string(),
            limit,
            reply,
        })?;
        answer
            .recv()
            .map_err(|_| StoreError::Worker("stopped".into()))?
    }

    /// Write a new note into the inbox bucket, stamped `created` (`YYYY-MM-DD`,
    /// the caller's local date). A title whose note already exists is refused
    /// and the existing file is left as it is. Waits for the worker, so call
    /// it off the UI thread.
    pub fn capture(
        &self,
        title: &str,
        body: Option<&str>,
        created: &str,
    ) -> Result<Captured, StoreError> {
        check_date(created)?;
        let (reply, answer) = mpsc::channel();
        self.send(Job::Capture {
            title: title.to_string(),
            body: body.map(str::to_string),
            created: created.to_string(),
            reply,
        })?;
        answer
            .recv()
            .map_err(|_| StoreError::Worker("stopped".into()))?
    }

    fn send(&self, job: Job) -> Result<(), StoreError> {
        self.jobs
            .as_ref()
            .and_then(|jobs| jobs.send(job).ok())
            .ok_or_else(|| StoreError::Worker("stopped".into()))
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.jobs.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn open_engine(config: &Config) -> Result<Engine, StoreError> {
    check_backup_location(&config.index_dir)?;
    // Confinement is the engine's: it resolves the directory, refuses `..`,
    // the vault and anything inside it, and writes nothing when it refuses.
    let engine =
        Engine::open_in(&config.vault, &config.index_dir, None).map_err(StoreError::Index)?;
    if let Some(dir) = engine.db_path().parent() {
        exclude_from_backup(dir)?;
    }
    Ok(engine)
}

fn run(mut engine: Engine, config: &Config, queue: Receiver<Job>, on_refresh: OnRefresh) {
    for job in queue {
        match job {
            Job::Refresh(trigger) => {
                on_refresh(trigger, engine.refresh().map_err(StoreError::Index))
            }
            Job::Search {
                query,
                limit,
                reply,
            } => {
                let _ = reply.send(engine.search(&query, limit).map_err(StoreError::Index));
            }
            Job::Capture {
                title,
                body,
                created,
                reply,
            } => {
                let _ = reply.send(capture(&mut engine, config, title, body, created));
            }
        }
    }
}

fn capture(
    engine: &mut Engine,
    config: &Config,
    title: String,
    body: Option<String>,
    created: String,
) -> Result<Captured, StoreError> {
    let req = CreateRequest {
        title,
        body,
        created,
        inbox: Some(config.inbox.clone()),
        ..Default::default()
    };
    let note = lapis_notes::create_new(&config.vault, &req, None).map_err(StoreError::Capture)?;
    let index_error = engine.reindex_path(&note.path).err().map(|e| e.to_string());
    Ok(Captured { note, index_error })
}

fn check_date(created: &str) -> Result<(), StoreError> {
    let b = created.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let ok = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && digits(0..4)
        && digits(5..7)
        && digits(8..10)
        && (1..=12).contains(&created[5..7].parse::<u8>().unwrap_or(0))
        && (1..=31).contains(&created[8..10].parse::<u8>().unwrap_or(0));
    if ok {
        Ok(())
    } else {
        Err(StoreError::Date(format!("{created:?} is not YYYY-MM-DD")))
    }
}

/// Android leaves `getNoBackupFilesDir()` out of Auto Backup and device
/// transfer. The index must sit under it.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn under_no_backup(index_dir: &Path) -> bool {
    index_dir.components().any(|c| c.as_os_str() == "no_backup")
}

fn check_backup_location(index_dir: &Path) -> Result<(), StoreError> {
    #[cfg(target_os = "android")]
    if !under_no_backup(index_dir) {
        return Err(StoreError::Backup(format!(
            "{} is not under the app's no_backup directory",
            index_dir.display()
        )));
    }
    let _ = index_dir;
    Ok(())
}

#[cfg(target_vendor = "apple")]
fn exclude_from_backup(dir: &Path) -> Result<(), StoreError> {
    use objc2_foundation::{NSNumber, NSString, NSURLIsExcludedFromBackupKey, NSURL};
    let path = dir
        .to_str()
        .ok_or_else(|| StoreError::Backup(format!("{} is not UTF-8", dir.display())))?;
    let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(path), true);
    let yes = NSNumber::new_bool(true);
    unsafe { url.setResourceValue_forKey_error(Some(&yes), NSURLIsExcludedFromBackupKey) }
        .map_err(|e| StoreError::Backup(e.localizedDescription().to_string()))
}

/// Android: the location is the exclusion, checked before the index opened.
/// Desktop dev hosts keep no phone backup.
#[cfg(not(target_vendor = "apple"))]
fn exclude_from_backup(_dir: &Path) -> Result<(), StoreError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;
    use std::thread::ThreadId;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// Unique per call: pid, nanos and a counter.
    fn dir(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "pendant-store-{name}-{}-{n}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Two made-up notes.
    fn vault() -> PathBuf {
        let d = dir("vault");
        std::fs::create_dir_all(d.join("notes")).unwrap();
        std::fs::write(
            d.join("notes/Welcome.md"),
            "---\nname: Welcome\n---\n# Welcome\n\nSee [[Alpha]].\n",
        )
        .unwrap();
        std::fs::write(
            d.join("notes/Alpha.md"),
            "---\nname: Alpha\n---\n# Alpha\n\nMarigold first, then back to [[Welcome]].\n",
        )
        .unwrap();
        d
    }

    /// Every file under `root` with its bytes, sorted.
    fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let ft = entry.file_type().unwrap();
                if ft.is_dir() {
                    walk(&entry.path(), out);
                } else if ft.is_file() {
                    out.push((entry.path(), std::fs::read(entry.path()).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, &mut out);
        out.sort();
        out
    }

    struct Refreshed {
        trigger: Trigger,
        result: Result<RefreshReport, StoreError>,
        thread: Option<String>,
        id: ThreadId,
    }

    fn config(vault: &Path, index_dir: &Path) -> Config {
        Config {
            vault: vault.to_path_buf(),
            index_dir: index_dir.to_path_buf(),
            inbox: "inbox".into(),
        }
    }

    fn open(config: Config) -> (Store, Receiver<Refreshed>) {
        let (tx, rx) = mpsc::channel();
        let store = Store::open(
            config,
            Box::new(move |trigger, result| {
                let me = thread::current();
                let _ = tx.send(Refreshed {
                    trigger,
                    result,
                    thread: me.name().map(str::to_string),
                    id: me.id(),
                });
            }),
        )
        .unwrap();
        (store, rx)
    }

    fn next(done: &Receiver<Refreshed>) -> Refreshed {
        done.recv_timeout(Duration::from_secs(60))
            .expect("a refresh reported back")
    }

    fn counts(r: &RefreshReport) -> (u64, u64, u64) {
        (r.added, r.updated, r.removed)
    }

    /// The `-shm` file is left out: SQLite updates its read marks on every read.
    fn index_files(db: &Path) -> Vec<Option<Vec<u8>>> {
        let wal = PathBuf::from(format!("{}-wal", db.display()));
        vec![std::fs::read(db).ok(), std::fs::read(wal).ok()]
    }

    fn rows(db: &Path) -> Vec<Vec<rusqlite::types::Value>> {
        let conn = rusqlite::Connection::open(db).unwrap();
        let mut stmt = conn
            .prepare("SELECT path, mtime, size, hash FROM documents ORDER BY path")
            .unwrap();
        stmt.query_map([], |r| (0..4).map(|i| r.get(i)).collect())
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn paths(store: &Store, query: &str) -> Vec<String> {
        store
            .search(query, 10)
            .unwrap()
            .into_iter()
            .map(|h| h.path)
            .collect()
    }

    #[cfg(target_vendor = "apple")]
    fn excluded_from_backup(dir: &Path) -> bool {
        use objc2_foundation::{NSNumber, NSString, NSURLIsExcludedFromBackupKey, NSURL};
        let url =
            NSURL::fileURLWithPath_isDirectory(&NSString::from_str(dir.to_str().unwrap()), true);
        let mut value = None;
        unsafe { url.getResourceValue_forKey_error(&mut value, NSURLIsExcludedFromBackupKey) }
            .unwrap();
        value
            .and_then(|v| v.downcast::<NSNumber>().ok())
            .is_some_and(|n| n.as_bool())
    }

    /// G1a: the index builds in app storage outside the vault, the vault gains
    /// no file, the directory is excluded from backups, and search reads it.
    #[test]
    fn the_index_lives_outside_the_vault_and_out_of_backups() {
        let v = vault();
        let before = tree(&v);
        let app = dir("app");
        let index = app.join("Library/Application Support/notes-index");
        let (store, done) = open(config(&v, &index));

        assert_eq!(store.index_dir(), index.canonicalize().unwrap());
        assert!(store.index_dir().join("lattice.sqlite").is_file());
        store.launched().unwrap();
        let r = next(&done);
        assert_eq!(counts(&r.result.unwrap()), (2, 0, 0));
        assert_eq!(paths(&store, "marigold"), ["notes/Alpha.md"]);
        assert_eq!(tree(&v), before, "the vault gains no file");
        #[cfg(target_vendor = "apple")]
        assert!(
            excluded_from_backup(store.index_dir()),
            "the index directory is excluded from backups"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&v);
        let _ = std::fs::remove_dir_all(&app);
    }

    /// G1b: the vault, a directory inside it, a `..` walk back into it, and a
    /// symlink into it are each refused, and nothing appears anywhere.
    #[cfg(unix)]
    #[test]
    fn an_index_in_the_vault_is_refused_before_a_write() {
        let v = vault();
        let before = tree(&v);
        let outside = dir("outside");
        std::os::unix::fs::symlink(v.join("notes"), outside.join("in-vault")).unwrap();
        let cases = [
            v.clone(),
            v.join("idx"),
            outside.join("..").join(v.file_name().unwrap()).join("idx"),
            outside.join("in-vault"),
            outside.join("in-vault/deeper"),
        ];
        for index in cases {
            let err = Store::open(config(&v, &index), Box::new(|_, _| {}))
                .err()
                .expect("refused");
            assert!(
                matches!(err, StoreError::Index(lapis_lattice::Error::Usage(_))),
                "{}: {err}",
                index.display()
            );
            assert!(
                !index.join("lattice.sqlite").exists(),
                "{}",
                index.display()
            );
        }
        assert_eq!(tree(&v), before, "a refusal writes nothing in the vault");
        let left: Vec<_> = std::fs::read_dir(&outside)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            left,
            ["in-vault"],
            "a refusal creates nothing outside it either"
        );
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&v);
    }

    /// G2a: launch and foreground refreshes run on the store's worker, not on
    /// the thread that asked for them.
    #[test]
    fn refresh_runs_off_the_calling_thread() {
        let v = vault();
        let app = dir("app");
        let (store, done) = open(config(&v, &app.join("index")));
        let ui = thread::current().id();

        store.launched().unwrap();
        store.foregrounded().unwrap();
        for want in [Trigger::Launch, Trigger::Foreground] {
            let r = next(&done);
            assert_eq!(r.trigger, want);
            assert!(r.result.is_ok());
            assert_eq!(r.thread.as_deref(), Some(WORKER));
            assert_ne!(r.id, ui, "refresh ran on the caller's thread");
        }
        drop(store);
        let _ = std::fs::remove_dir_all(&v);
        let _ = std::fs::remove_dir_all(&app);
    }

    /// G2b: a refresh with nothing changed commits nothing and leaves the
    /// index files' bytes as they were.
    #[test]
    fn a_no_change_refresh_writes_nothing() {
        let v = vault();
        let app = dir("app");
        let (store, done) = open(config(&v, &app.join("index")));
        store.launched().unwrap();
        assert_eq!(counts(&next(&done).result.unwrap()), (2, 0, 0));

        let db = store.index_dir().join("lattice.sqlite");
        // data_version moves when another connection commits.
        let probe = rusqlite::Connection::open(&db).unwrap();
        let version = || {
            probe
                .query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        let v0 = version();
        let files = index_files(&db);
        assert!(files[0].is_some(), "the index file exists");

        store.foregrounded().unwrap();
        assert_eq!(counts(&next(&done).result.unwrap()), (0, 0, 0));
        assert_eq!(version(), v0, "the no-change refresh committed");
        assert_eq!(index_files(&db), files, "the no-change refresh wrote bytes");
        drop(probe);
        drop(store);
        let _ = std::fs::remove_dir_all(&v);
        let _ = std::fs::remove_dir_all(&app);
    }

    /// G2b: a refresh that fails after it already rewrote a changed note rolls
    /// back whole: the rows and the index file's bytes are the prior ones, and
    /// the next refresh still applies the change.
    #[test]
    fn a_failed_refresh_rolls_back() {
        let v = vault();
        let app = dir("app");
        let (store, done) = open(config(&v, &app.join("index")));
        store.launched().unwrap();
        assert_eq!(counts(&next(&done).result.unwrap()), (2, 0, 0));

        let db = store.index_dir().join("lattice.sqlite");
        let probe = rusqlite::Connection::open(&db).unwrap();
        // Changed rows are rewritten before new files are added, so aborting
        // the add fails the refresh after a write in the same transaction.
        probe
            .execute_batch(
                "CREATE TRIGGER boom BEFORE INSERT ON documents WHEN NEW.path = 'notes/Boom.md' \
                 BEGIN SELECT RAISE(ABORT, 'forced failure'); END;",
            )
            .unwrap();
        probe
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
        let prior_rows = rows(&db);
        let prior_bytes = std::fs::read(&db).unwrap();

        std::fs::write(
            v.join("notes/Alpha.md"),
            "---\nname: Alpha\n---\n# Alpha\n\nZinnia now, a longer line.\n",
        )
        .unwrap();
        std::fs::write(v.join("notes/Boom.md"), "# Boom\n").unwrap();
        store.foregrounded().unwrap();
        let err = next(&done).result.expect_err("the forced failure surfaces");
        assert!(err.to_string().contains("forced failure"), "{err}");
        assert_eq!(rows(&db), prior_rows, "the rewritten row rolled back");
        assert_eq!(
            std::fs::read(&db).unwrap(),
            prior_bytes,
            "the index file kept its bytes"
        );
        assert!(
            paths(&store, "zinnia").is_empty(),
            "the rolled-back text is not searchable"
        );
        assert_eq!(
            paths(&store, "marigold"),
            ["notes/Alpha.md"],
            "the prior text still is"
        );

        probe.execute_batch("DROP TRIGGER boom;").unwrap();
        store.foregrounded().unwrap();
        assert_eq!(counts(&next(&done).result.unwrap()), (1, 1, 0));
        assert_eq!(paths(&store, "zinnia"), ["notes/Alpha.md"]);
        drop(probe);
        drop(store);
        let _ = std::fs::remove_dir_all(&v);
        let _ = std::fs::remove_dir_all(&app);
    }

    /// G3a, G3b: capture writes into the configured inbox with `created` set,
    /// ignores the vault's own inbox setting, is searchable at once, and a
    /// second capture of the same title is refused with the first file's
    /// bytes unchanged.
    #[test]
    fn capture_writes_a_new_note_and_never_overwrites() {
        let v = vault();
        std::fs::create_dir_all(v.join(".lapis")).unwrap();
        std::fs::write(
            v.join(".lapis/vault.json"),
            r#"{"buckets":{"inbox":"elsewhere"}}"#,
        )
        .unwrap();
        let app = dir("app");
        let (store, done) = open(config(&v, &app.join("index")));
        store.launched().unwrap();
        next(&done).result.unwrap();

        let first = store
            .capture("Seed order", Some("Cosmos and larkspur.\n"), "2026-10-30")
            .unwrap();
        assert!(first.note.path.starts_with("inbox/"), "{}", first.note.path);
        assert!(
            !v.join("elsewhere").exists(),
            "the vault's own inbox setting was read"
        );
        assert_eq!(first.index_error, None);
        let written = std::fs::read(v.join(&first.note.path)).unwrap();
        let text = String::from_utf8(written.clone()).unwrap();
        assert!(
            text.lines()
                .any(|l| l.starts_with("created:") && l.contains("2026-10-30")),
            "{text}"
        );
        assert_eq!(paths(&store, "larkspur"), [first.note.path.as_str()]);

        let second = store.capture("Seed order", Some("Different bytes.\n"), "2026-10-31");
        assert!(matches!(second, Err(StoreError::Capture(_))), "{second:?}");
        assert_eq!(
            std::fs::read(v.join(&first.note.path)).unwrap(),
            written,
            "the first note was overwritten"
        );
        assert_eq!(
            std::fs::read_dir(v.join("inbox")).unwrap().count(),
            1,
            "no second file appeared"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&v);
        let _ = std::fs::remove_dir_all(&app);
    }

    /// A malformed `created` is refused before the worker writes anything.
    #[test]
    fn capture_refuses_a_date_that_is_not_iso() {
        let v = vault();
        let app = dir("app");
        let (store, _done) = open(config(&v, &app.join("index")));
        for bad in [
            "30/10/2026",
            "2026-13-01",
            "2026-10-00",
            "2026-1-5",
            "2026-10-30T12:00",
            "",
        ] {
            let r = store.capture("Dated", None, bad);
            assert!(matches!(r, Err(StoreError::Date(_))), "{bad:?}: {r:?}");
        }
        assert!(!v.join("inbox").exists());
        drop(store);
        let _ = std::fs::remove_dir_all(&v);
        let _ = std::fs::remove_dir_all(&app);
    }

    #[test]
    fn android_index_must_sit_under_no_backup() {
        assert!(under_no_backup(Path::new(
            "/data/user/0/com.hedronite.pendant/no_backup/notes-index"
        )));
        assert!(!under_no_backup(Path::new(
            "/data/user/0/com.hedronite.pendant/files/notes-index"
        )));
    }
}
