use music_core::{Item, Manifest, Page, ResourceKey};
use std::collections::HashSet;

pub(super) struct Source {
    pub id: String,
    pub name: String,
    pub anonymous: bool,
    pub downloads: bool,
    pub likes: bool,
    page_size: u64,
    next: Option<u64>,
    pending: Option<u64>,
    pub error: Option<String>,
}

impl Source {
    fn eligible(&self, guest_only: bool) -> bool {
        !guest_only || (self.anonymous && self.downloads)
    }
}

#[derive(Default)]
pub(super) struct Search {
    pub sources: Vec<Source>,
    pub items: Vec<Item>,
    seen: HashSet<(ResourceKey, String)>,
    pub selected: Option<usize>,
}

impl Search {
    pub fn new(plugins: &[(Manifest, bool)], source: &str, kind: &str) -> Option<Self> {
        let mut sources: Vec<_> = plugins
            .iter()
            .filter(|(m, enabled)| {
                *enabled
                    && (source == "local" || source == m.id)
                    && (kind == "track" || !m.capabilities.iter().any(|c| c == "tracksOnly"))
            })
            .map(|(m, _)| Source {
                id: m.id.clone(),
                name: m.name.clone(),
                anonymous: m.capabilities.iter().any(|c| c == "anonymous"),
                downloads: m.capabilities.iter().any(|c| c == "download") && !m.media.is_empty(),
                likes: m.supports_likes(),
                page_size: m.search_page_size.map_or(50, |n| u64::from(n.get())),
                next: Some(0),
                pending: None,
                error: None,
            })
            .collect();
        sources.sort_by(|a, b| a.id.cmp(&b.id));
        (!sources.is_empty()).then(|| Self {
            sources,
            items: Vec::new(),
            seen: HashSet::new(),
            selected: None,
        })
    }

    pub fn replacements(plugins: &[(Manifest, bool)], original: &str, include_local: bool) -> Self {
        let mut search = Self::new(plugins, "local", "track").unwrap_or_default();
        search.sources.retain(|s| s.id != original);
        if include_local && original != "local" {
            search.sources.insert(
                0,
                Source {
                    id: "local".into(),
                    name: "本地音乐".into(),
                    anonymous: false,
                    downloads: false,
                    likes: true,
                    page_size: 100,
                    next: Some(0),
                    pending: None,
                    error: None,
                },
            );
        }
        search
    }

    pub fn requests(&mut self, guest_only: bool) -> Vec<(String, u64)> {
        self.take_requests(guest_only, false)
    }

    pub fn first_requests(&mut self) -> Vec<(String, u64)> {
        self.take_requests(false, true)
    }

    fn take_requests(&mut self, guest_only: bool, first_only: bool) -> Vec<(String, u64)> {
        let mut jobs = Vec::new();
        for (i, source) in self.sources.iter_mut().enumerate() {
            if self.selected.is_some_and(|v| v != i)
                || !source.eligible(guest_only)
                || source.pending.is_some()
                || (first_only && (source.next != Some(0) || source.error.is_some()))
            {
                continue;
            }
            if let Some(offset) = source.next.take() {
                source.pending = Some(offset);
                source.error = None;
                jobs.push((source.id.clone(), offset));
            }
        }
        jobs
    }

    pub fn accept(&mut self, id: &str, offset: u64, result: Result<Page, String>) {
        let Some(source) = self
            .sources
            .iter_mut()
            .find(|s| s.id == id && s.pending == Some(offset))
        else {
            return;
        };
        source.pending = None;
        match result {
            Ok(page) => {
                source.next = page.more.then(|| offset.saturating_add(source.page_size));
                for item in page.items {
                    if self.seen.insert((item.key.clone(), item.kind.clone())) {
                        self.items.push(item);
                    }
                }
            }
            Err(error) => {
                source.error = Some(error);
                source.next = Some(offset);
            }
        }
    }

    fn scope(&self, guest_only: bool) -> impl Iterator<Item = &Source> {
        self.sources.iter().enumerate().filter_map(move |(i, s)| {
            (self.selected.is_none_or(|v| v == i) && s.eligible(guest_only)).then_some(s)
        })
    }

    pub fn pending(&self, guest_only: bool) -> usize {
        self.scope(guest_only)
            .filter(|s| s.pending.is_some())
            .count()
    }
    pub fn more(&self, guest_only: bool) -> bool {
        self.scope(guest_only).any(|s| s.next.is_some())
    }
    pub fn next_label(&self, guest_only: bool) -> &'static str {
        if !self
            .scope(guest_only)
            .any(|s| s.next.is_some() && s.error.is_some())
        {
            "加载更多"
        } else if self
            .scope(guest_only)
            .any(|s| s.next.is_some() && s.error.is_none())
        {
            "加载更多 / 重试"
        } else {
            "重试失败源"
        }
    }

    pub fn errors(&self, guest_only: bool) -> Vec<String> {
        self.scope(guest_only)
            .filter_map(|s| s.error.as_ref().map(|e| format!("{}：{e}", s.name)))
            .collect()
    }

    pub fn visible(&self, guest_only: bool) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(move |item| {
            self.scope(guest_only).any(|s| {
                s.id == item.key.source
                    && (!guest_only || (item.kind == "track" && item.downloadable == Some(true)))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plugin(id: &str, caps: &[&str], enabled: bool) -> (Manifest, bool) {
        (
            serde_json::from_value(
                json!({"id":id,"name":id,"version":"1","abi":2,"capabilities":caps,
            "network":[],"media":["audio.example"]}),
            )
            .unwrap(),
            enabled,
        )
    }
    fn track(source: &str, id: &str, allowed: Option<bool>) -> Item {
        Item {
            key: ResourceKey {
                source: source.into(),
                id: id.into(),
            },
            kind: "track".into(),
            downloadable: allowed,
            ..Item::default()
        }
    }

    #[test]
    fn bilibili_uses_its_own_page_size_and_stays_out_of_guest_downloads() {
        let mut bili = plugin("bilibili", &["tracksOnly", "readOnly"], true);
        bili.0.search_page_size = std::num::NonZeroU16::new(20);
        let mut q = Search::new(&[bili.clone()], "local", "track").unwrap();
        assert!(q.requests(true).is_empty());
        assert_eq!(q.requests(false), [("bilibili".into(), 0)]);
        q.accept(
            "bilibili",
            0,
            Ok(Page {
                more: true,
                ..Page::default()
            }),
        );
        assert_eq!(q.requests(false), [("bilibili".into(), 20)]);
        assert!(Search::new(&[bili.clone()], "local", "album").is_none());
        bili.1 = false;
        assert!(Search::new(&[bili], "local", "track").is_none());
    }

    #[test]
    fn search_keeps_independent_pages_permissions_and_versions() {
        let plugins = [
            plugin("public", &["download", "anonymous", "tracksOnly"], true),
            plugin("account", &["download"], true),
            plugin("lookup", &[], true),
            plugin("off", &["download"], false),
        ];
        let mut q = Search::new(&plugins, "local", "track").unwrap();
        assert_eq!(
            q.requests(false),
            [
                ("account".into(), 0),
                ("lookup".into(), 0),
                ("public".into(), 0)
            ]
        );
        assert!(q.requests(false).is_empty());
        q.accept(
            "public",
            0,
            Ok(Page {
                items: vec![
                    track("public", "same", Some(true)),
                    track("public", "unset", None),
                    track("public", "closed", Some(false)),
                ],
                more: true,
                ..Page::default()
            }),
        );
        assert_eq!(q.pending(false), 2);
        assert_eq!(q.visible(true).count(), 1);
        q.accept(
            "account",
            0,
            Ok(Page {
                items: vec![track("account", "same", Some(true))],
                ..Page::default()
            }),
        );
        q.accept("lookup", 0, Err("offline".into()));
        assert_eq!(q.visible(false).count(), 4);
        assert_eq!(q.visible(true).next().unwrap().key.source, "public");
        assert_eq!(q.errors(false), ["lookup：offline"]);
        q.selected = Some(1); // Search-only source, waiting for a retry.
        assert!(q.more(false));
        assert!(!q.more(true));
        assert!(q.requests(true).is_empty());
        q.selected = None;
        assert_eq!(
            q.requests(false),
            [("lookup".into(), 0), ("public".into(), 50)]
        );
        q.accept("public", 0, Ok(Page::default()));
        assert_eq!(q.pending(false), 2);
        q.accept(
            "public",
            50,
            Ok(Page {
                items: vec![
                    track("public", "same", Some(true)),
                    track("public", "next", Some(true)),
                ],
                ..Page::default()
            }),
        );
        q.selected = Some(2);
        assert_eq!(q.visible(true).count(), 2);
        assert!(!q.more(false));
        assert_eq!(
            Search::new(&plugins, "local", "album")
                .unwrap()
                .sources
                .len(),
            2
        );
    }
}
