import { ImageTimelineViewer } from './ImageTimelineViewer';
import type { ImageFrame, ImageSeries } from '../hooks/useAeroboard';

interface ImagePanelProps {
  series: ImageSeries | null;
  getImageSource: (frame: ImageFrame, attempt?: number) => string;
}

export default function ImagePanel({ series, getImageSource }: ImagePanelProps) {
  return (
    <div className="image-section">
      <ImageTimelineViewer
        key={series ? `${series.run_id}:${series.name}` : 'empty-image-series'}
        series={series}
        getImageSource={getImageSource}
      />
    </div>
  );
}
