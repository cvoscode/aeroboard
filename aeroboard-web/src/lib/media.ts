import type { MediaKind } from "../hooks/aeroboard/types";

const MIME_BY_FORMAT: Record<string, string> = {
  wav: "audio/wav",
  mp3: "audio/mpeg",
  ogg: "audio/ogg",
  oga: "audio/ogg",
  opus: "audio/ogg",
  flac: "audio/flac",
  aac: "audio/aac",
  m4a: "audio/mp4",
  mp4: "video/mp4",
  m4v: "video/mp4",
  webm: "video/webm",
  ogv: "video/ogg",
  mov: "video/quicktime",
  gif: "image/gif",
  webp: "image/webp",
  apng: "image/apng",
};

/** Animated image formats render via <img>; everything else via <video>/<audio>. */
export function isAnimatedImageFormat(format: string): boolean {
  return format === "gif" || format === "webp" || format === "apng";
}

export function mediaMimeType(kind: MediaKind, format: string): string {
  const mime = MIME_BY_FORMAT[format.toLowerCase()];
  if (!mime) return "application/octet-stream";
  // "webm"/"ogg" are containers for both; keep the declared kind.
  if (kind === "audio" && mime.startsWith("video/")) return mime.replace("video/", "audio/");
  return mime;
}
