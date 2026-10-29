//! The live preview of web pages on Linux: a WebKitGTK window of its own, opened from the
//! artifact panel's "Open live preview" button.
//!
//! The release builds link no GTK or WebKitGTK (their build machines have no such libraries), so
//! the libraries are opened at run time with `libloading`, and only the few functions used here
//! are looked up. Without them, or without a display, the page opens in the browser instead
//! (`show` reports it, and `take_fallback_notice` says so once).
//!
//! GTK runs on one thread only, the preview thread: it pumps GLib's main loop between the
//! commands the app sends. The window's closing comes back on a second channel, read by
//! `is_open`.
//!
//! Every C call is in this file, next to a `SAFETY:` note. Callbacks catch panics, since a panic
//! must not unwind into C.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::ffi::{CStr, CString, c_char, c_int, c_ulong, c_void};
use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use libloading::Library;

use crate::config::HtmlPreviewSettings;
use crate::ui::webview;

const FALSE: c_int = 0;
const TRUE: c_int = 1;
/// `GTK_WINDOW_TOPLEVEL`.
const GTK_WINDOW_TOPLEVEL: c_int = 0;
/// `WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION` and `..._NEW_WINDOW_ACTION`.
const POLICY_NAVIGATION: c_int = 0;
const POLICY_NEW_WINDOW: c_int = 1;

/// The WebKitGTK libraries to try, newest first (both use GTK 3).
const WEBKIT_LIBRARIES: [&str; 2] = ["libwebkit2gtk-4.1.so.0", "libwebkit2gtk-4.0.so.37"];
const NOT_FOUND: &str = "WebKitGTK not found: install libwebkit2gtk-4.1 (with GTK 3) to preview \
                         web pages in their own window";
/// The least time between two reloads while a streaming reply changes the page.
const LOAD_GAP: Duration = Duration::from_millis(250);
/// How long the preview thread waits for a command before it pumps GTK again.
const TICK: Duration = Duration::from_millis(16);
/// Most GTK events handled per tick, so a busy page can't starve the commands.
const MAX_PUMP: usize = 64;
const WINDOW_SIZE: (c_int, c_int) = (960, 720);

// The C signatures of the functions used, as the GTK 3 and WebKit2GTK 4.x headers declare them.
// A wrong one would be undefined behaviour, so keep them in step with the headers.
type InitCheck = unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char) -> c_int;
type WindowNew = unsafe extern "C" fn(c_int) -> *mut c_void;
type WindowSetTitle = unsafe extern "C" fn(*mut c_void, *const c_char);
type WindowSetDefaultSize = unsafe extern "C" fn(*mut c_void, c_int, c_int);
type WindowPresent = unsafe extern "C" fn(*mut c_void);
type ContainerAdd = unsafe extern "C" fn(*mut c_void, *mut c_void);
type WidgetShowAll = unsafe extern "C" fn(*mut c_void);
type ViewNew = unsafe extern "C" fn() -> *mut c_void;
type ViewLoadHtml = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char);
type ViewGetSettings = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type SettingsSetJavascript = unsafe extern "C" fn(*mut c_void, c_int);
type NavigationDecisionAction = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type ActionGetRequest = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type RequestGetUri = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type DecisionUse = unsafe extern "C" fn(*mut c_void);
/// `g_signal_connect_data`, with its callback as the generic `GCallback`.
type SignalConnect = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    unsafe extern "C" fn(),
    *mut c_void,
    *mut c_void,
    c_int,
) -> c_ulong;
type MainIteration = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;

/// A "destroy" callback: (widget, data).
type CloseFn = unsafe extern "C" fn(*mut c_void, *mut c_void);
/// A "decide-policy" callback: (view, decision, decision type, data).
type PolicyFn = unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut c_void) -> c_int;
/// A signal callback as `g_signal_connect_data` takes it (`GCallback`).
type Callback = unsafe extern "C" fn();

/// The libraries (kept open for the program's life) and the functions looked up in them.
struct Api {
    _libraries: Vec<Library>,
    init_check: InitCheck,
    window_new: WindowNew,
    window_set_title: WindowSetTitle,
    window_set_default_size: WindowSetDefaultSize,
    window_present: WindowPresent,
    container_add: ContainerAdd,
    widget_show_all: WidgetShowAll,
    view_new: ViewNew,
    view_load_html: ViewLoadHtml,
    view_get_settings: ViewGetSettings,
    settings_set_javascript: SettingsSetJavascript,
    navigation_action: NavigationDecisionAction,
    action_get_request: ActionGetRequest,
    request_get_uri: RequestGetUri,
    decision_use: DecisionUse,
    decision_ignore: DecisionUse,
    signal_connect: SignalConnect,
    main_iteration: MainIteration,
}

/// The first library in `names` that `load` opens, or `None`. Tried in order.
pub fn first_loadable<T>(names: &[&str], mut load: impl FnMut(&str) -> Option<T>) -> Option<T> {
    names.iter().find_map(|name| load(name))
}

fn open(name: &str) -> Option<Library> {
    // SAFETY: opening a library runs its initialisers. These are the desktop's own GTK and
    // WebKitGTK libraries, which every program using them loads the same way.
    unsafe { Library::new(name) }.ok()
}

/// The function `name` of `lib`, typed as `T`.
///
/// SAFETY: `T` must be the C function pointer type that the symbol `name` has.
unsafe fn symbol<T: Copy>(lib: &Library, name: &str) -> Result<T, String> {
    // SAFETY: the caller guarantees that `T` matches the symbol (the function's contract).
    let found = unsafe { lib.get::<T>(format!("{name}\0").as_bytes()) }
        .map_err(|e| format!("{name}: {e}"))?;
    Ok(*found)
}

fn load_api() -> Result<Api, String> {
    let gtk = open("libgtk-3.so.0").ok_or(NOT_FOUND)?;
    let gobject = open("libgobject-2.0.so.0").ok_or(NOT_FOUND)?;
    let glib = open("libglib-2.0.so.0").ok_or(NOT_FOUND)?;
    let webkit = first_loadable(&WEBKIT_LIBRARIES, open).ok_or(NOT_FOUND)?;
    // SAFETY: each symbol is looked up with the C signature of its header declaration (above).
    unsafe {
        Ok(Api {
            init_check: symbol(&gtk, "gtk_init_check")?,
            window_new: symbol(&gtk, "gtk_window_new")?,
            window_set_title: symbol(&gtk, "gtk_window_set_title")?,
            window_set_default_size: symbol(&gtk, "gtk_window_set_default_size")?,
            window_present: symbol(&gtk, "gtk_window_present")?,
            container_add: symbol(&gtk, "gtk_container_add")?,
            widget_show_all: symbol(&gtk, "gtk_widget_show_all")?,
            view_new: symbol(&webkit, "webkit_web_view_new")?,
            view_load_html: symbol(&webkit, "webkit_web_view_load_html")?,
            view_get_settings: symbol(&webkit, "webkit_web_view_get_settings")?,
            settings_set_javascript: symbol(&webkit, "webkit_settings_set_enable_javascript")?,
            navigation_action: symbol(
                &webkit,
                "webkit_navigation_policy_decision_get_navigation_action",
            )?,
            action_get_request: symbol(&webkit, "webkit_navigation_action_get_request")?,
            request_get_uri: symbol(&webkit, "webkit_uri_request_get_uri")?,
            decision_use: symbol(&webkit, "webkit_policy_decision_use")?,
            decision_ignore: symbol(&webkit, "webkit_policy_decision_ignore")?,
            signal_connect: symbol(&gobject, "g_signal_connect_data")?,
            main_iteration: symbol(&glib, "g_main_context_iteration")?,
            _libraries: vec![gtk, gobject, glib, webkit],
        })
    }
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

/// The libraries and functions, loaded once. The error names what is missing.
fn api() -> Result<&'static Api, String> {
    API.get_or_init(load_api).as_ref().map_err(Clone::clone)
}

/// Whether WebKitGTK can be used: its libraries open and have the functions the preview needs.
/// (A display is only needed when a window opens; see `show`.)
pub fn probe() -> Result<(), String> {
    api().map(|_| ())
}

/// The note under the Settings checkbox: whether WebKitGTK was found.
pub fn status() -> String {
    match probe() {
        Ok(()) => "WebKitGTK found: web pages open in their own window.".to_owned(),
        Err(_) => "WebKitGTK not found: web pages open in your browser. Install libwebkit2gtk-4.1 \
                   (Debian and Ubuntu: libwebkit2gtk-4.1-0, Fedora: webkit2gtk4.1, Arch: \
                   webkit2gtk-4.1) to preview them live."
            .to_owned(),
    }
}

static NOTICE_DUE: AtomicBool = AtomicBool::new(true);

/// Whether the fallback notice is still to be shown: once per run of the app.
pub fn take_fallback_notice() -> bool {
    NOTICE_DUE.swap(false, Ordering::Relaxed)
}

/// Whether a link may load in the preview itself: the page and its in-page anchors.
pub fn loads_in_view(url: &str) -> bool {
    url.starts_with("about:") || url.starts_with("data:")
}

/// Whether a link opens in the browser instead (web links only, as on the other platforms).
pub fn opens_in_browser(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Whether a page may load now: the last load was at least `LOAD_GAP` ago, or there was none.
pub fn load_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= LOAD_GAP)
}

/// Loads at most once per `LOAD_GAP`, keeping only the newest queued item: a streaming reply
/// doesn't reload the window on every frame, and the last version still arrives.
#[derive(Default)]
struct Throttle<T> {
    last: Option<Instant>,
    pending: Option<T>,
}

impl<T> Throttle<T> {
    fn queue(&mut self, item: T) {
        self.pending = Some(item);
    }

    /// The pending item, if it may load now (and then counts as loaded).
    fn take(&mut self, now: Instant) -> Option<T> {
        if self.pending.is_some() && load_due(self.last, now) {
            self.mark_loaded(now);
            self.pending.take()
        } else {
            None
        }
    }

    fn mark_loaded(&mut self, now: Instant) {
        self.last = Some(now);
    }
}

/// The app's requests to the preview thread.
enum Command {
    /// Open the window if needed, show `html` (already with its policy) and bring it forward.
    Show {
        title: String,
        html: String,
        scripts: bool,
    },
    /// Show `html` in the open window, reloaded at most every `LOAD_GAP`.
    Update { html: String, scripts: bool },
}

/// What the preview thread tells the app.
enum Event {
    /// The window was closed (by the user).
    Closed,
}

#[derive(Default)]
struct Remote {
    commands: Option<Sender<Command>>,
    events: Option<Receiver<Event>>,
    /// Why the preview thread could not start, so later calls don't wait for it again.
    failed: Option<String>,
    /// The window is open.
    open: bool,
    /// Fingerprint of the page the window was last sent.
    shown: Option<u64>,
}

static REMOTE: Mutex<Remote> = Mutex::new(Remote {
    commands: None,
    events: None,
    failed: None,
    open: false,
    shown: None,
});

fn remote() -> MutexGuard<'static, Remote> {
    REMOTE.lock().unwrap_or_else(|e| e.into_inner())
}

fn fingerprint(page: &str) -> u64 {
    let mut h = DefaultHasher::new();
    page.hash(&mut h);
    h.finish()
}

thread_local! {
    // The GTK objects. Only the preview thread touches them, and so do the callbacks.
    static WINDOW: Cell<*mut c_void> = const { Cell::new(null_mut()) };
    static VIEW: Cell<*mut c_void> = const { Cell::new(null_mut()) };
    static EVENTS: RefCell<Option<Sender<Event>>> = const { RefCell::new(None) };
}

fn send_event(event: Event) {
    EVENTS.with(|e| {
        if let Some(tx) = e.borrow().as_ref() {
            let _ = tx.send(event);
        }
    });
}

/// Start the preview thread (once) and wait until GTK has started on it.
fn start(r: &mut Remote) -> Result<(), String> {
    if r.commands.is_some() {
        return Ok(());
    }
    if let Some(why) = &r.failed {
        return Err(why.clone());
    }
    api()?;
    let (commands, command_rx) = mpsc::channel();
    let (event_tx, events) = mpsc::channel();
    let (ready_tx, ready) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("live-preview".into())
        .spawn(move || run(command_rx, event_tx, ready_tx))
        .map_err(|e| e.to_string())?;
    match ready.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => {
            r.commands = Some(commands);
            r.events = Some(events);
            Ok(())
        }
        Ok(Err(why)) => {
            r.failed = Some(why.clone());
            Err(why)
        }
        Err(_) => {
            let why = "the live preview didn't start in time".to_owned();
            r.failed = Some(why.clone());
            Err(why)
        }
    }
}

/// Read what the window reported since the last call.
fn drain(r: &mut Remote) {
    let Remote { events, open, .. } = r;
    if let Some(events) = events {
        while let Ok(event) = events.try_recv() {
            match event {
                Event::Closed => *open = false,
            }
        }
    }
}

/// Show `html` in the preview window, opening it if need be and bringing it forward. An error
/// says why the window can't be used, and the caller opens the browser instead.
pub fn show(title: &str, html: &str, s: &HtmlPreviewSettings) -> Result<(), String> {
    let page = webview::page(html, s);
    let mut r = remote();
    start(&mut r)?;
    drain(&mut r);
    let commands = r
        .commands
        .as_ref()
        .ok_or("the live preview isn't running")?;
    commands
        .send(Command::Show {
            title: title.to_owned(),
            html: page.clone(),
            scripts: s.scripts,
        })
        .map_err(|_| "the live preview has stopped".to_owned())?;
    r.shown = Some(fingerprint(&page));
    r.open = true;
    Ok(())
}

/// Keep the open window on `html` (the page the panel shows), reloading it as it changes.
/// Does nothing while no window is open or the preview is off.
pub fn follow(html: &str, s: &HtmlPreviewSettings) {
    let mut r = remote();
    drain(&mut r);
    if !s.enabled || !r.open {
        return;
    }
    let page = webview::page(html, s);
    let print = fingerprint(&page);
    if r.shown == Some(print) {
        return;
    }
    let sent = r.commands.as_ref().is_some_and(|tx| {
        tx.send(Command::Update {
            html: page,
            scripts: s.scripts,
        })
        .is_ok()
    });
    if sent {
        r.shown = Some(print);
    }
}

/// Whether the preview window is open (the user may have closed it since the last call).
pub fn is_open() -> bool {
    let mut r = remote();
    drain(&mut r);
    r.open
}

/// The preview thread: GTK lives here. It takes the app's commands in turn with GTK's events.
fn run(commands: Receiver<Command>, events: Sender<Event>, ready: SyncSender<Result<(), String>>) {
    EVENTS.with(|e| *e.borrow_mut() = Some(events));
    let Ok(api) = api() else {
        return;
    };
    // SAFETY: gtk_init_check accepts null arguments (it then parses no command line). It runs on
    // this thread only, the one thread that uses GTK.
    if unsafe { (api.init_check)(null_mut(), null_mut()) } == FALSE {
        let _ = ready.send(Err(
            "no display to open the window on (DISPLAY and WAYLAND_DISPLAY are unset or unreachable)"
                .to_owned(),
        ));
        return;
    }
    let _ = ready.send(Ok(()));
    let mut throttle: Throttle<(String, bool)> = Throttle::default();
    loop {
        match commands.recv_timeout(TICK) {
            Ok(command) => handle(api, command, &mut throttle),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        loop {
            match commands.try_recv() {
                Ok(command) => handle(api, command, &mut throttle),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if let Some((html, scripts)) = throttle.take(Instant::now()) {
            load_page(api, &html, scripts);
        }
        for _ in 0..MAX_PUMP {
            // SAFETY: a null context is this thread's default main context. A non-blocking
            // iteration returns at once, so the loop keeps taking commands.
            if unsafe { (api.main_iteration)(null_mut(), FALSE) } == FALSE {
                break;
            }
        }
    }
}

fn handle(api: &Api, command: Command, throttle: &mut Throttle<(String, bool)>) {
    match command {
        Command::Show {
            title,
            html,
            scripts,
        } => {
            if VIEW.get().is_null() {
                create_window(api);
            }
            let title = CString::new(title.replace('\0', "")).unwrap_or_default();
            // SAFETY: the window exists (just made, or still open: the destroy callback clears
            // WINDOW when it goes), and the title is a NUL-terminated copy.
            unsafe {
                (api.window_set_title)(WINDOW.get(), title.as_ptr());
                (api.window_present)(WINDOW.get());
            }
            throttle.pending = None;
            load_page(api, &html, scripts);
            throttle.mark_loaded(Instant::now());
        }
        Command::Update { html, scripts } => throttle.queue((html, scripts)),
    }
}

/// Connect `callback` to `signal` of `object`, for the life of the object.
///
/// SAFETY: `object` is a live GObject on the GTK thread, and `callback` has the C signature that
/// `signal` calls it with.
unsafe fn connect(api: &Api, object: *mut c_void, signal: &CStr, callback: Callback) {
    // SAFETY: the arguments are as the caller guarantees above; no destroy notify is needed
    // (the callbacks take no data).
    unsafe {
        (api.signal_connect)(object, signal.as_ptr(), callback, null_mut(), null_mut(), 0);
    }
}

/// Make the window and its web view, with the link policy and the close signal connected.
fn create_window(api: &Api) {
    // SAFETY: called on the GTK thread after gtk_init_check. Each widget is added to its parent
    // (or kept for the program's life). The callbacks have the signatures GTK documents for
    // "destroy" (widget, data) and "decide-policy" (view, decision, type, data); they are only
    // cast to the generic callback type that g_signal_connect_data takes.
    unsafe {
        let window = (api.window_new)(GTK_WINDOW_TOPLEVEL);
        let view = (api.view_new)();
        (api.window_set_default_size)(window, WINDOW_SIZE.0, WINDOW_SIZE.1);
        (api.container_add)(window, view);
        let on_close: unsafe extern "C" fn(*mut c_void, *mut c_void) = on_destroy;
        connect(
            api,
            window,
            c"destroy",
            std::mem::transmute::<CloseFn, Callback>(on_close),
        );
        let on_policy: PolicyFn = on_decide_policy;
        connect(
            api,
            view,
            c"decide-policy",
            std::mem::transmute::<PolicyFn, Callback>(on_policy),
        );
        (api.widget_show_all)(window);
        WINDOW.set(window);
        VIEW.set(view);
    }
}

/// Load `html` into the window's view, with JavaScript on or off.
fn load_page(api: &Api, html: &str, scripts: bool) {
    let view = VIEW.get();
    if view.is_null() {
        return;
    }
    let html = CString::new(html.replace('\0', "")).unwrap_or_default();
    // SAFETY: `view` is the live web view from `create_window`. Its settings object belongs to
    // the view, and the page is a NUL-terminated copy that lives for the call.
    unsafe {
        let settings = (api.view_get_settings)(view);
        (api.settings_set_javascript)(settings, c_int::from(scripts));
        (api.view_load_html)(view, html.as_ptr(), null_mut::<c_char>());
    }
}

/// The window was closed: forget it, so the next preview makes a new one.
unsafe extern "C" fn on_destroy(_window: *mut c_void, _data: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        WINDOW.set(null_mut());
        VIEW.set(null_mut());
        send_event(Event::Closed);
    }));
}

/// Links: the page itself loads, web links open in the browser, anything else stays put.
unsafe extern "C" fn on_decide_policy(
    _view: *mut c_void,
    decision: *mut c_void,
    kind: c_int,
    _data: *mut c_void,
) -> c_int {
    catch_unwind(AssertUnwindSafe(|| {
        if kind != POLICY_NAVIGATION && kind != POLICY_NEW_WINDOW {
            return FALSE;
        }
        let Ok(api) = api() else {
            return FALSE;
        };
        // SAFETY: `decision` is the live navigation decision WebKit passed in, and the objects
        // read from it stay valid until the callback returns.
        let url = unsafe { navigation_url(api, decision) };
        // SAFETY: the decision is answered once, here: used for the page itself, ignored
        // otherwise.
        unsafe {
            if loads_in_view(&url) {
                (api.decision_use)(decision);
            } else {
                (api.decision_ignore)(decision);
                if opens_in_browser(&url) {
                    let _ = open::that_detached(&url);
                }
            }
        }
        TRUE
    }))
    .unwrap_or(FALSE)
}

/// The URL a navigation decision is for ("" when there is none).
///
/// SAFETY: `decision` is a navigation policy decision that WebKit passed to a callback, so it is
/// live for the call.
unsafe fn navigation_url(api: &Api, decision: *mut c_void) -> String {
    // SAFETY: the getters return objects owned by the decision or its request (not new
    // references), which stay valid while the decision does.
    unsafe {
        let action = (api.navigation_action)(decision);
        if action.is_null() {
            return String::new();
        }
        let request = (api.action_get_request)(action);
        if request.is_null() {
            return String::new();
        }
        let uri = (api.request_get_uri)(request);
        if uri.is_null() {
            return String::new();
        }
        CStr::from_ptr(uri).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_webkit_library_that_opens_wins() {
        let mut tried = Vec::new();
        let found = first_loadable(&WEBKIT_LIBRARIES, |name| {
            tried.push(name.to_owned());
            (name == WEBKIT_LIBRARIES[1]).then_some(name.len())
        });
        assert_eq!(found, Some(WEBKIT_LIBRARIES[1].len()));
        assert_eq!(tried, WEBKIT_LIBRARIES.map(str::to_owned));
        assert!(WEBKIT_LIBRARIES[0].contains("4.1"), "newest first");

        let mut tries = 0;
        let none: Option<()> = first_loadable(&WEBKIT_LIBRARIES, |_| {
            tries += 1;
            None
        });
        assert!(none.is_none());
        assert_eq!(tries, 2);
    }

    #[test]
    fn loads_are_spaced_and_only_the_newest_pending_load_goes() {
        let t0 = Instant::now();
        assert!(load_due(None, t0));
        assert!(!load_due(Some(t0), t0 + LOAD_GAP / 2));
        assert!(load_due(Some(t0), t0 + LOAD_GAP));

        let mut throttle = Throttle::default();
        assert_eq!(throttle.take(t0), None::<&str>);
        throttle.queue("first");
        throttle.queue("second");
        throttle.queue("third");
        assert_eq!(throttle.take(t0), Some("third"), "only the newest loads");
        throttle.queue("fourth");
        assert_eq!(
            throttle.take(t0 + LOAD_GAP / 2),
            None,
            "too soon: still pending"
        );
        assert_eq!(throttle.take(t0 + LOAD_GAP), Some("fourth"));
        assert_eq!(
            throttle.take(t0 + LOAD_GAP * 2),
            None,
            "nothing left to load"
        );
    }

    #[test]
    fn links_load_in_the_view_or_open_in_the_browser() {
        assert!(loads_in_view("about:blank"));
        assert!(loads_in_view("about:blank#top"));
        assert!(loads_in_view("data:text/plain,hi"));
        assert!(!loads_in_view("https://example.com/"));
        assert!(!loads_in_view("file:///etc/passwd"));
        assert!(opens_in_browser("https://example.com/"));
        assert!(opens_in_browser("http://example.com/"));
        assert!(!opens_in_browser("javascript:alert(1)"));
        assert!(!opens_in_browser("file:///etc/passwd"));
        assert!(!opens_in_browser("mailto:someone@example.com"));
    }

    #[test]
    fn the_probe_and_its_note_agree_without_panicking() {
        let found = probe().is_ok();
        assert_eq!(status().starts_with("WebKitGTK found"), found);
    }

    #[test]
    #[ignore = "opens a WebKitGTK window: needs a display and libwebkit2gtk-4.1"]
    fn the_preview_window_opens_and_follows_the_page() {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            eprintln!("no display: skipped");
            return;
        }
        if probe().is_err() {
            eprintln!("WebKitGTK not installed: skipped");
            return;
        }
        let s = HtmlPreviewSettings::default();
        show("Test", "<!doctype html><p>hello</p>", &s).expect("the window opens");
        assert!(is_open());
        follow("<!doctype html><p>changed</p>", &s);
        std::thread::sleep(Duration::from_millis(800));
        assert!(is_open(), "the window stays open while it follows");
        // With CLEANRSTAI_PREVIEW_HOLD set, keep the window up so it can be screenshotted.
        if let Some(secs) = std::env::var("CLEANRSTAI_PREVIEW_HOLD")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            follow(
                "<!doctype html><h1 style='color:#c15f3c'>CleanRSTAI live preview</h1>",
                &s,
            );
            std::thread::sleep(Duration::from_secs(secs));
        }
    }
}
