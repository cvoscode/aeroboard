import { ImageTimelineViewer } from './ImageTimelineViewer';
import type { ImageFrame, ImageSeries } from '../hooks/useAeroboard';

interface ImagePanelProps {
  series: ImageSeries | null;
  getImageSource: (frame: ImageFrame, attempt?: number) => string;
  onPositionSelect?: (frame: ImageFrame) => void;
}

export default function ImagePanel({ series, getImageSource, onPositionSelect }: ImagePanelProps) {
  return (
    <div className="image-section">
      <ImageTimelineViewer
        key={series ? `${series.run_id}:${series.name}` : 'empty-image-series'}
        series={series}
        getImageSource={getImageSource}
        onPositionSelect={onPositionSelect}
      />
    </div>
  );
}
