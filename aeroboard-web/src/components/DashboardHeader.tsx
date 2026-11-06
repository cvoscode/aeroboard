import { Menu, X } from "lucide-react";
import type { ReactNode } from "react";

interface DashboardHeaderProps {
  connected: boolean;
  effectiveActiveTab: "scalars" | "images" | "media" | "analysis";
  effectiveSelectedRuns: string[];
  visibleTabs: { id: "scalars" | "images" | "media" | "analysis"; label: string }[];
  activeTab: "scalars" | "images" | "media" | "analysis";
  onTabChange: (tab: "scalars" | "images" | "media" | "analysis") => void;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  deleteError: string | null;
  onDismissDeleteError: () => void;
  children?: ReactNode;
}

export function DashboardHeader({
  connected,
  effectiveActiveTab,
  effectiveSelectedRuns,
  visibleTabs,
  activeTab,
  onTabChange,
  sidebarOpen,
  onToggleSidebar,
  deleteError,
  onDismissDeleteError,
  children,
}: DashboardHeaderProps) {
  const title = effectiveSelectedRuns.length > 1 || effectiveActiveTab === "analysis"
    ? `${effectiveSelectedRuns.length} ${effectiveSelectedRuns.length === 1 ? "run" : "runs"} selected`
    : effectiveSelectedRuns[0] ?? "No run selected";

  return (
    <header className="main-header">
      <div className="main-header-content">
        <div className="header-info">
          <button
            type="button"
            className="sidebar-toggle-btn"
            onClick={onToggleSidebar}
            aria-expanded={sidebarOpen}
            aria-controls="dashboard-sidebar"
          >
            {sidebarOpen ? <X size={16} /> : <Menu size={16} />}
            <span>Runs</span>
          </button>

          <h2 className="viewport-title" title={effectiveSelectedRuns.join(", ")}>{title}</h2>

          <div className={`connection-badge ${connected ? "is-live" : ""}`}>
            <div className="pulse-dot" />
            <span>{connected ? "Live" : "Offline"}</span>
          </div>
        </div>

        {children}

        <div className="page-tabs" role="tablist" aria-label="Dashboard pages">
          {visibleTabs.map((tab) => (
            <button
              key={tab.id}
              type="button"
              role="tab"
              aria-selected={activeTab === tab.id}
              className={`page-tab-btn ${activeTab === tab.id ? "active" : ""}`}
              onClick={() => onTabChange(tab.id)}
            >
              {tab.label}
            </button>
          ))}
        </div>
      </div>

      {deleteError ? (
        <div className="status-banner status-banner--error" role="status">
          <p>{deleteError}</p>
          <button type="button" onClick={onDismissDeleteError}>
            Dismiss
          </button>
        </div>
      ) : null}
    </header>
  );
}
