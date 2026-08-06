//! Newline-delimited JSON output envelope for the --json mode.
//!
//! One JSON object per line on stdout, never a wrapping array, so a client can
//! render a long run as it streams instead of waiting for the end. Every record
//! carries {"v": VERSION, "kind": ...}; the final record of every invocation is
//! always a `result`, which is what makes a truncated stream detectable.
//!
//! Nothing but these records may reach stdout while --json is active; warnings
//! and diagnostics go to stderr, where the systemd journal already collects them.

use std::io::Write;

use serde_json::{Map, Value};

use crate::codes::Outcome;

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Profile,
    Game,
    Change,
    Finding,
    Progress,
    Result,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Profile => "profile",
            Kind::Game => "game",
            Kind::Change => "change",
            Kind::Finding => "finding",
            Kind::Progress => "progress",
            Kind::Result => "result",
        }
    }
}

/// An ordered field map for one record.
///
/// A builder rather than `serde_json::json!({...})` because the ordering
/// guarantee lives on `Map` (via the `preserve_order` feature) and is lost the
/// moment the fields go through a `Value`.
#[derive(Debug, Default, Clone)]
pub struct Fields(Map<String, Value>);

impl Fields {
    pub fn new() -> Self {
        Fields(Map::new())
    }

    pub fn set(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.0.insert(key.to_string(), value.into());
        self
    }

    /// Fold every key of a JSON object into these fields, in its own order.
    /// Used for records that are the serialization of one struct, such as
    /// `profile`. A non-object is ignored.
    pub fn merge(mut self, value: Value) -> Self {
        if let Value::Object(object) = value {
            for (key, item) in object {
                self.0.insert(key, item);
            }
        }
        self
    }

    pub fn into_map(self) -> Map<String, Value> {
        self.0
    }
}

impl From<Map<String, Value>> for Fields {
    fn from(map: Map<String, Value>) -> Self {
        Fields(map)
    }
}

/// Writes envelope records, or nothing at all when disabled.
///
/// A disabled emitter is a no-op so callers can emit unconditionally and let
/// the text-mode branch do its own printing; this keeps the two output modes
/// from growing separate control flow.
pub struct Emitter<'a> {
    stream: &'a mut dyn Write,
    enabled: bool,
    finished: bool,
}

impl<'a> Emitter<'a> {
    pub fn new(stream: &'a mut dyn Write, enabled: bool) -> Self {
        Emitter {
            stream,
            enabled,
            finished: false,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn emit(&mut self, kind: Kind, fields: Fields) {
        if !self.enabled {
            return;
        }
        // Both of these are programming errors rather than runtime conditions,
        // and Python raised on them. A release binary degrades instead of
        // panicking part-way through writing to a user's terminal.
        debug_assert!(
            kind != Kind::Result,
            "emit a result with result(), not emit()"
        );
        debug_assert!(!self.finished, "records cannot follow the result record");
        if kind == Kind::Result || self.finished {
            return;
        }
        let mut record = Map::new();
        record.insert("v".to_string(), Value::from(VERSION));
        record.insert("kind".to_string(), Value::from(kind.as_str()));
        self.write(record, fields);
    }

    /// Emit the terminal record. Exactly one per invocation.
    pub fn result(&mut self, ok: bool, outcome: Outcome, fields: Fields) {
        if !self.enabled {
            return;
        }
        debug_assert!(!self.finished, "result already emitted");
        if self.finished {
            return;
        }
        self.finished = true;
        let mut record = Map::new();
        record.insert("v".to_string(), Value::from(VERSION));
        record.insert("kind".to_string(), Value::from(Kind::Result.as_str()));
        record.insert("ok".to_string(), Value::from(ok));
        record.insert("outcome".to_string(), Value::from(outcome.as_str()));
        self.write(record, fields);
    }

    fn write(&mut self, mut record: Map<String, Value>, fields: Fields) {
        for (key, value) in fields.into_map() {
            record.insert(key, value);
        }
        let line = serde_json::to_string(&Value::Object(record))
            .expect("record fields are always serializable");
        let _ = writeln!(self.stream, "{line}");
        // A client streaming progress must see it now, not when the process
        // exits and the buffer happens to drain.
        let _ = self.stream.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(buf: &[u8]) -> Vec<Value> {
        String::from_utf8(buf.to_vec())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn every_record_carries_version_and_kind() {
        let mut buf = Vec::new();
        {
            let mut out = Emitter::new(&mut buf, true);
            out.emit(Kind::Game, Fields::new().set("appid", "100"));
            out.result(true, Outcome::Ok, Fields::new().set("written", 0));
        }
        let records = lines(&buf);
        assert_eq!(records[0]["v"], 1);
        assert_eq!(records[0]["kind"], "game");
        assert_eq!(records[0]["appid"], "100");
        assert_eq!(records[1]["kind"], "result");
        assert_eq!(records[1]["ok"], true);
        assert_eq!(records[1]["outcome"], "ok");
        assert_eq!(records[1]["written"], 0);
    }

    #[test]
    fn a_disabled_emitter_writes_nothing() {
        let mut buf = Vec::new();
        {
            let mut out = Emitter::new(&mut buf, false);
            assert!(!out.enabled());
            out.emit(Kind::Game, Fields::new().set("appid", "100"));
            out.result(true, Outcome::Ok, Fields::new());
        }
        assert!(buf.is_empty());
    }

    #[test]
    fn one_record_per_line() {
        let mut buf = Vec::new();
        {
            let mut out = Emitter::new(&mut buf, true);
            out.emit(Kind::Progress, Fields::new().set("done", 1).set("total", 2));
            out.emit(Kind::Progress, Fields::new().set("done", 2).set("total", 2));
            out.result(true, Outcome::Ok, Fields::new());
        }
        assert_eq!(String::from_utf8(buf).unwrap().lines().count(), 3);
    }

    #[test]
    fn head_fields_come_first() {
        let mut buf = Vec::new();
        {
            let mut out = Emitter::new(&mut buf, true);
            out.result(
                false,
                Outcome::Blocked,
                Fields::new().set("message", "busy"),
            );
        }
        let text = String::from_utf8(buf).unwrap();
        assert!(
            text.starts_with(r#"{"v":1,"kind":"result","ok":false,"outcome":"blocked""#),
            "unexpected head ordering: {text}"
        );
    }

    #[test]
    fn merge_folds_an_object_in_its_own_order() {
        let mut buf = Vec::new();
        {
            let mut out = Emitter::new(&mut buf, true);
            let profile = serde_json::json!({ "distro": "Arch Linux", "session": "wayland" });
            out.emit(Kind::Profile, Fields::new().merge(profile));
            out.result(true, Outcome::Ok, Fields::new());
        }
        let text = String::from_utf8(buf).unwrap();
        assert!(
            text.starts_with(
                r#"{"v":1,"kind":"profile","distro":"Arch Linux","session":"wayland"}"#
            ),
            "unexpected profile record: {text}"
        );
    }

    #[test]
    fn nothing_follows_the_result_record() {
        // Release-mode behaviour: the stray record is dropped rather than
        // panicking part-way through a user's stream.
        let mut buf = Vec::new();
        {
            let mut out = Emitter::new(&mut buf, true);
            out.result(true, Outcome::Ok, Fields::new());
        }
        assert_eq!(lines(&buf).len(), 1);
    }
}
