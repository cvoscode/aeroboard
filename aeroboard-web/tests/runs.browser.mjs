// Isolated browser regression checks. All API traffic uses fixtures; no saved runs are modified.
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { tableFromArrays, tableToIPC } from "apache-arrow";
import { createServer } from "vite";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const server = await createServer({server: {host: "127.0.0.1", port: 0}});
await server.listen();
const base = server.resolvedUrls.local[0];
const output = process.env.AEROBOARD_BROWSER_OUTPUT || "/tmp/aeroboard-runs-browser";
await mkdir(output, {recursive: true});
const browser = await chromium.launch({headless: true, ...(process.env.CHROMIUM_PATH ? {executablePath: process.env.CHROMIUM_PATH} : {})});
const context = await browser.newContext({viewport: {width: 1440, height: 1100}});
await context.addInitScript(() => {
  localStorage.setItem("aeroboard-theme", "dark");
  const key = "aeroboard-controls:browser-test:default:defaults";
  if (!localStorage.getItem(key)) localStorage.setItem(key, JSON.stringify({logCharts: {"train/loss": true}}));
});
const page = await context.newPage();
const errors = [];
page.on("pageerror", error => errors.push(String(error)));
const group = "reader_overfit/seed0";
const initialIds = [group, `${group}-v2`, `${group}-v3`, "retry", "retry-v2", "unrelated"];
const records = initialIds.map((run_id, index) => ({id: `uid-${index}`, parent: "default", revision: 12,
  data: {run_id, display_name: run_id, project_id: "default", status: "completed", display_status: "completed",
    tags: [], notes: "", archived: false, provenance: {}, metadata_revision: 7, started_at_ms: 1_700_000_000_000 + index * 60_000}}));
const deletions = [];
let rejectRun = "retry-v2";
let releaseDelete;
let releaseHistory;
let delayHistory = false;
let liveSocket;
const timestamp = 1_700_000_000_123;

function history(runIds) {
  const rows = runIds.flatMap(run_id => [
    {run_id, name: "train/loss", step: 0, value: 0},
    {run_id, name: "train/loss", step: 10, value: 0.8},
    {run_id, name: "train/loss", step: 20, value: 0.4},
    {run_id, name: "val/loss", step: 0, value: 1},
    {run_id, name: "val/loss", step: 20, value: 0.5},
  ]);
  return Buffer.from(tableToIPC(tableFromArrays({
    timestamp: Float64Array.from(rows, row => timestamp + row.step * 1000),
    run_id: rows.map(row => row.run_id), name: rows.map(row => row.name),
    step: Float64Array.from(rows, row => row.step), value: Float64Array.from(rows, row => row.value),
  })));
}

await page.routeWebSocket("**/ws/live", socket => {liveSocket = socket;});
await page.route("**/api/**", async route => {
  const url = new URL(route.request().url());
  const path = url.pathname.slice(4);
  if (route.request().method() === "DELETE") {
    const record = records.find(run => path === `/run-records/${run.id}`);
    assert.ok(record, `Unexpected deletion: ${path}`);
    assert.equal(url.searchParams.get("revision"), "7", "Use the metadata revision, not the entity revision");
    deletions.push(record.data.run_id);
    if (record.data.run_id === rejectRun) return route.fulfill({status: 409, json: {error: "metadata changed; reload before deleting"}});
    if (record.data.run_id === `${group}-v2`) await new Promise(resolve => {releaseDelete = resolve;});
    records.splice(records.indexOf(record), 1);
    return route.fulfill({json: {deleted: true}});
  }
  if (path === "/history") {
    const body = history(url.searchParams.get("run_id").split(","));
    if (delayHistory && url.searchParams.get("run_id") === "unrelated") {
      delayHistory = false;
      await new Promise(resolve => {releaseHistory = resolve;});
    }
    return route.fulfill({contentType: "application/vnd.apache.arrow.stream", body});
  }
  const responses = {
    "/capabilities": {server_id: "browser-test", persistent: true, features: [], durable_replay: true},
    "/projects": [{id: "default", parent: "", revision: 1, data: {name: "Default", default_view: null}}],
    "/projects/default/views": [], "/metric-definitions": [],
    "/run-records": {rows: records, next_offset: null},
    "/runs": {runs: records.map(run => run.data.run_id)},
    "/runs/summary": {run_summaries: records.map(run => ({run_id: run.data.run_id, hyperparameters: [], metric_summaries: []}))},
    "/config": {dashboard: {starred: ["loss"]}},
    "/scorecards": {rows: []}, "/events": {rows: []},
    "/history/images": {rows: []}, "/history/media": {rows: []},
  };
  assert.ok(path in responses, `Unexpected API request: ${path}`);
  return route.fulfill({json: responses[path]});
});

const tooltip = page.getByRole("tooltip");
async function hover(fraction) {
  const plot = page.locator(".u-over").first();
  await plot.hover();
  const bounds = await plot.boundingBox();
  await page.mouse.move(bounds.x + Math.max(1, Math.min(bounds.width - 1, bounds.width * fraction)), bounds.y + bounds.height / 2);
  await tooltip.waitFor();
}
async function tooltipValues(expected) {
  await page.waitForFunction(values => JSON.stringify(Array.from(document.querySelectorAll('.chart-tooltip:not([hidden]) .chart-tooltip-row:not([hidden]) .chart-tooltip-value'), el => el.textContent)) === JSON.stringify(values), expected);
}
async function smooth(value, trend = false) {
  await page.locator('input[type="range"]').first().evaluate((input, next) => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, String(next));
    input.dispatchEvent(new Event("input", {bubbles: true}));
  }, value);
  await page.waitForFunction(count => document.querySelectorAll(".chart-tooltip-row").length === count, (value ? 4 : 2) + (trend ? 2 : 0));
}
/** Share of the plot's height used by the train line, read from the drawn pixels. */
async function trainLineHeight() {
  return page.locator(".chart-card canvas").first().evaluate(canvas => {
    const {width, height} = canvas;
    const {data} = canvas.getContext("2d").getImageData(0, 0, width, height);
    const [red, green, blue] = [0x2f, 0x7f, 0xd9];
    let top = height, bottom = -1;
    for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
      const i = (y * width + x) * 4;
      if (Math.abs(data[i] - red) + Math.abs(data[i + 1] - green) + Math.abs(data[i + 2] - blue) < 40) {top = Math.min(top, y); bottom = Math.max(bottom, y);}
    }
    return (bottom - top) / height;
  });
}
async function zoomPlot(from, to) {
  const bounds = await page.locator(".u-over").first().boundingBox();
  await page.mouse.move(bounds.x + bounds.width * from, bounds.y + 50);
  await page.mouse.down();
  await page.mouse.move(bounds.x + bounds.width * to, bounds.y + 50, {steps: 5});
  await page.mouse.up();
  await page.waitForTimeout(200);
}
async function confirmDeletion(name) {
  await page.getByRole("button", {name, exact: true}).click();
  const dialog = page.getByRole("dialog");
  await dialog.waitFor();
  await dialog.locator(".confirm-dialog-confirm").click();
}

try {
  // The newest run leads the list, ahead of alphabetical order, and opens by default.
  await page.goto(base);
  await page.locator(".viewport-title", {hasText: "unrelated"}).waitFor();
  assert.deepEqual(await page.locator(".run-card-btn").evaluateAll(rows => rows.map(row => row.getAttribute("aria-label"))),
    ["unrelated", "retry-v2", "retry", `${group}-v3`, `${group}-v2`, group]);

  await page.goto(`${base}?runs=${encodeURIComponent(`${group}-v2`)}`);
  const scale = page.getByRole("combobox", {name: "Y-axis scale for loss", exact: true});
  await scale.waitFor();
  assert.equal(await scale.inputValue(), "log", "Existing log-scale preferences are preserved");
  await scale.selectOption("linear");
  await hover(0);
  assert.equal(await tooltip.locator(".chart-tooltip-heading").textContent(), "Step 0");
  await tooltipValues(["0", "1"]);
  await hover(0.5);
  await tooltipValues(["0.8", "—"]); // A sparse series must not borrow a value from a different step.
  await hover(1);
  await tooltipValues(["0.4", "0.5"]);
  await page.mouse.move(0, 0);
  await tooltip.waitFor({state: "hidden"});
  const plotBounds = await page.locator(".u-over").first().boundingBox();
  await page.mouse.move(plotBounds.x + plotBounds.width * 0.25, plotBounds.y + 50);
  await page.mouse.down();
  await page.mouse.move(plotBounds.x + plotBounds.width * 0.75, plotBounds.y + 50, {steps: 5});
  await page.mouse.up();
  await hover(0.5);
  await tooltipValues(["0.8", "—"]);
  await page.locator(".u-over").first().dblclick();
  await hover(0);
  await tooltipValues(["0", "1"]);

  // The y-axis follows the zoom. Steps 12 to 18 hold about a quarter of train's
  // range, so a fixed axis would leave the line at about a fifth of the plot height.
  await page.mouse.move(0, 0);
  const fullHeight = await trainLineHeight();
  await zoomPlot(0.6, 0.9);
  await page.mouse.move(0, 0);
  const zoomedHeight = await trainLineHeight();
  assert.ok(zoomedHeight > 0.55, `Zoomed line fills ${zoomedHeight} of the plot`);
  await page.locator(".u-over").first().dblclick();
  await page.mouse.move(0, 0);
  await page.waitForTimeout(200);
  assert.ok(Math.abs(await trainLineHeight() - fullHeight) < 0.02, "Resetting the zoom restores the y-axis");

  await smooth(0.5);
  await hover(0.5);
  await tooltipValues(["0.8", "0.4", "—", "—"]);
  assert.match(await tooltip.textContent(), /train raw.*train smoothed/);
  await page.getByRole("button", {name: "Relative time", exact: true}).click();
  await hover(0.5);
  assert.equal(await tooltip.locator(".chart-tooltip-heading").textContent(), "Elapsed · 00:00:10");
  await page.getByRole("button", {name: "Wall time", exact: true}).click();
  await hover(0.5);
  assert.match(await tooltip.locator(".chart-tooltip-heading").textContent(), /Wall time.*2023.*123/);
  await page.getByRole("button", {name: "Training step", exact: true}).click();
  await smooth(0);
  const trend = page.getByRole("button", {name: "Linear trend for loss", exact: true});
  assert.equal(await trend.getAttribute("aria-pressed"), "false");
  await trend.click();
  await hover(0.5);
  await tooltipValues(["0.8", "0.4", "—", "0.75"]);
  assert.match(await tooltip.textContent(), /train trend.*val trend/);
  await page.screenshot({path: `${output}/linear-trend.png`});
  await smooth(0.5, true);
  await hover(1);
  await tooltipValues(["0.4", "0.4", "0.6", "0.5", "0.75", "0.5"]);
  await smooth(0, true);
  const trendBounds = await page.locator(".u-over").first().boundingBox();
  await page.mouse.move(trendBounds.x, trendBounds.y + 50);
  await page.mouse.down();
  await page.mouse.move(trendBounds.x + trendBounds.width * 0.6, trendBounds.y + 50, {steps: 5});
  await page.mouse.up();
  await hover(0.8);
  await tooltipValues(["0.8", "0.8", "—"]); // Refit to steps 0..10; val has only one visible sample.
  await page.locator(".u-over").first().dblclick();
  await hover(0.5);
  await tooltipValues(["0.8", "0.4", "—", "0.75"]);
  await trend.click();
  await scale.selectOption("log");
  assert.equal(await page.getByText("Log scale shows positive finite samples only.", {exact: false}).count(), 0);
  await hover(0);
  await tooltipValues(["—", "1"]);
  await scale.selectOption("exponential");
  await hover(0);
  await tooltipValues(["0", "1"]);
  await hover(0.5);
  await tooltipValues(["0.8", "—"]);
  await trend.click();
  await hover(0.5);
  await tooltipValues(["0.8", "0.4", "—", "0.75"]);
  await page.screenshot({path: `${output}/exponential.png`});
  await page.reload();
  await scale.waitFor();
  assert.equal(await scale.inputValue(), "exponential", "The new scale overrides the legacy preference and survives reload");
  assert.equal(await trend.getAttribute("aria-pressed"), "true", "Trend settings survive reload");
  await hover(0.5);
  await tooltipValues(["0.8", "0.4", "—", "0.75"]);
  await trend.click();
  await scale.selectOption("linear");
  await page.getByRole("button", {name: "Switch to light theme"}).click();
  await hover(0.5);
  await tooltipValues(["0.8", "—"]);
  const tipBounds = await tooltip.boundingBox();
  const chartBounds = await page.locator(".uplot-wrapper").boundingBox();
  assert.ok(tipBounds.x >= chartBounds.x && tipBounds.x + tipBounds.width <= chartBounds.x + chartBounds.width);
  await page.screenshot({path: `${output}/tooltip.png`});
  await page.getByRole("button", {name: "Compare", exact: true}).click();
  await page.getByRole("button", {name: group, exact: true}).click();
  await page.locator(".chart-card").first().getByTitle(group, {exact: true}).waitFor();
  await hover(0.5);
  await tooltipValues(["0.8", "0.8"]);
  assert.match(await tooltip.textContent(), /reader_overfit\/seed0-v2/);
  await page.getByRole("combobox", {name: /^Y-axis scale for /}).first().selectOption("exponential");
  await page.getByRole("button", {name: /^Linear trend for /}).first().click();
  await hover(0.5);
  await tooltipValues(["0.8", "0.4", "0.8", "0.4"]);
  await page.getByRole("button", {name: "Single", exact: true}).click();
  assert.equal(await scale.inputValue(), "exponential", "Scale settings follow the metric across comparison and single-run views");
  assert.equal(await trend.getAttribute("aria-pressed"), "true", "Trend settings follow the metric across comparison and single-run views");
  await hover(0.5);
  await tooltipValues(["0.8", "0.4", "—", "0.75"]);
  await page.setViewportSize({width: 390, height: 844});
  await page.waitForFunction(() => document.querySelector(".sidebar").getBoundingClientRect().right <= 0);
  await hover(1);
  const mobileTip = await tooltip.boundingBox();
  assert.ok(mobileTip.x >= 0 && mobileTip.x + mobileTip.width <= 390);
  await page.screenshot({path: `${output}/mobile-tooltip.png`});
  await page.setViewportSize({width: 1440, height: 1100});
  assert.ok(liveSocket);
  liveSocket.send(Buffer.from(tableToIPC(tableFromArrays({
    timestamp: new Float64Array([timestamp + 30_000]), run_id: [`${group}-v2`], name: ["train/loss"],
    step: new Float64Array([30]), value: new Float64Array([0.6]),
  }))));
  await page.waitForFunction(() => document.querySelector(".legend-val")?.textContent === "0.6000");
  await hover(1 / 3);
  await tooltipValues(["0.8", "0.38", "—", "0.75"]);
  await hover(1);
  await tooltipValues(["0.6", "0.66", "—", "—"]);

  // Canceling leaves the current selection and data intact.
  await page.getByRole("button", {name: `Delete run ${group}-v2`, exact: true}).click();
  await page.getByRole("button", {name: "Cancel", exact: true}).click();
  assert.deepEqual(deletions, []);
  assert.equal(await page.getByRole("button", {name: `${group}-v2`, exact: true}).evaluate(el => el.closest(".run-card-wrapper").classList.contains("active")), true);
  await confirmDeletion(`Delete run ${group}-v2`);
  await page.waitForFunction(() => document.querySelector(".run-delete-btn:disabled") !== null);
  assert.equal(await page.getByRole("button", {name: `Delete run group ${group}`, exact: true}).isDisabled(), true);
  releaseDelete();
  await page.getByRole("button", {name: `${group}-v2`, exact: true}).waitFor({state: "detached"});
  await page.getByRole("button", {name: group, exact: true}).waitFor();

  // A filtered group still deletes every version in the group, leaving other groups alone.
  await page.getByRole("searchbox", {name: "Filter runs"}).fill("v3");
  await page.getByRole("button", {name: `Delete run group ${group}`, exact: true}).click();
  assert.match(await page.getByRole("dialog").textContent(), /all 2 run versions.*hidden by the filter/);
  await page.getByRole("dialog").getByRole("button", {name: "Delete group", exact: true}).click();
  await page.getByRole("button", {name: `Delete run group ${group}`, exact: true}).waitFor({state: "detached"});
  await page.getByRole("searchbox", {name: "Filter runs"}).fill("");
  assert.deepEqual(deletions.slice().sort(), [group, `${group}-v2`, `${group}-v3`].sort());
  await page.getByRole("button", {name: "unrelated", exact: true}).waitFor();

  // Partial failure removes successful runs and reports the failed one, which can be retried.
  await confirmDeletion("Delete run group retry");
  await page.getByText(/Could not delete 1 run.*retry-v2.*409/).waitFor();
  await page.getByRole("button", {name: "retry", exact: true}).waitFor({state: "detached"});
  await page.getByRole("button", {name: "retry-v2", exact: true}).waitFor();
  rejectRun = "";
  await confirmDeletion("Delete run retry-v2");
  await page.getByRole("button", {name: "retry-v2", exact: true}).waitFor({state: "detached"});

  // A history response started before deletion cannot resurrect the deleted run.
  delayHistory = true;
  await page.goto(`${base}?runs=unrelated`);
  await page.getByRole("button", {name: "unrelated", exact: true}).waitFor();
  await confirmDeletion("Delete run unrelated");
  await page.getByRole("button", {name: "unrelated", exact: true}).waitFor({state: "detached"});
  assert.ok(releaseHistory, "History was in flight before deleting");
  const historyReturned = page.waitForResponse(response => new URL(response.url()).pathname === "/api/history");
  releaseHistory();
  await historyReturned;
  await page.getByText("No runs detected", {exact: true}).waitFor();
  assert.equal(await page.locator(".chart-card").count(), 0);
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({checks: ["newest run first and opened by default", "hover values, zeros and sparse series", "zoom and reset", "y-axis follows zoom and resets", "raw and smoothed values", "all x-axis modes", "linear trend values and raw fitting with smoothing", "trend refits on zoom and skips insufficient samples", "trend persistence and comparison", "live data updates the trend", "log scale without the note", "exponential scale and unchanged tooltip values", "saved scale preferences and legacy compatibility", "light theme and tooltip bounds", "comparison and mobile tooltips", "cancel", "single deletion and busy state", "filtered group deletion", "partial failure and retry", "late history response"], output}));
} finally {
  releaseDelete?.(); releaseHistory?.();
  await context.close(); await browser.close(); await server.close();
}
