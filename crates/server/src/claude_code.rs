//! The `claude-code` dashboard panel: how many tokens Claude Code has used, read from the session
//! logs it keeps on this computer (`<dir>/projects/**/*.jsonl`). Nothing is sent anywhere.
//!
//! Each line of a log is a JSON record; replies from the model carry `message.usage` with token
//! counts. The same reply is often written several times (once per content block), so replies
//! are counted once per `message.id` + `requestId`. Input, output and cache creation tokens are
//! counted; cache reads (the conversation read again on every turn, billed at a fraction) are
//! not, or they would dwarf the rest. The log format is not a public contract: lines that do not
//! parse or lack fields are skipped.
//!
//! The figures go into the metric store as metric [`METRIC_ID`], which the panel shows. The
//! reader starts the first time a dashboard shows the panel and then rescans every
//! [`SCAN_INTERVAL`], reading only what was appended to each file.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde::Deserialize;

use crate::config::ClaudeCodeConfig;
use crate::metrics::{MetricUpdate, Metrics};
use crate::sources::number;

/// Id of the metric the panel shows.
pub const METRIC_ID: &str = "claude-code";
/// How often the logs are scanned.
pub const SCAN_INTERVAL: Duration = Duration::from_secs(30);
/// Length of a usage block.
const BLOCK: SignedDuration = SignedDuration::from_hours(5);
/// Usage older than this (and than the start of today) is forgotten. Enough to find where the
/// current block started.
const LOOKBACK: SignedDuration = SignedDuration::from_hours(10);
/// Minutes in the graph.
const GRAPH_MINUTES: usize = 60;

/// Registers the reader with `metrics`; it starts when a dashboard first shows the panel.
pub fn register(metrics: &Metrics, config: ClaudeCodeConfig) {
    let store = metrics.clone();
    metrics.provide(METRIC_ID, move || {
        let spawned = std::thread::Builder::new()
            .name("ssp-claude-code".into())
            .spawn(move || run(&store, &config));
        if let Err(err) = spawned {
            tracing::warn!("cannot start the Claude Code reader: {err}");
        }
    });
}

fn run(metrics: &Metrics, config: &ClaudeCodeConfig) {
    let dirs = match &config.dir {
        Some(dir) => vec![expand_home(dir)],
        None => default_dirs(),
    };
    tracing::info!(?dirs, "reading Claude Code usage");
    let mut usage = Usage::default();
    loop {
        let now = Timestamp::now();
        let projects: Vec<PathBuf> = dirs
            .iter()
            .map(|d| d.join("projects"))
            .filter(|d| d.is_dir())
            .collect();
        let update = if projects.is_empty() {
            no_logs(&dirs)
        } else {
            let tz = TimeZone::system();
            let started = std::time::Instant::now();
            usage.scan(&projects, cutoff(now, &tz));
            tracing::debug!(
                files = usage.files.len(),
                replies = usage.entries.len(),
                "scanned Claude Code logs in {:?}",
                started.elapsed()
            );
            usage.figures(now, &tz).update(&tz)
        };
        if let Err(err) = metrics.set(METRIC_ID, update) {
            tracing::warn!("cannot update the Claude Code panel: {err}");
        }
        std::thread::sleep(SCAN_INTERVAL);
    }
}

/// `$CLAUDE_CONFIG_DIR` (comma-separated), else `~/.config/claude` and `~/.claude`.
fn default_dirs() -> Vec<PathBuf> {
    if let Some(dirs) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        let dirs = dirs.to_string_lossy().into_owned();
        return dirs
            .split(',')
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(|d| expand_home(Path::new(d)))
            .collect();
    }
    let Some(home) = std::env::home_dir() else {
        return Vec::new();
    };
    vec![home.join(".config/claude"), home.join(".claude")]
}

fn expand_home(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::home_dir()) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_owned(),
    }
}

/// Usage before this does not count: the earlier of the start of today and [`LOOKBACK`] ago.
fn cutoff(now: Timestamp, tz: &TimeZone) -> Timestamp {
    let lookback = now - LOOKBACK;
    let today = now
        .to_zoned(tz.clone())
        .start_of_day()
        .map_or(lookback, |midnight| midnight.timestamp());
    today.min(lookback)
}

fn no_logs(dirs: &[PathBuf]) -> MetricUpdate {
    let detail = match dirs.first() {
        Some(dir) => format!("no logs in {}", dir.display()),
        None => "no home directory".into(),
    };
    MetricUpdate {
        label: Some("CLAUDE CODE".into()),
        text: Some("no data".into()),
        detail: Some(detail),
        ttl: Some(ttl()),
        ..MetricUpdate::default()
    }
}

/// Seconds the panel's value stays current: a few missed scans dim it.
fn ttl() -> u64 {
    SCAN_INTERVAL.as_secs() * 4
}

/// The tokens of one reply.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    at: Timestamp,
    /// Input, output and cache creation tokens.
    tokens: u64,
    /// `message.id` + `requestId`, when both are there.
    key: Option<String>,
}

/// What the reader knows: replies since the cutoff, and how far each file was read.
#[derive(Debug, Default)]
struct Usage {
    /// Bytes read per file, up to the end of its last complete line.
    files: HashMap<PathBuf, u64>,
    entries: Vec<Entry>,
    seen: HashSet<String>,
}

impl Usage {
    /// Reads what was appended to the logs under `projects` since the last scan, and forgets
    /// usage before `since`.
    fn scan(&mut self, projects: &[PathBuf], since: Timestamp) {
        let mut found = Vec::new();
        for dir in projects {
            find_logs(dir, SystemTime::from(since), 0, &mut found);
        }
        let found: HashSet<PathBuf> = found.into_iter().collect();
        // Files not written since the cutoff hold nothing that counts any more.
        self.files.retain(|path, _| found.contains(path));
        for path in found {
            if let Err(err) = self.read(&path, since) {
                tracing::debug!(path = %path.display(), "cannot read a Claude Code log: {err}");
            }
        }
        self.entries.retain(|e| e.at >= since);
        self.seen = self.entries.iter().filter_map(|e| e.key.clone()).collect();
    }

    /// Reads the complete lines appended to `path` since the last scan.
    fn read(&mut self, path: &Path, since: Timestamp) -> std::io::Result<()> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let offset = self.files.entry(path.to_owned()).or_insert(0);
        if len < *offset {
            // Rewritten: start over (seen replies are not counted twice).
            *offset = 0;
        }
        if len == *offset {
            return Ok(());
        }
        file.seek(SeekFrom::Start(*offset))?;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        let mut entries = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line)?;
            // Stop at the end, or before a line still being written.
            if read == 0 || line.last() != Some(&b'\n') {
                break;
            }
            *offset += read as u64;
            if let Some(entry) = parse(&line).filter(|e| e.at >= since) {
                entries.push(entry);
            }
        }
        for entry in entries {
            self.add(entry);
        }
        Ok(())
    }

    fn add(&mut self, entry: Entry) {
        if let Some(key) = &entry.key
            && !self.seen.insert(key.clone())
        {
            return;
        }
        self.entries.push(entry);
    }

    /// The figures at `now`.
    fn figures(&self, now: Timestamp, tz: &TimeZone) -> Figures {
        let mut entries: Vec<&Entry> = self.entries.iter().filter(|e| e.at <= now).collect();
        entries.sort_by_key(|e| e.at);

        // A block starts at the hour of the first reply after the previous block ended and
        // lasts five hours, the way Claude's usage limits are usually tracked.
        let mut block: Option<Block> = None;
        for entry in &entries {
            match &mut block {
                Some(b) if entry.at < b.end => b.tokens += entry.tokens,
                _ => {
                    block = Some(Block {
                        end: floor_hour(entry.at) + BLOCK,
                        tokens: entry.tokens,
                    });
                }
            }
        }

        let midnight = now
            .to_zoned(tz.clone())
            .start_of_day()
            .map_or(now, |m| m.timestamp());
        let today = entries
            .iter()
            .filter(|e| e.at >= midnight)
            .map(|e| e.tokens)
            .sum();

        let mut per_minute = vec![0.0; GRAPH_MINUTES];
        for entry in &entries {
            let minutes_ago = now.duration_since(entry.at).as_secs() / 60;
            if let Ok(minutes_ago) = usize::try_from(minutes_ago)
                && minutes_ago < GRAPH_MINUTES
            {
                per_minute[GRAPH_MINUTES - 1 - minutes_ago] += entry.tokens as f64;
            }
        }

        Figures {
            block: block.filter(|b| now < b.end),
            today,
            per_minute,
        }
    }
}

/// The figures shown on the panel.
#[derive(Debug, Clone, PartialEq)]
struct Figures {
    /// The current block, if there is one.
    block: Option<Block>,
    /// Tokens since midnight.
    today: u64,
    /// Tokens per minute over the last hour, oldest first.
    per_minute: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Block {
    end: Timestamp,
    tokens: u64,
}

impl Figures {
    fn update(&self, tz: &TimeZone) -> MetricUpdate {
        let today = format!("today {}", number(self.today as f64));
        let (value, detail) = match self.block {
            Some(block) => {
                let end = block.end.to_zoned(tz.clone()).strftime("%H:%M");
                (block.tokens, format!("until {end} · {today}"))
            }
            None => (0, today),
        };
        MetricUpdate {
            label: Some("CLAUDE CODE".into()),
            value: Some(value as f64),
            unit: Some("tokens".into()),
            detail: Some(detail),
            ttl: Some(ttl()),
            series: Some(self.per_minute.clone()),
            ..MetricUpdate::default()
        }
    }
}

fn floor_hour(at: Timestamp) -> Timestamp {
    let second = at.as_second();
    Timestamp::from_second(second - second.rem_euclid(3600)).unwrap_or(at)
}

/// Collects the `.jsonl` files under `dir` modified since `since` (session logs and, a level
/// deeper, those of subagents).
fn find_logs(dir: &Path, since: SystemTime, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() && depth < 3 {
            find_logs(&path, since, depth + 1, found);
        } else if kind.is_file()
            && path.extension().is_some_and(|e| e == "jsonl")
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|modified| modified >= since)
        {
            found.push(path);
        }
    }
}

#[derive(Deserialize)]
struct Line {
    timestamp: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    usage: Option<TokenUsage>,
}

#[derive(Deserialize)]
struct TokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

/// The reply in one log line, if it is one.
fn parse(line: &[u8]) -> Option<Entry> {
    // Most lines are prompts and tool results, some of them large: skip them unparsed.
    memchr::memmem::find(line, b"\"usage\"")?;
    let line: Line = serde_json::from_slice(line).ok()?;
    let message = line.message?;
    let usage = message.usage?;
    let at: Timestamp = line.timestamp?.parse().ok()?;
    let key = match (message.id, line.request_id) {
        (Some(id), Some(request)) => Some(format!("{id}:{request}")),
        _ => None,
    };
    Some(Entry {
        at,
        tokens: usage.input_tokens + usage.output_tokens + usage.cache_creation_input_tokens,
        key,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A log line like Claude Code writes for a reply (fields trimmed).
    fn reply(at: &str, id: &str, input: u64, output: u64, cache_read: u64) -> String {
        format!(
            r#"{{"parentUuid":"p","isSidechain":false,"type":"assistant","timestamp":"{at}","requestId":"req_{id}","message":{{"id":"msg_{id}","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{{"type":"text","text":"hi"}}],"usage":{{"input_tokens":{input},"cache_creation_input_tokens":100,"cache_read_input_tokens":{cache_read},"output_tokens":{output},"service_tier":"standard"}}}}}}"#
        ) + "\n"
    }

    fn user(at: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"{at}","message":{{"role":"user","content":"what is the usage of this flag?"}}}}"#
        ) + "\n"
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "ssp-claude-code-{}-{}",
                std::process::id(),
                COUNT.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(path.join("projects/-home-me-app/session/subagents")).unwrap();
            Self(path)
        }

        fn append(&self, file: &str, text: &str) {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.0.join("projects").join(file))
                .unwrap();
            f.write_all(text.as_bytes()).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn parses_replies_and_skips_other_lines() {
        let entry = parse(reply("2026-10-08T10:00:00Z", "a", 10, 20, 500).as_bytes()).unwrap();
        // Cache reads (500) do not count.
        assert_eq!(entry.tokens, 130);
        assert_eq!(entry.key.as_deref(), Some("msg_a:req_a"));
        assert!(parse(user("2026-10-08T10:00:00Z").as_bytes()).is_none());
        assert!(parse(b"{\"usage\": broken").is_none());
        assert!(parse(br#"{"type":"summary","summary":"usage"}"#).is_none());
    }

    #[test]
    fn reads_appended_lines_once_and_counts_replies_once() {
        let dir = TempDir::new();
        let projects = [dir.0.join("projects")];
        let since = ts("2026-10-08T00:00:00Z");
        let a = reply("2026-10-08T10:00:00Z", "a", 10, 20, 0);
        // The same reply written twice (one line per content block), and a prompt.
        dir.append(
            "-home-me-app/session.jsonl",
            &(a.clone() + &a + &user("2026-10-08T10:00:01Z")),
        );
        // A subagent's log, and a line still being written.
        dir.append(
            "-home-me-app/session/subagents/agent-1.jsonl",
            &reply("2026-10-08T10:01:00Z", "b", 1, 2, 0),
        );
        let partial = reply("2026-10-08T10:02:00Z", "c", 1000, 0, 0);
        let (head, tail) = partial.split_at(40);
        dir.append("-home-me-app/session.jsonl", head);

        let mut usage = Usage::default();
        usage.scan(&projects, since);
        let total = |u: &Usage| u.entries.iter().map(|e| e.tokens).sum::<u64>();
        assert_eq!(total(&usage), 130 + 103);

        dir.append("-home-me-app/session.jsonl", tail);
        usage.scan(&projects, since);
        usage.scan(&projects, since);
        assert_eq!(total(&usage), 130 + 103 + 1100);

        // Usage before the cutoff is forgotten.
        usage.scan(&projects, ts("2026-10-08T10:01:30Z"));
        assert_eq!(total(&usage), 1100);
    }

    #[test]
    fn finds_the_current_block_today_and_the_last_hour() {
        let mut usage = Usage::default();
        for (at, tokens) in [
            ("2026-10-08T01:30:00Z", 1_000), // an earlier block (01:00-06:00)
            ("2026-10-08T07:20:00Z", 200),   // the current block starts at 07:00
            ("2026-10-08T09:40:00Z", 30),
            ("2026-10-08T11:59:30Z", 4),
        ] {
            usage.add(Entry {
                at: ts(at),
                tokens,
                key: None,
            });
        }
        let figures = usage.figures(ts("2026-10-08T11:59:59Z"), &TimeZone::UTC);
        let block = figures.block.unwrap();
        assert_eq!((block.end, block.tokens), (ts("2026-10-08T12:00:00Z"), 234));
        assert_eq!(figures.today, 1_234);
        assert_eq!(figures.per_minute.len(), GRAPH_MINUTES);
        assert_eq!(figures.per_minute[GRAPH_MINUTES - 1], 4.0);
        assert_eq!(figures.per_minute.iter().sum::<f64>(), 4.0);
        let update = figures.update(&TimeZone::UTC);
        assert_eq!(update.value, Some(234.0));
        assert_eq!(update.detail.as_deref(), Some("until 12:00 · today 1234"));

        // At 12:00 the block is over.
        let later = usage.figures(ts("2026-10-08T12:00:00Z"), &TimeZone::UTC);
        assert_eq!(later.block, None);
    }

    #[test]
    fn says_when_there_is_no_block() {
        let figures = Usage::default().figures(ts("2026-10-08T12:00:00Z"), &TimeZone::UTC);
        assert_eq!(figures.block, None);
        let update = figures.update(&TimeZone::UTC);
        assert_eq!(update.value, Some(0.0));
        assert_eq!(update.detail.as_deref(), Some("today 0"));
    }

    #[test]
    fn looks_back_to_midnight_or_ten_hours() {
        let tz = TimeZone::UTC;
        assert_eq!(
            cutoff(ts("2026-10-08T22:00:00Z"), &tz),
            ts("2026-10-08T00:00:00Z")
        );
        assert_eq!(
            cutoff(ts("2026-10-08T03:00:00Z"), &tz),
            ts("2026-10-07T17:00:00Z")
        );
    }
}
