//! Drops of promised files — what Apple Photos (and Mail, and some other apps) put on the drag
//! pasteboard instead of ordinary files. WebKit tells the page "Files" are coming but hands it an
//! empty list, so the web view's `performDragOperation:` is wrapped: when the drag carries file
//! promises, the files are written to a folder and their paths are handed to the page
//! (`window.gwDroppedPaths`); any other drop goes to WebKit as before.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, ProtocolObject, Sel};
use objc2::{sel, ClassType};
use objc2_app_kit::{NSDraggingInfo, NSFilePromiseReceiver};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSOperationQueue, NSURL};
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, OnceLock};

type Perform = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut ProtocolObject<dyn NSDraggingInfo>) -> Bool;

static ORIGINAL: OnceLock<Perform> = OnceLock::new();
static SINK: OnceLock<Box<dyn Fn(Vec<String>, Vec<String>) + Send + Sync>> = OnceLock::new();

/// Where promised files are written: one folder per drop, under the gitwebsite cache.
fn drop_dir() -> PathBuf {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    gw_core::config::expand_home("~/.gitwebsite/cache/drops").join(format!("{}-{}", t.as_secs(), t.subsec_nanos()))
}

/// Wrap the web view's drop handling. `sink(paths, errors)` runs once every promised file of a
/// drop has arrived (on a background queue).
pub fn install(webview: *mut c_void, sink: impl Fn(Vec<String>, Vec<String>) + Send + Sync + 'static) {
    if webview.is_null() || SINK.set(Box::new(sink)).is_err() {
        return;
    }
    // SAFETY: `webview` is the window's WKWebView (a live Objective-C object); we replace the
    // implementation of one method on its class with a function of the same signature and keep
    // the original to call for every drop we don't handle.
    unsafe {
        let obj = &*(webview as *const AnyObject);
        let cls: &AnyClass = obj.class();
        let m = objc2::ffi::class_getInstanceMethod(cls, sel!(performDragOperation:));
        if m.is_null() {
            return;
        }
        let ours: Imp = std::mem::transmute::<Perform, Imp>(perform);
        if let Some(old) = objc2::ffi::method_setImplementation(m, ours) {
            let _ = ORIGINAL.set(std::mem::transmute::<Imp, Perform>(old));
        }
    }
}

unsafe extern "C-unwind" fn perform(this: *mut AnyObject, cmd: Sel, info: *mut ProtocolObject<dyn NSDraggingInfo>) -> Bool {
    if let Some(handled) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| receive_promises(&*info))).ok().flatten() {
        return handled;
    }
    match ORIGINAL.get() {
        Some(f) => f(this, cmd, info),
        None => Bool::NO,
    }
}

/// `Some(YES)` when the drag carried file promises (now being received); `None` to let WebKit
/// handle it.
fn receive_promises(info: &ProtocolObject<dyn NSDraggingInfo>) -> Option<Bool> {
    let pb = info.draggingPasteboard();
    let classes = NSArray::from_slice(&[NSFilePromiseReceiver::class()]);
    // SAFETY: NSFilePromiseReceiver conforms to NSPasteboardReading; no options.
    let objs = unsafe { pb.readObjectsForClasses_options(&classes, None) }?;
    let receivers: Vec<Retained<NSFilePromiseReceiver>> = objs.iter().filter_map(|o| o.downcast::<NSFilePromiseReceiver>().ok()).collect();
    if receivers.is_empty() {
        return None;
    }
    let dir = drop_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let dest = NSURL::from_directory_path(&dir)?;
    let expected: usize = receivers.iter().map(|r| r.fileNames().count().max(1)).sum();
    // (paths, errors, files seen, handed over)
    let state = Arc::new(Mutex::new((Vec::<String>::new(), Vec::<String>::new(), 0usize, false)));
    let queue = NSOperationQueue::new();
    let options = NSDictionary::new();
    for r in &receivers {
        let state = state.clone();
        let reader = RcBlock::new(move |url: NonNull<NSURL>, err: *mut NSError| {
            let mut s = state.lock().unwrap();
            if err.is_null() {
                // SAFETY: the reader is called with a valid file URL when there is no error.
                if let Some(p) = unsafe { url.as_ref() }.to_file_path() {
                    s.0.push(p.display().to_string());
                }
            } else {
                // SAFETY: non-null error from AppKit.
                s.1.push(unsafe { &*err }.localizedDescription().to_string());
            }
            s.2 += 1;
            if s.2 >= expected && !s.3 {
                s.3 = true;
                if let Some(sink) = SINK.get() {
                    sink(std::mem::take(&mut s.0), std::mem::take(&mut s.1));
                }
            }
        });
        // SAFETY: a folder URL, an empty options dictionary and a private queue.
        unsafe { r.receivePromisedFilesAtDestination_options_operationQueue_reader(&dest, &options, &queue, &reader) };
    }
    // If a promise never reports back (or names fewer files than it delivers), hand over what
    // has arrived after a minute rather than waiting forever.
    let late = state.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(60));
        let mut s = late.lock().unwrap();
        if !s.3 && (!s.0.is_empty() || !s.1.is_empty()) {
            s.3 = true;
            if let Some(sink) = SINK.get() {
                sink(std::mem::take(&mut s.0), std::mem::take(&mut s.1));
            }
        }
    });
    Some(Bool::YES)
}
