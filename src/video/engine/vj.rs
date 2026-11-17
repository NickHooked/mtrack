// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
// SPDX-License-Identifier: GPL-3.0-only
//! Live deck renderer: one capture cache across every layer and output.
use super::*;
use mtrack_core::{
    config::video::vj::LayerSettings,
    video::{mapping, vj::Deck},
};
use rayon::prelude::*;
use std::collections::HashMap;

pub(crate) struct RenderedCanvas {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
#[derive(Clone)]
pub(crate) struct EncodedCanvas {
    pub id: String,
    pub jpeg: Vec<u8>,
}
pub(crate) struct DeckRenderer {
    workers: rayon::ThreadPool,
    request: WorkerRequest,
    compounds: compound::CompoundRenderer,
    live: HashMap<String, ClipDecoder>,
    frozen_layers: Vec<Vec<u8>>,
    frozen_instances: Vec<Option<String>>,
}
impl DeckRenderer {
    pub fn new(
        config: VideoConfig,
        base: &Path,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, VideoError> {
        let engine = GstVideoEngine::preview(config, base)?;
        let (config, bound) = clone_bound_media(&engine.config, &engine._bound_media)?;
        let (frame_tx, frame_rx) = bounded(1);
        let (runtime_error_tx, _) = bounded(1);
        let request = WorkerRequest {
            config,
            timeline: engine.timeline.clone(),
            base_path: base.to_path_buf(),
            _bound_media: bound,
            output: OutputMode::Preview,
            song_position: Duration::ZERO,
            generation: 0,
            cancel,
            latest: Arc::new(Mutex::new(None)),
            rendered_position: Arc::new(Mutex::new(Duration::ZERO)),
            frame_tx,
            frame_rx,
            latest_only: true,
            runtime_error_tx,
            #[cfg(test)]
            test_release: None,
            #[cfg(test)]
            test_observed: None,
        };
        let workers = rayon::ThreadPoolBuilder::new()
            .num_threads(
                std::thread::available_parallelism()
                    .map_or(1, |n| n.get())
                    .min(4),
            )
            .thread_name(|i| format!("mtrack-vj-output-{i}"))
            .build()
            .map_err(|e| worker_error("creating output workers", e))?;
        Ok(Self {
            workers,
            request,
            compounds: compound::CompoundRenderer::default(),
            live: HashMap::new(),
            frozen_layers: vec![],
            frozen_instances: vec![],
        })
    }
    pub fn render(&mut self, deck: &Deck) -> Result<Option<Vec<RenderedCanvas>>, VideoError> {
        let canvas = self.request.config.canvas();
        let width = canvas.width();
        let height = canvas.height();
        let timing = frame_timing(canvas.frame_rate())?;
        let mut live_frames = HashMap::new();
        let mut raw_layers = vec![];
        let instances = (0..deck.state.layers.len())
            .map(|i| deck.sample(i).map(|s| s.instance))
            .collect::<Vec<_>>();
        if deck.state.frozen
            && instances == self.frozen_instances
            && self.frozen_layers.len() == instances.len()
        {
            raw_layers = self.frozen_layers.clone();
        } else {
            self.compounds.begin_frame();
            for i in 0..deck.state.layers.len() {
                let pixels = if let Some(sample) = deck.sample(i) {
                    match self.compounds.render_scope(
                        None,
                        Some(i),
                        &sample.instance,
                        Duration::from_secs_f64(sample.position),
                        &self.request,
                        width,
                        height,
                        timing.fps_n,
                        timing.fps_d,
                        &mut self.live,
                        &mut live_frames,
                    )? {
                        DecoderPoll::Ready(pixels) => pixels,
                        DecoderPoll::Cancelled => return Ok(None),
                    }
                } else {
                    vec![0; rgba_buffer_len(width, height)?]
                };
                raw_layers.push(pixels);
            }
            self.compounds.end_frame();
            self.live.retain(|key, _| live_frames.contains_key(key));
            self.frozen_layers = raw_layers.clone();
            self.frozen_instances = instances;
        }
        let mut composition = solid_frame(width, height, parse_color(canvas.background())?)?;
        let mut layers = vec![];
        for (i, raw) in raw_layers.iter().enumerate() {
            if self.request.cancel.load(Ordering::Acquire) {
                return Ok(None);
            }
            let settings = &deck.state.layers[i].settings;
            let mut isolated = vec![0; raw.len()];
            // Color/effects/fader apply once. The blend mode is relative to the composition.
            let mut isolated_settings = settings.clone();
            isolated_settings.blend = mtrack_core::config::video::vj::BlendMode::Normal;
            mapping::composite(
                &mut isolated,
                raw,
                deck.layer_opacity(i),
                &isolated_settings,
            );
            mapping::composite(
                &mut composition,
                &isolated,
                1.0,
                &LayerSettings {
                    blend: settings.blend,
                    ..Default::default()
                },
            );
            layers.push(isolated);
        }
        let cancel = &self.request.cancel;
        let rendered: Option<Vec<RenderedCanvas>> = self.workers.install(|| {
            deck.settings
                .outputs
                .par_iter()
                .map(|output| {
                    let rgba = mapping::render_output(
                        output,
                        &composition,
                        &layers,
                        width,
                        height,
                        deck.state.master,
                        deck.state.blackout,
                        cancel,
                    )?;
                    Some(RenderedCanvas {
                        id: output.id.clone(),
                        width: output.width,
                        height: output.height,
                        rgba,
                    })
                })
                .collect()
        });
        let Some(mut frames) = rendered else {
            return Ok(None);
        };
        let master = if deck.state.blackout {
            0.0
        } else {
            deck.state.master
        };
        for pixel in composition.chunks_exact_mut(4) {
            for channel in pixel.iter_mut().take(3) {
                *channel = (f64::from(*channel) * master).round() as u8;
            }
        }
        frames.insert(
            0,
            RenderedCanvas {
                id: "composition".into(),
                width,
                height,
                rgba: composition,
            },
        );
        Ok(Some(frames))
    }
    /// Encode independent canvases on the same bounded worker pool. Capture and
    /// compound evaluation remain single-owner, preserving physical input sharing.
    pub fn render_encoded(
        &mut self,
        deck: &Deck,
    ) -> Result<Option<Vec<EncodedCanvas>>, VideoError> {
        let Some(canvases) = self.render(deck)? else {
            return Ok(None);
        };
        let cancel = &self.request.cancel;
        let encoded: Result<Vec<EncodedCanvas>, VideoError> = self.workers.install(|| {
            canvases
                .into_par_iter()
                .map(|canvas| {
                    let mut jpeg = vec![];
                    if !cancel.load(Ordering::Acquire) {
                        let mut rgb = Vec::with_capacity(canvas.rgba.len() / 4 * 3);
                        for pixel in canvas.rgba.chunks_exact(4) {
                            rgb.extend_from_slice(&pixel[..3]);
                        }
                        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88)
                            .encode(
                                &rgb,
                                canvas.width,
                                canvas.height,
                                image::ExtendedColorType::Rgb8,
                            )
                            .map_err(|e| worker_error("encoding VJ output", e))?;
                    }
                    Ok(EncodedCanvas {
                        id: canvas.id,
                        jpeg,
                    })
                })
                .collect()
        });
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        encoded.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrack_core::config::video::vj::ControlTarget;
    use mtrack_core::video::vj::ControlEvent;

    #[test]
    fn file_playback_catches_accumulated_frame_overruns() {
        let directory = tempfile::tempdir().unwrap();
        let raw = directory.path().join("sequence.rgba");
        // Red for two seconds, then blue. A decoder pulled only once per
        // render would still show red at 2.4s after several modest overruns.
        let colors = [[255u8, 0, 0, 255]; 4]
            .into_iter()
            .chain([[0u8, 0, 255, 255]; 6]);
        let bytes: Vec<u8> = colors
            .flat_map(|color| std::iter::repeat_n(color, 16).flatten())
            .collect();
        std::fs::write(&raw, bytes).unwrap();
        let media = directory.path().join("sequence.mkv");
        let generated = std::process::Command::new("timeout")
            .args([
                "10s",
                "gst-launch-1.0",
                "-q",
                "filesrc",
                &format!("location={}", raw.display()),
                "!",
                "rawvideoparse",
                "format=rgba",
                "width=4",
                "height=4",
                "framerate=2/1",
                "!",
                "videoconvert",
                "!",
                "avenc_ffv1",
                "!",
                "matroskamux",
                "!",
                "filesink",
                &format!("location={}", media.display()),
            ])
            .status()
            .expect("bounded GStreamer fixture generator");
        assert!(generated.success());
        let config: VideoConfig = serde_json::from_value(serde_json::json!({
            "canvas":{"width":4,"height":4,"frame_rate":2},
            "tracks":[{"name":"File","clips":[{"id":"file","file":"sequence.mkv","start":0,"duration":5}]}]
        })).unwrap();
        let mut deck = Deck::new(config.clone()).unwrap();
        deck.control(
            &ControlEvent {
                target: ControlTarget::Launch {
                    layer: 0,
                    column: 0,
                },
                value: 1.0,
            },
            0.0,
        )
        .unwrap();
        let mut renderer =
            DeckRenderer::new(config, directory.path(), Arc::new(AtomicBool::new(false))).unwrap();
        let first = renderer.render(&deck).unwrap().unwrap();
        assert!(first[0].rgba[0] > 240 && first[0].rgba[2] < 10);
        for time in [0.8, 1.6] {
            deck.advance(time);
            renderer.render(&deck).unwrap().unwrap();
        }
        deck.advance(2.4);
        let latest = renderer.render(&deck).unwrap().unwrap();
        assert!(
            latest[0].rgba[2] > 240 && latest[0].rgba[0] < 10,
            "file fell behind the deck clock: {:?}",
            &latest[0].rgba[..4]
        );
    }
}
