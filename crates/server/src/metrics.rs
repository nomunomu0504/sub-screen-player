//! Figures sent from outside the daemon (`PUT /api/v1/metrics/{id}`), shown on the dashboard by
//! `metric:<id>` panels. Kept in memory only.
//!
//! Built-in panels that need figures gathered in the background (such as `claude-code`) use the
//! same store: their collector is registered with [`Metrics::provide`] and started the first time
//! a dashboard shows the panel.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::sources::stats::HISTORY;

/// How long a value counts as current unless the update says otherwise.
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);
/// Longest `ttl` accepted, in seconds (a week).
pub const MAX_TTL_SECS: u64 = 7 * 24 * 3600;
/// Most metrics kept at once.
pub const MAX_METRICS: usize = 64;

/// Longest label, unit, detail and text, in characters.
const MAX_LABEL: usize = 40;
const MAX_UNIT: usize = 16;
const MAX_DETAIL: usize = 120;
const MAX_TEXT: usize = 40;
/// Longest `series` accepted; only the last [`HISTORY`] values are kept.
const MAX_SERIES: usize = 1000;

/// New figures for a metric (`PUT /api/v1/metrics/{id}`). Fields left out keep their previous
/// value; at least one of `value`, `text` and `series` is needed to create a metric.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricUpdate {
    /// Name shown above the value. Defaults to the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A number; it is also added to the graph of recent values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// A short text shown instead of a number (no graph).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Shown small after the value, e.g. `"%"` or `"failed"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// The line under the value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Top of the graph. Defaults to the largest recent value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Seconds until the value counts as stale (default 300).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u64>,
    /// Replaces the graph's recent values (oldest first); the last one becomes the value unless
    /// `value` is given too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<Vec<f64>>,
}

/// The current state of one metric.
#[derive(Debug, Clone)]
pub struct Metric {
    /// Name shown above the value; `None` shows the id.
    pub label: Option<String>,
    /// The number shown, unless `text` is set.
    pub value: Option<f64>,
    /// Text shown instead of a number.
    pub text: Option<String>,
    /// Shown small after the value.
    pub unit: String,
    /// The line under the value.
    pub detail: String,
    /// Top of the graph.
    pub max: Option<f64>,
    /// How long the value counts as current.
    pub ttl: Duration,
    /// When the value last changed (monotonic, for staleness).
    pub updated: Instant,
    /// When the value last changed (wall clock, for the API).
    pub updated_at: Timestamp,
    /// Recent values, oldest first, at most [`HISTORY`].
    pub history: VecDeque<f32>,
}

impl Metric {
    fn new() -> Self {
        Self {
            label: None,
            value: None,
            text: None,
            unit: String::new(),
            detail: String::new(),
            max: None,
            ttl: DEFAULT_TTL,
            updated: Instant::now(),
            updated_at: Timestamp::now(),
            history: VecDeque::new(),
        }
    }

    /// Time since the value last changed.
    pub fn age(&self) -> Duration {
        self.updated.elapsed()
    }

    /// Whether the value is older than its `ttl`.
    pub fn is_stale(&self) -> bool {
        self.age() > self.ttl
    }

    fn apply(&mut self, update: MetricUpdate) {
        if let Some(label) = update.label {
            self.label = Some(label).filter(|l| !l.is_empty());
        }
        if let Some(unit) = update.unit {
            self.unit = unit;
        }
        if let Some(detail) = update.detail {
            self.detail = detail;
        }
        if let Some(max) = update.max {
            self.max = Some(max);
        }
        if let Some(ttl) = update.ttl {
            self.ttl = Duration::from_secs(ttl);
        }
        let changed = update.value.is_some() || update.text.is_some() || update.series.is_some();
        if let Some(series) = update.series {
            let skip = series.len().saturating_sub(HISTORY);
            self.history = series.iter().skip(skip).map(|&v| v as f32).collect();
            self.value = update.value.or(series.last().copied());
            self.text = None;
        } else if let Some(value) = update.value {
            if self.history.len() == HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(value as f32);
            self.value = Some(value);
            self.text = None;
        }
        if let Some(text) = update.text {
            self.text = Some(text);
            self.value = None;
            self.history.clear();
        }
        if changed {
            self.updated = Instant::now();
            self.updated_at = Timestamp::now();
        }
    }
}

/// Starts the collector of a built-in metric.
type Start = Box<dyn FnOnce() + Send>;

/// All metrics, shared by the API and the dashboards.
#[derive(Clone, Default)]
pub struct Metrics {
    values: Arc<Mutex<HashMap<String, Metric>>>,
    /// Collectors not started yet, by metric id.
    providers: Arc<Mutex<HashMap<String, Start>>>,
}

impl std::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metrics")
            .field("values", &*self.lock())
            .finish_non_exhaustive()
    }
}

impl Metrics {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Metric>> {
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Registers a metric the daemon gathers itself: `start` runs the first time
    /// [`Metrics::activate`] is called for `id`, typically to spawn a thread that keeps the metric
    /// up to date with [`Metrics::set`].
    pub fn provide(&self, id: &str, start: impl FnOnce() + Send + 'static) {
        self.providers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.to_owned(), Box::new(start));
    }

    /// Starts the collector of metric `id`, if it has one and it has not started yet.
    pub fn activate(&self, id: &str) {
        let start = self
            .providers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(id);
        if let Some(start) = start {
            start();
        }
    }

    /// Creates or updates metric `id`.
    pub fn set(&self, id: &str, update: MetricUpdate) -> Result<(), String> {
        validate_id(id)?;
        validate(&update)?;
        let mut metrics = self.lock();
        let creates = !metrics.contains_key(id);
        if creates {
            if update.value.is_none() && update.text.is_none() && update.series.is_none() {
                return Err(format!(
                    "metric {id:?} does not exist yet: send a value, text or series"
                ));
            }
            if metrics.len() >= MAX_METRICS {
                return Err(format!(
                    "at most {MAX_METRICS} metrics; remove one first (DELETE /api/v1/metrics/{{id}})"
                ));
            }
        }
        metrics
            .entry(id.to_owned())
            .or_insert_with(Metric::new)
            .apply(update);
        Ok(())
    }

    /// A copy of metric `id`.
    pub fn get(&self, id: &str) -> Option<Metric> {
        self.lock().get(id).cloned()
    }

    /// Copies of all metrics, sorted by id.
    pub fn list(&self) -> Vec<(String, Metric)> {
        let mut all: Vec<_> = self
            .lock()
            .iter()
            .map(|(id, metric)| (id.clone(), metric.clone()))
            .collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all
    }

    /// Removes metric `id`; `false` if there was none.
    pub fn remove(&self, id: &str) -> bool {
        self.lock().remove(id).is_some()
    }
}

/// Checks a metric id: 1 to 32 of `a-z`, `0-9` and `-`.
pub fn validate_id(id: &str) -> Result<(), String> {
    let ok = (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid metric id {id:?}: use 1-32 of a-z, 0-9 and -"
        ))
    }
}

fn validate(update: &MetricUpdate) -> Result<(), String> {
    let too_long = |name: &str, text: &Option<String>, max: usize| match text {
        Some(text) if text.chars().count() > max => {
            Err(format!("{name} is longer than {max} characters"))
        }
        _ => Ok(()),
    };
    too_long("label", &update.label, MAX_LABEL)?;
    too_long("unit", &update.unit, MAX_UNIT)?;
    too_long("detail", &update.detail, MAX_DETAIL)?;
    too_long("text", &update.text, MAX_TEXT)?;
    if update.value.is_some_and(|v| !v.is_finite()) {
        return Err("value must be a finite number".into());
    }
    if update.max.is_some_and(|m| !m.is_finite() || m <= 0.0) {
        return Err("max must be a positive number".into());
    }
    if update.ttl.is_some_and(|t| !(1..=MAX_TTL_SECS).contains(&t)) {
        return Err(format!("ttl must be 1..={MAX_TTL_SECS} seconds"));
    }
    if let Some(series) = &update.series {
        if series.len() > MAX_SERIES {
            return Err(format!("series takes at most {MAX_SERIES} values"));
        }
        if series.iter().any(|v| !v.is_finite()) {
            return Err("series values must be finite numbers".into());
        }
    }
    if update.value.is_some() && update.text.is_some() {
        return Err("send either value or text, not both".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(v: f64) -> MetricUpdate {
        MetricUpdate {
            value: Some(v),
            ..MetricUpdate::default()
        }
    }

    #[test]
    fn keeps_values_and_their_history() {
        let metrics = Metrics::default();
        metrics
            .set(
                "ci",
                MetricUpdate {
                    label: Some("CI".into()),
                    unit: Some("failed".into()),
                    ..value(3.0)
                },
            )
            .unwrap();
        metrics.set("ci", value(1.0)).unwrap();
        let ci = metrics.get("ci").unwrap();
        assert_eq!(ci.label.as_deref(), Some("CI"));
        assert_eq!(ci.unit, "failed");
        assert_eq!(ci.value, Some(1.0));
        assert_eq!(ci.history, [3.0, 1.0]);
        assert!(!ci.is_stale());
    }

    #[test]
    fn series_replaces_the_history_and_text_clears_the_number() {
        let metrics = Metrics::default();
        let series = (0..100).map(f64::from).collect();
        metrics
            .set(
                "load",
                MetricUpdate {
                    series: Some(series),
                    ..MetricUpdate::default()
                },
            )
            .unwrap();
        let load = metrics.get("load").unwrap();
        assert_eq!(load.history.len(), HISTORY);
        assert_eq!(load.value, Some(99.0));

        metrics
            .set(
                "load",
                MetricUpdate {
                    text: Some("offline".into()),
                    ..MetricUpdate::default()
                },
            )
            .unwrap();
        let load = metrics.get("load").unwrap();
        assert_eq!((load.value, load.history.len()), (None, 0));
    }

    #[test]
    fn rejects_bad_input() {
        let metrics = Metrics::default();
        assert!(metrics.set("CI", value(1.0)).is_err());
        assert!(metrics.set("", value(1.0)).is_err());
        assert!(metrics.set("ci", value(f64::NAN)).is_err());
        let label_only = MetricUpdate {
            label: Some("CI".into()),
            ..MetricUpdate::default()
        };
        assert!(metrics.set("ci", label_only.clone()).is_err());
        metrics.set("ci", value(1.0)).unwrap();
        // Changing only the label of an existing metric is fine.
        metrics.set("ci", label_only).unwrap();
        assert!(metrics.remove("ci"));
        assert!(!metrics.remove("ci"));
    }

    #[test]
    fn starts_a_provider_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let metrics = Metrics::default();
        let started = Arc::new(AtomicUsize::new(0));
        let counter = started.clone();
        metrics.provide("built-in", move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        metrics.activate("other");
        metrics.activate("built-in");
        metrics.activate("built-in");
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn has_a_limit() {
        let metrics = Metrics::default();
        for i in 0..MAX_METRICS {
            metrics.set(&format!("m{i}"), value(1.0)).unwrap();
        }
        assert!(metrics.set("one-more", value(1.0)).is_err());
        assert_eq!(metrics.list().len(), MAX_METRICS);
    }
}
