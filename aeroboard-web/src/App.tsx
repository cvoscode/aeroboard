import { useMemo, useState, useEffect, useRef, useCallback } from 'react';
import { createPortal } from 'react-dom';
import {
  Activity, List, Clock, RotateCcw, Hash,
  GripVertical, ChevronDown, ChevronRight, Sliders,
  Maximize2, Layers, Search, Settings2, Columns, Link2
} from 'lucide-react';
import {
  DndContext, closestCenter, KeyboardSensor, PointerSensor,
  useSensor, useSensors, type DragEndEvent
} from '@dnd-kit/core';
import {
  arrayMove, SortableContext, sortableKeyboardCoordinates,
  verticalListSortingStrategy, useSortable
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';

import { useAeroboard } from './hooks/useAeroboard';
import type { TimeSeriesData } from './hooks/useAeroboard';
import { LineChart } from './components/LineChart';
import type { XAxisType } from './components/LineChart';
import './App.css';

// --- Custom Color Picker (Portal Modal) ---

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

function ColorPicker({ color, onChange }: ColorPickerProps) {
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
        onClick={e => e.stopPropagation()}
      >
        <p className="color-modal-title">Trace Color</p>
        <div className="color-swatch-grid">
          {PALETTE.map(c => (
            <button
              key={c}
              className={`swatch-btn ${c === color ? 'active' : ''}`}
              style={{ backgroundColor: c }}
              onClick={() => { onChange(c); setOpen(false); }}
              title={c}
            />
          ))}
        </div>
        <label className="custom-hex-row">
          <span>Custom HEX</span>
          <input type="color" value={color} onChange={e => onChange(e.target.value)} />
        </label>
      </div>
    </>,
    document.body
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

// --- Custom Dual Range Slider (JS-driven, no CSS overlap issues) ---

interface DualRangeSliderProps {
  min: number;
  max: number;
  value: [number, number];
  onChange: (range: [number, number]) => void;
}

function DualRangeSlider({ min, max, value, onChange }: DualRangeSliderProps) {
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

// --- SortableChart ---

interface SortableChartProps {
  id: string;
  groupKey: string;
  seriesData: { name: string; fullData: TimeSeriesData; color?: string }[];
  xAxisType: XAxisType;
  smoothing: number;
  removeOutliers: boolean;
  xRange: [number, number] | null;
  sharedXRange: [number, number] | null;
  onRangeChange: (range: [number, number]) => void;
  onToggleCollapse: () => void;
  collapsed: boolean;
}

function SortableChart({ id, groupKey, seriesData, xAxisType, smoothing, removeOutliers, xRange, sharedXRange, onRangeChange, onToggleCollapse, collapsed }: SortableChartProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    zIndex: isDragging ? 100 : 1,
    opacity: isDragging ? 0.8 : 1
  };

  return (
    <div ref={setNodeRef} style={style} className={`chart-card ${collapsed ? 'collapsed' : ''}`}>
      <div className="chart-card-glow" />
      <div className="chart-header">
        <div className="chart-header-left">
          <button className="collapse-toggle" onClick={onToggleCollapse}>
            {collapsed ? <ChevronRight size={18} /> : <ChevronDown size={18} />}
          </button>
          <div className="title-area">
            <h3 className="chart-title">{groupKey.toUpperCase()}</h3>
            <small className="chart-meta">{seriesData.length} traces</small>
          </div>
        </div>

        <div className="chart-legend">
          {seriesData.map((s) => (
            <div key={s.name} className="legend-item">
              <span className="legend-dot" style={{ backgroundColor: s.color || '#00d2ff' }} />
              <span className="legend-val" style={{ color: s.color || 'var(--accent-secondary)' }}>
                {s.fullData.values[s.fullData.count - 1]?.toFixed(4)}
              </span>
              <span className="legend-label">{s.name}</span>
            </div>
          ))}
        </div>

        <div className="drag-handle-subtle" {...attributes} {...listeners}>
          <GripVertical size={16} />
        </div>
      </div>

      {!collapsed && (
        <div className="plot-container">
          <LineChart
            title={groupKey}
            seriesData={seriesData}
            xAxisType={xAxisType}
            smoothing={smoothing}
            removeOutliers={removeOutliers}
            xRange={xRange}
            sharedXRange={sharedXRange}
            onRangeChange={onRangeChange}
            height={280}
          />
        </div>
      )}
    </div>
  );
}

// --- Main App ---

const DEFAULT_COLORS = ['#00d2ff', '#ff4c6b', '#00e676', '#ffeb3b', '#e040fb', '#ff9800', '#9c27b0'];
const STAGE_COLORS: Record<string, string> = {
  'train': '#00d2ff',
  'val': '#ff4c6b',
  'test': '#00e676'
};

export default function App() {
  const { connected, getChartData, getMetricsForRun, getGlobalXBounds, runs, tick } = useAeroboard();

  const [mode, setMode] = useState<'single' | 'compare'>('single');
  const [selectedRuns, setSelectedRuns] = useState<string[]>([]);
  const [runColors, setRunColors] = useState<Record<string, string>>({});

  const [xAxisType, setXAxisType] = useState<XAxisType>('step');
  const [smoothing, setSmoothing] = useState(0);
  const [removeOutliers, setRemoveOutliers] = useState(false);
  const [globalXRange, setGlobalXRange] = useState<[number, number] | null>(null);
  const [syncXAxis, setSyncXAxis] = useState(false);
  const [syncedRange, setSyncedRange] = useState<[number, number] | null>(null);

  const [collapsedCharts, setCollapsedCharts] = useState<Record<string, boolean>>({});
  const [orderedMetrics, setOrderedMetrics] = useState<string[]>([]);

  const sensors = useSensors(
    useSensor(PointerSensor),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates })
  );

  // Assign default colors to runs
  useEffect(() => {
    setRunColors(prev => {
      const next = { ...prev };
      let changed = false;
      runs.forEach((r, i) => {
        if (!next[r]) { next[r] = DEFAULT_COLORS[i % DEFAULT_COLORS.length]; changed = true; }
      });
      return changed ? next : prev;
    });
  }, [runs]);

  useEffect(() => {
    if (selectedRuns.length === 0 && runs.length > 0) setSelectedRuns([runs[0]]);
  }, [runs, selectedRuns]);

  // Reset range when axis mode changes
  useEffect(() => {
    setGlobalXRange(null);
    setSyncedRange(null);
  }, [xAxisType]);

  const bounds = useMemo(
    () => getGlobalXBounds(selectedRuns, xAxisType),
    [selectedRuns, xAxisType, getGlobalXBounds, tick]
  );

  // Effective window = user-set globalXRange, else full bounds
  const xRangeToUse = globalXRange;

  // Callback for charts to broadcast their zoom to all others
  const handleRangeChange = useCallback((range: [number, number]) => {
    if (syncXAxis) setSyncedRange(range);
  }, [syncXAxis]);

  const groupedMetrics = useMemo(() => {
    if (selectedRuns.length === 0) return {};
    const groups: Record<string, string[]> = {};
    if (mode === 'single') {
      const run = selectedRuns[0];
      getMetricsForRun(run).forEach(m => {
        const parts = m.split('/');
        const base = parts.length > 1 ? parts[1] : parts[0];
        if (base === 'step') return;
        if (!groups[base]) groups[base] = [];
        groups[base].push(m);
      });
    } else {
      selectedRuns.forEach(run => {
        getMetricsForRun(run).forEach(m => {
          if (m === 'step' || m.endsWith('/step')) return;
          if (!groups[m]) groups[m] = [];
          if (!groups[m].includes(m)) groups[m].push(m);
        });
      });
    }
    return groups;
  }, [mode, selectedRuns, getMetricsForRun, tick]);

  const groupKeys = useMemo(() => Object.keys(groupedMetrics).sort(), [groupedMetrics]);

  useEffect(() => {
    setOrderedMetrics(prev => {
      const keys = new Set(groupKeys);
      return [...prev.filter(k => keys.has(k)), ...groupKeys.filter(k => !new Set(prev).has(k))];
    });
  }, [groupKeys]);

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (over && active.id !== over.id) {
      setOrderedMetrics(items => {
        const oldIdx = items.indexOf(active.id as string);
        const newIdx = items.indexOf(over.id as string);
        return arrayMove(items, oldIdx, newIdx);
      });
    }
  };

  const toggleRun = (runId: string) => {
    if (mode === 'single') setSelectedRuns([runId]);
    else setSelectedRuns(prev => prev.includes(runId) ? prev.filter(r => r !== runId) : [...prev, runId]);
  };

  const changeMode = (m: 'single' | 'compare') => {
    setMode(m);
    if (m === 'single') setSelectedRuns(runs.slice(0, 1));
  };

  return (
    <div className="layout-root">
      <aside className="sidebar">
        <div className="sidebar-header">
          <div className="logo-box"><Activity size={24} color="#00d2ff" /></div>
          <h1 className="logo-text">Aeroboard</h1>
        </div>

        <div className="sidebar-section">
          <div className="section-header"><Layers size={14} /><h3>Mode</h3></div>
          <div className="mode-toggle">
            <button className={mode === 'single' ? 'active' : ''} onClick={() => changeMode('single')}>
              <Search size={14} /> Single
            </button>
            <button className={mode === 'compare' ? 'active' : ''} onClick={() => changeMode('compare')}>
              <Columns size={14} /> Compare
            </button>
          </div>
        </div>

        <div className="sidebar-section">
          <div className="section-header"><List size={14} /><h3>Experiments</h3></div>
          <div className="run-list scrollbar-hidden">
            {runs.map(run => (
              <div key={run} className={`run-card-wrapper ${selectedRuns.includes(run) ? 'active' : ''}`}>
                <button className="run-card-btn" onClick={() => toggleRun(run)}>
                  <div className="run-card-indicator" style={{ backgroundColor: runColors[run] }} />
                  <span className="run-name">{run}</span>
                </button>
                {mode === 'compare' && (
                  <ColorPicker
                    color={runColors[run] || '#00d2ff'}
                    onChange={c => setRunColors(p => ({ ...p, [run]: c }))}
                  />
                )}
              </div>
            ))}
            {runs.length === 0 && <p className="empty-hint">No runs detected</p>}
          </div>
        </div>

        <div className="sidebar-section">
          <div className="section-header"><Settings2 size={14} /><h3>View Configuration</h3></div>
          <div className="control-card glass-card">
            <label>X-Axis Mode</label>
            <div className="axis-tabs">
              <button className={xAxisType === 'wall' ? 'active' : ''} onClick={() => setXAxisType('wall')}><Clock size={14} /></button>
              <button className={xAxisType === 'rel' ? 'active' : ''} onClick={() => setXAxisType('rel')}><RotateCcw size={14} /></button>
              <button className={xAxisType === 'step' ? 'active' : ''} onClick={() => setXAxisType('step')}><Hash size={14} /></button>
            </div>

            <div className="control-item">
              <div className="label-row">
                <Sliders size={12} /> smoothing <span className="val">{Math.round(smoothing * 100)}%</span>
              </div>
              <input type="range" min="0" max="0.95" step="0.05" value={smoothing} onChange={e => setSmoothing(parseFloat(e.target.value))} className="styled-slider" />
            </div>

            <div className="control-item row">
              <label className="checkbox-wrap">
                <input type="checkbox" checked={removeOutliers} onChange={e => setRemoveOutliers(e.target.checked)} />
                <div className={`check-box ${removeOutliers ? 'checked' : ''}`}><div className="inner" /></div>
                <span>Remove Outliers</span>
              </label>
            </div>

            <div className="control-item row">
              <label className="checkbox-wrap">
                <input type="checkbox" checked={syncXAxis} onChange={e => setSyncXAxis(e.target.checked)} />
                <div className={`check-box ${syncXAxis ? 'checked' : ''}`}><div className="inner" /></div>
                <span><Link2 size={12} style={{ marginRight: 4 }} />Sync X Axis</span>
              </label>
            </div>
          </div>
        </div>

        <div className="sidebar-section">
          <div className="section-header"><Maximize2 size={14} /><h3>Active Window</h3></div>
          <div className="control-card glass-card">
            {bounds[0] !== Infinity ? (
              <>
                <DualRangeSlider
                  min={bounds[0]} max={bounds[1]}
                  value={globalXRange ?? bounds}
                  onChange={setGlobalXRange}
                />
                <button className="reset-range-btn" onClick={() => { setGlobalXRange(null); setSyncedRange(null); }}>
                  Reset Window
                </button>
              </>
            ) : (
              <p className="empty-hint">Waiting for data...</p>
            )}
          </div>
        </div>
      </aside>

      <div className="main-viewport">
        <header className="main-header">
          <div className="header-info">
            <h2 className="viewport-title">
              {mode === 'single' ? (selectedRuns[0] ?? 'No run selected') : `${selectedRuns.length} Runs Compared`}
            </h2>
            <div className={`connection-badge ${connected ? 'is-live' : ''}`}>
              <div className="pulse-dot" />
              <span>{connected ? 'LIVE' : 'OFFLINE'}</span>
            </div>
          </div>
        </header>

        <section className="dashboard-content">
          {selectedRuns.length === 0 ? (
            <div className="placeholder-hero">
              <Activity size={64} className="floating-icon" />
              <h2>Awaiting Telemetry...</h2>
              <p>Start a training run to see metrics here.</p>
            </div>
          ) : orderedMetrics.length === 0 ? (
            <div className="placeholder-hero">
              <h2>Ingesting Metrics...</h2>
            </div>
          ) : (
            <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
              <SortableContext items={orderedMetrics} strategy={verticalListSortingStrategy}>
                <div className="grid-layout">
                  {orderedMetrics.map((groupKey) => {
                    const targets = groupedMetrics[groupKey];
                    if (!targets) return null;

                    let seriesData: { name: string; fullData: TimeSeriesData; color?: string }[] = [];
                    if (mode === 'single') {
                      const run = selectedRuns[0];
                      seriesData = targets.map(m => {
                        const stage = m.split('/')[0];
                        const data = getChartData(run, m);
                        if (!data) return null;
                        return { name: stage, fullData: data, color: STAGE_COLORS[stage] };
                      }).filter((s): s is { name: string; fullData: TimeSeriesData; color: string } => s !== null);
                    } else {
                      seriesData = selectedRuns.map(runId => {
                        const data = getChartData(runId, groupKey);
                        if (!data) return null;
                        return { name: runId, fullData: data, color: runColors[runId] };
                      }).filter((s): s is { name: string; fullData: TimeSeriesData; color: string } => s !== null);
                    }

                    if (seriesData.length === 0) return null;

                    return (
                      <SortableChart
                        key={groupKey} id={groupKey} groupKey={groupKey}
                        seriesData={seriesData} xAxisType={xAxisType}
                        smoothing={smoothing} removeOutliers={removeOutliers}
                        xRange={xRangeToUse}
                        sharedXRange={syncXAxis ? syncedRange : null}
                        onRangeChange={handleRangeChange}
                        collapsed={collapsedCharts[groupKey]}
                        onToggleCollapse={() => setCollapsedCharts(p => ({ ...p, [groupKey]: !p[groupKey] }))}
                      />
                    );
                  })}
                </div>
              </SortableContext>
            </DndContext>
          )}
        </section>
      </div>
    </div>
  );
}
