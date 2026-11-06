import { useRef, useState } from 'react';
import { createPortal } from 'react-dom';

import { RUN_COLORS } from '../lib/runColors';

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
        <p className="color-modal-title">Run color</p>
        <div className="color-swatch-grid">
          {RUN_COLORS.map((paletteColor) => (
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
