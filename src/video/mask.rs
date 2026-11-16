//! Clip-local alpha masks. Rasterized once per active clip, applied before composition.
use crate::config::video::{MaskShape, VideoMask};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn rasterize(
    masks: &[VideoMask],
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    let mut alpha = vec![255; width as usize * height as usize];
    for mask in masks {
        let left = mask.x * f64::from(output_width);
        let top = mask.y * f64::from(output_height);
        let w = mask.width * f64::from(output_width);
        let h = mask.height * f64::from(output_height);
        for y in 0..height {
            if cancel.load(Ordering::Acquire) {
                return None;
            }
            let py = (f64::from(y) + 0.5) / f64::from(height) * f64::from(output_height) - top;
            for x in 0..width {
                let px = (f64::from(x) + 0.5) / f64::from(width) * f64::from(output_width) - left;
                let distance = match mask.shape {
                    MaskShape::Polygon => polygon_distance(px, py, w, h, &mask.points),
                    MaskShape::Rectangle => px.min(w - px).min(py.min(h - py)),
                    MaskShape::Ellipse => {
                        let u = (px - w / 2.0) / (w / 2.0);
                        let v = (py - h / 2.0) / (h / 2.0);
                        let radius = u.hypot(v);
                        // First-order signed distance gives an approximately pixel-wide
                        // inner feather, including non-circular ellipses.
                        let gradient = (u / (w / 2.0)).hypot(v / (h / 2.0));
                        if gradient < 1e-12 {
                            w.min(h) / 2.0
                        } else {
                            (1.0 - radius) * radius / gradient
                        }
                    }
                };
                let coverage = if mask.feather == 0.0 {
                    if distance >= 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    let t = (distance / mask.feather).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                };
                let keep = if mask.invert {
                    1.0 - coverage
                } else {
                    coverage
                };
                let strength = 1.0 - mask.opacity + mask.opacity * keep;
                let i = (y * width + x) as usize;
                alpha[i] = (f64::from(alpha[i]) * strength).round() as u8;
            }
        }
    }
    Some(alpha)
}

fn polygon_distance(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    points: &[crate::config::video::MaskPoint],
) -> f64 {
    let mut inside = false;
    let mut distance = f64::INFINITY;
    for i in 0..points.len() {
        let a = &points[i];
        let b = &points[(i + 1) % points.len()];
        let (ax, ay, bx, by) = (a.x * w, a.y * h, b.x * w, b.y * h);
        if (ay > y) != (by > y) && x < (bx - ax) * (y - ay) / (by - ay) + ax {
            inside = !inside;
        }
        let (dx, dy) = (bx - ax, by - ay);
        let length = dx * dx + dy * dy;
        let t = if length > 0.0 {
            ((x - ax) * dx + (y - ay) * dy) / length
        } else {
            0.0
        }
        .clamp(0.0, 1.0);
        distance = distance.min((x - ax - t * dx).hypot(y - ay - t * dy));
    }
    if inside {
        distance
    } else {
        -distance
    }
}

pub(super) fn apply(pixels: &mut [u8], mask: &[u8]) {
    for (pixel, alpha) in pixels.chunks_exact_mut(4).zip(mask) {
        pixel[3] = ((u16::from(pixel[3]) * u16::from(*alpha) + 127) / 255) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rectangle() -> VideoMask {
        VideoMask {
            shape: MaskShape::Rectangle,
            points: vec![],
            x: 0.25,
            y: 0.25,
            width: 0.5,
            height: 0.5,
            feather: 0.0,
            invert: false,
            opacity: 1.0,
        }
    }
    #[test]
    fn polygon_handles_concavity_feather_and_inversion() {
        use crate::config::video::MaskPoint;
        let mut m = rectangle();
        m.shape = MaskShape::Polygon;
        m.x = 0.0;
        m.y = 0.0;
        m.width = 1.0;
        m.height = 1.0;
        m.points = [
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 0.5),
            (0.5, 0.5),
            (0.5, 1.0),
            (0.0, 1.0),
        ]
        .into_iter()
        .map(|(x, y)| MaskPoint { x, y })
        .collect();
        let raster = rasterize(&[m.clone()], 16, 16, 16, 16, &AtomicBool::new(false)).unwrap();
        assert_eq!(raster[4 * 16 + 4], 255);
        assert_eq!(raster[12 * 16 + 12], 0);
        m.feather = 2.0;
        let raster = rasterize(&[m.clone()], 16, 16, 16, 16, &AtomicBool::new(false)).unwrap();
        assert!(raster[7 * 16 + 12] > 0 && raster[7 * 16 + 12] < 255);
        m.invert = true;
        let inverse = rasterize(&[m], 16, 16, 16, 16, &AtomicBool::new(false)).unwrap();
        assert_eq!(inverse[12 * 16 + 12], 255);
        assert_eq!(inverse[4 * 16 + 4], 0);
    }
    #[test]
    fn feather_preserves_source_alpha_and_softens_edges() {
        let mut m = rectangle();
        m.feather = 2.0;
        let raster = rasterize(&[m], 16, 16, 16, 16, &AtomicBool::new(false)).unwrap();
        assert_eq!(raster[0], 0);
        assert!(raster[4 * 16 + 4] > 0 && raster[4 * 16 + 4] < 255);
        assert_eq!(raster[8 * 16 + 8], 255);
        let mut pixels = [80, 90, 100, 128, 80, 90, 100, 0];
        apply(&mut pixels, &[128, 255]);
        assert_eq!(pixels, [80, 90, 100, 64, 80, 90, 100, 0]);
    }
    #[test]
    fn inverted_ellipse_cuts_a_hole_and_multiple_masks_intersect() {
        let mut hole = rectangle();
        hole.shape = MaskShape::Ellipse;
        hole.invert = true;
        let raster = rasterize(
            &[rectangle(), hole],
            16,
            16,
            16,
            16,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(raster[0], 0);
        assert_eq!(raster[8 * 16 + 8], 0);
        assert_eq!(raster[4 * 16 + 4], 255);
    }
}
