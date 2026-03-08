import { ImageTimelineViewer } from './ImageTimelineViewer';
import type { ImageSeries } from '../hooks/useAeroboard';

interface ImagePanelProps {
  series: ImageSeries | null;
}

export default function ImagePanel({ series }: ImagePanelProps) {
  return (
    <div className="image-section">
      <ImageTimelineViewer key={series ? `${series.run_id}:${series.name}` : 'empty-image-series'} series={series} />
    </div>
  );
}
