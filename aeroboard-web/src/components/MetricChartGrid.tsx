import { DndContext, KeyboardSensor, PointerSensor, closestCenter, pointerWithin, useSensor, useSensors, type CollisionDetection, type DragEndEvent } from "@dnd-kit/core";
import { SortableContext, rectSortingStrategy, sortableKeyboardCoordinates, verticalListSortingStrategy } from "@dnd-kit/sortable";

import { memo, useCallback } from "react";
import { MetricChartCard } from "./MetricChartCard";
import { useChartSettings } from "./chartSettingsContext";
import { chartBaseName } from "../lib/dashboardDefaults";
import type { MetricGroup } from "../lib/metricGroups";
import { SortableMetricGroup } from "./SortableMetricGroup";

/** Cards differ in height, so the drop follows the pointer; the keyboard has none and uses card centers. */
const dropTarget: CollisionDetection = (args) => {
  const underPointer = pointerWithin(args);
  return underPointer.length > 0 ? underPointer : closestCenter(args);
};

interface MetricChartGridProps {
  visibleMetricKeys: string[];
  metricGroups: MetricGroup[];
  metricSearch: string;
  onToggleHiddenGroup: (id: string) => void;
  onReorderGroup: (active: string, over: string) => void;
  groupedMetrics: Record<string, string[]>;
}

export const MetricChartGrid = memo(function MetricChartGrid({
  visibleMetricKeys, metricGroups, metricSearch, onToggleHiddenGroup, onReorderGroup, groupedMetrics,
}: MetricChartGridProps) {
  const {settings,actions}=useChartSettings();
  const widthMode=settings.widthMode;
  const sensors = useSensors(
    // The card header starts drags too, so a plain click on it must not count as one.
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const handleDragEnd=useCallback((event:DragEndEvent)=>{
    const {active,over}=event;
    if(over&&active.id!==over.id)actions.reorder(String(active.id),String(over.id));
  },[actions]);

  const renderCharts = (keys: string[]) => (
    <DndContext sensors={sensors} collisionDetection={dropTarget} onDragEnd={handleDragEnd}>
      <SortableContext items={keys} strategy={rectSortingStrategy}>
        <div className={`grid-layout grid-layout--${widthMode}`}>
          {keys.map((groupKey) => {
            const targets = groupedMetrics[groupKey];
            if (!targets) return null;

            return <MetricChartCard key={groupKey} groupKey={groupKey} targets={targets} />;
          })}
        </div>
      </SortableContext>
    </DndContext>
  );

  if (!metricGroups.length) return renderCharts(visibleMetricKeys);

  const assigned = new Set(metricGroups.flatMap(group => group.metrics));
  const ungrouped = visibleMetricKeys.filter(key => !assigned.has(chartBaseName(key)));
  const needle = metricSearch.trim().toLowerCase();
  const sections = metricGroups.map(group => ({
    group, keys: visibleMetricKeys.filter(key => group.metrics.includes(chartBaseName(key))),
  })).filter(({ group, keys }) => !needle || keys.length > 0 || group.name.toLowerCase().includes(needle));

  return (
    <div className="metric-group-stack">
      <DndContext sensors={sensors} collisionDetection={dropTarget} onDragEnd={({ active, over }) => {
        if (over && active.id !== over.id) onReorderGroup(String(active.id), String(over.id));
      }}>
        <SortableContext items={sections.map(({ group }) => group.id)} strategy={verticalListSortingStrategy}>
          {sections.map(({ group, keys }, index) => (
            <SortableMetricGroup key={group.id} group={group} chartCount={keys.length}
              canMoveUp={index > 0} canMoveDown={index < sections.length - 1}
              onToggleHidden={() => onToggleHiddenGroup(group.id)}
              onMoveUp={() => onReorderGroup(group.id, sections[index - 1].group.id)}
              onMoveDown={() => onReorderGroup(group.id, sections[index + 1].group.id)}>
              {keys.length ? renderCharts(keys) : <p className="metric-group-empty">No visible charts in the selected runs.</p>}
            </SortableMetricGroup>
          ))}
        </SortableContext>
      </DndContext>
      {ungrouped.length ? (
        <section className="metric-group" aria-label="Ungrouped metrics">
          <h2 className="ungrouped-metrics-title">Ungrouped</h2>
          {renderCharts(ungrouped)}
        </section>
      ) : null}
    </div>
  );
});
