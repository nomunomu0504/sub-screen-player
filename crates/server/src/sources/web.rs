//! Shows a web page, rendered by headless Chrome at the panel's size.
//!
//! The browser runs only while the page is shown. Its screencast sends a JPEG whenever the page
//! repaints, so a still page costs nothing after the first frame and an animated one is sent as
//! fast as the display takes it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::json;
use ssp_core::{Fit, Frame};

use super::Source;
use crate::text::{TextStyle, builtin_font};
use crate::web::Web;
use crate::web::cdp::{Browser, Connection};
use crate::web::page_token::PageToken;

/// How long [`Page::render`] waits for a new frame before showing the last one again.
const FRAME_WAIT: Duration = Duration::from_millis(500);
/// How often the browser thread looks for a stop request.
const POLL: Duration = Duration::from_millis(100);
/// How long to wait for a page to load before showing it anyway.
const LOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// What to show: a page and how often to reload it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebPage {
    /// An `http`, `https` or `file` URL.
    pub url: String,
    /// Reload every this many seconds, if set.
    pub reload: Option<u64>,
}

impl WebPage {
    /// Checks that `url` is an `http`, `https` or `file` URL.
    pub fn new(url: &str, reload: Option<u64>) -> Result<Self, String> {
        let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL {url:?}: {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https" | "file") {
            return Err(format!(
                "only http, https and file URLs can be shown, not {url:?}"
            ));
        }
        if reload == Some(0) {
            return Err("reload must be at least 1 second".into());
        }
        Ok(Self {
            url: parsed.into(),
            reload,
        })
    }
}

/// Shows a [`WebPage`].
pub struct Page {
    page: WebPage,
    web: Web,
    /// Lets the page read the API while it is shown (`window.ssp`).
    token: PageToken,
    running: Option<Running>,
    shown: u64,
    last: Option<Frame>,
    /// Why the page stopped, once it has.
    failure: Option<String>,
    ended: bool,
}

impl Page {
    /// Shows `page`; the browser starts with the first frame, when the panel size is known.
    pub fn new(page: WebPage, web: Web) -> Self {
        Self {
            page,
            web,
            token: PageToken::issue(),
            running: None,
            shown: 0,
            last: None,
            failure: None,
            ended: false,
        }
    }
}

impl Source for Page {
    fn render(&mut self, frame: &mut Frame) {
        let size = (frame.width(), frame.height());
        if !self.ended && self.running.as_ref().is_none_or(|r| r.size != size) {
            self.running = None;
            self.shown = 0;
            self.running = Some(Running::start(
                &self.page,
                self.web.clone(),
                self.token.as_str(),
                size,
            ));
        }
        if let Some(running) = &self.running {
            let mut slot = running.wait_for(self.shown);
            if slot.count > self.shown
                && let Some(new) = slot.frame.take()
            {
                self.shown = slot.count;
                self.last = Some(new);
            }
            if let Some(reason) = slot.ended.clone() {
                drop(slot);
                tracing::warn!(url = %self.page.url, "web page stopped: {reason}");
                self.ended = true;
                // Without a picture of the page, say what went wrong.
                if self.last.is_none() {
                    self.last = Some(message(size, &reason));
                }
                self.failure = Some(reason);
            }
        }
        if let Some(last) = &self.last {
            frame.clone_from(last);
        }
    }

    fn next_change(&self) -> Option<Duration> {
        // `render` waits for the browser's next frame.
        (!self.ended).then_some(Duration::ZERO)
    }
}

/// A browser showing the page for one panel size, on its own thread.
struct Running {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    size: (u32, u32),
}

#[derive(Default)]
struct Shared {
    slot: Mutex<Slot>,
    changed: Condvar,
}

#[derive(Default)]
struct Slot {
    frame: Option<Frame>,
    count: u64,
    ended: Option<String>,
}

impl Running {
    fn start(page: &WebPage, web: Web, token: &str, size: (u32, u32)) -> Self {
        let shared = Arc::new(Shared::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (to_thread, stop_thread, page) = (shared.clone(), stop.clone(), page.clone());
        let token = token.to_owned();
        let thread = std::thread::Builder::new()
            .name("ssp-web".into())
            .spawn(move || {
                let result = run(&page, &web, &token, size, &to_thread, &stop_thread);
                let reason = match result {
                    Ok(()) => return,
                    Err(reason) => reason,
                };
                lock(&to_thread.slot).ended = Some(reason);
                to_thread.changed.notify_all();
            })
            .ok();
        if thread.is_none() {
            lock(&shared.slot).ended = Some("cannot start a thread".into());
        }
        Self {
            shared,
            stop,
            thread,
            size,
        }
    }

    fn wait_for(&self, shown: u64) -> MutexGuard<'_, Slot> {
        let slot = lock(&self.shared.slot);
        self.shared
            .changed
            .wait_timeout_while(slot, FRAME_WAIT, |s| s.count == shown && s.ended.is_none())
            .map_or_else(|poisoned| poisoned.into_inner().0, |(slot, _)| slot)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts the browser, opens the page and passes its screencast frames on until `stop`.
fn run(
    page: &WebPage,
    web: &Web,
    token: &str,
    (width, height): (u32, u32),
    shared: &Shared,
    stop: &AtomicBool,
) -> Result<(), String> {
    let chrome = web.ensure()?;
    let browser = Browser::launch(&chrome, width, height)?;
    let mut cdp = Connection::connect(&browser.url)?;
    // Use the page the browser opened at start rather than another tab.
    let targets = cdp.call("Target.getTargets", json!({}), None)?;
    let opened = targets["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["type"] == "page")
        .and_then(|t| t["targetId"].as_str())
        .map(str::to_owned);
    let target = match opened {
        Some(target) => target,
        None => {
            let created = cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None)?;
            created["targetId"]
                .as_str()
                .ok_or("no target id")?
                .to_owned()
        }
    };
    let attached = cdp.call(
        "Target.attachToTarget",
        json!({ "targetId": target, "flatten": true }),
        None,
    )?;
    let session = attached["sessionId"]
        .as_str()
        .ok_or("no session id")?
        .to_owned();
    let session = Some(session.as_str());
    cdp.call(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": width, "height": height, "deviceScaleFactor": 1, "mobile": false }),
        session,
    )?;
    cdp.call("Page.enable", json!({}), session)?;
    // Before the page's own scripts, in every document it loads (also after a reload).
    if let Some(api) = web.api() {
        let ssp = json!({ "api": api, "token": token });
        cdp.call(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": format!("window.ssp = Object.freeze({ssp});") }),
            session,
        )?;
    }
    let navigated = cdp.call("Page.navigate", json!({ "url": page.url }), session)?;
    if let Some(error) = navigated.get("errorText").and_then(|e| e.as_str()) {
        return Err(format!("cannot open {}: {error}", page.url));
    }
    // The page may move to another renderer process while it loads; the screencast has to
    // start after that.
    let loading = Instant::now();
    while loading.elapsed() < LOAD_TIMEOUT && !stop.load(Ordering::SeqCst) {
        if cdp
            .next_event(POLL)?
            .is_some_and(|event| event.method == "Page.loadEventFired")
        {
            break;
        }
    }
    // Chrome's own headless mode treats the page as hidden, without screencast frames, unless
    // it is told the page has focus.
    cdp.call(
        "Emulation.setFocusEmulationEnabled",
        json!({ "enabled": true }),
        session,
    )?;
    cdp.call(
        "Page.startScreencast",
        json!({ "format": "jpeg", "quality": 92, "maxWidth": width, "maxHeight": height }),
        session,
    )?;
    tracing::info!(url = %page.url, "showing a web page");

    let mut reloaded = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        if let Some(every) = page.reload
            && reloaded.elapsed() >= Duration::from_secs(every)
        {
            cdp.send("Page.reload", json!({ "ignoreCache": true }), session)?;
            reloaded = Instant::now();
        }
        let Some(event) = cdp.next_event(POLL)? else {
            continue;
        };
        match event.method.as_str() {
            "Page.screencastFrame" => {
                // Acknowledge first: the browser sends the next frame only after that.
                let id = event.params["sessionId"].clone();
                cdp.send(
                    "Page.screencastFrameAck",
                    json!({ "sessionId": id }),
                    session,
                )?;
                let Some(data) = event.params["data"].as_str() else {
                    continue;
                };
                match decode(data, width, height) {
                    Ok(frame) => {
                        let mut slot = lock(&shared.slot);
                        slot.frame = Some(frame);
                        slot.count += 1;
                        drop(slot);
                        shared.changed.notify_all();
                    }
                    Err(err) => tracing::debug!("bad screencast frame: {err}"),
                }
            }
            "Inspector.targetCrashed" => return Err("the page crashed".into()),
            "Target.detachedFromTarget" | "Inspector.detached" => {
                return Err("the browser closed the page".into());
            }
            _ => {}
        }
    }
    drop(browser);
    Ok(())
}

/// A screencast frame (base64 JPEG) as a panel-sized frame.
fn decode(data: &str, width: u32, height: u32) -> Result<Frame, String> {
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| e.to_string())?;
    let image = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
        .map_err(|e| e.to_string())?;
    if image.width() == width && image.height() == height {
        return Ok(Frame::new(image.into_rgb8()));
    }
    Ok(Frame::fit(&image, width, height, Fit::Contain))
}

/// A frame with `text` in grey on black, for errors.
fn message((width, height): (u32, u32), text: &str) -> Frame {
    let mut frame = Frame::blank(width, height);
    let px = (height as f32 / 14.0).clamp(12.0, 34.0);
    let style = TextStyle {
        font: builtin_font(),
        px,
        tabular: false,
    };
    let margin = px;
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if style.width(&candidate) > width as f32 - 2.0 * margin && !line.is_empty() {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = candidate;
        }
    }
    lines.push(line);
    let leading = px * 1.4;
    let top = (height as f32 - leading * lines.len() as f32) / 2.0 + px;
    for (i, line) in lines.iter().enumerate() {
        let baseline = top + leading * i as f32;
        style.draw(frame.image_mut(), margin, baseline, line, [200, 200, 200]);
    }
    frame
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::page_token;

    #[test]
    fn accepts_web_and_file_urls_only() {
        assert!(WebPage::new("https://example.com/panel", Some(60)).is_ok());
        assert!(WebPage::new("file:///tmp/panel.html", None).is_ok());
        assert!(WebPage::new("javascript:alert(1)", None).is_err());
        assert!(WebPage::new("ftp://example.com", None).is_err());
        assert!(WebPage::new("not a url", None).is_err());
        assert!(WebPage::new("https://example.com", Some(0)).is_err());
    }

    #[test]
    fn says_what_went_wrong_without_a_browser() {
        let web = Web::with_dir(None, std::env::temp_dir().join("ssp-no-chrome-for-test"));
        let page = WebPage::new("https://example.com", None).unwrap();
        let mut source = Page::new(page, web);
        let mut frame = Frame::blank(480, 120);
        source.render(&mut frame);
        assert_eq!(source.next_change(), None);
        let lit = frame.image().pixels().filter(|p| p.0 != [0, 0, 0]).count();
        assert!(lit > 100, "the message is drawn");
    }

    /// Shows a local page with the browser in `SSP_TEST_CHROME`, if set.
    #[test]
    fn shows_a_page_in_chrome() {
        let Some(chrome) = std::env::var_os("SSP_TEST_CHROME") else {
            eprintln!("skipped: set SSP_TEST_CHROME to a Chrome to run it");
            return;
        };
        let dir = std::env::temp_dir().join(format!("ssp-web-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let html = dir.join("page.html");
        // Red only if the page got where the API is and its token before its own script ran.
        let page = "<body style='margin:0;background:#00f'><script>\
            const ok = window.ssp && ssp.api === 'http://127.0.0.1:7920/api/v1' \
            && ssp.token.length === 32;\
            if (ok) document.body.style.background = '#f00';</script></body>";
        std::fs::write(&html, page).unwrap();
        let url = url::Url::from_file_path(&html).unwrap();
        let web = Web::with_dir(Some(chrome.into()), dir.clone())
            .with_api("127.0.0.1:7920".parse().unwrap());
        let mut source = Page::new(WebPage::new(url.as_str(), None).unwrap(), web);
        let token = source.token.as_str().to_owned();
        assert!(page_token::is_valid(&token));
        let mut frame = Frame::blank(320, 80);
        let started = Instant::now();
        while frame.image().get_pixel(160, 40).0[0] < 200
            && source.failure.is_none()
            && started.elapsed().as_secs() < 90
        {
            source.render(&mut frame);
        }
        assert_eq!(source.failure, None);
        let pixel = frame.image().get_pixel(160, 40).0;
        assert!(
            pixel[0] > 200 && pixel[1] < 60,
            "{pixel:?} after {:?}",
            started.elapsed()
        );
        drop(source);
        assert!(
            !page_token::is_valid(&token),
            "the token ends with the page"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
