import type { VideoCanvas, VideoClip } from "./types";
export interface AutomationPoint {
  time: number;
  value: number;
  curve: number;
  hold: boolean;
}
export interface AutomationLane {
  enabled: boolean;
  points: AutomationPoint[];
}
export type Automation = Record<string, AutomationLane>;
export function valueAt(
  lane: AutomationLane | undefined,
  time: number,
): number | undefined {
  if (!lane?.enabled || !lane.points.length) return;
  if (time <= lane.points[0].time) return lane.points[0].value;
  for (let i = 0; i < lane.points.length - 1; i++) {
    const a = lane.points[i],
      b = lane.points[i + 1];
    if (time < b.time) {
      if (a.hold) return a.value;
      const t = (time - a.time) / (b.time - a.time),
        k = (a.curve ?? 0) * 6;
      return (
        a.value +
        (b.value - a.value) *
          (Math.abs(k) < 1e-8 ? t : Math.expm1(k * t) / Math.expm1(k))
      );
    }
  }
  return lane.points.at(-1)!.value;
}
export function propertyValue(clip: VideoClip, path: string): number {
  const value = path
    .slice(1)
    .split("/")
    .reduce<unknown>((v, k) => (v as Record<string, unknown>)?.[k], clip);
  return typeof value === "boolean" ? Number(value) : Number(value ?? 0);
}
export function evaluatedClip(
  clip: VideoClip,
  canvas: VideoCanvas,
  time: number,
): VideoClip {
  const result = structuredClone($state.snapshot(clip));
  result.width ??= canvas.width;
  result.height ??= canvas.height;
  result.opacity ??= 1;
  for (const [path, lane] of Object.entries(clip.automation ?? {})) {
    const v = valueAt(lane, time);
    if (v === undefined) continue;
    const keys = path.slice(1).split("/");
    const parent = keys
      .slice(0, -1)
      .reduce<unknown>((v, k) => (v as Record<string, unknown>)?.[k], result);
    if (parent && typeof parent === "object")
      (parent as Record<string, unknown>)[keys.at(-1)!] = path.endsWith(
        "/invert",
      )
        ? v >= 0.5
        : ["/x", "/y", "/width", "/height"].includes(path)
          ? Math.round(v)
          : v;
  }
  result.height = Math.min(result.height, Math.floor(8847360 / result.width));
  for (const m of result.masks ?? []) {
    m.x = Math.min(m.x, 0.999999);
    m.y = Math.min(m.y, 0.999999);
    m.width = Math.min(m.width, 1 - m.x);
    m.height = Math.min(m.height, 1 - m.y);
  }
  return result;
}
export function properties(clip: VideoClip, canvas: VideoCanvas) {
  const out = [
    { path: "/opacity", label: "Opacity", min: 0, max: 1 },
    { path: "/x", label: "X", min: -4096, max: 4096 },
    { path: "/y", label: "Y", min: -4096, max: 4096 },
    { path: "/width", label: "Width", min: 1, max: 4096 },
    { path: "/height", label: "Height", min: 1, max: 4096 },
    { path: "/fade_in", label: "Fade in", min: 0, max: clip.duration },
    { path: "/fade_out", label: "Fade out", min: 0, max: clip.duration },
  ];
  for (const [i, m] of (clip.masks ?? []).entries()) {
    for (const key of [
      "x",
      "y",
      "width",
      "height",
      "feather",
      "opacity",
      "invert",
    ])
      out.push({
        path: `/masks/${i}/${key}`,
        label: `Mask ${i + 1} · ${key}`,
        min: ["width", "height"].includes(key) ? 0.000001 : 0,
        max: key === "feather" ? 4096 : 1,
      });
    for (const [j] of (m.points ?? []).entries())
      for (const key of ["x", "y"])
        out.push({
          path: `/masks/${i}/points/${j}/${key}`,
          label: `Mask ${i + 1} · point ${j + 1} ${key}`,
          min: 0,
          max: 1,
        });
  }
  return out;
}
export function insertPoint(
  lane: AutomationLane,
  time: number,
  value: number,
): AutomationLane {
  time = Math.round(time * 1000) / 1000;
  const p = lane.points.find((p) => Math.abs(p.time - time) < 0.001);
  return {
    ...lane,
    points: [
      ...lane.points.filter((p) => Math.abs(p.time - time) >= 0.001),
      { time, value, curve: p?.curve ?? 0, hold: p?.hold ?? false },
    ].sort((a, b) => a.time - b.time),
  };
}

/** Preserve the sampled envelope when cutting a clip, including exponential segments. */
export function trimAutomation(
  automation: Automation | undefined,
  offset: number,
  duration: number,
): Automation | undefined {
  if (!automation) return automation;
  return Object.fromEntries(
    Object.entries(automation).map(([path, lane]) => {
      if (!lane.points.length) return [path, lane];
      const active = { ...lane, enabled: true };
      const times = [
        offset,
        ...lane.points
          .map((p) => p.time)
          .filter((t) => t > offset && t < offset + duration),
        offset + duration,
      ];
      const points = times.map((t, i) => {
        const a = lane.points.findLast((p) => p.time <= t);
        const b = lane.points.find((p) => p.time > t);
        const within = a && b;
        return {
          time: t - offset,
          value: Math.min(
            path.startsWith("/fade_") ? duration : Infinity,
            valueAt(active, t)!,
          ),
          hold: within ? a.hold : false,
          curve:
            within && i + 1 < times.length
              ? ((a.curve ?? 0) * (times[i + 1] - t)) / (b.time - a.time)
              : 0,
        };
      });
      return [path, { ...lane, points }];
    }),
  );
}
export function automationError(
  clip: VideoClip,
  canvas: VideoCanvas,
): string | null {
  const entries = Object.entries(clip.automation ?? {});
  if (
    entries.length > 256 ||
    entries.reduce((n, [, l]) => n + (l.points?.length ?? 0), 0) > 8192
  )
    return "Too many automation lanes or points.";
  const allowed = properties(clip, canvas);
  for (const [path, lane] of entries) {
    const prop = allowed.find((p) => p.path === path);
    if (!prop || !Array.isArray(lane.points))
      return "Unknown automation property.";
    if (
      lane.points.some(
        (p, i) =>
          ![p.time, p.value, p.curve ?? 0].every(Number.isFinite) ||
          p.time < 0 ||
          p.time > clip.duration ||
          (i > 0 && p.time <= lane.points[i - 1].time) ||
          p.value < prop.min ||
          p.value > prop.max ||
          Math.abs(p.curve ?? 0) > 1,
      )
    )
      return "Invalid automation point.";
  }
  return null;
}
