//! Nested source composition. All physical capture inputs share the outer frame cache.
use super::*;
use std::collections::{HashMap, HashSet};
#[derive(Default)]
pub(super) struct CompoundRenderer {
    files: HashMap<String, ClipDecoder>,
    used: HashSet<String>,
    masks: HashMap<String, (String, Vec<u8>)>,
}
impl CompoundRenderer {
    pub fn begin_frame(&mut self) {
        self.used.clear();
    }
    pub fn end_frame(&mut self) {
        self.files.retain(|key, _| self.used.contains(key));
        self.masks.retain(|key, _| self.used.contains(key));
    }
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        id: &str,
        instance: &str,
        position: Duration,
        request: &WorkerRequest,
        width: u32,
        height: u32,
        fps_n: i32,
        fps_d: i32,
        live_decoders: &mut HashMap<String, ClipDecoder>,
        live_frames: &mut HashMap<String, Vec<u8>>,
    ) -> Result<DecoderPoll<Vec<u8>>, VideoError> {
        let config = &request.config.compounds()[id];
        let timeline = &request.timeline.compounds()[id];
        let mut output = vec![0; width as usize * height as usize * 4];
        for original in timeline.active_clips_at(position) {
            if request.cancel.load(Ordering::Acquire) {
                return Ok(DecoderPoll::Cancelled);
            }
            let clip = original.evaluated_at(position);
            let configured = config
                .tracks()
                .iter()
                .flat_map(|t| t.clips())
                .find(|c| c.id() == clip.id())
                .expect("resolved child");
            let key =
                serde_json::to_string(&(instance, id, clip.id())).expect("source instance key");
            self.used.insert(key.clone());
            let mut frame = if let Some(crate::config::video::VideoSource::Compound { id: child }) =
                clip.source()
            {
                match self.render(
                    child,
                    &key,
                    clip.source_position_at(position),
                    request,
                    width,
                    height,
                    fps_n,
                    fps_d,
                    live_decoders,
                    live_frames,
                )? {
                    DecoderPoll::Ready(frame) => frame,
                    DecoderPoll::Cancelled => return Ok(DecoderPoll::Cancelled),
                }
            } else if let Some(source) = clip.source() {
                let source_key = live_source_key(source);
                if !live_decoders.contains_key(&source_key) {
                    match ClipDecoder::open(
                        configured,
                        &clip,
                        &request.base_path,
                        width,
                        height,
                        fps_n,
                        fps_d,
                        position,
                        &request.cancel,
                    )? {
                        DecoderPoll::Ready(d) => {
                            live_decoders.insert(source_key.clone(), d);
                        }
                        DecoderPoll::Cancelled => return Ok(DecoderPoll::Cancelled),
                    }
                }
                if !live_frames.contains_key(&source_key) {
                    match live_decoders
                        .get_mut(&source_key)
                        .unwrap()
                        .next_frame(&request.cancel)?
                    {
                        DecoderPoll::Ready(frame) => {
                            live_frames.insert(
                                source_key.clone(),
                                frame.unwrap_or_else(|| vec![0; output.len()]),
                            );
                        }
                        DecoderPoll::Cancelled => return Ok(DecoderPoll::Cancelled),
                    }
                }
                live_frames[&source_key].clone()
            } else {
                if !self.files.contains_key(&key) {
                    match ClipDecoder::open(
                        configured,
                        &clip,
                        &request.base_path,
                        width,
                        height,
                        fps_n,
                        fps_d,
                        position,
                        &request.cancel,
                    )? {
                        DecoderPoll::Ready(d) => {
                            self.files.insert(key.clone(), d);
                        }
                        DecoderPoll::Cancelled => return Ok(DecoderPoll::Cancelled),
                    }
                }
                match self
                    .files
                    .get_mut(&key)
                    .unwrap()
                    .next_frame(&request.cancel)?
                {
                    DecoderPoll::Ready(frame) => frame.unwrap_or_else(|| vec![0; output.len()]),
                    DecoderPoll::Cancelled => return Ok(DecoderPoll::Cancelled),
                }
            };
            let cw = clip.width().unwrap_or(config.canvas().width());
            let ch = clip.height().unwrap_or(config.canvas().height());
            if !clip.masks().is_empty() {
                let stamp = format!(
                    "{cw}:{ch}:{}",
                    serde_json::to_string(clip.masks()).expect("validated masks")
                );
                if self.masks.get(&key).map(|m| m.0.as_str()) != Some(stamp.as_str()) {
                    let Some(alpha) = crate::video::mask::rasterize(
                        clip.masks(),
                        width,
                        height,
                        cw,
                        ch,
                        &request.cancel,
                    ) else {
                        return Ok(DecoderPoll::Cancelled);
                    };
                    self.masks.insert(key.clone(), (stamp, alpha));
                }
                crate::video::mask::apply(&mut frame, &self.masks[&key].1);
            }
            let sx = f64::from(width) / f64::from(config.canvas().width());
            let sy = f64::from(height) / f64::from(config.canvas().height());
            let left = f64::from(clip.x()) * sx;
            let top = f64::from(clip.y()) * sy;
            let w = f64::from(cw) * sx;
            let h = f64::from(ch) * sy;
            let opacity = clip.opacity_at(position);
            for y in
                top.floor().max(0.0) as u32..(top + h).ceil().max(0.0).min(f64::from(height)) as u32
            {
                if request.cancel.load(Ordering::Acquire) {
                    return Ok(DecoderPoll::Cancelled);
                }
                for x in left.floor().max(0.0) as u32
                    ..(left + w).ceil().max(0.0).min(f64::from(width)) as u32
                {
                    let u = ((f64::from(x) + 0.5 - left) / w * f64::from(width))
                        .floor()
                        .clamp(0.0, f64::from(width - 1)) as u32;
                    let v = ((f64::from(y) + 0.5 - top) / h * f64::from(height))
                        .floor()
                        .clamp(0.0, f64::from(height - 1)) as u32;
                    let si = ((v * width + u) * 4) as usize;
                    let di = ((y * width + x) * 4) as usize;
                    let a = f64::from(frame[si + 3]) / 255.0 * opacity;
                    let b = f64::from(output[di + 3]) / 255.0 * (1.0 - a);
                    let total = a + b;
                    if total > 0.0 {
                        for c in 0..3 {
                            output[di + c] = ((f64::from(frame[si + c]) * a
                                + f64::from(output[di + c]) * b)
                                / total)
                                .round() as u8;
                        }
                        output[di + 3] = (total * 255.0).round() as u8;
                    }
                }
            }
        }
        Ok(DecoderPoll::Ready(output))
    }
}
