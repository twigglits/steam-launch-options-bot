//! Rules engine: SystemProfile + Game -> launch options string.
//!
//! The baseline is derived from the local machine (GPU vendor, session type,
//! installed tools, Proton vs native) rather than copied from community sites:
//! ProtonDB-style recommendations are submitted from *different* hardware, so
//! they belong in the per-appid `overrides` config, applied by the user's own
//! judgement. Built-in rules stay conservative - a wrong option can break a
//! game, and an option that helps elsewhere can hurt here.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::steam::{Game, Runtime};
use crate::sysinfo::SystemProfile;

/// Written into every generated config file. This is what a user reads when
/// they open it, so it is the documentation as much as the code is.
const DOC: &str = concat!(
    "Edit and save; next run picks it up. gpu_vendor: force nvidia/amd/intel ",
    "when autodetection fails ('' = autodetect); set it with `steamtrain setup`. ",
    "enable_* toggle built-in rules. ",
    "overrides: map of appid -> launch options used verbatim; the string ",
    "'{auto}' inside an override expands to the generated baseline. ",
    "exclude: list of appids this tool must never touch. ",
    "Find per-game tips on protondb.com, but remember they come from other ",
    "people's hardware - put the ones you trust in overrides."
);

pub fn default_config_path() -> PathBuf {
    crate::home().join(".config/steamtrain/config.json")
}

/// Key order here is the key order of a freshly written config.json.
pub fn default_config() -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("_doc".to_string(), Value::from(DOC));
    map.insert("gpu_vendor".to_string(), Value::from(""));
    map.insert("enable_gamemode".to_string(), Value::from(true));
    map.insert("enable_mangohud".to_string(), Value::from(false));
    map.insert("enable_nvapi".to_string(), Value::from(true));
    map.insert(
        "enable_shader_cache_skip_cleanup".to_string(),
        Value::from(true),
    );
    map.insert("enable_mesa_glthread".to_string(), Value::from(true));
    map.insert("enable_proton_wayland".to_string(), Value::from(false));
    map.insert("advisor_command".to_string(), Value::from("claude -p"));
    map.insert("overrides".to_string(), Value::Object(Map::new()));
    map.insert("exclude".to_string(), Value::Array(Vec::new()));
    map
}

/// Config file exists but cannot be used.
#[derive(Debug)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ConfigError {}

/// Python truthiness, which is what the `enable_*` checks used. A user who
/// writes `"enable_gamemode": 1` gets the behaviour they expected.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
        Value::Null => false,
    }
}

/// Python's `str()` for the JSON types that can appear in `exclude`.
fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

/// The merged configuration: defaults, with the file's keys laid over them.
///
/// The whole map is kept rather than a struct of known fields, because
/// `save_override` and `save_gpu_vendor` write the file back and must not drop
/// a key some future version - or the user - put there.
#[derive(Debug, Clone)]
pub struct Config {
    data: Map<String, Value>,
}

impl Config {
    pub fn from_map(data: Map<String, Value>) -> Self {
        Config { data }
    }

    /// Defaults with `value`'s keys laid over them. The shape `load_config`
    /// produces, and the convenient one for tests that care about one key.
    pub fn from_value(value: Value) -> Option<Self> {
        let object = value.as_object()?;
        let mut data = default_config();
        for (key, item) in object {
            data.insert(key.clone(), item.clone());
        }
        Some(Config { data })
    }

    pub fn defaults() -> Self {
        Config {
            data: default_config(),
        }
    }

    pub fn as_map(&self) -> &Map<String, Value> {
        &self.data
    }

    /// The raw `gpu_vendor` value, before normalisation. A falsy value reads as
    /// empty, which is what `str(raw or "")` did; the CLI trims and lowercases
    /// it, and warns when what is left is not a vendor it knows.
    pub fn gpu_vendor_raw(&self) -> String {
        match self.data.get("gpu_vendor") {
            Some(value) if truthy(value) => value_to_string(value),
            _ => String::new(),
        }
    }

    pub fn flag(&self, key: &str) -> bool {
        self.data.get(key).is_some_and(truthy)
    }

    pub fn advisor_command(&self) -> String {
        self.data
            .get("advisor_command")
            .and_then(Value::as_str)
            .unwrap_or("claude -p")
            .to_string()
    }

    pub fn override_for(&self, appid: &str) -> Option<&str> {
        self.data
            .get("overrides")?
            .as_object()?
            .get(appid)?
            .as_str()
    }

    /// Excluded appids, stringified: a config written with JSON numbers has to
    /// match appids, which are strings everywhere else.
    pub fn exclude(&self) -> Vec<String> {
        match self.data.get("exclude") {
            Some(Value::Array(items)) => items.iter().map(value_to_string).collect(),
            _ => Vec::new(),
        }
    }
}

fn type_name(value: &Value) -> &'static str {
    // Python's type(...).__name__, because the message was written for it.
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) => {
            if number.is_f64() {
                "float"
            } else {
                "int"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn read_json(path: &Path) -> Result<Map<String, Value>, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|err| {
        ConfigError(format!(
            "cannot read {}: {err}. Fix the file, or delete it to regenerate \
             defaults.",
            path.display()
        ))
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|err| {
        ConfigError(format!(
            "invalid JSON in {}: {err}. Fix the file, or delete it to \
             regenerate defaults.",
            path.display()
        ))
    })?;
    match value {
        Value::Object(map) => Ok(map),
        other => Err(ConfigError(format!(
            "invalid config in {}: top level must be a JSON object, not {}. \
             Fix the file, or delete it to regenerate defaults.",
            path.display(),
            type_name(&other)
        ))),
    }
}

fn write_json(path: &Path, data: &Map<String, Value>) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| ConfigError(format!("cannot create {}: {err}", parent.display())))?;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(data.clone()))
        .map_err(|err| ConfigError(format!("cannot serialize config: {err}")))?;
    text.push('\n');
    std::fs::write(path, text)
        .map_err(|err| ConfigError(format!("cannot write {}: {err}", path.display())))
}

/// Load config, creating a documented default file on first run.
pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    if !path.is_file() {
        write_json(path, &default_config())?;
    }
    let mut data = default_config();
    for (key, value) in read_json(path)? {
        data.insert(key, value);
    }
    Ok(Config { data })
}

fn build_baseline(game: &Game, profile: &SystemProfile, config: &Config) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let proton = game.runtime == Runtime::Proton;
    let nvidia = profile.gpu_vendor == "nvidia";
    let mesa = profile.gpu_vendor == "amd" || profile.gpu_vendor == "intel";

    if proton && nvidia && config.flag("enable_nvapi") {
        parts.push("PROTON_ENABLE_NVAPI=1");
    }
    if proton && profile.session == "wayland" && config.flag("enable_proton_wayland") {
        parts.push("PROTON_ENABLE_WAYLAND=1");
    }
    if nvidia && config.flag("enable_shader_cache_skip_cleanup") {
        parts.push("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP=1");
    }
    if !proton && mesa && config.flag("enable_mesa_glthread") {
        parts.push("mesa_glthread=true");
    }

    if profile.has_gamemode && config.flag("enable_gamemode") {
        parts.push("gamemoderun");
    }
    if profile.has_mangohud && config.flag("enable_mangohud") {
        parts.push("mangohud");
    }

    parts.push("%command%");
    parts.join(" ")
}

/// The generated hardware baseline for a game - what `{auto}` expands to.
pub fn baseline(game: &Game, profile: &SystemProfile, config: &Config) -> String {
    build_baseline(game, profile, config)
}

/// Launch options for one game, or None if the game is excluded.
pub fn build_options(game: &Game, profile: &SystemProfile, config: &Config) -> Option<String> {
    if config.exclude().contains(&game.appid) {
        return None;
    }
    let base = build_baseline(game, profile, config);
    match config.override_for(&game.appid) {
        Some(value) => Some(value.replace("{auto}", &base)),
        None => Some(base),
    }
}

/// Merge overrides[appid]=value into the config file, preserving everything else.
pub fn save_override(path: &Path, appid: &str, value: &str) -> Result<(), ConfigError> {
    load_config(path)?; // create the documented default file if it is not there yet
    let mut data = read_json(path)?;
    let entry = data
        .entry("overrides".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry
        .as_object_mut()
        .expect("coerced to an object above")
        .insert(appid.to_string(), Value::from(value));
    write_json(path, &data)
}

/// Merge gpu_vendor=vendor into the config file, preserving everything else.
pub fn save_gpu_vendor(path: &Path, vendor: &str) -> Result<(), ConfigError> {
    load_config(path)?;
    let mut data = read_json(path)?;
    data.insert("gpu_vendor".to_string(), Value::from(vendor));
    write_json(path, &data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_config_keys_are_in_a_fixed_order() {
        let config = default_config();
        let keys: Vec<&str> = config.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "_doc",
                "gpu_vendor",
                "enable_gamemode",
                "enable_mangohud",
                "enable_nvapi",
                "enable_shader_cache_skip_cleanup",
                "enable_mesa_glthread",
                "enable_proton_wayland",
                "advisor_command",
                "overrides",
                "exclude",
            ]
        );
    }

    #[test]
    fn flags_follow_python_truthiness() {
        let config = Config::from_value(serde_json::json!({
            "enable_gamemode": 1,
            "enable_mangohud": 0,
            "enable_nvapi": "",
        }))
        .unwrap();
        assert!(config.flag("enable_gamemode"));
        assert!(!config.flag("enable_mangohud"));
        assert!(!config.flag("enable_nvapi"));
        assert!(!config.flag("no_such_key"));
    }

    #[test]
    fn a_falsy_gpu_vendor_reads_as_unset() {
        let config = Config::from_value(serde_json::json!({ "gpu_vendor": null })).unwrap();
        assert_eq!(config.gpu_vendor_raw(), "");
    }
}
