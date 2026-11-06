// Reproduce many-axis, sparse-run, and nested-configuration failures using isolated fixtures.
import assert from "node:assert/strict";
import { mkdir, readFile } from "node:fs/promises";
import { tableFromArrays, tableToIPC } from "apache-arrow";
import { createServer } from "vite";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const server = await createServer({ server: { host: "127.0.0.1", port: 0 } });
await server.listen();
const base = server.resolvedUrls.local[0];
const output = process.env.AEROBOARD_BROWSER_OUTPUT || "/tmp/aeroboard-analysis-browser";
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true, ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
const context = await browser.newContext({ viewport: { width: 1440, height: 1100 } });
const page = await context.newPage();
const errors = [];
page.on("pageerror", error => errors.push(String(error)));
page.on("console", message => {if (message.type() === "error" && message.text().includes("AG Grid")) errors.push(message.text());});
const runIds = ["position_sigreg_units/seed0", "prep/seed0", "reader_corrupt/seed0", "reader_corrupt/seed1"];
const keys = ["encoder", "planner", "_instantiator", "batch_size", "warmup_fraction", ...Array.from({ length: 65 }, (_, index) => `encoder_training_configuration_parameter_${index}`)];
const metrics = ["val/loss"];
const encoder = "{'n_mels': 80, 'dim': 384, 'layers': 8, 'heads': 6, 'dropout': 0.1, 'checkpointing': True, 'channels_last': False}";
const records = runIds.map((run_id, index) => ({ id: `uid-${index}`, parent: "default", revision: 1,
  data: { run_id, display_name: run_id, project_id: "default", status: index === 1 ? "failed" : "completed",
    display_status: index === 1 ? "failed" : "completed", tags: [], notes: "", archived: false,
    metadata_revision: 1, provenance: { git: { commit: index === 0 ? "abc123" : "def456", branch: "main" } } } }));
const summaries = runIds.map((run_id, index) => ({ run_id,
  hyperparameters: [
    { key: "encoder", value: index === 0 ? encoder : encoder.replace("'dropout': 0.1", "'dropout': 0.3"), value_type: "string" },
    { key: "planner", value: "{'dim': 512, 'layers': 6, 'dropout': 0.3, 'memory_attn_dropout': None}", value_type: "string" },
    { key: "_instantiator", value: "lightning.pytorch.cli.instantiate_module", value_type: "string" },
    { key: "batch_size", value: "64", value_type: "int" },
    ...(index === 0 ? [] : [{ key: "warmup_fraction", value: "0.02", value_type: "float" }]),
    ...keys.slice(5).flatMap((key, axis) => axis % 4 === index ? [] : [{ key, value: String((axis + 1) * (index + 1) / 100), value_type: "float" }]),
  ], metric_summaries: [{ name: "val/loss", last: 0.8 + index / 10, min: 0.1 + index / 10, max: 2 }] }));
await context.addInitScript(({ keys, metrics }) => {
  localStorage.setItem("aeroboard-theme", "dark");
  const key = "aeroboard-controls:analysis-browser-test:default:defaults";
  if (!localStorage.getItem(key)) localStorage.setItem(key, JSON.stringify({ mode: "compare", compareHyperparameters: keys, compareMetrics: metrics,
    reducer: "best", baseline: "position_sigreg_units/seed0" }));
}, { keys, metrics });
await page.routeWebSocket("**/ws/live", () => {});
await page.route("**/api/**", async route => {
  const url = new URL(route.request().url());
  const path = url.pathname.slice(4);
  if (path === "/history") {
    const selected = url.searchParams.get("run_id").split(",");
    return route.fulfill({ contentType: "application/vnd.apache.arrow.stream", body: Buffer.from(tableToIPC(tableFromArrays({
      timestamp: Float64Array.from(selected, () => 1_700_000_000_000), run_id: selected,
      name: selected.map(() => "val/loss"), step: Float64Array.from(selected, () => 20), value: Float64Array.from(selected, () => 1),
    }))) });
  }
  const responses = {
    "/capabilities": { server_id: "analysis-browser-test", persistent: true, features: [], durable_replay: true },
    "/projects": [{ id: "default", parent: "", revision: 1, data: { name: "Default", default_view: null } }],
    "/projects/default/views": [], "/metric-definitions": [],
    "/run-records": { rows: records, next_offset: null }, "/runs": { runs: runIds },
    "/runs/summary": { run_summaries: summaries }, "/config": { dashboard: {} },
    "/scorecards": { rows: runIds.map((run_id, index) => ({ run_id, name: "val/loss", goal: "minimize", unit: null,
      last: 0.8 + index / 10, best: 0.1 + index / 10, at_step: url.searchParams.get("step") === "10" ? index / 10 : null })) },
    "/events": { rows: [] }, "/comparison/aggregate": { groups: [], excluded_runs: 1 },
  };
  assert.ok(path in responses, `Unexpected API request: ${path}`);
  return route.fulfill({ json: responses[path] });
});

const table = page.getByRole("region", { name: "Run comparison table", exact: true });
const plot = page.getByRole("region", { name: "Scrollable parallel coordinates", exact: true });
async function metricPoint() {
  return page.locator(`.parallel-run[data-run="${runIds[0]}"] circle title`).filter({ hasText: /val\/loss:/ }).textContent();
}
async function waitForMetric(value) {
  await page.waitForFunction(({ runId, value }) => Array.from(document.querySelectorAll(".parallel-run circle title"))
    .some(node => node.textContent === `${runId} • val/loss: ${value}`), { runId: runIds[0], value });
}
async function noPageOverflow() {
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
  assert.equal(await page.locator(".dashboard-content").evaluate(element => element.scrollWidth <= element.clientWidth + 1), true);
}

try {
  await page.goto(`${base}?runs=${encodeURIComponent(runIds.join(","))}`);
  await page.getByRole("tab", { name: "Hyperparameters", exact: true }).click();
  const rows = table.locator(".ag-row[role='row']");
  await rows.nth(3).waitFor();
  await waitForMetric(0.1);
  assert.equal(await page.locator(".parallel-axis-group").count(), 68);
  assert.equal(await plot.evaluate(element => element.scrollWidth > element.clientWidth * 5), true);
  assert.equal(await page.locator(".parallel-svg").evaluate(element => element.getBoundingClientRect().height), 400);
  for (const runId of runIds) {
    const line = page.locator(`.parallel-run[data-run="${runId}"]`);
    assert.ok(await line.locator("circle").count() > 40, "Sparse runs still have observations");
    assert.ok((await line.locator("path").getAttribute("d")).match(/M/g).length > 1, "Missing values create path gaps");
  }
  assert.equal(await page.locator('.parallel-axis-group[data-axis="hp:batch_size"]').textContent(), "batch_sizebatch_size64 (constant)");
  const labelsOverlap = await page.locator(".parallel-axis-label").evaluateAll(nodes => nodes.some((node, index) => {
    if (!index) return false;
    const previous = nodes[index - 1].getBBox(), current = node.getBBox();
    return previous.x + previous.width > current.x;
  }));
  assert.equal(labelsOverlap, false, "Axis labels do not overlap even with 68 axes");
  const heights = await rows.evaluateAll(rows => rows.map(row => row.getBoundingClientRect().height));
  assert.ok(heights.every(height => height < 90), `Compact rows: ${heights}`);
  assert.equal(await table.getByRole("grid").getAttribute("aria-colcount"), "73");
  assert.ok(await table.locator(".ag-header-cell[col-id]").count() < 20, "Only viewport columns are mounted");
  await noPageOverflow();

  const firstRun = table.locator(".ag-grid-pinned-left-cells .comparison-run-name").first();
  const initialLeft = (await firstRun.boundingBox()).x;
  await table.locator(".ag-body-horizontal-scroll-viewport").evaluate(element => {element.scrollLeft = 700;});
  assert.equal((await firstRun.boundingBox()).x, initialLeft, "Run names stay visible while scrolling columns");
  await table.locator(".ag-body-horizontal-scroll-viewport").evaluate(element => {element.scrollLeft = 0;});
  await table.getByRole("button", { name: `Inspect encoder for ${runIds[0]}`, exact: true }).click();
  const inspector = page.getByRole("dialog", { name: `encoder for ${runIds[0]}`, exact: true });
  assert.match(await inspector.locator("pre").textContent(), /"dim": 384/);
  await page.keyboard.press("Escape");
  assert.equal(await inspector.count(), 0);

  const differences = page.locator(".configuration-differences");
  assert.equal(await differences.locator(".configuration-run-diff[open]").count(), 0);
  await differences.locator(".configuration-run-diff").first().locator(":scope > summary").click();
  const dropout = differences.getByRole("row").filter({ has: page.getByRole("rowheader", { name: "encoder.dropout", exact: true }) });
  assert.match(await dropout.textContent(), /encoder\.dropout0\.10\.3/);
  assert.equal(await differences.getByRole("rowheader", { name: "encoder.dim", exact: true }).count(), 0, "Unchanged nested fields are omitted");
  assert.equal(await differences.getByRole("rowheader", { name: "provenance.git.commit", exact: true }).count(), 1);
  assert.match(await differences.getByRole("row").filter({ hasText: "warmup_fraction" }).textContent(), /Missing0\.02/);

  const csv = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export table CSV", exact: true }).click();
  await (await csv).saveAs(`${output}/comparison.csv`);
  assert.match(await readFile(`${output}/comparison.csv`, "utf8"), /'checkpointing': True/);
  await page.getByRole("combobox", { name: "Metric reducer", exact: true }).selectOption("last");
  await waitForMetric(0.8);
  assert.match(await metricPoint(), /val\/loss: 0\.8/);
  await page.getByRole("combobox", { name: "Metric reducer", exact: true }).selectOption("at_step");
  await page.getByRole("spinbutton", { name: "Comparison step", exact: true }).fill("10");
  await waitForMetric(0);
  assert.match(await metricPoint(), /val\/loss: 0$/);
  await page.getByRole("searchbox", { name: "Filter comparison runs", exact: true }).fill("prep/seed0");
  await page.waitForFunction(() => document.querySelectorAll(".comparison-grid .ag-row[role='row']").length === 1);
  await page.getByRole("searchbox", { name: "Filter comparison runs", exact: true }).fill("");
  await page.screenshot({ path: `${output}/desktop.png`, fullPage: true });
  await page.locator(".parallel-card").screenshot({ path: `${output}/desktop-parallel.png` });

  await page.setViewportSize({ width: 390, height: 844 });
  await noPageOverflow();
  assert.equal(await plot.evaluate(element => element.scrollWidth > element.clientWidth), true);
  await page.screenshot({ path: `${output}/mobile.png`, fullPage: true });
  await page.locator(".parallel-card").screenshot({ path: `${output}/mobile-parallel.png` });
  await plot.focus();
  await page.keyboard.press("ArrowRight");
  await page.waitForFunction(() => document.querySelector(".parallel-svg-wrap").scrollLeft > 0);
  await page.setViewportSize({ width: 1440, height: 1100 });
  await page.getByRole("button", { name: "Switch to light theme", exact: true }).click();
  await noPageOverflow();
  await page.screenshot({ path: `${output}/light.png`, fullPage: true });
  await page.locator(".parallel-card").screenshot({ path: `${output}/light-parallel.png` });
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ checks: ["68 axes without overlapping labels", "sparse runs and path gaps", "honest constant ranges", "compact rows and sticky run names", "expand full configurations", "nested and provenance differences", "raw CSV values", "consistent best/last/at-step values including zero", "filtering", "mobile bounds and keyboard scrolling", "light and dark themes"], output }));
} catch (error) {
  await page.screenshot({ path: `${output}/failure.png`, fullPage: true });
  throw error;
} finally {
  await browser.close();
  await server.close();
}
