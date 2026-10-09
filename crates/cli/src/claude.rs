//! `ssp claude-code hooks`: the Claude Code hooks that put a notification on the display when
//! Claude Code needs you or is done, shown, or added to (and removed from) its settings.json.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

/// Marks the hooks this command adds: only `ssp notify` takes `--if-running`.
const MARK: &str = "--if-running";

/// The hooks for the `ssp` at `program`: the event and the command.
pub fn hooks(program: &Path) -> [(&'static str, String); 2] {
    let ssp = quote(&program.display().to_string());
    [
        (
            "Notification",
            format!("{ssp} notify --stdin --for 60 {MARK}"),
        ),
        (
            "Stop",
            format!("{ssp} notify \"Claude Code is done\" --stdin --color green {MARK}"),
        ),
    ]
}

/// `text` in double quotes if it has anything a shell would split or expand.
fn quote(text: &str) -> String {
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-:\\".contains(c))
    {
        text.to_owned()
    } else {
        format!("\"{}\"", text.replace('"', "\\\""))
    }
}

/// The `hooks` part of settings.json with these hooks, for showing.
pub fn snippet(program: &Path) -> String {
    let mut settings = Value::Object(Map::new());
    add(&mut settings, program);
    serde_json::to_string_pretty(&settings).expect("JSON values always serialize")
}

/// Claude Code's user settings: `$CLAUDE_CONFIG_DIR/settings.json` (the first directory if it
/// lists several), else `~/.claude/settings.json`.
pub fn settings_path() -> Result<PathBuf> {
    if let Some(dirs) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        let first = dirs
            .to_string_lossy()
            .split(',')
            .next()
            .unwrap_or_default()
            .to_owned();
        if !first.is_empty() {
            return Ok(PathBuf::from(first).join("settings.json"));
        }
    }
    let home = directories::BaseDirs::new().context("cannot find the home directory")?;
    Ok(home.home_dir().join(".claude").join("settings.json"))
}

/// Adds the hooks to `settings` unless they are there. `true` if something changed.
pub fn add(settings: &mut Value, program: &Path) -> bool {
    let Some(root) = settings.as_object_mut() else {
        return false;
    };
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for (event, command) in self::hooks(program) {
        let groups = hooks.entry(event).or_insert_with(|| json!([]));
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        if groups.iter().any(has_ours) {
            continue;
        }
        groups.push(json!({ "hooks": [{ "type": "command", "command": command }] }));
        changed = true;
    }
    changed
}

/// Removes the hooks this command added, and what is left empty. `true` if something changed.
pub fn remove(settings: &mut Value) -> bool {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = list.len();
                list.retain(|hook| !is_ours(hook));
                changed |= list.len() != before;
            }
        }
        groups.retain(|group| {
            group
                .get("hooks")
                .and_then(Value::as_array)
                .is_none_or(|list| !list.is_empty())
        });
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|g| !g.is_empty()));
    if hooks.is_empty()
        && let Some(root) = settings.as_object_mut()
    {
        root.remove("hooks");
    }
    changed
}

fn is_ours(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains(" notify ") && c.contains(MARK))
}

fn has_ours(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|list| list.iter().any(is_ours))
}

/// Reads settings.json (an empty object if there is none).
pub fn read(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Value::Object(Map::new())),
        Ok(text) => {
            let value: Value = serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON; not changed", path.display()))?;
            if !value.is_object() {
                bail!(
                    "{} does not hold a JSON object; not changed",
                    path.display()
                );
            }
            Ok(value)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Writes `settings` to `path`, keeping the old file as `settings.json.bak`.
pub fn write(path: &Path, settings: &Value) -> Result<()> {
    if path.exists() {
        let backup = path.with_extension("json.bak");
        std::fs::copy(path, &backup)
            .with_context(|| format!("cannot keep a copy as {}", backup.display()))?;
    } else if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(settings).expect("JSON values always serialize");
    std::fs::write(path, text + "\n").with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssp() -> PathBuf {
        PathBuf::from("/Users/me/.local/bin/ssp")
    }

    #[test]
    fn adds_the_hooks_once_and_keeps_the_rest() {
        let mut settings: Value = serde_json::from_str(
            r#"{"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]}, "z": 1}"#,
        )
        .unwrap();
        assert!(add(&mut settings, &ssp()));
        assert!(!add(&mut settings, &ssp()), "a second time changes nothing");
        // Keys keep their order.
        let keys: Vec<&str> = settings
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["model", "hooks", "z"]);
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "say done");
        assert_eq!(
            stop[1]["hooks"][0]["command"],
            "/Users/me/.local/bin/ssp notify \"Claude Code is done\" --stdin --color green --if-running"
        );
        assert_eq!(
            settings["hooks"]["Notification"][0]["hooks"][0]["command"],
            "/Users/me/.local/bin/ssp notify --stdin --for 60 --if-running"
        );
    }

    #[test]
    fn removes_only_its_own_hooks() {
        let mut settings: Value = serde_json::from_str(
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]}}"#,
        )
        .unwrap();
        let before = settings.clone();
        add(&mut settings, &ssp());
        assert!(remove(&mut settings));
        assert_eq!(settings, before);
        assert!(!remove(&mut settings));

        let mut only_ours = Value::Object(Map::new());
        add(&mut only_ours, &ssp());
        remove(&mut only_ours);
        assert_eq!(only_ours, json!({}));
    }

    #[test]
    fn quotes_programs_with_spaces() {
        let [(_, command), _] = hooks(Path::new("C:\\Program Files\\ssp\\ssp.exe"));
        assert!(
            command.starts_with("\"C:\\Program Files\\ssp\\ssp.exe\" notify"),
            "{command}"
        );
        let [(_, command), _] = hooks(&ssp());
        assert!(
            command.starts_with("/Users/me/.local/bin/ssp notify"),
            "{command}"
        );
    }

    #[test]
    fn reads_and_writes_settings() {
        let dir = std::env::temp_dir().join(format!("ssp-claude-{}", std::process::id()));
        let path = dir.join("settings.json");
        assert_eq!(read(&path).unwrap(), json!({}));
        let mut settings = read(&path).unwrap();
        add(&mut settings, &ssp());
        write(&path, &settings).unwrap();
        assert!(
            !path.with_extension("json.bak").exists(),
            "nothing to keep the first time"
        );
        write(&path, &settings).unwrap();
        assert!(path.with_extension("json.bak").exists());
        assert_eq!(read(&path).unwrap(), settings);
        std::fs::write(&path, "{ not json").unwrap();
        assert!(read(&path).is_err());
        std::fs::write(&path, "[1]").unwrap();
        assert!(read(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
