use super::*;
use music_player::{Equalizer, Normalization, Outputs};

impl UiState {
    pub(super) fn replace_queue_from_page(&mut self) {
        self.queue.items = self
            .items
            .iter()
            .filter(|i| i.kind == "track" && !i.unavailable)
            .cloned()
            .collect();
        self.queue.album_origin = (self.last.op == "album" && self.online.is_none())
            .then(|| ResourceKey {
                source: self.page_source.clone(),
                id: self.last.args["id"].as_str().unwrap_or_default().into(),
            })
            .filter(|k| !k.id.is_empty());
    }

    pub(super) fn normalization(&self) -> Normalization {
        if !self.normalize {
            return Normalization::Off;
        }
        if matches!(
            self.queue.playback_mode(),
            PlaybackMode::Sequential | PlaybackMode::RepeatAll
        ) && let Some(key) = &self.queue.album_origin
        {
            return Normalization::Album(
                serde_json::to_string(key).expect("resource keys are strings"),
            );
        }
        Normalization::Track
    }
}

#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct DeviceEq {
    preset: usize,
    custom: Equalizer,
}
impl DeviceEq {
    fn gains(self) -> Equalizer {
        Equalizer::PRESETS
            .get(self.preset)
            .copied()
            .unwrap_or(Equalizer(self.custom.bands()))
    }
}

pub(super) struct Sound {
    db: Library,
    outputs: Option<Outputs>,
    eq: DeviceEq,
}

impl Sound {
    pub fn install(
        ui: &AppWindow,
        db: Library,
        state: Rc<RefCell<UiState>>,
        player: mpsc::Sender<Command>,
    ) -> Rc<RefCell<Self>> {
        let enabled = db.get("normalize-loudness").as_bool().unwrap_or(false);
        ui.set_normalize_loudness(enabled);
        state.borrow_mut().normalize = enabled;
        if let Some(id) = db.get("audio-output").as_str() {
            let _ = player.send(Command::Output(id.into()));
        }
        let sound = Rc::new(RefCell::new(Self {
            db,
            outputs: None,
            eq: DeviceEq::default(),
        }));
        let weak = ui.as_weak();
        let prefs = sound.clone();
        let pc = player.clone();
        ui.on_toggle_loudness(move |enabled| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = state.borrow_mut();
            if let Err(e) = prefs.borrow().db.set("normalize-loudness", json!(enabled)) {
                ui.invoke_notify_row(e.to_string().into());
                return;
            }
            s.normalize = enabled;
            ui.set_normalize_loudness(enabled);
            let _ = pc.send(Command::Normalize(s.normalization()));
        });
        let weak = ui.as_weak();
        let prefs = sound.clone();
        let pc = player.clone();
        ui.on_select_output(move |i| {
            let Some(ui) = weak.upgrade() else { return };
            let mut bank = prefs.borrow_mut();
            let Some(id) = bank
                .outputs
                .as_ref()
                .and_then(|o| o.devices.get(i as usize))
                .map(|d| d.name.clone())
            else {
                return;
            };
            if let Err(e) = bank.db.set("audio-output", json!(id)) {
                ui.invoke_notify_row(e.to_string().into());
                return;
            }
            bank.outputs = None;
            ui.set_eq_available(false);
            let _ = pc.send(Command::Output(id));
        });
        let weak = ui.as_weak();
        let prefs = sound.clone();
        let pc = player.clone();
        ui.on_select_eq(move |i| {
            if let Some(ui) = weak.upgrade()
                && (0..=Equalizer::PRESETS.len() as i32).contains(&i)
            {
                let mut bank = prefs.borrow_mut();
                let eq = DeviceEq {
                    preset: i as usize,
                    ..bank.eq
                };
                bank.change(&ui, &pc, eq);
            }
        });
        let weak = ui.as_weak();
        let prefs = sound.clone();
        ui.on_adjust_eq(move |band, value| {
            if let Some(ui) = weak.upgrade() {
                let mut bank = prefs.borrow_mut();
                let mut bands = bank.eq.gains();
                if let Some(gain) = bands.0.get_mut(band as usize) {
                    let next = value.round().clamp(-9., 9.) as i8;
                    if *gain == next {
                        return;
                    }
                    *gain = next;
                    bank.change(
                        &ui,
                        &player,
                        DeviceEq {
                            preset: Equalizer::PRESETS.len(),
                            custom: bands,
                        },
                    );
                }
            }
        });
        sound
    }

    pub fn outputs(&mut self, ui: &AppWindow, player: &mpsc::Sender<Command>, outputs: Outputs) {
        let device_name = outputs
            .devices
            .iter()
            .find(|d| d.name == outputs.active)
            .map(|d| &d.description);
        let labels = outputs
            .devices
            .iter()
            .map(|d| {
                if d.name == "auto" {
                    device_name
                        .filter(|_| outputs.selected == "auto")
                        .map_or_else(|| "跟随系统".into(), |s| format!("跟随系统 · {s}").into())
                } else {
                    d.description.as_str().into()
                }
            })
            .collect::<Vec<_>>();
        ui.set_output_names(ModelRc::new(VecModel::from(labels)));
        ui.set_output_index(
            outputs
                .devices
                .iter()
                .position(|d| d.name == outputs.selected)
                .unwrap_or(0) as i32,
        );
        ui.set_eq_available(!outputs.active.is_empty());
        self.eq = serde_json::from_value(self.db.get(&format!("device-eq/{}", outputs.active)))
            .unwrap_or_default();
        if self.eq.preset > Equalizer::PRESETS.len() {
            self.eq = DeviceEq::default();
        }
        self.outputs = Some(outputs);
        self.apply(ui, player);
    }
    fn change(&mut self, ui: &AppWindow, player: &mpsc::Sender<Command>, eq: DeviceEq) {
        let Some(output) = self.outputs.as_ref().filter(|v| !v.active.is_empty()) else {
            return;
        };
        if self.eq.preset == eq.preset && self.eq.custom == eq.custom {
            return;
        }
        if let Err(e) = self
            .db
            .set(&format!("device-eq/{}", output.active), json!(eq))
        {
            ui.invoke_notify_row(e.to_string().into());
            return;
        }
        self.eq = eq;
        self.apply(ui, player);
    }
    fn apply(&self, ui: &AppWindow, player: &mpsc::Sender<Command>) {
        let [low, mid, high] = self.eq.gains().bands();
        ui.set_eq_low(low.into());
        ui.set_eq_mid(mid.into());
        ui.set_eq_high(high.into());
        ui.set_eq_index(self.eq.preset as i32);
        if let Some(output) = &self.outputs
            && !output.active.is_empty()
        {
            let _ = player.send(Command::Equalizer {
                device: output.active.clone(),
                eq: self.eq.gains(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn album_origin_survives_navigation_and_restart_but_not_queue_edits() {
        let mut s = UiState::new(Queue::default(), Default::default(), vec![]);
        s.normalize = true;
        s.items = vec![Item {
            kind: "track".into(),
            ..Default::default()
        }];
        s.last = Request::new("album", json!({"id":"a"}));
        s.replace_queue_from_page();
        let mode = s.normalization();
        assert!(matches!(mode, Normalization::Album(_)));
        s.last = Request::new("liked", Value::Null);
        s.queue = serde_json::from_value(json!(s.queue)).unwrap();
        assert_eq!(s.normalization(), mode);
        s.queue.toggle_shuffle();
        assert_eq!(s.normalization(), Normalization::Track);
        s.queue.toggle_shuffle();
        assert_eq!(s.normalization(), mode);
        s.queue.remove(0);
        assert_eq!(s.normalization(), Normalization::Track);
        s.replace_queue_from_page();
        assert_eq!(s.normalization(), Normalization::Track);
        s.queue = serde_json::from_value(json!({"items":[],"index":null})).unwrap();
        assert_eq!(s.normalization(), Normalization::Track);
    }

    #[cfg(feature = "ui-smoke")]
    #[test]
    fn device_eq_and_loudness_restore_from_settings() -> Result<()> {
        let _clock = crate::test_ui::test_clock()?;
        let ui = AppWindow::new()?;
        let dir = tempfile::tempdir()?;
        let state = Rc::new(RefCell::new(UiState::new(
            Queue::default(),
            Default::default(),
            vec![],
        )));
        let (tx, _rx) = mpsc::channel();
        let outputs = |id: &str| {
            Outputs{selected:"auto".into(),active:id.into(),devices:serde_json::from_value(json!([
            {"name":"auto","description":"Auto"},{"name":"a","description":"耳机"},{"name":"b","description":"耳机"}
        ])).unwrap()}
        };
        let bank = Sound::install(&ui, Library::open(dir.path())?, state.clone(), tx.clone());
        assert!(!ui.get_normalize_loudness());
        ui.invoke_toggle_loudness(true);
        bank.borrow_mut().outputs(&ui, &tx, outputs("a"));
        ui.invoke_select_eq(2);
        ui.invoke_adjust_eq(1, 4.);
        assert_eq!(ui.get_eq_index(), 5);
        ui.invoke_select_eq(1);
        ui.invoke_select_eq(5);
        assert_eq!(ui.get_eq_mid(), 4);
        bank.borrow_mut().outputs(&ui, &tx, outputs("b"));
        assert_eq!((ui.get_eq_index(), ui.get_eq_mid()), (0, 0));
        ui.invoke_select_eq(3);
        drop(bank);
        let reopened = Sound::install(&ui, Library::open(dir.path())?, state, tx.clone());
        assert!(ui.get_normalize_loudness());
        reopened.borrow_mut().outputs(&ui, &tx, outputs("a"));
        assert_eq!((ui.get_eq_index(), ui.get_eq_mid()), (5, 4));
        reopened.borrow_mut().outputs(&ui, &tx, outputs("b"));
        assert_eq!(ui.get_eq_index(), 3);
        Ok(())
    }
}
