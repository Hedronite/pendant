//! iOS: `UIDocumentPickerViewController` for one folder, opened in place.
//!
//! The picker runs on the main thread. Its delegate does the whole look
//! there, while the picked URL's security scope is open: start access,
//! count through the URL's POSIX path, make a bookmark, stop access. The
//! report goes back to the caller through a channel. A bookmark made on
//! iOS needs no security-scope option; resolving it gives a URL that is
//! again security-scoped.

use std::cell::RefCell;
use std::sync::mpsc::{self, Sender};

use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{
    NSArray, NSData, NSObject, NSObjectProtocol, NSURLBookmarkCreationOptions,
    NSURLBookmarkResolutionOptions, NSURL,
};
use objc2_ui_kit::{
    UIApplication, UIDocumentPickerDelegate, UIDocumentPickerViewController, UIViewController,
    UIWindowScene,
};
use objc2_uniform_type_identifiers::{UTType, UTTypeFolder};

use super::{count_tree, kept_file, FolderError, FolderReport};

type Outcome = Result<FolderReport, FolderError>;

struct Ivars {
    reply: RefCell<Option<Sender<Outcome>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PendantFolderPicker"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UIDocumentPickerDelegate for Delegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn did_pick(&self, _picker: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
            let outcome = match urls.firstObject() {
                Some(url) => look(&url, true),
                None => Err(FolderError::Cancelled),
            };
            self.answer(outcome);
        }

        #[unsafe(method(documentPickerWasCancelled:))]
        fn cancelled(&self, _picker: &UIDocumentPickerViewController) {
            self.answer(Err(FolderError::Cancelled));
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, reply: Sender<Outcome>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            reply: RefCell::new(Some(reply)),
        });
        // SAFETY: NSObject's init on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }

    fn answer(&self, outcome: Outcome) {
        if let Some(reply) = self.ivars().reply.borrow_mut().take() {
            let _ = reply.send(outcome);
        }
        // The picker is gone; let the delegate go too.
        PENDING.with(|pending| pending.borrow_mut().take());
    }
}

thread_local! {
    /// The live delegate: the picker holds it weakly, so the main thread
    /// keeps it until it has answered.
    static PENDING: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
}

/// Counts the folder at `url` inside its security scope. With `keep`, a
/// bookmark is written for [`remembered`](super::remembered).
fn look(url: &NSURL, keep: bool) -> Outcome {
    // SAFETY: a live file URL from the picker or a resolved bookmark.
    let scoped = unsafe { url.startAccessingSecurityScopedResource() };
    let outcome = (|| {
        let path = url
            .path()
            .ok_or_else(|| FolderError::Platform("the picked URL has no path".into()))?;
        let mut report = count_tree(std::path::Path::new(&path.to_string()))
            .map_err(|err| FolderError::Platform(err.kind().to_string()))?;
        if keep {
            report.persisted = keep_bookmark(url).is_ok();
        }
        Ok(report)
    })();
    if scoped {
        // SAFETY: balances the start above.
        unsafe { url.stopAccessingSecurityScopedResource() };
    }
    outcome
}

fn keep_bookmark(url: &NSURL) -> Result<(), FolderError> {
    let data = url
        .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
            NSURLBookmarkCreationOptions(0),
            None,
            None,
        )
        .map_err(|err| FolderError::Platform(err.localizedDescription().to_string()))?;
    let file = kept_file().ok_or(FolderError::Platform("no data directory".into()))?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| FolderError::Platform(err.kind().to_string()))?;
    }
    std::fs::write(&file, data.to_vec())
        .map_err(|err| FolderError::Platform(err.kind().to_string()))
}

/// The view controller to present from: the key window's root, then down
/// through whatever it already presents.
fn presenter(mtm: MainThreadMarker) -> Option<Retained<UIViewController>> {
    let app = UIApplication::sharedApplication(mtm);
    let scenes = app.connectedScenes();
    let window = scenes
        .allObjects()
        .iter()
        .filter_map(|scene| scene.downcast::<UIWindowScene>().ok())
        .find_map(|scene| scene.keyWindow().or_else(|| scene.windows().firstObject()))?;
    let mut top = window.rootViewController()?;
    while let Some(next) = top.presentedViewController() {
        top = next;
    }
    Some(top)
}

pub(super) async fn pick() -> Outcome {
    let (reply, answer) = mpsc::channel::<Outcome>();
    let shown = nsh_core::prelude::run_on_main_thread(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            return Err(FolderError::Platform("not on the main thread".into()));
        };
        let presenter =
            presenter(mtm).ok_or(FolderError::Platform("no window to present from".into()))?;
        // SAFETY: a framework constant, alive for the process.
        let folder: &UTType = unsafe { UTTypeFolder };
        let types = NSArray::from_slice(&[folder]);
        let picker = UIDocumentPickerViewController::initForOpeningContentTypes(
            UIDocumentPickerViewController::alloc(mtm),
            &types,
        );
        picker.setAllowsMultipleSelection(false);
        let delegate = Delegate::new(mtm, reply);
        picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        PENDING.with(|pending| *pending.borrow_mut() = Some(delegate));
        presenter.presentViewController_animated_completion(&picker, true, None);
        Ok(())
    })
    .await
    .map_err(|err| FolderError::Platform(format!("{err:?}")))?;
    shown?;
    wait(answer).await
}

pub(super) async fn remembered() -> Outcome {
    let file = kept_file().ok_or(FolderError::NothingKept)?;
    let bytes = std::fs::read(file).map_err(|_| FolderError::NothingKept)?;
    let (reply, answer) = mpsc::channel::<Outcome>();
    let _ = nsh_core::prelude::run_on_main_thread(move || {
        let data = NSData::with_bytes(&bytes);
        let mut stale = Bool::NO;
        // SAFETY: `stale` is a live Bool for the call.
        let url = unsafe {
            NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
                &data,
                NSURLBookmarkResolutionOptions(0),
                None,
                &mut stale,
            )
        };
        let outcome = match url {
            Ok(url) => look(&url, stale.as_bool()).map(|report| FolderReport {
                persisted: true,
                restored: true,
                ..report
            }),
            Err(_) => Err(FolderError::NothingKept),
        };
        let _ = reply.send(outcome);
    })
    .await;
    wait(answer).await
}

/// Waits for the main thread's answer without blocking the engine.
async fn wait(answer: mpsc::Receiver<Outcome>) -> Outcome {
    crate::net::pair::off_thread(move || answer.recv())
        .await
        .and_then(Result::ok)
        .unwrap_or(Err(FolderError::Platform("the picker went away".into())))
}
