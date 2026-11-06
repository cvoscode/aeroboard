import { useMemo, useState } from 'react';
import type { ImageFrame, ImageSeries } from '../hooks/useAeroboard';

interface ImageTimelineViewerProps {
  series: ImageSeries | null;
  getImageSource: (frame: ImageFrame, attempt?: number) => string;
  onPositionSelect?: (frame: ImageFrame) => void;
}

/** A frame shown right after it was logged may not be on disk yet; retry a few times. */
const MAX_FRAME_RETRIES = 3;

function formatTimestamp(tsMs: number): string {
  return new Date(tsMs).toLocaleString();
}

export function ImageTimelineViewer({ series, getImageSource, onPositionSelect }: ImageTimelineViewerProps) {
  const [index, setIndex] = useState(0);
  const [followLatest, setFollowLatest] = useState(true);
  const [failure, setFailure] = useState<{ key: string; attempt: number } | null>(null);

  // `series` is included intentionally — it is the ImageSeries object reference, so any
  // new frame appended by the WS ingestion will trigger a recalculation to show the latest
  // frame when followLatest=true. The `frames.length` guard inside the memo handles emptiness.
  const clampedIndex = useMemo(() => {
    if (!series || series.frames.length === 0) return 0;
    if (followLatest) return series.frames.length - 1;
    return Math.min(index, series.frames.length - 1);
  }, [followLatest, index, series]);

  const frame = series && series.frames.length > 0 ? series.frames[clampedIndex] : undefined;
  const frameKey = frame ? frame.asset_id ?? `${frame.run_id}:${frame.name}:${frame.ts_ms}` : '';
  const attempt = failure?.key === frameKey ? failure.attempt : 0;
  const failed = attempt > MAX_FRAME_RETRIES;

  if (!series || !frame) {
    return (
      <div className="comparison-card empty-state-card">
        <h3>Image timeline</h3>
        <p>Select a run and image stream to scrub through historized frames.</p>
      </div>
    );
  }

  const source = getImageSource(frame, attempt);

  return (
    <div className="comparison-card image-viewer-card">
      <div className="comparison-card-header">
        <div>
          <h3>{series.name}</h3>
          <p>{series.run_id} • frame {clampedIndex + 1} / {series.frames.length}</p>
        </div>
        <span className="image-time-pill">{formatTimestamp(frame.ts_ms)}</span>
        {frame.step != null ? <button type="button" onClick={() => onPositionSelect?.(frame)}>Show step {frame.step} on charts</button> : null}
      </div>
      <div className="image-viewer-stage">
        {failed ? (
          <div className="empty-state-card">
            <h3>Frame unavailable</h3>
            <p>The image for this timestamp could not be loaded.</p>
          </div>
        ) : (
          <img
            key={source}
            src={source}
            alt={`${series.name} at ${formatTimestamp(frame.ts_ms)}`}
            className="image-viewer-frame"
            onError={() => {
              const next = attempt + 1;
              if (next > MAX_FRAME_RETRIES) {
                setFailure({ key: frameKey, attempt: next });
              } else {
                setTimeout(() => setFailure({ key: frameKey, attempt: next }), 500 * next);
              }
            }}
          />
        )}
      </div>
      <div className="image-slider-wrap">
        <input
          type="range"
          min={0}
          max={Math.max(series.frames.length - 1, 0)}
          step={1}
          value={clampedIndex}
          onChange={(event) => {
            setFollowLatest(false);
            setIndex(Number(event.target.value));
          }}
          className="styled-slider"
        />
        <div className="image-slider-labels">
          <span>{formatTimestamp(series.frames[0].ts_ms)}</span>
          <span>{formatTimestamp(series.frames[series.frames.length - 1].ts_ms)}</span>
        </div>
      </div>
    </div>
  );
}
