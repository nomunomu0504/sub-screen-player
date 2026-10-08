//! Video files, played in a loop by the ffmpeg installed on the computer.
//!
//! No video decoder is built into `ssp`: ffmpeg runs as a child process that reads the file at
//! its own speed (`-re`), scales each frame to the panel and writes raw RGB frames to its
//! standard output. A reader thread keeps only the newest frame, so a display slower than the
//! video skips frames instead of falling behind.

use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ssp_core::{Fit, Frame};

use super::Source;

/// Largest video file accepted through the API.
pub const MAX_VIDEO_BYTES: u64 = 4 << 30;
/// Frame rate videos are capped at (the fastest any supported display goes).
const MAX_FPS: u32 = 60;
/// How long the check that ffmpeg can read a file may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// How long [`Player::render`] waits for a new frame before showing the last one again.
const FRAME_WAIT: Duration = Duration::from_millis(500);
/// Where ffmpeg is usually installed, for when it is not on the daemon's `PATH` (launchd starts
/// the daemon with a minimal one).
const USUAL_PLACES: &[&str] = &[
    "/opt/homebrew/bin/ffmpeg",
    "/usr/local/bin/ffmpeg",
    "/usr/bin/ffmpeg",
    "/snap/bin/ffmpeg",
];
/// How to install ffmpeg, shown when it cannot be found.
pub const INSTALL_HINT: &str = "video playback needs ffmpeg: `brew install ffmpeg` (macOS), \
    `sudo apt install ffmpeg` (Debian, Ubuntu) or `winget install ffmpeg` (Windows), \
    or set `[video] ffmpeg` in the config";

/// Whether `head`, the first bytes of a file, looks like a video file ffmpeg can read: MP4 and
/// QuickTime, Matroska and WebM, AVI or MPEG transport streams. Image files (AVIF and HEIF also
/// start like MP4) are not videos.
pub fn is_video(head: &[u8]) -> bool {
    let at = |offset: usize, magic: &[u8]| head.get(offset..offset + magic.len()) == Some(magic);
    if at(4, b"ftyp") {
        let brand = head.get(8..12).unwrap_or_default();
        return ![&b"avif"[..], b"avis", b"heic", b"heix", b"mif1", b"msf1"].contains(&brand);
    }
    at(0, &[0x1A, 0x45, 0xDF, 0xA3])
        || (at(0, b"RIFF") && at(8, b"AVI "))
        || (head.first() == Some(&0x47) && head.get(188) == Some(&0x47))
        || [&b"moov"[..], b"mdat", b"wide"]
            .iter()
            .any(|atom| at(4, atom))
}

/// Finds ffmpeg: `configured`, else on `PATH`, else where it is usually installed.
pub fn find_ffmpeg(configured: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = configured {
        return if path.is_file() {
            Ok(path.to_owned())
        } else {
            Err(format!("[video] ffmpeg: {} does not exist", path.display()))
        };
    }
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(name));
    on_path
        .chain(USUAL_PLACES.iter().map(PathBuf::from))
        .find(|path| path.is_file())
        .ok_or_else(|| INSTALL_HINT.to_owned())
}

/// A video file to play, with the ffmpeg that plays it. A file the daemon was sent is deleted
/// once no content uses it any more.
#[derive(Debug)]
pub struct VideoFile {
    path: PathBuf,
    ffmpeg: PathBuf,
    temporary: bool,
}

impl VideoFile {
    /// Checks that `ffmpeg` can read a video stream from `path`. A `temporary` file is deleted
    /// when the result is dropped, also when the check fails.
    pub fn open(path: PathBuf, ffmpeg: PathBuf, temporary: bool) -> Result<Self, String> {
        let video = Self {
            path,
            ffmpeg,
            temporary,
        };
        video.probe()?;
        Ok(video)
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Decodes the first frame, to fail early on files ffmpeg cannot play.
    fn probe(&self) -> Result<(), String> {
        let mut command = Command::new(&self.ffmpeg);
        command
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(&self.path)
            .args(["-map", "0:v:0", "-frames:v", "1", "-f", "null", "-"]);
        let (status, errors) = run(command, PROBE_TIMEOUT)
            .map_err(|err| format!("cannot run {}: {err}", self.ffmpeg.display()))?;
        if status {
            return Ok(());
        }
        let reason = errors
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .unwrap_or("unknown error")
            .trim()
            .to_owned();
        Err(format!("ffmpeg cannot play this video: {reason}"))
    }
}

impl Drop for VideoFile {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Runs `command` to the end (or kills it after `timeout`); whether it succeeded, and what it
/// wrote to standard error.
fn run(mut command: Command, timeout: Duration) -> std::io::Result<(bool, String)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn_quietly()?;
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status.success();
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                ErrorKind::TimedOut,
                "ffmpeg took too long",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok((status, errors.join().unwrap_or_default()))
}

trait SpawnQuietly {
    /// Spawns without opening a console window on Windows.
    fn spawn_quietly(&mut self) -> std::io::Result<Child>;
}

impl SpawnQuietly for Command {
    fn spawn_quietly(&mut self) -> std::io::Result<Child> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self.spawn()
    }
}

/// What the API needs to play videos: the ffmpeg to use, and where files sent to
/// `POST /displays/{id}/image` are kept while they play.
#[derive(Debug)]
pub struct Videos {
    ffmpeg: Option<PathBuf>,
    dir: PathBuf,
    next: AtomicU64,
}

impl Videos {
    /// Keeps uploads in `dir`, which no other daemon may use; it is emptied now. `ffmpeg` is
    /// the configured program, if any.
    pub fn new(ffmpeg: Option<PathBuf>, dir: PathBuf) -> std::io::Result<Self> {
        if dir.exists() {
            for entry in std::fs::read_dir(&dir)?.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        } else {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(Self {
            ffmpeg,
            dir,
            next: AtomicU64::new(0),
        })
    }

    /// The ffmpeg program (see [`find_ffmpeg`]).
    pub fn ffmpeg(&self) -> Result<PathBuf, String> {
        find_ffmpeg(self.ffmpeg.as_deref())
    }

    /// A new file name for an upload.
    pub fn file(&self) -> PathBuf {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        self.dir.join(format!("video-{n}"))
    }
}

/// Plays a [`VideoFile`] in a loop.
pub struct Player {
    video: Arc<VideoFile>,
    fit: Fit,
    running: Option<Running>,
    /// Number of the last frame shown.
    shown: u64,
    last: Option<Frame>,
    ended: bool,
}

impl Player {
    /// Plays `video`; ffmpeg starts with the first frame, when the panel size is known.
    pub fn new(video: Arc<VideoFile>, fit: Fit) -> Self {
        Self {
            video,
            fit,
            running: None,
            shown: 0,
            last: None,
            ended: false,
        }
    }
}

impl Source for Player {
    fn render(&mut self, frame: &mut Frame) {
        let size = (frame.width(), frame.height());
        if self.running.as_ref().is_none_or(|r| r.size != size) && !self.ended {
            self.running = None;
            self.shown = 0;
            match Running::start(&self.video, self.fit, size) {
                Ok(running) => self.running = Some(running),
                Err(err) => {
                    tracing::warn!("cannot start ffmpeg: {err}");
                    self.ended = true;
                }
            }
        }
        if let Some(running) = &self.running {
            let mut slot = running.wait_for(self.shown);
            if slot.count > self.shown
                && let Some(pixels) = slot.pixels.take()
            {
                self.shown = slot.count;
                match Frame::from_rgb(size.0, size.1, pixels) {
                    Ok(new) => self.last = Some(new),
                    Err(err) => tracing::warn!("bad frame from ffmpeg: {err}"),
                }
            }
            if let Some(reason) = &slot.ended {
                tracing::warn!(video = %self.video.path().display(), "video stopped: {reason}");
                self.ended = true;
            }
        }
        if let Some(last) = &self.last {
            frame.clone_from(last);
        }
    }

    fn next_change(&self) -> Option<Duration> {
        // `render` waits for ffmpeg's next frame.
        (!self.ended).then_some(Duration::ZERO)
    }
}

/// An ffmpeg process decoding the video for one panel size.
struct Running {
    child: Child,
    shared: Arc<Shared>,
    size: (u32, u32),
}

#[derive(Default)]
struct Shared {
    slot: Mutex<Slot>,
    changed: Condvar,
}

/// The newest frame from ffmpeg.
#[derive(Default)]
struct Slot {
    pixels: Option<Vec<u8>>,
    /// Frames received so far.
    count: u64,
    /// Why ffmpeg stopped, once it has.
    ended: Option<String>,
}

impl Running {
    fn start(video: &VideoFile, fit: Fit, (width, height): (u32, u32)) -> std::io::Result<Self> {
        let mut child = Command::new(&video.ffmpeg)
            .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
            .args(["-re", "-stream_loop", "-1", "-i"])
            .arg(&video.path)
            .args(["-map", "0:v:0", "-vf", &filter(fit, width, height)])
            .args(["-fpsmax", &MAX_FPS.to_string()])
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn_quietly()?;
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let shared = Arc::new(Shared::default());
        let errors = std::thread::Builder::new()
            .name("ssp-video-log".into())
            .spawn(move || {
                // Keeps the end of what ffmpeg reports, for the log.
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text);
                text
            })?;
        let to_reader = shared.clone();
        let frame_bytes = width as usize * height as usize * 3;
        std::thread::Builder::new()
            .name("ssp-video".into())
            .spawn(move || {
                let shared = to_reader;
                let mut buffer = vec![0; frame_bytes];
                let reason = loop {
                    if let Err(err) = stdout.read_exact(&mut buffer) {
                        drop(stdout);
                        let errors = errors.join().unwrap_or_default();
                        break match errors.lines().rfind(|l| !l.trim().is_empty()) {
                            Some(line) => line.trim().to_owned(),
                            None if err.kind() == ErrorKind::UnexpectedEof => "ffmpeg ended".into(),
                            None => err.to_string(),
                        };
                    }
                    let mut slot = lock(&shared.slot);
                    let spare = slot.pixels.replace(std::mem::take(&mut buffer));
                    slot.count += 1;
                    drop(slot);
                    shared.changed.notify_all();
                    buffer = spare.unwrap_or_else(|| vec![0; frame_bytes]);
                };
                lock(&shared.slot).ended = Some(reason);
                shared.changed.notify_all();
            })?;
        Ok(Self {
            child,
            shared,
            size: (width, height),
        })
    }

    /// The slot, once it has a frame newer than `shown` or ffmpeg ended, or after
    /// [`FRAME_WAIT`].
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The ffmpeg filter that fits the video to `width` x `height` like [`Fit`] fits images.
fn filter(fit: Fit, width: u32, height: u32) -> String {
    match fit {
        Fit::Contain => format!(
            "scale={width}:{height}:force_original_aspect_ratio=decrease,\
             pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1"
        ),
        Fit::Cover => format!(
            "scale={width}:{height}:force_original_aspect_ratio=increase,\
             crop={width}:{height},setsar=1"
        ),
        Fit::Stretch => format!("scale={width}:{height},setsar=1"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_video_files() {
        let mp4 = b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2avc1mp41";
        let mov = b"\0\0\0\x14ftypqt  \0\0\0\0qt  ";
        let webm = [0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81];
        let avi = b"RIFF\x10\0\0\0AVI LIST";
        for video in [&mp4[..], &mov[..], &webm[..], &avi[..]] {
            assert!(is_video(video), "{video:?}");
        }
        let avif = b"\0\0\0\x1cftypavif\0\0\0\0avifmif1";
        let png = b"\x89PNG\r\n\x1a\n";
        let webp = b"RIFF\x10\0\0\0WEBPVP8 ";
        for other in [&avif[..], &png[..], &webp[..], b"", b"short"] {
            assert!(!is_video(other), "{other:?}");
        }
    }

    #[test]
    fn builds_the_fit_filters() {
        assert!(filter(Fit::Contain, 1920, 462).contains("decrease,pad=1920:462"));
        assert!(filter(Fit::Cover, 1920, 462).contains("increase,crop=1920:462"));
        assert_eq!(filter(Fit::Stretch, 8, 4), "scale=8:4,setsar=1");
    }

    /// A 1-second test video made by ffmpeg, or `None` when ffmpeg is not installed.
    fn clip(dir: &Path) -> Option<(PathBuf, PathBuf)> {
        let ffmpeg = find_ffmpeg(None).ok()?;
        let path = dir.join("clip.mp4");
        let made = Command::new(&ffmpeg)
            .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi"])
            .args([
                "-i",
                "testsrc2=size=96x54:rate=30",
                "-t",
                "1",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&path)
            .status()
            .ok()?;
        made.success().then_some((ffmpeg, path))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ssp-video-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn plays_a_video_with_ffmpeg() {
        let dir = temp_dir("play");
        let Some((ffmpeg, path)) = clip(&dir) else {
            eprintln!("skipped: ffmpeg is not installed");
            return;
        };
        let video = Arc::new(VideoFile::open(path.clone(), ffmpeg.clone(), true).unwrap());
        let mut player = Player::new(video.clone(), Fit::Contain);
        let mut frame = Frame::blank(64, 16);
        let mut frames = Vec::new();
        for _ in 0..40 {
            player.render(&mut frame);
            frames.push(frame.image().as_raw().clone());
            assert_eq!(player.next_change(), Some(Duration::ZERO));
        }
        // The picture is drawn (the clip is letterboxed into the middle) and it moves, also
        // past the end of the 1-second clip.
        let lit = |f: &Vec<u8>| f.iter().filter(|&&v| v > 16).count();
        assert!(lit(&frames[39]) > 300, "the frame is black");
        assert!(frames.windows(2).filter(|w| w[0] != w[1]).count() > 20);

        drop(player);
        drop(video);
        assert!(!path.exists(), "the temporary file is deleted");

        // Not a video.
        let text = dir.join("text.mp4");
        std::fs::write(&text, b"not a video").unwrap();
        let err = VideoFile::open(text, ffmpeg, false).unwrap_err();
        assert!(err.starts_with("ffmpeg cannot play this video"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn empties_the_upload_directory() {
        let dir = temp_dir("uploads");
        std::fs::write(dir.join("video-7"), b"left over").unwrap();
        let videos = Videos::new(None, dir.clone()).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert_ne!(videos.file(), videos.file());
        let _ = std::fs::remove_dir_all(dir);
    }
}
