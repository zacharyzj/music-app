use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ABI_VERSION: u32 = 1;
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
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
    pub subtitle: String,
    pub cover: String,
    pub duration_ms: u64,
    pub unavailable: bool,
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
    pub network: Vec<String>,
    pub media: Vec<String>,
    #[serde(default)]
    pub navigation: Vec<Navigation>,
    #[serde(default)]
    pub settings: Vec<Setting>,
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Queue {
    pub items: Vec<Item>,
    pub index: Option<usize>,
    pub repeat: u8,
    pub shuffle: bool,
}
impl Queue {
    pub fn advance(&mut self, natural: bool, random: u64) -> Option<Item> {
        if self.items.is_empty() {
            self.index = None;
            return None;
        }
        let old = self.index;
        let next = match old {
            None => 0,
            Some(i) if natural && self.repeat == 2 => i,
            Some(i) if self.shuffle && self.items.len() > 1 => {
                (i + 1 + random as usize % (self.items.len() - 1)) % self.items.len()
            }
            Some(i) if i + 1 < self.items.len() => i + 1,
            Some(_) if self.repeat == 1 => 0,
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
        q.repeat = 1;
        assert!(q.advance(true, 0).is_some());
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
