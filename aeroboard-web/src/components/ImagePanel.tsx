import { ImageTimelineViewer } from './ImageTimelineViewer';
import type { ImageSeries } from '../hooks/useAeroboard';

interface ImagePanelProps {
  series: ImageSeries | null;
}

export default function ImagePanel({ series }: ImagePanelProps) {
  return (
    <div className="image-section">
      <ImageTimelineViewer series={series} />
    </div>
  );
}
