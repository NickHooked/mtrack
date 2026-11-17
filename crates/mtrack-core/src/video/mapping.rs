// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
// SPDX-License-Identifier: GPL-3.0-only
//! Perspective-correct output slices and straight-alpha layer compositing.
use crate::config::video::vj::{BlendMode, LayerSettings, OutputCanvas, OutputSource, Point};
use std::sync::atomic::{AtomicBool, Ordering};

/// Inverse projective transform from output coordinates to slice UV.
/// Solves the eight homography coefficients with partial pivoting.
pub fn inverse_quad(corners: &[Point; 4]) -> Option<[f64; 9]> {
    let uv = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    let mut a = [[0.0; 9]; 8];
    for i in 0..4 {
        let p = corners[i];
        let (u, v) = uv[i];
        a[i * 2] = [p.x, p.y, 1.0, 0.0, 0.0, 0.0, -u * p.x, -u * p.y, u];
        a[i * 2 + 1] = [0.0, 0.0, 0.0, p.x, p.y, 1.0, -v * p.x, -v * p.y, v];
    }
    for col in 0..8 {
        let pivot = (col..8).max_by(|x, y| a[*x][col].abs().total_cmp(&a[*y][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        let divisor = a[col][col];
        for j in col..9 {
            a[col][j] /= divisor;
        }
        for row in 0..8 {
            if row != col {
                let factor = a[row][col];
                for j in col..9 {
                    a[row][j] -= factor * a[col][j];
                }
            }
        }
    }
    Some([
        a[0][8], a[1][8], a[2][8], a[3][8], a[4][8], a[5][8], a[6][8], a[7][8], 1.0,
    ])
}
pub fn project(matrix: &[f64; 9], x: f64, y: f64) -> Option<(f64, f64)> {
    let denominator = matrix[6] * x + matrix[7] * y + 1.0;
    if denominator.abs() < 1e-12 {
        None
    } else {
        Some((
            (matrix[0] * x + matrix[1] * y + matrix[2]) / denominator,
            (matrix[3] * x + matrix[4] * y + matrix[5]) / denominator,
        ))
    }
}
fn alpha_over(dst: &mut [u8], src: [f64; 4], opacity: f64, blend: BlendMode) {
    let sa = (src[3] / 255.0 * opacity).clamp(0.0, 1.0);
    if sa == 0.0 {
        return;
    }
    let da = f64::from(dst[3]) / 255.0;
    let alpha = sa + da * (1.0 - sa);
    for c in 0..3 {
        let s = (src[c] / 255.0).clamp(0.0, 1.0);
        let d = f64::from(dst[c]) / 255.0;
        let blended = match blend {
            BlendMode::Normal => s,
            BlendMode::Add => (s + d).min(1.0),
            BlendMode::Screen => 1.0 - (1.0 - s) * (1.0 - d),
            BlendMode::Multiply => s * d,
        };
        let premult = sa * ((1.0 - da) * s + da * blended) + da * (1.0 - sa) * d;
        dst[c] = (premult / alpha * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (alpha * 255.0).round() as u8;
}
pub fn composite(dst: &mut [u8], source: &[u8], opacity: f64, settings: &LayerSettings) {
    if opacity <= 0.0 {
        return;
    }
    let neutral = settings.brightness == 0.0
        && settings.contrast == 1.0
        && settings.saturation == 1.0
        && settings.invert == 0.0;
    for (d, s) in dst.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
        if s[3] == 0 {
            continue;
        }
        if neutral {
            if opacity == 1.0 && settings.blend == BlendMode::Normal && s[3] == 255 {
                d.copy_from_slice(s);
            } else {
                alpha_over(
                    d,
                    [
                        f64::from(s[0]),
                        f64::from(s[1]),
                        f64::from(s[2]),
                        f64::from(s[3]),
                    ],
                    opacity,
                    settings.blend,
                );
            }
            continue;
        }
        let rgb = [
            f64::from(s[0]) / 255.0,
            f64::from(s[1]) / 255.0,
            f64::from(s[2]) / 255.0,
        ];
        let luma = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
        let mut color = [0.0, 0.0, 0.0, f64::from(s[3])];
        for c in 0..3 {
            let value = ((luma + (rgb[c] - luma) * settings.saturation - 0.5) * settings.contrast
                + 0.5
                + settings.brightness)
                .clamp(0.0, 1.0);
            color[c] = (value * (1.0 - settings.invert) + (1.0 - value) * settings.invert) * 255.0;
        }
        alpha_over(d, color, opacity, settings.blend);
    }
}
fn sample(frame: &[u8], width: u32, height: u32, u: f64, v: f64) -> [f64; 4] {
    let x = (u * f64::from(width) - 0.5).clamp(0.0, f64::from(width - 1));
    let y = (v * f64::from(height) - 0.5).clamp(0.0, f64::from(height - 1));
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let dx = x - f64::from(x0);
    let dy = y - f64::from(y0);
    let mut result = [0.0; 4];
    // Interpolate premultiplied colors so transparent borders don't bleed black.
    for (xx, yy, weight) in [
        (x0, y0, (1.0 - dx) * (1.0 - dy)),
        (x1, y0, dx * (1.0 - dy)),
        (x0, y1, (1.0 - dx) * dy),
        (x1, y1, dx * dy),
    ] {
        let i = ((yy * width + xx) * 4) as usize;
        let alpha = f64::from(frame[i + 3]) / 255.0;
        for c in 0..3 {
            result[c] += f64::from(frame[i + c]) * alpha * weight;
        }
        result[3] += alpha * weight;
    }
    if result[3] > 0.0 {
        for c in 0..3 {
            result[c] /= result[3];
        }
    }
    result[3] *= 255.0;
    result
}
fn feather(distance: f64, width: f64) -> f64 {
    if width <= 0.0 {
        1.0
    } else {
        let t = (distance / width).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

/// Renders one physical output canvas. Source frames all use the composition dimensions.
#[allow(clippy::too_many_arguments)]
pub fn render_output(
    config: &OutputCanvas,
    composition: &[u8],
    layers: &[Vec<u8>],
    width: u32,
    height: u32,
    master: f64,
    blackout: bool,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    let mut output = vec![0; (config.width * config.height * 4) as usize];
    if !config.enabled || blackout {
        return Some(output);
    }
    if config.test_pattern {
        for y in 0..config.height {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            for x in 0..config.width {
                let i = ((y * config.width + x) * 4) as usize;
                let line =
                    x % (config.width / 16).max(1) == 0 || y % (config.height / 9).max(1) == 0;
                output[i] = if line {
                    255
                } else {
                    (x * 255 / config.width) as u8
                };
                output[i + 1] = if line {
                    255
                } else {
                    (y * 255 / config.height) as u8
                };
                output[i + 2] = if line { 255 } else { 48 };
                output[i + 3] = 255;
            }
        }
    } else {
        for slice in config.slices.iter().filter(|s| s.enabled) {
            let source = match slice.source {
                OutputSource::Composition => composition,
                OutputSource::Layer { index } => layers.get(index)?.as_slice(),
            };
            if source.len() != width as usize * height as usize * 4 {
                return None;
            }
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if config.slices.len() == 1
                && config.width == width
                && config.height == height
                && !config.flip_x
                && !config.flip_y
                && slice.input.x == 0.0
                && slice.input.y == 0.0
                && slice.input.width == 1.0
                && slice.input.height == 1.0
                && slice.opacity == 1.0
                && slice.feather == [0.0; 4]
                && slice.corners
                    == [
                        Point { x: 0.0, y: 0.0 },
                        Point { x: 1.0, y: 0.0 },
                        Point { x: 1.0, y: 1.0 },
                        Point { x: 0.0, y: 1.0 },
                    ]
            {
                output.copy_from_slice(source);
                continue;
            }
            let matrix = inverse_quad(&slice.corners)?;
            let minx = slice
                .corners
                .iter()
                .map(|p| p.x)
                .fold(f64::INFINITY, f64::min)
                .clamp(0.0, 1.0);
            let maxx = slice
                .corners
                .iter()
                .map(|p| p.x)
                .fold(f64::NEG_INFINITY, f64::max)
                .clamp(0.0, 1.0);
            let miny = slice
                .corners
                .iter()
                .map(|p| p.y)
                .fold(f64::INFINITY, f64::min)
                .clamp(0.0, 1.0);
            let maxy = slice
                .corners
                .iter()
                .map(|p| p.y)
                .fold(f64::NEG_INFINITY, f64::max)
                .clamp(0.0, 1.0);
            for y in (miny * f64::from(config.height)).floor() as u32
                ..(maxy * f64::from(config.height)).ceil() as u32
            {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                for x in (minx * f64::from(config.width)).floor() as u32
                    ..(maxx * f64::from(config.width)).ceil() as u32
                {
                    let Some((u, v)) = project(
                        &matrix,
                        (f64::from(x) + 0.5) / f64::from(config.width),
                        (f64::from(y) + 0.5) / f64::from(config.height),
                    ) else {
                        continue;
                    };
                    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                        continue;
                    }
                    let edge = feather(u, slice.feather[0])
                        * feather(v, slice.feather[1])
                        * feather(1.0 - u, slice.feather[2])
                        * feather(1.0 - v, slice.feather[3]);
                    let rgba = sample(
                        source,
                        width,
                        height,
                        slice.input.x + u * slice.input.width,
                        slice.input.y + v * slice.input.height,
                    );
                    let xx = if config.flip_x {
                        config.width - 1 - x
                    } else {
                        x
                    };
                    let yy = if config.flip_y {
                        config.height - 1 - y
                    } else {
                        y
                    };
                    let i = ((yy * config.width + xx) * 4) as usize;
                    alpha_over(
                        &mut output[i..i + 4],
                        rgba,
                        edge * slice.opacity,
                        BlendMode::Normal,
                    );
                }
            }
        }
    }
    // Displays are opaque black behind all slices; gamma affects RGB, never geometry.
    let gain = config.opacity * master;
    let lut: [u8; 256] = std::array::from_fn(|value| {
        ((value as f64 / 255.0 * config.brightness)
            .clamp(0.0, 1.0)
            .powf(1.0 / config.gamma)
            * gain
            * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    });
    for pixel in output.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        for c in pixel.iter_mut().take(3) {
            *c = lut[((u16::from(*c) * alpha + 127) / 255) as usize];
        }
        pixel[3] = 255;
    }
    Some(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn output() -> OutputCanvas {
        serde_json::from_value(serde_json::json!({"id":"out","name":"Out","width":4,"height":4,"slices":[{"id":"s","name":"Slice"}]})).unwrap()
    }
    #[test]
    fn homography_hits_all_four_corners_of_trapezoid() {
        let points = [
            Point { x: 0.2, y: 0.1 },
            Point { x: 0.8, y: 0.1 },
            Point { x: 1.0, y: 0.9 },
            Point { x: 0.0, y: 0.9 },
        ];
        let h = inverse_quad(&points).unwrap();
        for (p, (u, v)) in points
            .iter()
            .zip([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])
        {
            let q = project(&h, p.x, p.y).unwrap();
            assert!((q.0 - u).abs() < 1e-9);
            assert!((q.1 - v).abs() < 1e-9);
        }
    }
    #[test]
    fn identity_preserves_pixels_and_routes_independent_layer() {
        let mut out = output();
        let red = [200, 0, 0, 255].repeat(16);
        let blue = [0, 0, 180, 255].repeat(16);
        let cancel = AtomicBool::new(false);
        assert_eq!(
            render_output(&out, &red, &[], 4, 4, 1.0, false, &cancel).unwrap(),
            red
        );
        out.slices[0].source = OutputSource::Layer { index: 0 };
        assert_eq!(
            render_output(&out, &red, &[blue.clone()], 4, 4, 1.0, false, &cancel).unwrap(),
            blue
        );
    }
    #[test]
    fn crop_warp_feather_blackout_and_cancel() {
        let mut out = output();
        let red = [200, 0, 0, 255].repeat(16);
        let cancel = AtomicBool::new(false);
        out.slices[0].corners[0].x = 0.5;
        out.slices[0].corners[3].x = 0.5;
        let result = render_output(&out, &red, &[], 4, 4, 1.0, false, &cancel).unwrap();
        assert_eq!(result[0], 0);
        assert_eq!(result[12], 200);
        out.slices[0].feather[0] = 1.0;
        let faded = render_output(&out, &red, &[], 4, 4, 1.0, false, &cancel).unwrap();
        assert!(faded[8] < faded[12]);
        let black = render_output(&out, &red, &[], 4, 4, 1.0, true, &cancel).unwrap();
        assert!(black.iter().all(|b| *b == 0));
        cancel.store(true, Ordering::Relaxed);
        assert!(render_output(&out, &red, &[], 4, 4, 1.0, false, &cancel).is_none());
    }
    #[test]
    fn additive_and_multiply_blending() {
        let source = [100, 100, 100, 255];
        let mut dest = [100, 100, 100, 255];
        let mut settings = LayerSettings {
            blend: BlendMode::Add,
            ..Default::default()
        };
        composite(&mut dest, &source, 1.0, &settings);
        assert_eq!(dest, [200, 200, 200, 255]);
        settings.blend = BlendMode::Multiply;
        composite(&mut dest, &source, 1.0, &settings);
        assert_eq!(dest, [78, 78, 78, 255]);
    }
}
