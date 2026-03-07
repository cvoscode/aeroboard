import { useRef, useState } from 'react';
import { createPortal } from 'react-dom';

const PALETTE = [
  '#00d2ff', '#00e676', '#ff4c6b', '#ffeb3b', '#e040fb',
  '#ff9800', '#9c27b0', '#60efff', '#ff6b9d', '#c3f584',
  '#ffd166', '#06d6a0', '#ef476f', '#118ab2', '#5d7af6',
  '#f8edeb', '#b7b7a4', '#a8dadc', '#e76f51', '#ffffff',
];

interface ColorPickerProps {
  color: string;
  onChange: (color: string) => void;
}

export function ColorPicker({ color, onChange }: ColorPickerProps) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState({ top: 0, left: 0 });
  const triggerRef = useRef<HTMLButtonElement>(null);

  const openPicker = () => {
    if (triggerRef.current) {
      const r = triggerRef.current.getBoundingClientRect();
      setPos({ top: r.bottom + 8, left: r.right + 8 });
    }
    setOpen(true);
  };

  const modal = open ? createPortal(
    <>
      <div className="color-modal-backdrop" onClick={() => setOpen(false)} />
      <div
        className="color-modal"
        style={{ top: pos.top, left: pos.left }}
        onClick={(e) => e.stopPropagation()}
      >
        <p className="color-modal-title">Trace Color</p>
        <div className="color-swatch-grid">
          {PALETTE.map((paletteColor) => (
            <button
              key={paletteColor}
              className={`swatch-btn ${paletteColor === color ? 'active' : ''}`}
              style={{ backgroundColor: paletteColor }}
              onClick={() => { onChange(paletteColor); setOpen(false); }}
              title={paletteColor}
            />
          ))}
        </div>
        <label className="custom-hex-row">
          <span>Custom HEX</span>
          <input type="color" value={color} onChange={(e) => onChange(e.target.value)} />
        </label>
      </div>
    </>,
    document.body,
  ) : null;

  return (
    <div className="color-picker-root">
      <button ref={triggerRef} className="color-trigger" onClick={openPicker}>
        <span className="color-trigger-dot" style={{ backgroundColor: color }} />
      </button>
      {modal}
    </div>
  );
}
