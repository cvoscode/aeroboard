# Projects, objectives, and recovery

Aeroboard stores run metadata, sessions, metric objectives, saved views, and events in a server catalog. Existing runs appear in the **Default** project with an **unknown** execution outcome. Their telemetry stays readable.

## Start and finish a run

```python
import aeroboard_client as aeroboard

with aeroboard.init(
    run_id="baseline",
    project="classifier",
    tags=["baseline", "small-model"],
    metadata={"experiment": "classifier-v1", "dataset": "digits-v2"},
    capture_provenance=True,
    spool_dir="aeroboard-spool",
    dashboard={"starred": ["loss"], "order": ["loss", "accuracy"]},
) as run:
    run.define_metric("val/loss", goal="minimize", target=0.1)
    run.define_metric("val/accuracy", goal="maximize", target=0.95, unit="fraction")
    run.log_hyperparameters({"learning_rate": 0.001, "seed": 42})
    run.log_metrics({"val/loss": 0.08, "val/accuracy": 0.96}, step=100)
    run.log_event("Saved best checkpoint", kind="checkpoint", step=100,
                  details={"path": "checkpoints/best.pt"})
```

A normal context exit records `completed`, an exception records `failed`, and a keyboard interrupt records `cancelled`. You can also call `run.finish("completed")` explicitly. `close()` flushes and closes the transport without inventing a successful execution outcome.

Without a disk outbox, first-use capability negotiation and registration are synchronous. Enter the context or resolve `run.run_id` before the training loop. With `spool_dir`, registration is queued locally and logging can start without contacting the network. `AEROBOARD_STALE_AFTER_MS` configures the server's freshness threshold; the default is 60 seconds.

Heartbeats run independently of telemetry uploads, every 15 seconds. After 60 seconds without a heartbeat, an unfinished run displays **stale**. This indicates lost contact, not a confirmed failure. Browser connectivity cannot keep a training session alive. Execution outcome and telemetry delivery appear separately in run details.

`capture_provenance=True` captures the Git commit, dirty working tree flag, and installed framework versions. It does not collect environment variables or machine credentials. Use `metadata` for explicit dataset and experiment identities. Provenance capture is optional.

## Resume using a stable identity

`run.run_uid` identifies a run independently of its canonical name and editable display name. Copy the resume ID from dashboard run details, or save `run.run_uid` alongside your checkpoint.

```python
with aeroboard.init(run_id=existing_run_uid, resume=True) as run:
    run.log({"val/loss": 0.07}, step=101)
```

Resuming creates a new session attempt. A fresh active writer prevents a concurrent resume. An earlier session's heartbeat or finish cannot change the current session's outcome.

Continue global steps when possible. If checkpoint restoration restarts step numbering, supply a segment name:

```python
with aeroboard.init(run_id=existing_run_uid, resume=True, segment="restart-1") as run:
    run.log({"val/loss": 0.06}, step=0)
```

Aggregation keeps named segments separate. At-step scores across multiple segments remain missing until you select a segment; use `__global__` for the unnamed original sequence. Best and last scores describe the full run unless the API request selects a segment.

## Organize and compare

Open **Runs and project settings** to create projects, edit display names, tags, and notes, archive runs, or compare selected runs. Archiving is reversible. **Delete run** requires a second **Delete permanently** action and retains a tombstone so delayed uploads cannot recreate the deleted run.

The run browser supports combined structured filters. Example fields:

| Field | Example operator/value |
| --- | --- |
| `params.learning_rate` | `lte`, `0.001` |
| `metrics.val/loss.best` | `lt`, `0.1` |
| `metrics.val/accuracy.last` | `gte`, `0.95` |
| `provenance.dataset` | `eq`, `digits-v2` |
| `tags` | `contains`, `baseline` |

Numbers and booleans are typed; plain text is a string. All filters must match. Missing values match `missing`, and do not satisfy numeric comparisons.

Objectives apply to full metric names such as `train/loss` and `val/loss`. Project definitions provide defaults; a run definition overrides its project definition. Chart target edits affect presentation, while objective targets determine ranking and first target crossing.

Scorecards show the best finite value, its step, the last observation, and elapsed captured wall time to the first target crossing. Equal best values prefer the earliest observation. NaN and infinity stay diagnostic observations and cannot win a ranking. At-step comparison uses the last finite sample at or before the requested step within the observed segment range; it never extrapolates.

Choose a baseline in the comparison table for direction-aware absolute and percentage improvements. A zero baseline has no percentage delta. Different goals or units suppress incompatible baseline deltas. Configuration differences include hyperparameters and provenance.

Mean curves and **sample standard deviation** bands use exact step matching, with one observation per run at each position. Hover for the contributor count. A single contributor has no variability band. Completed runs form the default population; the aggregation control can explicitly include unfinished runs. Default groups separate project, experiment, dataset, objective, segment, and configuration other than seed. Explicit grouping fields allow deliberate configuration comparisons. Time alignment is available for data without steps.

## Save and share a view

Name the current view and choose **Save view**. Views capture selected runs, stars, order, display targets, removed targets, collapsed charts, metric search, hidden charts, logarithmic axes, smoothing, range, comparison settings, and run-browser filters. **Make default** selects the project's initial view. **Copy view link** shares a URL on the same deployment.

Precedence is browser overrides, saved view, run dashboard settings, then server configuration. **Reset chart layout** clears the current scope's browser overrides and restores inherited settings. Theme remains a browser preference. The Default project offers **Import previous browser layout** for preferences saved by older versions.

Saving uses expected revisions captured when the view is opened. If another browser saves the same view first, reload or save a new copy. **Reload view** clears this scope's browser layout overrides and loads the latest saved settings. A live metadata refresh cannot silently upgrade your expected revision and overwrite someone else's changes. Views and metadata survive persistent server restarts; live-only workspaces identify their ephemeral state.

Metric search filters chart names without losing their settings. A per-chart logarithmic axis omits nonpositive samples and identifies invalid log targets. PNG export includes the visible chart, title, legend, and target lines. **Export raw CSV** streams selected raw observations, including captured time, step, and segment; smoothing does not alter exported values. Export while writers are paused if you need a fixed snapshot across pages.

## Link events and media

```python
asset_id = run.log_image("prediction", image, step=100)
run.log_event("Reviewed prediction", kind="note", step=100,
              media_ids=[asset_id] if asset_id else [])
```

Image, audio, and video methods accept optional `step` and `segment`, and return a stable asset ID on capable servers. Legacy servers return `None`. Clips with the same stream name and millisecond keep distinct IDs. Events can reference media that arrives later. Selecting an event marker opens its details and attachment links; media with a step offers **Show step on charts**. Heavy media bytes load on demand.

Dashboard annotations can be added, edited, and deleted. SDK event IDs are idempotent even after an annotation is edited. Event kinds are `note`, `checkpoint`, `learning_rate`, and `warning`. Checkpoints are references and metadata; model weights are not uploaded by `log_event`.

## Capture offline and recover

Set `spool_dir` to enable a disk outbox. Each SDK session owns a separate directory under that root. Logging queues work in bounded memory; a local worker commits Arrow IPC and control operations to SQLite with synchronous durability. A separate worker performs network delivery.

```python
assert run.flush(local_only=True)  # wait for local durable commit
print(run.delivery_status())
delivered = run.flush()           # also attempt remote acknowledgement
```

Ordinary logging has an uncommitted memory window. Only a successful local flush establishes local durability. Normal `flush()` returns `False` on delivery failure or timeout. `close()` retains locally committed, unacknowledged items for recovery. On quota or disk failure, new capture can fail while training continues; inspect `local_failures`, `dropped_rows`, pending rows, and rejected entries. Committed unacknowledged entries are never silently evicted. The default quota is 1 GiB of queued payloads per session, plus SQLite overhead; configure it with `max_spool_bytes`.

After the process stops:

```bash
aeroboard sync --spool-dir aeroboard-spool --status
aeroboard sync --spool-dir aeroboard-spool --timeout 60
```

Only one process may own an outbox. Recovery preserves identities, ordering, captured timestamps, definitions, events, and finish watermarks. It does not replay historical heartbeats as fresh contact. Corrupt or permanently rejected entries are quarantined with diagnostics and block dependent operations. The command exits unsuccessfully if recovery leaves pending or rejected work.

An offline first start records the configured endpoints. The first successful handshake binds the outbox to the persistent server-store identity. Further recovery refuses a different store at that endpoint. Keep the server catalog together with its telemetry in backups. Disk outboxes require a server advertising `durable_replay`; live-only mode and older servers cannot provide that guarantee.

The delivery guarantee is **at least once with idempotent visible effects**. Filesystem batches use immutable batch-ID files; Greptime batches use an immutable batch-ID table. Retrying after a lost receipt or acknowledgement does not count an observation twice. See the [storage model](../developer-guide/storage-model.md) and [verification workflow](../developer-guide/development-workflow.md).
