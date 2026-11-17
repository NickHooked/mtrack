// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
// SPDX-License-Identifier: GPL-3.0-only
//! Deterministic deck transport. Wall-clock and device I/O live outside this model.
use crate::config::video::{vj::*, VideoConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize)]
pub struct ActiveClip {
    pub id: String,
    pub column: usize,
    pub started: f64,
    pub generation: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct QueuedClip {
    pub column: usize,
    pub due: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct LayerState {
    pub active: Option<ActiveClip>,
    pub queued: Option<QueuedClip>,
    pub settings: LayerSettings,
}
#[derive(Clone, Debug, Serialize)]
pub struct PerformanceState {
    pub clock: f64,
    pub bpm: f64,
    pub beat: f64,
    pub master: f64,
    pub crossfader: f64,
    pub blackout: bool,
    pub frozen: bool,
    pub layers: Vec<LayerState>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlEvent {
    pub target: ControlTarget,
    /// Normalized continuous value or button on/off.
    pub value: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct SamplePosition {
    pub id: String,
    pub position: f64,
    pub instance: String,
}

pub struct Deck {
    pub config: VideoConfig,
    pub settings: VjConfig,
    pub state: PerformanceState,
    last_now: f64,
    generation: u64,
    midi_held: HashMap<String, bool>,
    tap_times: Vec<f64>,
}
impl Deck {
    pub fn new(config: VideoConfig) -> Result<Self, String> {
        config.validate().map_err(|e| e.join("; "))?;
        let settings = config.vj().cloned().unwrap_or_default();
        let errors = settings.validate(&config);
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        let layers = (0..config.tracks().len())
            .map(|i| LayerState {
                active: None,
                queued: None,
                settings: settings.layers.get(i).cloned().unwrap_or_default(),
            })
            .collect();
        Ok(Self {
            config,
            state: PerformanceState {
                clock: 0.0,
                bpm: settings.bpm,
                beat: 0.0,
                master: 1.0,
                crossfader: 0.5,
                blackout: false,
                frozen: false,
                layers,
            },
            settings,
            last_now: 0.0,
            generation: 0,
            midi_held: HashMap::new(),
            tap_times: vec![],
        })
    }
    pub fn update_settings(&mut self, settings: VjConfig) -> Result<(), String> {
        let errors = settings.validate(&self.config);
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        self.state.bpm = settings.bpm;
        for (i, layer) in self.state.layers.iter_mut().enumerate() {
            let previous = self.settings.layers.get(i).cloned().unwrap_or_default();
            let mut next = settings.layers.get(i).cloned().unwrap_or_default();
            // Mapping/effect edits must not reset faders adjusted live by MIDI.
            if next.opacity == previous.opacity {
                next.opacity = layer.settings.opacity;
            }
            if next.mute == previous.mute {
                next.mute = layer.settings.mute;
            }
            if next.solo == previous.solo {
                next.solo = layer.settings.solo;
            }
            layer.settings = next;
            if layer
                .queued
                .as_ref()
                .is_some_and(|q| settings.clip_at(&self.config, i, q.column).is_none())
            {
                layer.queued = None;
            }
        }
        self.midi_held.retain(|id, _| {
            settings.midi.iter().find(|m| &m.id == id)
                == self.settings.midi.iter().find(|m| &m.id == id)
        });
        self.settings = settings;
        Ok(())
    }
    pub fn advance(&mut self, now: f64) {
        if !now.is_finite() || now < self.last_now {
            return;
        }
        if !self.state.frozen {
            self.state.clock += now - self.last_now;
        }
        self.last_now = now;
        self.state.beat = self.state.clock * self.state.bpm / 60.0;
        let clock = self.state.clock;
        for i in 0..self.state.layers.len() {
            if let Some(queued) = self.state.layers[i]
                .queued
                .clone()
                .filter(|p| p.due <= clock + 1e-9)
            {
                self.state.layers[i].queued = None;
                self.launch(i, queued.column, queued.due);
            }
            let Some(active) = self.state.layers[i].active.clone() else {
                continue;
            };
            let Some(clip) = self.config.tracks()[i]
                .clips()
                .iter()
                .find(|c| c.id() == active.id)
            else {
                continue;
            };
            let options = self
                .settings
                .clips
                .get(&active.id)
                .cloned()
                .unwrap_or_default();
            if clock - active.started >= clip.duration() && !options.looping {
                self.state.layers[i].active = None;
                if options.auto_next {
                    for offset in 1..=self.settings.columns {
                        let next = (active.column + offset) % self.settings.columns;
                        if self.settings.clip_at(&self.config, i, next).is_some() {
                            self.launch(i, next, clock);
                            break;
                        }
                    }
                }
            }
        }
    }
    fn launch(&mut self, layer: usize, column: usize, at: f64) {
        let Some(id) = self.settings.clip_at(&self.config, layer, column) else {
            return;
        };
        self.generation = self.generation.wrapping_add(1);
        self.state.layers[layer].active = Some(ActiveClip {
            id: id.to_string(),
            column,
            started: at,
            generation: self.generation,
        });
    }
    fn queue(&mut self, layer: usize, column: usize, pressed: bool) {
        let Some(id) = self.settings.clip_at(&self.config, layer, column) else {
            return;
        };
        let mode = self
            .settings
            .clips
            .get(id)
            .map(|s| s.trigger)
            .unwrap_or_default();
        let state = &mut self.state.layers[layer];
        if !pressed {
            if mode == TriggerMode::Piano {
                if state.active.as_ref().is_some_and(|a| a.column == column) {
                    state.active = None;
                }
                if state.queued.as_ref().is_some_and(|q| q.column == column) {
                    state.queued = None;
                }
            }
            return;
        }
        if mode == TriggerMode::Toggle
            && (state.active.as_ref().is_some_and(|a| a.id == id)
                || state.queued.as_ref().is_some_and(|q| q.column == column))
        {
            state.active = None;
            state.queued = None;
            return;
        }
        let interval = self.settings.quantize * 60.0 / self.state.bpm;
        let due = if interval > 0.0 {
            ((self.state.clock / interval - 1e-9).ceil() * interval).max(self.state.clock)
        } else {
            self.state.clock
        };
        if due <= self.state.clock + 1e-9 {
            self.launch(layer, column, self.state.clock);
        } else {
            state.queued = Some(QueuedClip { column, due });
        }
    }
    pub fn control(&mut self, event: &ControlEvent, now: f64) -> Result<(), String> {
        if !event.value.is_finite()
            || !(0.0..=1.0).contains(&event.value)
            || !event
                .target
                .validate(self.state.layers.len(), self.settings.columns)
        {
            return Err("invalid VJ control target or value".into());
        }
        self.advance(now);
        let pressed = event.value > 0.0;
        match event.target {
            ControlTarget::Launch { layer, column } => self.queue(layer, column, pressed),
            ControlTarget::Column { column } => {
                for i in 0..self.state.layers.len() {
                    self.queue(i, column, pressed)
                }
            }
            ControlTarget::StopLayer { layer } if pressed => {
                self.state.layers[layer].active = None;
                self.state.layers[layer].queued = None;
            }
            ControlTarget::StopAll if pressed => {
                for l in &mut self.state.layers {
                    l.active = None;
                    l.queued = None;
                }
            }
            ControlTarget::LayerOpacity { layer } => {
                self.state.layers[layer].settings.opacity = event.value
            }
            ControlTarget::LayerMute { layer } => self.state.layers[layer].settings.mute = pressed,
            ControlTarget::LayerSolo { layer } => self.state.layers[layer].settings.solo = pressed,
            ControlTarget::MasterOpacity => self.state.master = event.value,
            ControlTarget::Crossfader => self.state.crossfader = event.value,
            ControlTarget::Blackout => self.state.blackout = pressed,
            ControlTarget::Freeze => self.state.frozen = pressed,
            ControlTarget::TapTempo if pressed => {
                if self.tap_times.last().is_some_and(|last| now - last > 3.0) {
                    self.tap_times.clear();
                }
                self.tap_times.push(now);
                if self.tap_times.len() > 5 {
                    self.tap_times.remove(0);
                }
                if self.tap_times.len() >= 2 {
                    let elapsed = now - self.tap_times[0];
                    if elapsed > 0.0 {
                        self.state.bpm =
                            (60.0 * (self.tap_times.len() - 1) as f64 / elapsed).clamp(20.0, 300.0);
                        self.settings.bpm = self.state.bpm;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub fn value(&self, target: &ControlTarget) -> f64 {
        match *target {
            ControlTarget::MasterOpacity => self.state.master,
            ControlTarget::Crossfader => self.state.crossfader,
            ControlTarget::Blackout => f64::from(self.state.blackout),
            ControlTarget::Freeze => f64::from(self.state.frozen),
            ControlTarget::LayerOpacity { layer } => self
                .state
                .layers
                .get(layer)
                .map_or(0.0, |l| l.settings.opacity),
            ControlTarget::LayerMute { layer } => f64::from(
                self.state
                    .layers
                    .get(layer)
                    .is_some_and(|l| l.settings.mute),
            ),
            ControlTarget::LayerSolo { layer } => f64::from(
                self.state
                    .layers
                    .get(layer)
                    .is_some_and(|l| l.settings.solo),
            ),
            _ => 0.0,
        }
    }
    pub fn midi(&mut self, device: &str, data: &[u8], now: f64) -> Vec<ControlEvent> {
        let Some(input) = MidiInput::parse(data) else {
            return vec![];
        };
        let matching = self
            .settings
            .midi
            .iter()
            .filter(|m| {
                (m.device.is_empty() || m.device == device)
                    && m.channel == input.channel
                    && m.kind == input.kind
                    && m.number == input.number
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut events = vec![];
        for mapping in matching {
            let was = self
                .midi_held
                .insert(mapping.id.clone(), input.value > 0)
                .unwrap_or(false);
            let pressed = input.value > 0;
            if mapping.mode == MidiMode::Toggle && pressed && !was {
                // Trigger buttons have no persistent scalar value. Toggle an active
                // clip/column off explicitly, irrespective of its mouse trigger mode.
                let to_stop: Vec<usize> = match mapping.target {
                    ControlTarget::Launch { layer, column }
                        if self.state.layers[layer]
                            .active
                            .as_ref()
                            .is_some_and(|a| a.column == column)
                            || self.state.layers[layer]
                                .queued
                                .as_ref()
                                .is_some_and(|q| q.column == column) =>
                    {
                        vec![layer]
                    }
                    ControlTarget::Column { column } => self
                        .state
                        .layers
                        .iter()
                        .enumerate()
                        .filter(|(_, l)| {
                            l.active.as_ref().is_some_and(|a| a.column == column)
                                || l.queued.as_ref().is_some_and(|q| q.column == column)
                        })
                        .map(|(i, _)| i)
                        .collect(),
                    _ => vec![],
                };
                if !to_stop.is_empty() {
                    for layer in to_stop {
                        let event = ControlEvent {
                            target: ControlTarget::StopLayer { layer },
                            value: 1.0,
                        };
                        if self.control(&event, now).is_ok() {
                            events.push(event);
                        }
                    }
                    continue;
                }
            }
            let value = if mapping.target.is_continuous() && mapping.mode == MidiMode::Absolute {
                mapping.min + (mapping.max - mapping.min) * f64::from(input.value) / 127.0
            } else if mapping.mode == MidiMode::Toggle {
                if !pressed || was {
                    continue;
                }
                if self.value(&mapping.target) > 0.5 {
                    mapping.min
                } else {
                    mapping.max
                }
            } else {
                if pressed == was {
                    continue;
                }
                if pressed {
                    mapping.max
                } else {
                    mapping.min
                }
            };
            let event = ControlEvent {
                target: mapping.target,
                value,
            };
            if self.control(&event, now).is_ok() {
                events.push(event);
            }
        }
        events
    }
    pub fn sample(&self, layer: usize) -> Option<SamplePosition> {
        let active = self.state.layers.get(layer)?.active.as_ref()?;
        let clip = self.config.tracks()[layer]
            .clips()
            .iter()
            .find(|c| c.id() == active.id)?;
        let elapsed = (self.state.clock - active.started).max(0.0);
        let cycle = (elapsed / clip.duration()).floor() as u64;
        let local = elapsed % clip.duration();
        Some(SamplePosition {
            id: active.id.clone(),
            position: clip.start() + local,
            instance: format!("{layer}:{}:{cycle}", active.generation),
        })
    }
    pub fn layer_opacity(&self, layer: usize) -> f64 {
        let l = &self.state.layers[layer].settings;
        if l.mute || (self.state.layers.iter().any(|l| l.settings.solo) && !l.solo) {
            return 0.0;
        }
        let cross = match l.crossfader {
            CrossfaderSide::None => 1.0,
            CrossfaderSide::A => (2.0 * (1.0 - self.state.crossfader)).min(1.0),
            CrossfaderSide::B => (2.0 * self.state.crossfader).min(1.0),
        };
        l.opacity * cross
    }
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct MidiInput {
    pub kind: MidiKind,
    pub channel: u8,
    pub number: u8,
    pub value: u8,
}
impl MidiInput {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() != 3 || data[1] > 127 || data[2] > 127 {
            return None;
        }
        let (kind, value) = match data[0] & 0xf0 {
            0x80 => (MidiKind::Note, 0),
            0x90 => (MidiKind::Note, data[2]),
            0xb0 => (MidiKind::Cc, data[2]),
            _ => return None,
        };
        Some(Self {
            kind,
            channel: data[0] & 0xf,
            number: data[1],
            value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn deck() -> Deck {
        Deck::new(serde_json::from_value(serde_json::json!({"tracks":[{"name":"A","clips":[{"id":"a","start":3,"duration":2,"source":{"type":"test_pattern"}},{"id":"b","start":5,"duration":2,"source":{"type":"test_pattern"}}]},{"name":"B","clips":[{"id":"c","start":0,"duration":2,"source":{"type":"test_pattern"}}]}]})).unwrap()).unwrap()
    }
    fn control(d: &mut Deck, target: ControlTarget, value: f64, time: f64) {
        d.control(&ControlEvent { target, value }, time).unwrap();
    }
    #[test]
    fn quantized_column_starts_all_layers_together_and_stop_cancels_pending() {
        let mut d = deck();
        d.settings.quantize = 4.0;
        control(&mut d, ControlTarget::Column { column: 0 }, 1.0, 0.3);
        assert!(d.sample(0).is_none());
        assert_eq!(d.state.layers[0].queued.as_ref().unwrap().due, 2.0);
        d.advance(2.0);
        assert_eq!(d.sample(0).unwrap().position, 3.0);
        assert_eq!(d.sample(1).unwrap().position, 0.0);
        control(
            &mut d,
            ControlTarget::Launch {
                layer: 0,
                column: 1,
            },
            1.0,
            2.2,
        );
        control(&mut d, ControlTarget::StopAll, 1.0, 2.3);
        d.advance(5.0);
        assert!(d.sample(0).is_none());
    }
    #[test]
    fn loop_retrigger_freeze_and_piano_release_have_distinct_instances() {
        let mut d = deck();
        let launch = ControlTarget::Launch {
            layer: 0,
            column: 0,
        };
        control(&mut d, launch.clone(), 1.0, 0.0);
        let first = d.sample(0).unwrap().instance;
        d.advance(2.25);
        assert_eq!(d.sample(0).unwrap().position, 3.25);
        assert_ne!(d.sample(0).unwrap().instance, first);
        control(&mut d, ControlTarget::Freeze, 1.0, 2.25);
        d.advance(10.0);
        assert_eq!(d.sample(0).unwrap().position, 3.25);
        control(&mut d, ControlTarget::Freeze, 0.0, 10.0);
        d.advance(10.5);
        assert_eq!(d.sample(0).unwrap().position, 3.75);
        d.settings.clips.insert(
            "a".into(),
            ClipSettings {
                trigger: TriggerMode::Piano,
                ..Default::default()
            },
        );
        control(&mut d, launch, 0.0, 10.5);
        assert!(d.sample(0).is_none());
    }
    #[test]
    fn note_off_and_zero_velocity_release_momentary_mappings() {
        let mut d = deck();
        d.settings.midi.push(MidiMapping {
            id: "black".into(),
            device: "host".into(),
            channel: 2,
            kind: MidiKind::Note,
            number: 60,
            target: ControlTarget::Blackout,
            mode: MidiMode::Momentary,
            min: 0.0,
            max: 1.0,
        });
        d.midi("other", &[0x92, 60, 100], 0.0);
        assert!(!d.state.blackout);
        d.midi("host", &[0x92, 60, 100], 0.0);
        assert!(d.state.blackout);
        d.midi("host", &[0x82, 60, 64], 0.1);
        assert!(!d.state.blackout);
        d.midi("host", &[0x92, 60, 100], 0.2);
        d.midi("host", &[0x92, 60, 0], 0.3);
        assert!(!d.state.blackout);
        assert!(MidiInput::parse(&[0xf8]).is_none());
        assert!(MidiInput::parse(&[0x92, 255, 0]).is_none());
    }
    #[test]
    fn cc_normalizes_ranges_and_toggle_only_fires_on_rising_edge() {
        let mut d = deck();
        d.settings.midi.push(MidiMapping {
            id: "fader".into(),
            device: "".into(),
            channel: 0,
            kind: MidiKind::Cc,
            number: 7,
            target: ControlTarget::MasterOpacity,
            mode: MidiMode::Absolute,
            min: 1.0,
            max: 0.0,
        });
        d.midi("x", &[0xb0, 7, 127], 0.0);
        assert_eq!(d.state.master, 0.0);
        d.settings.midi[0].mode = MidiMode::Toggle;
        d.settings.midi[0].target = ControlTarget::Blackout;
        d.settings.midi[0].min = 0.0;
        d.settings.midi[0].max = 1.0;
        d.midi_held.clear();
        d.midi("x", &[0xb0, 7, 127], 0.1);
        d.midi("x", &[0xb0, 7, 100], 0.2);
        assert!(d.state.blackout);
        d.midi("x", &[0xb0, 7, 0], 0.3);
        d.midi("x", &[0xb0, 7, 127], 0.4);
        assert!(!d.state.blackout);
    }
    #[test]
    fn one_shot_auto_next_and_solo_crossfader() {
        let mut d = deck();
        d.settings.clips.insert(
            "a".into(),
            ClipSettings {
                looping: false,
                auto_next: true,
                ..Default::default()
            },
        );
        control(
            &mut d,
            ControlTarget::Launch {
                layer: 0,
                column: 0,
            },
            1.0,
            0.0,
        );
        d.advance(2.0);
        assert_eq!(d.sample(0).unwrap().id, "b");
        d.state.layers[0].settings.crossfader = CrossfaderSide::A;
        d.state.crossfader = 1.0;
        assert_eq!(d.layer_opacity(0), 0.0);
        d.state.layers[0].settings.solo = true;
        assert_eq!(d.layer_opacity(1), 0.0);
    }
    #[test]
    fn mapping_edits_preserve_live_faders_and_held_midi() {
        let mut d = deck();
        control(&mut d, ControlTarget::LayerOpacity { layer: 0 }, 0.2, 0.0);
        let mut settings = d.settings.clone();
        settings.quantize = 1.0;
        d.update_settings(settings).unwrap();
        assert_eq!(d.state.layers[0].settings.opacity, 0.2);
        let mut settings = d.settings.clone();
        settings.layers = vec![LayerSettings {
            opacity: 0.7,
            ..Default::default()
        }];
        d.update_settings(settings).unwrap();
        assert_eq!(d.state.layers[0].settings.opacity, 0.7);
    }
    #[test]
    fn midi_toggle_stops_a_clip_even_when_mouse_mode_is_restart() {
        let mut d = deck();
        d.settings.midi.push(MidiMapping {
            id: "launch".into(),
            device: "".into(),
            channel: 0,
            kind: MidiKind::Note,
            number: 60,
            target: ControlTarget::Launch {
                layer: 0,
                column: 0,
            },
            mode: MidiMode::Toggle,
            min: 0.0,
            max: 1.0,
        });
        d.midi("host", &[0x90, 60, 127], 0.0);
        assert!(d.sample(0).is_some());
        d.midi("host", &[0x80, 60, 0], 0.1);
        assert!(d.sample(0).is_some());
        d.midi("host", &[0x90, 60, 127], 0.2);
        assert!(d.sample(0).is_none());
    }
}
