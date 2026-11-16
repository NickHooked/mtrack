// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
export const VIDEO_MEDIA_TYPE = "application/x-mtrack-video-file";
export const VIDEO_ACCEPT =
  ".mp4,.mov,.mkv,.webm,.ogv,.avi,.m4v,.mpeg,.mpg,.mxf";
export function isVideoFile(name: string): boolean {
  return /\.(mp4|mov|mkv|webm|ogv|avi|m4v|mpeg|mpg|mxf)$/i.test(name);
}

/** Best effort for browser-supported codecs; GStreamer supports more formats. */
export function localVideoDuration(file: File): Promise<number | null> {
  return new Promise((resolve) => {
    const video = document.createElement("video");
    const url = URL.createObjectURL(file);
    const finish = (duration: number | null) => {
      clearTimeout(timer);
      video.onloadedmetadata = null;
      video.onerror = null;
      video.removeAttribute("src");
      video.load();
      URL.revokeObjectURL(url);
      resolve(duration);
    };
    const timer = setTimeout(() => finish(null), 3000);
    video.preload = "metadata";
    video.onloadedmetadata = () =>
      finish(
        Number.isFinite(video.duration) && video.duration > 0
          ? video.duration
          : null,
      );
    video.onerror = () => finish(null);
    video.src = url;
  });
}
