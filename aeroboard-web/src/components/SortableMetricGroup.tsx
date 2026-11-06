import type { CSSProperties, ReactNode } from "react";
import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, GripVertical } from "lucide-react";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import type { MetricGroup } from "../lib/metricGroups";

interface SortableMetricGroupProps {
  group: MetricGroup;
  chartCount: number;
  canMoveUp: boolean;
  canMoveDown: boolean;
  onToggleHidden: () => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
  children: ReactNode;
}

export function SortableMetricGroup({ group, chartCount, canMoveUp, canMoveDown, onToggleHidden, onMoveUp, onMoveDown, children }: SortableMetricGroupProps) {
  const { attributes, listeners, setNodeRef, setActivatorNodeRef, transform, transition, isDragging } = useSortable({ id: group.id });
  const style: CSSProperties = {
    transform: CSS.Translate.toString(transform), transition,
    zIndex: isDragging ? 100 : undefined, opacity: isDragging ? 0.8 : 1,
  };

  return (
    <section ref={setNodeRef} style={style} className={`metric-group${group.hidden ? " is-hidden" : ""}`}
      aria-label={`${group.name} metric group`}>
      <header className="metric-group-header">
        <h2>
          <button type="button" className="metric-group-toggle" onClick={onToggleHidden}
            aria-label={`${group.hidden ? "Show" : "Hide"} metric group ${group.name}`} aria-expanded={!group.hidden}>
            {group.hidden ? <ChevronRight size={18} /> : <ChevronDown size={18} />}
            <span>{group.name}</span>
            <small>{chartCount} chart{chartCount === 1 ? "" : "s"}{group.hidden ? " · hidden" : ""}</small>
          </button>
        </h2>
        <div className="metric-group-actions">
          <button type="button" className="chart-icon-btn" aria-label={`Move metric group ${group.name} up`} title="Move group up"
            disabled={!canMoveUp} onClick={onMoveUp}><ArrowUp size={15} /></button>
          <button type="button" className="chart-icon-btn" aria-label={`Move metric group ${group.name} down`} title="Move group down"
            disabled={!canMoveDown} onClick={onMoveDown}><ArrowDown size={15} /></button>
          <button type="button" ref={setActivatorNodeRef} className="chart-icon-btn metric-group-drag"
            {...attributes} {...listeners} aria-label={`Drag metric group ${group.name}`} title="Drag group">
            <GripVertical size={17} />
          </button>
        </div>
      </header>
      {group.hidden ? null : children}
    </section>
  );
}
