# Issues Found During Testing (May 8, 2026)

## Testing Environment
- Aeroboard started: `uv run aeroboard start --storage filesystem`
- Server endpoints: HTTP:3000, gRPC Flight:50051
- Test client ran: `examples/lightning_example.py` (completed successfully)
- Training runs completed for: `lightning-eval-filesystem-baseline` and `lightning-eval-filesystem-lr-high`
- Data persisted to: `logs/runs/` directory

---

## Confirmed Issues

### 1. **CRITICAL: UI shows "No run selected" even when runs appear in sidebar**
**Status:** Open
**Severity:** High
**Symptom:** Runs are listed in the sidebar but dashboard shows "Awaiting Telemetry..." instead of displaying metrics after clicking a run
**Root cause:** React state management issue - `selectedRuns` state updates but chart rendering not triggered

### 2. **"Live" indicator shows but data isn't displayed**
**Status:** Open
**Severity:** High
**Root cause:** Same as Issue 1 - WebSocket connection works but data hook doesn't connect run selection to metric display

### 3. **Run version display confusion**
**Status:** Open
**Severity:** Medium
**Symptom:** UI shows run entries as versioned (e.g., "1 version latest v1") but selection logic may not properly account for version suffixes

---

## Wishlist Items
- width setting
- grid where you can drag the plots
- all plots start collapsed, but we can set favorites which are shown

---

## Completed Fixes (Do not remove)
- **TypeScript build errors fixed**: Added `React` import, fixed duplicate export in App.tsx
- **API endpoints verified working**: `/api/runs` and `/api/history?run_id=<run_id>&limit=N` return correct data