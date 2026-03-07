import { useRef } from 'react';

interface DualRangeSliderProps {
  min: number;
  max: number;
  value: [number, number];
  onChange: (range: [number, number]) => void;
}

export function DualRangeSlider({ min, max, value, onChange }: DualRangeSliderProps) {
  const trackRef = useRef<HTMLDivElement>(null);
  const dragging = useRef<'left' | 'right' | null>(null);

  const toPercent = (v: number) => ((v - min) / (max - min)) * 100;
  const fromPercent = (pct: number) => min + (max - min) * Math.max(0, Math.min(1, pct / 100));

  const getEventPct = (e: MouseEvent | TouchEvent) => {
    if (!trackRef.current) return 0;
    const rect = trackRef.current.getBoundingClientRect();
    const clientX = 'touches' in e ? e.touches[0].clientX : e.clientX;
    return ((clientX - rect.left) / rect.width) * 100;
  };

  const startDrag = (thumb: 'left' | 'right') => (e: React.MouseEvent | React.TouchEvent) => {
    e.preventDefault();
    dragging.current = thumb;

    const onMove = (ev: MouseEvent | TouchEvent) => {
      if (!dragging.current) return;
      const pct = getEventPct(ev);
      const rawVal = fromPercent(pct);
      if (dragging.current === 'left') {
        onChange([Math.min(rawVal, value[1] - (max - min) * 0.005), value[1]]);
      } else {
        onChange([value[0], Math.max(rawVal, value[0] + (max - min) * 0.005)]);
      }
    };

    const onUp = () => {
      dragging.current = null;
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('touchmove', onMove);
      window.removeEventListener('mouseup', onUp);
      window.removeEventListener('touchend', onUp);
    };

    window.addEventListener('mousemove', onMove);
    window.addEventListener('touchmove', onMove);
    window.addEventListener('mouseup', onUp);
    window.addEventListener('touchend', onUp);
  };

  const leftPct = toPercent(value[0]);
  const rightPct = toPercent(value[1]);

  const fmt = (v: number) => {
    if (max > 10000) return v.toFixed(0);
    if (max > 100) return v.toFixed(1);
    return v.toFixed(2);
  };

  return (
    <div className="dual-slider">
      <div className="dual-slider-track" ref={trackRef}>
        <div className="dual-slider-fill" style={{ left: `${leftPct}%`, right: `${100 - rightPct}%` }} />
        <div
          className="dual-slider-thumb"
          style={{ left: `${leftPct}%` }}
          onMouseDown={startDrag('left')}
          onTouchStart={startDrag('left')}
        />
        <div
          className="dual-slider-thumb"
          style={{ left: `${rightPct}%` }}
          onMouseDown={startDrag('right')}
          onTouchStart={startDrag('right')}
        />
      </div>
      <div className="dual-slider-labels">
        <span>{fmt(value[0])}</span>
        <span>{fmt(value[1])}</span>
      </div>
    </div>
  );
}
