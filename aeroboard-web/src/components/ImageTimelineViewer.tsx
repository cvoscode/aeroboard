import { useMemo, useState } from 'react';
import type { ImageSeries } from '../hooks/useAeroboard';

interface ImageTimelineViewerProps {
  series: ImageSeries | null;
}

function formatTimestamp(tsMs: number): string {
  return new Date(tsMs).toLocaleString();
}

export function ImageTimelineViewer({ series }: ImageTimelineViewerProps) {
  const [index, setIndex] = useState(0);

  const clampedIndex = useMemo(() => {
    if (!series || series.frames.length === 0) return 0;
    return Math.min(index, series.frames.length - 1);
  }, [index, series]);

  if (!series || series.frames.length === 0) {
    return (
      <div className="comparison-card empty-state-card">
        <h3>Image Timeline</h3>
        <p>Select a run and image stream to scrub through historized frames.</p>
      </div>
    );
  }

  const frame = series.frames[clampedIndex];
  const dataUrl = `data:image/${frame.format};base64,${frame.image_base64}`;

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
        <img src={dataUrl} alt={`${series.name} at ${formatTimestamp(frame.ts_ms)}`} className="image-viewer-frame" />
      </div>
      <div className="image-slider-wrap">
        <input
          type="range"
          min={0}
          max={Math.max(series.frames.length - 1, 0)}
          step={1}
          value={clampedIndex}
          onChange={(event) => setIndex(Number(event.target.value))}
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
