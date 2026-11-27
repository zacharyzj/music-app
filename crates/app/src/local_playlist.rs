use super::*;

#[derive(PartialEq)]
enum SourceFilter {
    All,
    Unavailable,
    Source(String),
}

#[derive(Default)]
pub(super) struct LocalPlaylist {
    pub tracks: Vec<Item>,
    filters: Vec<SourceFilter>,
}

impl LocalPlaylist {
    fn filter_at(&self, index: i32) -> &SourceFilter {
        self.filters
            .get(index as usize)
            .unwrap_or(&SourceFilter::All)
    }
}

impl UiState {
    pub(super) fn source_name<'a>(&'a self, id: &'a str) -> &'a str {
        if id == "local" {
            return "本地音乐";
        }
        self.plugins
            .iter()
            .find(|(m, _)| m.id == id)
            .map_or(id, |(m, _)| &m.name)
    }

    pub(super) fn refresh_local_playlist(&mut self, ui: &AppWindow) {
        let selected = self
            .local_playlist
            .filter_at(ui.get_playlist_filter_index());
        let mut sources: Vec<_> = self
            .local_playlist
            .tracks
            .iter()
            .map(|t| t.key.source.as_str())
            .collect();
        sources.sort_unstable();
        sources.dedup();
        let mut filters = vec![SourceFilter::All, SourceFilter::Unavailable];
        filters.extend(
            sources
                .into_iter()
                .map(|id| SourceFilter::Source(id.to_owned())),
        );
        let index = filters.iter().position(|f| f == selected).unwrap_or(0);
        ui.set_playlist_filter_sources(playlist_import::labels(filters.iter().map(|filter| {
            match filter {
                SourceFilter::All => "全部来源",
                SourceFilter::Unavailable => "仅看无源",
                SourceFilter::Source(id) => self.source_name(id),
            }
            .to_owned()
        })));
        self.local_playlist.filters = filters;
        ui.set_playlist_filter_index(index as i32);
        self.filter_local_playlist(ui);
    }

    pub(super) fn filter_local_playlist(&mut self, ui: &AppWindow) {
        let query = ui.get_playlist_query().trim().to_lowercase();
        let filter = self
            .local_playlist
            .filter_at(ui.get_playlist_filter_index());
        self.items = self
            .local_playlist
            .tracks
            .iter()
            .filter(|t| match filter {
                SourceFilter::All => true,
                SourceFilter::Unavailable => !playable(t, &self.plugins),
                SourceFilter::Source(id) => &t.key.source == id,
            })
            .filter(|t| {
                if query.is_empty() {
                    return true;
                }
                let mut text = format!("{} {} {}", t.title, t.artist, t.album).to_lowercase();
                for alias in &t.search_aliases {
                    text.push(' ');
                    text.push_str(&alias.to_lowercase());
                }
                query.split_whitespace().all(|word| text.contains(word))
            })
            .cloned()
            .collect();
        ui.set_status(
            if filter != &SourceFilter::All || !query.is_empty() {
                format!(
                    "{} / {} 首歌曲",
                    self.items.len(),
                    self.local_playlist.tracks.len()
                )
            } else {
                format!("{} 首歌曲", self.items.len())
            }
            .into(),
        );
        self.update_rows(ui);
    }
}

#[cfg(all(test, feature = "ui-smoke"))]
mod tests {
    use super::*;

    #[test]
    fn filters_the_whole_playlist_and_preserves_row_identity() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let ui = AppWindow::new()?;
        ui.set_local_playlist(true);
        let enabled = serde_json::from_value(
            json!({"id":"one","name":"第一来源","version":"1","abi":2,"capabilities":[],"network":[],"media":[]}),
        )?;
        let mut s = UiState::new(Queue::default(), Default::default(), vec![(enabled, true)]);
        let track = |source: &str, id: &str, title: &str, unavailable| Item {
            key: ResourceKey {
                source: source.into(),
                id: id.into(),
            },
            kind: "track".into(),
            title: title.into(),
            artist: "Singer".into(),
            album: "夜曲".into(),
            unavailable,
            ..Default::default()
        };
        s.local_playlist.tracks = vec![
            track("one", "7", "第一首", false),
            track("off", "7", "另一首", false),
            track("one", "8", "同曲", true),
        ];
        s.refresh_local_playlist(&ui);
        assert_eq!(
            ui.get_playlist_filter_sources().iter().collect::<Vec<_>>(),
            ["全部来源", "仅看无源", "off", "第一来源"]
        );
        let visible = || {
            ui.get_rows()
                .iter()
                .map(|row| row.resource)
                .collect::<Vec<_>>()
        };
        assert_eq!(visible(), ["one:7", "off:7", "one:8"]);
        ui.set_playlist_query("sInGeR 夜曲".into());
        ui.set_playlist_filter_index(1);
        s.filter_local_playlist(&ui);
        assert_eq!(visible(), ["off:7", "one:8"]);
        ui.set_playlist_filter_index(0);
        s.filter_local_playlist(&ui);
        assert_eq!(visible(), ["one:7", "off:7", "one:8"]);
        ui.set_playlist_filter_index(3);
        s.filter_local_playlist(&ui);
        assert_eq!(visible(), ["one:7", "one:8"]);
        ui.set_playlist_filter_index(1);
        ui.set_playlist_query("同曲".into());
        s.filter_local_playlist(&ui);
        assert_eq!(visible(), ["one:8"]);
        assert!(ui.get_rows().row_data(0).unwrap().disabled);
        let key = s.items[0].key.clone();
        s.remove_visible_track(&ui, &key);
        assert!(s.items.is_empty());
        assert_eq!(ui.get_playlist_filter_index(), 1);
        ui.set_playlist_filter_index(0);
        ui.set_playlist_query("第一首".into());
        s.filter_local_playlist(&ui);
        s.replace_queue_from_page();
        assert_eq!(s.queue.items.len(), 1);
        assert_eq!(s.queue.items[0].key.source, "one");
        assert_eq!(s.local_playlist.tracks.len(), 2);
        s.clear_page(&ui);
        assert!(ui.get_playlist_query().is_empty());
        assert_eq!(ui.get_playlist_filter_index(), 0);
        Ok(())
    }
}
