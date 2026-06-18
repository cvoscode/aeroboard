import { ImageTimelineViewer } from './ImageTimelineViewer';
import type { ImageSeries } from '../hooks/useAeroboard';

interface ImagePanelProps {
  series: ImageSeries | null;
  onRequestFrame: (runId: string, name: string, tsMs: number) => Promise<void>;
}

export default function ImagePanel({ series, onRequestFrame }: ImagePanelProps) {
  return (
    <div className="image-section">
      <ImageTimelineViewer
        key={series ? `${series.run_id}:${series.name}` : 'empty-image-series'}
        series={series}
        onRequestFrame={onRequestFrame}
      />
    </div>
  );
}
