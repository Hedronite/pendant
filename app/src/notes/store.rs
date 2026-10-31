//! Pendant's notes store: the Lapis vault on the phone, offline.
//!
//! One worker thread owns the [`Engine`]. The UI thread only hands it jobs, so
//! opening the index, refreshing it, searching and capturing never run on the
//! UI thread. The index lives in a directory the store picks in app-private
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

// Until app/src/lib.rs lands this file is the library root (see Cargo.toml),
// so the platform keystore mounts here.
#[path = "../platform/keystore.rs"]
pub mod keystore;

/// Name of the store's worker thread.
pub const WORKER: &str = "pendant-notes";

/// The index directory's name inside the app's private storage.
const INDEX: &str = "notes-index";

/// Where the store reads the vault. There is no index directory here: the
/// store picks it in app-private storage (see [`Store::open`]), so a caller
/// cannot name one, a symlink included.
#[derive(Debug, Clone)]
pub struct Config {
    /// The vault root. The store writes nothing in it except a new capture.
    pub vault: PathBuf,
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
    /// way, and the next refresh indexes it. Never carries a path, so a UI
    /// may log it.
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
    /// Start the worker and open the index in the app's private storage:
    /// `Library/Application Support/Pendant/notes-index` under the app's home
    /// on iOS, `no_backup/notes-index` in the app's data directory on Android.
    /// Returns once the index is open, or with the refusal. Nothing the caller
    /// passes names that directory:
    ///
    /// ```no_run
    /// use pendant_app::{Config, Store};
    /// let config = Config { vault: "/vault".into(), inbox: "inbox".into() };
    /// let store = Store::open(config, Box::new(|_, _| {}));
    /// ```
    ///
    /// ```compile_fail,E0560
    /// use pendant_app::{Config, Store};
    /// let config = Config {
    ///     vault: "/vault".into(),
    ///     index_dir: "/tmp/link-to-anywhere".into(),
    ///     inbox: "inbox".into(),
    /// };
    /// let store = Store::open(config, Box::new(|_, _| {}));
    /// ```
    ///
    /// ```compile_fail,E0061
    /// use pendant_app::{Config, Store};
    /// let config = Config { vault: "/vault".into(), inbox: "inbox".into() };
    /// let store = Store::open(config, "/tmp/link-to-anywhere", Box::new(|_, _| {}));
    /// ```
    pub fn open(config: Config, on_refresh: OnRefresh) -> Result<Store, StoreError> {
        Store::open_in(config, app_index_dir()?, on_refresh)
    }

    /// The engine still refuses an index directory that is the vault, inside
    /// it, reached through `..`, or through a symlink into it, before anything
    /// is written. Not public: only [`app_index_dir`] and tests choose `index_dir`.
    fn open_in(
        config: Config,
        index_dir: PathBuf,
        on_refresh: OnRefresh,
    ) -> Result<Store, StoreError> {
        let (jobs, queue) = mpsc::channel();
        let (opened, open_result) = mpsc::channel();
        let worker = thread::Builder::new()
            .name(WORKER.into())
            .spawn(move || match open_engine(&config, &index_dir) {
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

fn open_engine(config: &Config, index_dir: &Path) -> Result<Engine, StoreError> {
    check_backup_location(index_dir)?;
    // Confinement is the engine's: it resolves the directory, refuses `..`,
    // the vault and anything inside it, and writes nothing when it refuses.
    let engine = Engine::open_in(&config.vault, index_dir, None).map_err(StoreError::Index)?;
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
    let index_error = engine
        .reindex_path(&note.path)
        .err()
        .map(|e| index_failure(&e));
    Ok(Captured { note, index_error })
}

/// The engine's own messages can carry vault paths (an I/O error, a refused
/// note, a SQLite message), so only the kind of failure goes out.
fn index_failure(e: &lapis_lattice::Error) -> String {
    match e {
        lapis_lattice::Error::Usage(_) => "the index refused the note".into(),
        lapis_lattice::Error::Io(e) => {
            format!("the index could not read the note ({:?})", e.kind())
        }
        lapis_lattice::Error::Sqlite(e) => match e.sqlite_error_code() {
            Some(code) => format!("the index could not store the note (SQLite {code:?})"),
            None => "the index could not store the note (SQLite)".into(),
        },
    }
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

/// The index directory in the app's private storage. It comes from the
/// platform, never from a caller.
fn app_index_dir() -> Result<PathBuf, StoreError> {
    #[cfg(target_os = "android")]
    let dir = android_data_dir()?.join("no_backup").join(INDEX);
    // iOS sets HOME to the app's sandbox container. macOS dev hosts use the
    // user's own Library.
    #[cfg(target_vendor = "apple")]
    let dir = home()?
        .join("Library/Application Support/Pendant")
        .join(INDEX);
    #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
    let dir = home()?.join(".local/share/pendant").join(INDEX);
    Ok(dir)
}

#[cfg(not(target_os = "android"))]
fn home() -> Result<PathBuf, StoreError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .ok_or_else(|| StoreError::Backup("HOME is not set to an absolute path".into()))
}

/// This app's data directory on internal storage, laid out the way Android
/// lays out `Context.getDataDir()`: `/data/user/<user>/<package>`. The user is
/// the process uid over 100000; the package is the process name up to `:`.
#[cfg(target_os = "android")]
fn android_data_dir() -> Result<PathBuf, StoreError> {
    use std::os::unix::fs::MetadataExt;
    let proc_err = |e: std::io::Error| StoreError::Backup(format!("/proc/self: {e}"));
    let uid = std::fs::metadata("/proc/self").map_err(proc_err)?.uid();
    let cmdline = std::fs::read("/proc/self/cmdline").map_err(proc_err)?;
    data_dir_for(uid, &cmdline)
        .ok_or_else(|| StoreError::Backup("the process name is not an app package".into()))
}

#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn data_dir_for(uid: u32, cmdline: &[u8]) -> Option<PathBuf> {
    let name = cmdline.split(|b| *b == 0).next()?;
    let package = std::str::from_utf8(name).ok()?.split(':').next()?;
    let segment_ok = |s: &str| {
        s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    if !package.contains('.') || !package.split('.').all(segment_ok) {
        return None;
    }
    Some(PathBuf::from(format!(
        "/data/user/{}/{package}",
        uid / 100_000
    )))
}

/// Backup hygiene, not the platform no-backup API. This is not
/// `Context.getNoBackupFilesDir()`: nothing here calls into Android, and no
/// symlink is resolved. It checks the path's shape only: the index must sit
/// in the `no_backup` directory directly inside `data_dir`, the app's own
/// data directory, which Auto Backup and device transfer leave out. A
/// `no_backup` component anywhere else is a decoy and fails, and so does any
/// `..`. It is not a security boundary.
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
fn in_no_backup(index_dir: &Path, data_dir: &Path) -> bool {
    use std::path::Component;
    let Ok(rest) = index_dir.strip_prefix(data_dir.join("no_backup")) else {
        return false;
    };
    rest.components().next().is_some()
        && rest.components().all(|c| matches!(c, Component::Normal(_)))
}

fn check_backup_location(index_dir: &Path) -> Result<(), StoreError> {
    #[cfg(target_os = "android")]
    if !in_no_backup(index_dir, &android_data_dir()?) {
        return Err(StoreError::Backup(
            "the index is not in the app's no_backup directory".into(),
        ));
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
    // SAFETY: `url` is a typed file NSURL built from a UTF-8 path above, and
    // `yes` is an NSNumber, the value type NSURLIsExcludedFromBackupKey takes.
    // The key is a Foundation constant read only here. objc2 turns the
    // NSError out-parameter into the `Err` that is mapped below, so a failure
    // is returned, not dropped.
    let set =
        unsafe { url.setResourceValue_forKey_error(Some(&yes), NSURLIsExcludedFromBackupKey) };
    set.map_err(|e| StoreError::Backup(e.localizedDescription().to_string()))
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

    fn config(vault: &Path) -> Config {
        Config {
            vault: vault.to_path_buf(),
            inbox: "inbox".into(),
        }
    }

    /// Tests pick a scratch index directory through the crate-private
    /// `open_in`; the public `open` takes none.
    fn open(config: Config, index_dir: &Path) -> (Store, Receiver<Refreshed>) {
        let (tx, rx) = mpsc::channel();
        let store = Store::open_in(
            config,
            index_dir.to_path_buf(),
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
        // SAFETY: a typed file NSURL, a Foundation key constant, and an
        // out-slot objc2 types as Option<Retained<AnyObject>>.
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
        let (store, done) = open(config(&v), &index);

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
            let err = Store::open_in(config(&v), index.clone(), Box::new(|_, _| {}))
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
        let (store, done) = open(config(&v), &app.join("index"));
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
        let (store, done) = open(config(&v), &app.join("index"));
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
        let (store, done) = open(config(&v), &app.join("index"));
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
        let (store, done) = open(config(&v), &app.join("index"));
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
        let (store, _done) = open(config(&v), &app.join("index"));
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

    /// G0d: a reindex failure whose engine message carries the vault's path
    /// comes back on `index_error` with no path in it, and the note is still
    /// written.
    #[test]
    fn a_reindex_failure_carries_no_vault_path() {
        let v = vault();
        let app = dir("app");
        let (store, done) = open(config(&v), &app.join("index"));
        store.launched().unwrap();
        next(&done).result.unwrap();

        let leak = v.canonicalize().unwrap().join("inbox/Leak.md");
        let probe = rusqlite::Connection::open(store.index_dir().join("lattice.sqlite")).unwrap();
        probe
            .execute_batch(&format!(
                "CREATE TRIGGER leak BEFORE INSERT ON documents WHEN NEW.path LIKE 'inbox/%' \
                 BEGIN SELECT RAISE(ABORT, '{}'); END;",
                leak.display()
            ))
            .unwrap();

        let captured = store
            .capture("Leak", Some("Body.\n"), "2026-10-30")
            .unwrap();
        assert!(v.join(&captured.note.path).is_file(), "the note is written");
        let message = captured.index_error.expect("the reindex failed");
        assert!(!message.contains('/'), "{message}");
        assert!(!message.contains(v.to_str().unwrap()), "{message}");
        assert!(!message.contains("Leak"), "{message}");
        drop(probe);
        drop(store);
        let _ = std::fs::remove_dir_all(&v);
        let _ = std::fs::remove_dir_all(&app);
    }

    /// G0b: only the `no_backup` directly inside the app's data directory
    /// passes. A decoy `no_backup` anywhere else fails, and so does `..`.
    #[test]
    fn a_decoy_no_backup_component_fails_the_hygiene_check() {
        let data = Path::new("/data/user/0/com.hedronite.pendant");
        let ok = |p: &str| in_no_backup(Path::new(p), data);
        assert!(ok(
            "/data/user/0/com.hedronite.pendant/no_backup/notes-index"
        ));
        for decoy in [
            "/data/user/0/com.hedronite.pendant/files/no_backup/notes-index",
            "/data/user/0/com.hedronite.pendant/cache/no_backup",
            "/sdcard/no_backup/notes-index",
            "/storage/emulated/0/Android/data/com.hedronite.pendant/no_backup/notes-index",
            "/data/user/0/com.other.app/no_backup/notes-index",
            "/data/user/10/com.hedronite.pendant/no_backup/notes-index",
            "/data/user/0/com.hedronite.pendant/no_backup/../files/notes-index",
            "/data/user/0/com.hedronite.pendant/no_backup/./notes-index/..",
            "/data/user/0/com.hedronite.pendant/no_backup",
            "/data/user/0/com.hedronite.pendant/files/notes-index",
        ] {
            assert!(!ok(decoy), "{decoy}");
        }
    }

    /// The data directory comes from the process uid and name, never from a
    /// caller, and a process name that is not a package gives none.
    #[test]
    fn the_android_data_dir_is_derived_from_the_process() {
        let d = |uid, cmd: &[u8]| data_dir_for(uid, cmd).map(|p| p.display().to_string());
        assert_eq!(
            d(10_123, b"com.hedronite.pendant\0"),
            Some("/data/user/0/com.hedronite.pendant".into())
        );
        assert_eq!(
            d(1_010_123, b"com.hedronite.pendant:sync\0"),
            Some("/data/user/10/com.hedronite.pendant".into())
        );
        for bad in [
            &b"sh\0"[..],
            b"../../tmp\0",
            b"com.x/../y\0",
            b"com..x\0",
            b"\0",
            b"",
        ] {
            assert_eq!(d(10_123, bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }

    /// G0c: the public `open` names its own index directory in app-private
    /// storage; `Config` has no field to name another (see the compile_fail
    /// examples on `Store::open`).
    #[cfg(not(target_os = "android"))]
    #[test]
    fn the_index_directory_is_the_apps_own() {
        let dir = app_index_dir().unwrap();
        assert!(dir.starts_with(home().unwrap()), "{}", dir.display());
        assert!(dir.ends_with(INDEX), "{}", dir.display());
    }
}
