import { useEffect, useMemo, useState } from 'react';
import type { ImageSeries } from '../hooks/useAeroboard';

interface ImageTimelineViewerProps {
  series: ImageSeries | null;
  onRequestFrame: (runId: string, name: string, tsMs: number) => Promise<void>;
}

function formatTimestamp(tsMs: number): string {
  return new Date(tsMs).toLocaleString();
}

export function ImageTimelineViewer({ series, onRequestFrame }: ImageTimelineViewerProps) {
  const [index, setIndex] = useState(0);
  const [followLatest, setFollowLatest] = useState(true);
  const [frameError, setFrameError] = useState<string | null>(null);

  // `series` is included intentionally — it is the ImageSeries object reference, so any
  // new frame appended by the WS ingestion will trigger a recalculation to show the latest
  // frame when followLatest=true. The `frames.length` guard inside the memo handles emptiness.
  const clampedIndex = useMemo(() => {
    if (!series || series.frames.length === 0) return 0;
    if (followLatest) return series.frames.length - 1;
    return Math.min(index, series.frames.length - 1);
  }, [followLatest, index, series]);

  if (!series || series.frames.length === 0) {
    return (
      <div className="comparison-card empty-state-card">
        <h3>Image Timeline</h3>
        <p>Select a run and image stream to scrub through historized frames.</p>
      </div>
    );
  }

  const frame = series.frames[clampedIndex];
  const dataUrl = frame.image_base64 ? `data:image/${frame.format};base64,${frame.image_base64}` : null;

  useEffect(() => {
    if (frame.image_base64) return;
    let cancelled = false;

    void (async () => {
      try {
        setFrameError(null);
        await onRequestFrame(frame.run_id, frame.name, frame.ts_ms);
      } catch (error) {
        if (!cancelled) {
          setFrameError(error instanceof Error ? error.message : "Failed to load frame");
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [frame.image_base64, frame.name, frame.run_id, frame.ts_ms, onRequestFrame]);

  return (
    <div className="comparison-card image-viewer-card">
      <div className="comparison-card-header">
        <div>
          <h3>{series.name}</h3>
          <p>{series.run_id} • frame {clampedIndex + 1} / {series.frames.length}</p>
        </div>
        <span className="image-time-pill">{formatTimestamp(frame.ts_ms)}</span>
      </div>
      <div className="image-viewer-stage">
        {dataUrl ? (
          <img src={dataUrl} alt={`${series.name} at ${formatTimestamp(frame.ts_ms)}`} className="image-viewer-frame" />
        ) : (
          <div className="empty-state-card">
            <h3>Loading frame…</h3>
            <p>{frameError ?? "Fetching persisted image bytes for the selected timestamp."}</p>
          </div>
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
