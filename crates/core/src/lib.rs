use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ABI_VERSION: u32 = 1;
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ResourceKey {
    pub source: String,
    pub id: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    pub key: ResourceKey,
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub search_aliases: Vec<String>,
    pub subtitle: String,
    pub cover: String,
    pub duration_ms: u64,
    pub unavailable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloadable: Option<bool>,
    pub license: String,
    pub album: String,
    pub artist: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Page {
    pub title: String,
    pub items: Vec<Item>,
    pub more: bool,
    pub offset: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub op: String,
    #[serde(default)]
    pub args: Value,
}
impl Request {
    pub fn new(op: &str, args: Value) -> Self {
        Self {
            op: op.into(),
            args,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}
impl SourceError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }
}
impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for SourceError {}
pub type SourceResult = Result<Value, SourceError>;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub abi: u32,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_page_size: Option<std::num::NonZeroU16>,
    pub network: Vec<String>,
    pub media: Vec<String>,
    #[serde(default)]
    pub navigation: Vec<Navigation>,
    #[serde(default)]
    pub settings: Vec<Setting>,
}
impl Manifest {
    pub fn supports_likes(&self) -> bool {
        !self
            .capabilities
            .iter()
            .any(|c| matches!(c.as_str(), "anonymous" | "readOnly"))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Navigation {
    pub title: String,
    pub op: String,
    #[serde(default)]
    pub args: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    pub label: String,
    pub options: Vec<String>,
    pub default: String,
}
pub trait MusicSource {
    fn call(&mut self, request: Request) -> SourceResult;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackMode {
    Sequential,
    RepeatAll,
    RepeatOne,
    Shuffle,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Queue {
    pub items: Vec<Item>,
    pub index: Option<usize>,
    // Keep the existing queue format and remember the mode to restore after shuffle.
    #[serde(default)]
    repeat: u8,
    #[serde(default)]
    shuffle: bool,
}
impl Queue {
    pub fn playback_mode(&self) -> PlaybackMode {
        match (self.shuffle, self.repeat) {
            (true, _) => PlaybackMode::Shuffle,
            (false, 1) => PlaybackMode::RepeatAll,
            (false, 2) => PlaybackMode::RepeatOne,
            _ => PlaybackMode::Sequential,
        }
    }

    pub fn cycle_playback_mode(&mut self) {
        match self.playback_mode() {
            PlaybackMode::Sequential => self.repeat = 1,
            PlaybackMode::RepeatAll => self.repeat = 2,
            PlaybackMode::RepeatOne => self.shuffle = true,
            PlaybackMode::Shuffle => {
                self.shuffle = false;
                self.repeat = 0;
            }
        }
    }

    pub fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
    }

    pub fn advance(&mut self, natural: bool, random: u64) -> Option<Item> {
        let count = self.items.len();
        if count == 0 {
            self.index = None;
            return None;
        }
        let mode = self.playback_mode();
        let next = match self.index.filter(|&i| i < count) {
            None if mode == PlaybackMode::Shuffle => random as usize % count,
            None => 0,
            Some(i) if natural && mode == PlaybackMode::RepeatOne => i,
            Some(i) if mode == PlaybackMode::Shuffle && count > 1 => {
                (i + 1 + random as usize % (count - 1)) % count
            }
            Some(i) if i + 1 < count => i + 1,
            Some(_) if matches!(mode, PlaybackMode::RepeatAll | PlaybackMode::Shuffle) => 0,
            _ => return None,
        };
        self.index = Some(next);
        self.items.get(next).cloned()
    }
    pub fn remove(&mut self, n: usize) {
        if n >= self.items.len() {
            return;
        }
        self.items.remove(n);
        self.index = match self.index {
            Some(i) if i == n => None,
            Some(i) if i > n => Some(i - 1),
            x => x,
        };
    }
}
pub fn parse_lrc(input: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut offset = 0i64;
    for line in input.lines() {
        if let Some(v) = line
            .strip_prefix("[offset:")
            .and_then(|v| v.strip_suffix(']'))
        {
            offset = v.parse().unwrap_or(0);
        }
    }
    for line in input.lines() {
        let mut tail = line;
        let mut stamps = Vec::new();
        while let Some(rest) = tail.strip_prefix('[') {
            let Some((tag, next)) = rest.split_once(']') else {
                break;
            };
            if let Some((m, s)) = tag.split_once(':')
                && let (Ok(m), Ok(s)) = (m.parse::<u64>(), s.parse::<f64>())
                && s.is_finite()
                && s >= 0.0
            {
                stamps.push(m.saturating_mul(60000).saturating_add((s * 1000.0) as u64));
            }
            tail = next;
        }
        for t in stamps {
            out.push((t.saturating_add_signed(offset), tail.to_string()));
        }
    }
    out.sort_by_key(|x| x.0);
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_end_and_remove() {
        let mut q = Queue {
            items: vec![Item::default(); 2],
            index: Some(0),
            ..Default::default()
        };
        assert!(q.advance(true, 0).is_some());
        assert!(q.advance(true, 0).is_none());
        q.remove(0);
        assert_eq!(q.index, Some(0));
        q.cycle_playback_mode();
        assert!(q.advance(true, 0).is_some());
    }

    #[test]
    fn playback_mode_drives_manual_and_automatic_advance() {
        use PlaybackMode::*;
        let mut q = Queue {
            items: vec![Item::default(); 3],
            index: Some(2),
            ..Default::default()
        };
        assert_eq!(q.playback_mode(), Sequential);
        assert!(q.advance(true, 0).is_none());
        q.cycle_playback_mode();
        assert_eq!(q.playback_mode(), RepeatAll);
        assert!(q.advance(true, 0).is_some());
        assert_eq!(q.index, Some(0));
        q.cycle_playback_mode();
        assert_eq!(q.playback_mode(), RepeatOne);
        assert!(q.advance(true, 1).is_some());
        assert_eq!(q.index, Some(0));
        assert!(q.advance(false, 1).is_some());
        assert_eq!(q.index, Some(1));
        q.cycle_playback_mode();
        assert_eq!(q.playback_mode(), Shuffle);
        assert!(q.advance(true, 1).is_some());
        assert_eq!(q.index, Some(0));
        q.index = None;
        assert!(q.advance(false, 2).is_some());
        assert_eq!(q.index, Some(2));
        q.items.truncate(1);
        q.index = Some(0);
        assert!(q.advance(true, 0).is_some());
        q.cycle_playback_mode();
        assert_eq!(q.playback_mode(), Sequential);
        assert!(q.advance(true, 0).is_none());
    }

    #[test]
    fn legacy_queue_restores_one_mode_and_remembers_the_previous_repeat_setting() {
        let old = serde_json::json!({
            "items": [{"title":"A"}], "index":0, "repeat":2, "shuffle":true
        });
        let mut queue: Queue = serde_json::from_value(old).unwrap();
        assert_eq!(queue.playback_mode(), PlaybackMode::Shuffle);
        queue.toggle_shuffle();
        assert_eq!(queue.playback_mode(), PlaybackMode::RepeatOne);
        queue.toggle_shuffle();
        let saved = serde_json::to_value(&queue).unwrap();
        assert_eq!(saved["repeat"], 2);
        assert_eq!(saved["shuffle"], true);
        let restored: Queue = serde_json::from_value(saved).unwrap();
        assert_eq!(restored.playback_mode(), PlaybackMode::Shuffle);
        assert_eq!(restored.items[restored.index.unwrap()].title, "A");
    }
    #[test]
    fn lrc_multiple_offset() {
        assert_eq!(
            parse_lrc("[offset:-500]\n[00:01.20][00:02.30]你好"),
            vec![(700, "你好".into()), (1800, "你好".into())]
        );
        assert_eq!(parse_lrc("[18446744073709551615:59]long")[0].0, u64::MAX);
    }
}
