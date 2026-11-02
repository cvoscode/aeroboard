import { memo, useEffect, useRef, useState } from "react";
import { Film, Music } from "lucide-react";

import type { MediaClip, MediaSeries } from "../hooks/useAeroboard";
import { isAnimatedImageFormat } from "../lib/media";

interface MediaPanelProps {
  series: MediaSeries[];
  getMediaSource: (clip: MediaClip, attempt?: number) => string;
}

const MAX_LOAD_ATTEMPTS = 3;
/** Wait for the slider to settle before mounting a player: each mount fetches clip metadata. */
const SCRUB_SETTLE_MS = 150;

function formatTimestamp(tsMs: number): string {
  return new Date(tsMs).toLocaleString();
}

export default function MediaPanel({ series, getMediaSource }: MediaPanelProps) {
  return (
    <div className="media-grid">
      {series.map((entry) => (
        <MediaSeriesCard
          key={`${entry.run_id}:${entry.kind}:${entry.name}`}
          series={entry}
          clipCount={entry.clips.length}
          getMediaSource={getMediaSource}
        />
      ))}
    </div>
  );
}

interface MediaSeriesCardProps {
  series: MediaSeries;
  /** Passed separately so memo() re-renders when live clips are appended in place. */
  clipCount: number;
  getMediaSource: (clip: MediaClip, attempt?: number) => string;
}

const MediaSeriesCard = memo(function MediaSeriesCard({ series, clipCount, getMediaSource }: MediaSeriesCardProps) {
  const [index, setIndex] = useState(0);
  const [followLatest, setFollowLatest] = useState(true);

  const clampedIndex = followLatest ? clipCount - 1 : Math.min(index, clipCount - 1);
  const [playerIndex, setPlayerIndex] = useState(clampedIndex);

  useEffect(() => {
    if (playerIndex === clampedIndex) return;
    const timer = window.setTimeout(() => setPlayerIndex(clampedIndex), SCRUB_SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [clampedIndex, playerIndex]);

  const clip = series.clips[clampedIndex];
  const playerClip = series.clips[Math.min(playerIndex, clipCount - 1)] ?? clip;
  if (!clip || !playerClip) return null;

  const newerClips = clipCount - 1 - clampedIndex;
  // Starting playback pins the clip, so a newly logged clip can't swap the player mid-play.
  const pinPlayingClip = () => {
    if (!followLatest) return;
    setIndex(playerIndex);
    setFollowLatest(false);
  };

  const KindIcon = series.kind === "audio" ? Music : Film;

  return (
    <div className="comparison-card media-card">
      <div className="comparison-card-header">
        <div>
          <h3 className="media-card-title">
            <KindIcon size={16} aria-hidden="true" /> {series.name}
          </h3>
          <p>
            {series.run_id} • clip {clampedIndex + 1} / {clipCount} • {clip.format}
          </p>
        </div>
        <div className="media-card-meta">
          <span className="image-time-pill">{formatTimestamp(clip.ts_ms)}</span>
          {!followLatest && newerClips > 0 ? (
            <button type="button" className="selector-action-btn" onClick={() => setFollowLatest(true)}>
              Latest ({newerClips} newer)
            </button>
          ) : null}
        </div>
      </div>

      <div className={`media-stage media-stage-${series.kind}`}>
        <MediaPlayer key={`${playerClip.ts_ms}`} clip={playerClip} getMediaSource={getMediaSource} onPlay={pinPlayingClip} />
      </div>

      {clipCount > 1 ? (
        <div className="image-slider-wrap">
          <input
            type="range"
            min={0}
            max={clipCount - 1}
            step={1}
            value={clampedIndex}
            aria-label={`${series.name} clip`}
            onChange={(event) => {
              const next = Number(event.target.value);
              setIndex(next);
              setFollowLatest(next === clipCount - 1);
            }}
            className="styled-slider"
          />
          <div className="image-slider-labels">
            <span>{formatTimestamp(series.clips[0].ts_ms)}</span>
            <span>{formatTimestamp(series.clips[clipCount - 1].ts_ms)}</span>
          </div>
        </div>
      ) : null}
    </div>
  );
});

interface MediaPlayerProps {
  clip: MediaClip;
  getMediaSource: (clip: MediaClip, attempt?: number) => string;
  onPlay: () => void;
}

function MediaPlayer({ clip, getMediaSource, onPlay }: MediaPlayerProps) {
  // A live clip can be announced over the WebSocket a moment before the server
  // has finished writing it to disk, so retry a few times before giving up.
  const [attempt, setAttempt] = useState(0);
  const [failed, setFailed] = useState(false);

  const retryTimer = useRef<number | null>(null);
  useEffect(() => () => {
    if (retryTimer.current !== null) window.clearTimeout(retryTimer.current);
  }, []);

  const handleError = () => {
    if (clip.objectUrl || attempt + 1 >= MAX_LOAD_ATTEMPTS) {
      setFailed(true);
      return;
    }
    retryTimer.current = window.setTimeout(() => setAttempt((value) => value + 1), 400 * (attempt + 1));
  };

  if (failed) {
    return (
      <div className="empty-state-card">
        <h3>Clip unavailable</h3>
        <p>The browser could not load this {clip.kind} clip ({clip.format}). Check that the format is browser-playable.</p>
      </div>
    );
  }

  const src = getMediaSource(clip, attempt);
  const label = `${clip.name} at ${formatTimestamp(clip.ts_ms)}`;

  if (isAnimatedImageFormat(clip.format)) {
    return <img src={src} alt={label} className="media-player" loading="lazy" decoding="async" onError={handleError} />;
  }

  if (clip.kind === "audio") {
    return <audio src={src} controls preload="metadata" className="media-player" aria-label={label} onError={handleError} onPlay={onPlay} />;
  }

  return (
    <video src={src} controls preload="metadata" playsInline className="media-player" aria-label={label} onError={handleError} onPlay={onPlay} />
  );
}
