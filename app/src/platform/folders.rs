//! The D1 probe: what folder access each phone gives an app, measured on a
//! made-up fixture (25 top-level `.md`, 3 other files, one nested folder
//! with 5 more `.md`). It answers D1's questions and nothing else: there is
//! no Lapis link here, and nothing is indexed or kept but the grant.
//!
//! - **iOS:** `UIDocumentPickerViewController` for a folder. The picked URL
//!   is security-scoped: access is started, the folder is counted through
//!   its POSIX path, a bookmark is kept, and access is stopped. After a
//!   restart, [`remembered`] resolves the bookmark and counts again.
//! - **Android:** `ACTION_OPEN_DOCUMENT_TREE`. The tree is a `content://`
//!   URI, not a path; the persistable read permission is taken and the
//!   tree is counted through `DocumentsContract`. After a restart,
//!   [`remembered`] checks the persisted permission and counts again.
//!
//! A report says whether a POSIX path was available (what Lapis opens) and
//! whether access was persisted or restored.

use std::fmt;
use std::path::Path;

/// What one look at a folder found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderReport {
    /// `.md` files directly in the folder.
    pub md_top: usize,
    /// `.md` files in the folder and every folder under it.
    pub md_total: usize,
    /// Files that are not `.md`, at any depth.
    pub other_files: usize,
    /// Folders under the picked one, at any depth.
    pub folders: usize,
    /// The platform gave a POSIX path the app could open.
    pub posix_path: bool,
    /// Access was kept for later: a bookmark (iOS) or a persisted URI
    /// permission (Android).
    pub persisted: bool,
    /// This report came from kept access after a restart, not a new pick.
    pub restored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderError {
    /// The person closed the picker.
    Cancelled,
    /// Nothing was picked before, or its access is gone.
    NothingKept,
    /// The platform refused, with what it said.
    Platform(String),
    /// This target has no folder picker.
    Unsupported(&'static str),
}

impl fmt::Display for FolderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("no folder was picked"),
            Self::NothingKept => f.write_str("no folder access is kept"),
            Self::Platform(why) => write!(f, "the folder could not be read: {why}"),
            Self::Unsupported(why) => write!(f, "no folder picker here: {why}"),
        }
    }
}

/// Whether `name` is a Markdown file name (`.md`, any case).
pub fn is_markdown(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// Counts a folder through its path. Hidden entries (a leading `.`) are
/// skipped, as a vault's tooling skips them; symlinks are not followed.
pub fn count_tree(root: &Path) -> std::io::Result<FolderReport> {
    let mut report = FolderReport {
        posix_path: true,
        ..FolderReport::default()
    };
    walk(root, true, &mut report)?;
    Ok(report)
}

fn walk(dir: &Path, top: bool, report: &mut FolderReport) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            report.other_files += 1;
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            report.folders += 1;
            walk(&entry.path(), false, report)?;
        } else if kind.is_file() {
            if is_markdown(name) {
                report.md_total += 1;
                if top {
                    report.md_top += 1;
                }
            } else {
                report.other_files += 1;
            }
        }
    }
    Ok(())
}

/// Shows the platform's folder picker and reports on the folder picked.
/// Keeps access for [`remembered`].
pub async fn pick() -> Result<FolderReport, FolderError> {
    imp::pick().await
}

/// Reports on the folder picked before, through the access kept then.
pub async fn remembered() -> Result<FolderReport, FolderError> {
    imp::remembered().await
}

/// Where kept access is written: a small file in the app's data directory
/// (an iOS bookmark, or an Android tree URI). Not a secret.
/// Call it on the engine thread: nsh's `data_dir` panics outside its own
/// context (a UIKit delegate on the main thread, a worker thread).
#[cfg(any(target_os = "ios", target_os = "android"))]
fn kept_file() -> Option<std::path::PathBuf> {
    nsh_core::prelude::data_dir().map(|dir| dir.join("d1-folder"))
}

#[cfg(target_os = "ios")]
#[path = "folders_ios.rs"]
mod imp;

#[cfg(target_os = "android")]
#[path = "folders_android.rs"]
mod imp;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod imp {
    use super::{FolderError, FolderReport};

    const WHY: &str = "not a phone";

    pub(super) async fn pick() -> Result<FolderReport, FolderError> {
        Err(FolderError::Unsupported(WHY))
    }

    pub(super) async fn remembered() -> Result<FolderReport, FolderError> {
        Err(FolderError::Unsupported(WHY))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The D1 fixture's shape, made up here: 25 top-level notes, 3 other
    /// files, one nested folder with 5 more notes.
    fn fixture() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pendant-d1-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        for n in 1..=25 {
            std::fs::write(
                root.join(format!("note-{n:02}.md")),
                format!("# Fixture note {n}\n"),
            )
            .unwrap();
        }
        for (name, body) in [
            ("readme.txt", "made up"),
            ("data.json", "{}"),
            ("image.PNG", "x"),
        ] {
            std::fs::write(root.join(name), body).unwrap();
        }
        for n in 1..=5 {
            std::fs::write(nested.join(format!("deep-{n}.MD")), "# Nested\n").unwrap();
        }
        std::fs::write(root.join(".hidden.md"), "skipped").unwrap();
        root
    }

    #[test]
    fn the_fixture_counts_as_d1_expects() {
        let root = fixture();
        let report = count_tree(&root).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(report.md_top, 25);
        assert_eq!(report.md_total, 30);
        assert_eq!(report.other_files, 3);
        assert_eq!(report.folders, 1);
        assert!(report.posix_path);
        assert!(!report.persisted && !report.restored);
    }

    #[test]
    fn markdown_is_the_md_extension_in_any_case() {
        for yes in ["a.md", "b.MD", "c.Md", ".x.md"] {
            assert!(is_markdown(yes), "{yes}");
        }
        for no in ["a.markdown", "md", "a.md.txt", "a.mdx", "readme"] {
            assert!(!is_markdown(no), "{no}");
        }
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    #[test]
    fn the_host_has_no_picker() {
        let error = futures_block(pick()).unwrap_err();
        assert!(matches!(error, FolderError::Unsupported(_)));
    }

    /// Polls a future that is ready at once (the host stubs are).
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    fn futures_block<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut cx = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("the host stub is not immediate"),
        }
    }
}
