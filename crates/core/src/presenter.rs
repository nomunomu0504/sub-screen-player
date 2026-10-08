use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{Display, DisplayInfo, EncodedImage, Encoder, Error, Frame, Result};

/// What a [`Presenter`] does to the screen when it is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StopAction {
    /// Leave the screen alone. What happens next is up to the device.
    #[default]
    Leave,
    /// Store the last frame on the device so it stays visible without the host.
    SaveLast,
    /// Blank the screen.
    Clear,
    /// Switch the screen off.
    Sleep,
}

/// Settings of a [`Presenter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenterOptions {
    /// Upper bound for the frame rate. The display's own limit applies as well.
    pub max_fps: u32,
    /// JPEG quality (1..=100) when the device keeps up.
    pub quality: u8,
    /// Lowest JPEG quality used to keep up when frames come faster than the device takes them
    /// (fast animations, streams). Equal to `quality` keeps the quality fixed.
    pub min_quality: u8,
    /// Do not resend a frame that is identical to the one on screen.
    pub skip_duplicates: bool,
}

impl Default for PresenterOptions {
    fn default() -> Self {
        Self {
            max_fps: 60,
            quality: Encoder::DEFAULT_QUALITY,
            min_quality: Encoder::DEFAULT_MIN_QUALITY,
            skip_duplicates: true,
        }
    }
}

/// Counters describing what a [`Presenter`] did so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresenterStats {
    /// Frames passed to [`Presenter::submit`].
    pub submitted: u64,
    /// Frames sent to the device.
    pub shown: u64,
    /// Frames replaced by a newer one before they were sent.
    pub dropped: u64,
    /// Frames not sent because they equal the one on screen.
    pub duplicates: u64,
    /// Time the last frame took to encode.
    pub last_encode: Duration,
    /// Time the last frame took to send.
    pub last_send: Duration,
    /// Size of the last frame sent.
    pub last_bytes: usize,
    /// JPEG quality frames are encoded with now (see [`PresenterOptions::min_quality`]).
    pub quality: u8,
}

/// Owns one [`Display`] and feeds it frames from any thread.
///
/// Frames are encoded on one worker thread and sent on another, so encoding the next frame
/// overlaps with sending the current one. Only the newest frame is kept: if frames arrive faster
/// than the device takes them, older ones are dropped. Keep-alives are sent automatically.
pub struct Presenter {
    info: DisplayInfo,
    shared: Arc<Shared>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

struct Shared {
    state: Mutex<State>,
    /// Wakes the device thread: new encoded frame, new command or shutdown.
    device: Condvar,
    /// Wakes the encoder thread: new frame or shutdown.
    encoder: Condvar,
}

struct State {
    /// JPEG quality for the next frames, lowered while the device falls behind.
    quality: u8,
    pending: Option<Frame>,
    encoded: Option<EncodedImage>,
    commands: VecDeque<Command>,
    running: bool,
    failure: Option<String>,
    stats: PresenterStats,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

enum Op {
    Save(Box<Frame>),
    Brightness(u8),
    Wake,
    Sleep,
    Clear,
    Stop(StopAction),
}

struct Command {
    op: Op,
    reply: mpsc::SyncSender<Result<()>>,
}

impl Presenter {
    /// Takes ownership of `display` and starts the worker threads.
    pub fn spawn(display: Box<dyn Display>, options: PresenterOptions) -> Self {
        let info = display.info().clone();
        let quality = options.quality.clamp(1, 100);
        let min_quality = options.min_quality.clamp(1, quality);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                quality,
                pending: None,
                encoded: None,
                commands: VecDeque::new(),
                running: true,
                failure: None,
                stats: PresenterStats {
                    quality,
                    ..PresenterStats::default()
                },
            }),
            device: Condvar::new(),
            encoder: Condvar::new(),
        });
        let fps = options.max_fps.min(info.capabilities.max_fps).max(1);
        let worker = Worker {
            shared: shared.clone(),
            display,
            encoder: Encoder::new(quality),
            min_interval: Duration::from_secs(1) / fps,
            skip_duplicates: options.skip_duplicates,
            quality: QualityControl::new(quality, min_quality),
        };
        let encoder_shared = shared.clone();
        let encoder_info = info.clone();
        let threads = vec![
            spawn_named(format!("ssp-encode-{}", info.id()), move || {
                encode_loop(&encoder_shared, &encoder_info, Encoder::new(quality));
            }),
            spawn_named(format!("ssp-device-{}", info.id()), move || worker.run()),
        ];
        Self {
            info,
            shared,
            threads: Mutex::new(threads),
        }
    }

    /// The display's identity and capabilities.
    pub fn info(&self) -> &DisplayInfo {
        &self.info
    }

    /// Queues `frame` to be shown as soon as the device can take it. Returns immediately.
    /// A frame still waiting from an earlier call is replaced.
    pub fn submit(&self, frame: Frame) -> Result<()> {
        self.check_size(&frame)?;
        let mut state = self.shared.lock();
        state.ensure_running()?;
        state.stats.submitted += 1;
        if state.pending.replace(frame).is_some() {
            state.stats.dropped += 1;
        }
        self.shared.encoder.notify_one();
        Ok(())
    }

    /// Shows `frame` and stores it on the device. Blocks until the device took it.
    pub fn save(&self, frame: Frame) -> Result<()> {
        self.check_size(&frame)?;
        self.call(Op::Save(Box::new(frame)))
    }

    /// Sets the backlight (0..=100 %). Blocks until sent.
    pub fn set_brightness(&self, percent: u8) -> Result<()> {
        if percent > 100 {
            return Err(Error::InvalidArgument(format!(
                "brightness {percent} is over 100"
            )));
        }
        self.call(Op::Brightness(percent))
    }

    /// Switches the screen on. Blocks until sent.
    pub fn wake(&self) -> Result<()> {
        self.call(Op::Wake)
    }

    /// Switches the screen off. Blocks until sent.
    pub fn sleep(&self) -> Result<()> {
        self.call(Op::Sleep)
    }

    /// Blanks the screen. Blocks until sent.
    pub fn clear(&self) -> Result<()> {
        self.call(Op::Clear)
    }

    /// Counters so far.
    pub fn stats(&self) -> PresenterStats {
        self.shared.lock().stats.clone()
    }

    /// `false` once the presenter stopped, e.g. because the device was unplugged.
    pub fn is_running(&self) -> bool {
        self.shared.lock().running
    }

    /// Why the presenter stopped on its own, if it did.
    pub fn failure(&self) -> Option<String> {
        self.shared.lock().failure.clone()
    }

    /// Applies `action`, then stops the worker threads and closes the display.
    /// Does nothing if the presenter already stopped.
    pub fn stop(&self, action: StopAction) -> Result<()> {
        let result = match self.call(Op::Stop(action)) {
            Err(Error::Closed) => Ok(()),
            other => other,
        };
        let threads = std::mem::take(&mut *self.threads.lock().unwrap_or_else(|p| p.into_inner()));
        for thread in threads {
            let _ = thread.join();
        }
        result
    }

    fn check_size(&self, frame: &Frame) -> Result<()> {
        let panel = &self.info.panel;
        if (frame.width(), frame.height()) == (panel.width, panel.height) {
            Ok(())
        } else {
            Err(Error::InvalidArgument(format!(
                "frame is {}x{}, display is {}x{}",
                frame.width(),
                frame.height(),
                panel.width,
                panel.height
            )))
        }
    }

    fn call(&self, op: Op) -> Result<()> {
        let (reply, response) = mpsc::sync_channel(1);
        {
            let mut state = self.shared.lock();
            state.ensure_running()?;
            state.commands.push_back(Command { op, reply });
        }
        self.shared.device.notify_one();
        response.recv().unwrap_or(Err(Error::Closed))
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        let _ = self.stop(StopAction::Leave);
    }
}

impl State {
    fn ensure_running(&self) -> Result<()> {
        match (self.running, &self.failure) {
            (true, _) => Ok(()),
            (false, Some(_)) => Err(Error::Disconnected),
            (false, None) => Err(Error::Closed),
        }
    }
}

fn spawn_named(name: String, f: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(name)
        .spawn(f)
        .expect("failed to spawn a thread")
}

fn encode_loop(shared: &Shared, info: &DisplayInfo, mut encoder: Encoder) {
    loop {
        let frame = {
            let mut state = shared.lock();
            loop {
                if !state.running {
                    return;
                }
                if let Some(frame) = state.pending.take() {
                    encoder.set_quality(state.quality);
                    break frame;
                }
                state = shared
                    .encoder
                    .wait(state)
                    .unwrap_or_else(|p| p.into_inner());
            }
        };
        let started = Instant::now();
        match encoder.encode(&frame, &info.panel, info.capabilities.max_image_bytes) {
            Ok(image) => {
                let mut state = shared.lock();
                state.stats.last_encode = started.elapsed();
                if state.encoded.replace(image).is_some() {
                    state.stats.dropped += 1;
                }
                shared.device.notify_one();
            }
            Err(err) => tracing::warn!(display = %info.id(), "dropping a frame: {err}"),
        }
    }
}

enum Job {
    Command(Command),
    Show(EncodedImage),
    KeepAlive,
}

struct Worker {
    shared: Arc<Shared>,
    display: Box<dyn Display>,
    encoder: Encoder,
    min_interval: Duration,
    skip_duplicates: bool,
    quality: QualityControl,
}

/// Lowers the JPEG quality while the device falls behind and raises it again when it keeps up.
///
/// Falling behind shows as dropped frames: a newer frame replaced one that was never sent.
/// Smaller frames send faster, so the quality settles where the device just keeps up. Content
/// that changes slowly (a clock, a dashboard) never drops frames and stays at full quality.
struct QualityControl {
    max: u8,
    min: u8,
    /// Dropped frames counted at the last adjustment.
    dropped: u64,
    /// Frames sent in a row without a drop.
    calm: u32,
}

impl QualityControl {
    /// Frames sent without a drop before the quality goes up a step.
    const CALM_FRAMES: u32 = 4;

    fn new(max: u8, min: u8) -> Self {
        Self {
            max,
            min,
            dropped: 0,
            calm: 0,
        }
    }

    /// The quality for the next frames, after a frame took `send` to send.
    /// `idle` tells that the frame came after a pause, i.e. the source is slow.
    fn adjust(
        &mut self,
        current: u8,
        dropped: u64,
        send: Duration,
        interval: Duration,
        idle: bool,
    ) -> u8 {
        let behind = dropped > self.dropped;
        self.dropped = dropped;
        if behind {
            self.calm = 0;
            // Far behind: bigger steps, so a stream settles within a second.
            let step = if send > interval * 3 / 2 { 5 } else { 2 };
            return current.saturating_sub(step).max(self.min);
        }
        if idle {
            self.calm = 0;
            return self.max;
        }
        self.calm += 1;
        if self.calm >= Self::CALM_FRAMES {
            self.calm = 0;
            return (current + 1).min(self.max);
        }
        current
    }
}

impl Worker {
    fn run(mut self) {
        let keep_alive = self.display.info().capabilities.keep_alive_interval;
        let mut last_show: Option<Instant> = None;
        let mut last_keep_alive = Instant::now();
        // The last frame sent, and whether it is still what the screen shows.
        let mut last: Option<EncodedImage> = None;
        let mut on_screen = false;

        loop {
            let job = self.next_job(last_show, keep_alive.map(|every| last_keep_alive + every));
            let result = match job {
                Job::KeepAlive => {
                    last_keep_alive = Instant::now();
                    self.display.keep_alive()
                }
                Job::Show(image) => {
                    if self.skip_duplicates && on_screen && last.as_ref() == Some(&image) {
                        self.shared.lock().stats.duplicates += 1;
                        continue;
                    }
                    let started = Instant::now();
                    let idle = last_show.is_none_or(|t| started - t > self.min_interval * 4);
                    last_show = Some(started);
                    let result = self.display.show(&image);
                    if result.is_ok() {
                        let send = started.elapsed();
                        let mut state = self.shared.lock();
                        state.stats.shown += 1;
                        state.stats.last_send = send;
                        state.stats.last_bytes = image.data.len();
                        let dropped = state.stats.dropped;
                        state.quality = self.quality.adjust(
                            state.quality,
                            dropped,
                            send,
                            self.min_interval,
                            idle,
                        );
                        state.stats.quality = state.quality;
                        last = Some(image);
                        on_screen = true;
                    }
                    result
                }
                Job::Command(Command { op, reply }) => {
                    let stop = matches!(op, Op::Stop(_));
                    let result = match op {
                        Op::Save(frame) => self.save(&frame).map(|image| {
                            last = Some(image);
                            on_screen = true;
                        }),
                        Op::Brightness(percent) => self.display.set_brightness(percent),
                        Op::Wake => self.display.wake().inspect(|()| on_screen = false),
                        Op::Sleep => self.display.sleep().inspect(|()| on_screen = false),
                        Op::Clear => self.display.clear().inspect(|()| on_screen = false),
                        Op::Stop(action) => {
                            let latest = self.shared.lock().encoded.take().or(last.take());
                            self.apply_stop(action, latest.as_ref())
                        }
                    };
                    let fatal = result.as_ref().is_err_and(Error::is_fatal);
                    // Close before replying, so a caller that gets the error already sees the
                    // presenter as stopped.
                    if stop {
                        self.close(None);
                    } else if fatal {
                        self.close(result.as_ref().err().map(ToString::to_string));
                    }
                    let _ = reply.send(result);
                    if stop || fatal {
                        return;
                    }
                    continue;
                }
            };
            if let Err(err) = result {
                if err.is_fatal() {
                    tracing::warn!(display = %self.display.info().id(), "display lost: {err}");
                    self.close(Some(err.to_string()));
                    return;
                }
                tracing::warn!(display = %self.display.info().id(), "{err}");
            }
        }
    }

    /// Waits until there is something to do.
    fn next_job(&self, last_show: Option<Instant>, keep_alive_at: Option<Instant>) -> Job {
        let mut state = self.shared.lock();
        loop {
            if let Some(command) = state.commands.pop_front() {
                return Job::Command(command);
            }
            let now = Instant::now();
            let ready_at = last_show.map_or(now, |t| t + self.min_interval);
            if state.encoded.is_some() && now >= ready_at {
                return Job::Show(state.encoded.take().expect("checked above"));
            }
            if keep_alive_at.is_some_and(|t| now >= t) {
                return Job::KeepAlive;
            }
            let wake_at = match (state.encoded.is_some(), keep_alive_at) {
                (true, Some(t)) => Some(ready_at.min(t)),
                (true, None) => Some(ready_at),
                (false, t) => t,
            };
            state = match wake_at {
                Some(t) => {
                    self.shared
                        .device
                        .wait_timeout(state, t - now)
                        .unwrap_or_else(|p| p.into_inner())
                        .0
                }
                None => self
                    .shared
                    .device
                    .wait(state)
                    .unwrap_or_else(|p| p.into_inner()),
            };
        }
    }

    fn save(&mut self, frame: &Frame) -> Result<EncodedImage> {
        let info = self.display.info();
        let image = self
            .encoder
            .encode(frame, &info.panel, info.capabilities.max_image_bytes)?;
        self.display.save(&image)?;
        Ok(image)
    }

    fn apply_stop(&mut self, action: StopAction, latest: Option<&EncodedImage>) -> Result<()> {
        match action {
            StopAction::Leave => Ok(()),
            StopAction::SaveLast => latest.map_or(Ok(()), |image| self.display.save(image)),
            StopAction::Clear => self.display.clear(),
            StopAction::Sleep => self.display.sleep(),
        }
    }

    /// Marks the presenter stopped and fails everything still queued.
    fn close(&self, failure: Option<String>) {
        let mut state = self.shared.lock();
        state.running = false;
        state.failure = failure;
        state.pending = None;
        state.encoded = None;
        let failed = state.failure.is_some();
        for command in state.commands.drain(..) {
            let _ = command.reply.send(Err(if failed {
                Error::Disconnected
            } else {
                Error::Closed
            }));
        }
        self.shared.encoder.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Call, FakeDisplay};

    fn frame(shade: u8) -> Frame {
        let mut frame = Frame::blank(1920, 462);
        frame
            .image_mut()
            .pixels_mut()
            .for_each(|p| p.0 = [shade; 3]);
        frame
    }

    fn shows(calls: &[Call]) -> usize {
        calls.iter().filter(|c| matches!(c, Call::Show(_))).count()
    }

    fn wait_until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn shows_submitted_frames_and_skips_duplicates() {
        let (display, log) = FakeDisplay::new();
        let presenter = Presenter::spawn(Box::new(display), PresenterOptions::default());
        presenter.submit(frame(10)).unwrap();
        wait_until(|| shows(&log.calls()) == 1);
        presenter.submit(frame(10)).unwrap();
        wait_until(|| presenter.stats().duplicates == 1);
        presenter.submit(frame(20)).unwrap();
        wait_until(|| shows(&log.calls()) == 2);
        presenter.stop(StopAction::Leave).unwrap();
    }

    #[test]
    fn latest_frame_wins_when_the_device_is_slow() {
        let (display, log) = FakeDisplay::new();
        let display = display.show_delay(Duration::from_millis(100));
        // A fixed quality, so the last frame can be compared byte for byte.
        let options = PresenterOptions {
            min_quality: Encoder::DEFAULT_QUALITY,
            ..Default::default()
        };
        let presenter = Presenter::spawn(Box::new(display), options);
        for shade in 0..20 {
            presenter.submit(frame(shade)).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        wait_until(|| presenter.stats().shown + presenter.stats().dropped >= 20);
        let stats = presenter.stats();
        assert!(stats.dropped > 0, "{stats:?}");
        // The last frame submitted is the last one shown.
        wait_until(|| {
            log.calls()
                .iter()
                .rev()
                .find(|c| matches!(c, Call::Show(_)))
                .is_some_and(|c| {
                    let Call::Show(data) = c else { unreachable!() };
                    *data
                        == Encoder::default()
                            .encode(&frame(19), &presenter.info().panel, usize::MAX)
                            .unwrap()
                            .data
                })
        });
        presenter.stop(StopAction::Leave).unwrap();
    }

    #[test]
    fn paces_frames_to_max_fps() {
        let (display, log) = FakeDisplay::new();
        let options = PresenterOptions {
            max_fps: 10,
            skip_duplicates: false,
            ..Default::default()
        };
        let presenter = Presenter::spawn(Box::new(display), options);
        let started = Instant::now();
        for _ in 0..3 {
            presenter.submit(frame(1)).unwrap();
            wait_until(|| presenter.stats().submitted == presenter.stats().shown);
        }
        wait_until(|| shows(&log.calls()) == 3);
        assert!(started.elapsed() >= Duration::from_millis(200));
        presenter.stop(StopAction::Leave).unwrap();
    }

    #[test]
    fn lowers_the_quality_while_the_device_falls_behind() {
        let (display, _log) = FakeDisplay::new();
        // A device that needs 40 ms per frame cannot keep up with frames every 5 ms.
        let display = display.show_delay(Duration::from_millis(40));
        let options = PresenterOptions {
            quality: 85,
            min_quality: 70,
            skip_duplicates: false,
            ..Default::default()
        };
        let presenter = Presenter::spawn(Box::new(display), options);
        assert_eq!(presenter.stats().quality, 85);
        for shade in 0..200u8 {
            presenter.submit(frame(shade)).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(presenter.stats().quality, 70, "{:?}", presenter.stats());

        // A slow source (a clock) gets full quality back.
        wait_until(|| {
            presenter.stats().submitted == presenter.stats().shown + presenter.stats().dropped
        });
        std::thread::sleep(Duration::from_millis(300));
        presenter.submit(frame(1)).unwrap();
        wait_until(|| presenter.stats().quality == 85);
        presenter.stop(StopAction::Leave).unwrap();
    }

    #[test]
    fn keeps_a_fixed_quality_when_min_equals_quality() {
        let (display, _log) = FakeDisplay::new();
        let display = display.show_delay(Duration::from_millis(40));
        let options = PresenterOptions {
            quality: 80,
            min_quality: 80,
            skip_duplicates: false,
            ..Default::default()
        };
        let presenter = Presenter::spawn(Box::new(display), options);
        for shade in 0..60u8 {
            presenter.submit(frame(shade)).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(presenter.stats().quality, 80);
        presenter.stop(StopAction::Leave).unwrap();
    }

    #[test]
    fn sends_keep_alives() {
        let (display, log) = FakeDisplay::new();
        let display = display.keep_alive_interval(Duration::from_millis(20));
        let presenter = Presenter::spawn(Box::new(display), PresenterOptions::default());
        wait_until(|| {
            log.calls()
                .iter()
                .filter(|c| **c == Call::KeepAlive)
                .count()
                >= 3
        });
        presenter.stop(StopAction::Leave).unwrap();
    }

    #[test]
    fn commands_run_in_order_and_stop_saves_the_last_frame() {
        let (display, log) = FakeDisplay::new();
        let presenter = Presenter::spawn(Box::new(display), PresenterOptions::default());
        presenter.set_brightness(40).unwrap();
        presenter.submit(frame(7)).unwrap();
        wait_until(|| shows(&log.calls()) == 1);
        presenter.clear().unwrap();
        presenter.stop(StopAction::SaveLast).unwrap();
        let calls = log.take();
        assert_eq!(calls[0], Call::Brightness(40));
        assert!(matches!(calls[1], Call::Show(_)));
        assert_eq!(calls[2], Call::Clear);
        assert!(matches!(&calls[3], Call::Save(d) if Call::Show(d.clone()) == calls[1]));
    }

    #[test]
    fn rejects_bad_input_without_stopping() {
        let (display, _log) = FakeDisplay::new();
        let presenter = Presenter::spawn(Box::new(display), PresenterOptions::default());
        assert!(matches!(
            presenter.submit(Frame::blank(10, 10)),
            Err(Error::InvalidArgument(_))
        ));
        assert!(matches!(
            presenter.set_brightness(101),
            Err(Error::InvalidArgument(_))
        ));
        assert!(presenter.is_running());
    }

    #[test]
    fn stops_when_the_device_is_unplugged() {
        let (display, _log) = FakeDisplay::new();
        let unplug = display.unplug_handle();
        let presenter = Presenter::spawn(Box::new(display), PresenterOptions::default());
        unplug.unplug();
        assert!(matches!(presenter.wake(), Err(Error::Disconnected)));
        assert!(!presenter.is_running());
        assert!(presenter.failure().is_some());
        assert!(matches!(
            presenter.submit(frame(1)),
            Err(Error::Disconnected)
        ));
    }
}
