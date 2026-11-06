// Isolated regression checks: API requests and saved views use in-memory fixtures.
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { tableFromArrays, tableToIPC } from "apache-arrow";
import { createServer } from "vite";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const server = await createServer({ server: { host: "127.0.0.1", port: 0 } });
await server.listen();
const base = server.resolvedUrls.local[0];
const output = process.env.AEROBOARD_BROWSER_OUTPUT || "/tmp/aeroboard-metric-groups-browser";
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true, ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
const errors = [];
const views = [];
const runIds = ["baseline", "candidate"];
const metrics = ["train/loss", "val/loss", "train/aux_loss", "train/accuracy", "val/accuracy", "train/lr"];
const records = runIds.map((run_id, index) => ({ id: `uid-${index}`, parent: "default", revision: 1,
  data: { run_id, display_name: run_id, project_id: "default", display_status: "completed", status: "completed",
    tags: [], notes: "", archived: false, provenance: {}, metadata_revision: 1 } }));

async function newPage() {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  const page = await context.newPage();
  page.on("pageerror", error => errors.push(String(error)));
  await page.routeWebSocket("**/ws/live", () => {});
  await page.route("**/api/**", async route => {
    const url = new URL(route.request().url());
    const path = url.pathname.slice(4);
    if (path === "/history") {
      const rows = url.searchParams.get("run_id").split(",").flatMap(run_id => metrics.flatMap(name => [0, 10, 20].map(step => ({ run_id, name, step }))));
      return route.fulfill({ contentType: "application/vnd.apache.arrow.stream", body: Buffer.from(tableToIPC(tableFromArrays({
        timestamp: Float64Array.from(rows, row => 1_700_000_000_000 + row.step * 1000),
        run_id: rows.map(row => row.run_id), name: rows.map(row => row.name),
        step: Float64Array.from(rows, row => row.step), value: Float64Array.from(rows, row => 1 / (row.step + 1)),
      }))) });
    }
    if (path === "/projects/default/views" && route.request().method() === "POST") {
      const { name, settings } = route.request().postDataJSON();
      const view = { id: `view-${views.length}`, parent: "default", revision: 1, data: { name, settings } };
      views.push(view);
      return route.fulfill({ json: view });
    }
    const responses = {
      "/capabilities": { server_id: "groups-browser-test", persistent: true, features: [], durable_replay: true },
      "/projects": [{ id: "default", parent: "", revision: 1, data: { name: "Default", default_view: null } }],
      "/projects/default/views": views, "/metric-definitions": [],
      "/run-records": { rows: records, next_offset: null }, "/runs": { runs: runIds },
      "/runs/summary": { run_summaries: runIds.map(run_id => ({ run_id, hyperparameters: [], metric_summaries: [] })) },
      "/config": { dashboard: { starred: ["loss", "aux_loss", "accuracy", "lr"] } },
      "/scorecards": { rows: [] }, "/events": { rows: [] },
    };
    assert.ok(path in responses, `Unexpected API request: ${path}`);
    return route.fulfill({ json: responses[path] });
  });
  return page;
}

function group(page, name) { return page.getByRole("region", { name: `${name} metric group`, exact: true }); }
async function chartNames(scope) { return scope.locator(".chart-title").evaluateAll(nodes => nodes.map(node => node.getAttribute("aria-label"))); }
async function groupNames(page) { return page.locator(".metric-group-stack > .metric-group[aria-label$='metric group']").evaluateAll(nodes => nodes.map(node => node.getAttribute("aria-label"))); }
async function openGroups(page, name) {
  await page.getByRole("button", { name: /^Metric groups/ }).click();
  const dialog = page.getByRole("dialog", { name: "Metric groups", exact: true });
  if (name) await dialog.getByLabel("Edit metric group", { exact: true }).selectOption({ label: name });
  return dialog;
}
async function createGroup(page, name, members) {
  const dialog = await openGroups(page);
  await dialog.getByLabel("Group name", { exact: true }).fill(name);
  for (const metric of members) await dialog.getByRole("checkbox", { name: `Include ${metric}`, exact: true }).check();
  await dialog.getByRole("button", { name: "Create group", exact: true }).click();
  await group(page, name).waitFor();
}
async function drag(page, from, to) {
  await from.scrollIntoViewIfNeeded();
  const start = await from.boundingBox();
  const target = await to.boundingBox();
  await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2);
  await page.mouse.down();
  await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2 + 8, { steps: 3 });
  await page.mouse.move(target.x + target.width / 2, target.y + target.height / 2, { steps: 20 });
  await page.mouse.up();
  // dnd-kit suppresses clicks for 50ms after a pointer drag.
  await page.waitForTimeout(60);
}

try {
  const page = await newPage();
  await page.goto(base);
  await page.locator(".chart-title").nth(3).waitFor();
  await createGroup(page, "Losses", ["loss", "aux_loss"]);
  await createGroup(page, "Metrics", ["accuracy"]);
  assert.deepEqual(await chartNames(group(page, "Losses")), ["aux_loss", "loss"]);
  assert.deepEqual(await chartNames(group(page, "Metrics")), ["accuracy"]);
  assert.deepEqual(await chartNames(page.getByRole("region", { name: "Ungrouped metrics", exact: true })), ["lr"]);

  await page.getByRole("button", { name: "Hide loss", exact: true }).click();
  await page.getByRole("button", { name: "Hide metric group Losses", exact: true }).click();
  assert.equal(await group(page, "Losses").locator(".chart-card").count(), 0);
  assert.equal(await page.locator(".metric-toolbar-heading > span").textContent(), "2");
  assert.deepEqual(await chartNames(group(page, "Metrics")), ["accuracy"]);
  await page.getByRole("button", { name: "Show metric group Losses", exact: true }).click();
  assert.deepEqual(await chartNames(group(page, "Losses")), ["aux_loss"], "Individually hidden charts stay hidden");
  await page.getByRole("button", { name: /^Show hidden charts/ }).click();
  const hidden = page.getByRole("dialog", { name: "Hidden charts", exact: true });
  await hidden.getByRole("checkbox", { name: "loss", exact: true }).check();
  await hidden.getByRole("button", { name: /^Show selected/ }).click();
  assert.deepEqual(await chartNames(group(page, "Losses")), ["aux_loss", "loss"]);

  // Reorder cards within a group without moving another group's charts.
  await drag(page, group(page, "Losses").locator(".chart-title").nth(1), group(page, "Losses").locator(".chart-title").first());
  assert.deepEqual(await chartNames(group(page, "Losses")), ["loss", "aux_loss"]);
  assert.deepEqual(await chartNames(group(page, "Metrics")), ["accuracy"]);

  await page.getByRole("button", { name: "Move metric group Metrics up", exact: true }).click();
  assert.deepEqual(await groupNames(page), ["Metrics metric group", "Losses metric group"]);
  await drag(page, page.getByRole("button", { name: "Drag metric group Losses", exact: true }), group(page, "Metrics").locator(".metric-group-header"));
  assert.deepEqual(await groupNames(page), ["Losses metric group", "Metrics metric group"]);
  assert.deepEqual(await chartNames(group(page, "Losses")), ["loss", "aux_loss"]);
  await page.getByRole("button", { name: "Move metric group Losses down", exact: true }).click();
  await page.getByRole("button", { name: "Hide metric group Losses", exact: true }).click();
  await drag(page, page.getByRole("button", { name: "Drag metric group Losses", exact: true }), group(page, "Metrics").locator(".metric-group-header"));
  assert.deepEqual(await groupNames(page), ["Losses metric group", "Metrics metric group"]);
  assert.equal(await group(page, "Losses").locator(".chart-card").count(), 0);
  await page.reload();
  await group(page, "Losses").waitFor();
  assert.deepEqual(await groupNames(page), ["Losses metric group", "Metrics metric group"]);
  assert.equal(await group(page, "Losses").locator(".chart-card").count(), 0);

  await page.getByRole("button", { name: "Compare", exact: true }).click();
  await page.getByRole("button", { name: "candidate", exact: true }).click();
  await page.getByRole("button", { name: "baseline", exact: true }).click();
  await group(page, "Metrics").getByRole("heading", { name: "val/accuracy", exact: true }).waitFor();
  assert.deepEqual(await chartNames(group(page, "Metrics")), ["train/accuracy", "val/accuracy"]);
  assert.equal(await group(page, "Losses").locator(".chart-card").count(), 0);
  await page.getByRole("button", { name: "Show metric group Losses", exact: true }).click();
  assert.deepEqual(await chartNames(group(page, "Losses")), ["train/loss", "val/loss", "train/aux_loss"]);

  const search = page.getByRole("searchbox", { name: "Search metrics", exact: true });
  await search.fill("Losses");
  assert.equal(await group(page, "Metrics").count(), 0);
  assert.equal(await group(page, "Losses").locator(".chart-card").count(), 3);
  await page.getByRole("button", { name: "Hide metric group Losses", exact: true }).click();
  await search.fill("");

  await page.getByRole("button", { name: "Project and view settings", exact: true }).click();
  await page.getByLabel("View name", { exact: true }).fill("Grouped metrics");
  await page.getByRole("button", { name: "Save view", exact: true }).click();
  await page.getByRole("status").filter({ hasText: "View saved." }).waitFor();
  assert.deepEqual(views[0].data.settings.controls.metricGroups.map(({ name, hidden }) => ({ name, hidden })), [
    { name: "Losses", hidden: true }, { name: "Metrics", hidden: false },
  ]);
  await page.getByRole("button", { name: "Close project settings", exact: true }).click();
  const shared = await newPage();
  await shared.goto(`${base}?view=${views[0].id}`);
  await group(shared, "Metrics").getByRole("heading", { name: "val/accuracy", exact: true }).waitFor();
  assert.deepEqual(await groupNames(shared), ["Losses metric group", "Metrics metric group"]);
  assert.equal(await group(shared, "Losses").locator(".chart-card").count(), 0);

  let dialog = await openGroups(shared, "Metrics");
  await dialog.getByLabel("Group name", { exact: true }).fill(" losses ");
  assert.equal(await dialog.getByRole("button", { name: "Save group", exact: true }).isDisabled(), true);
  await dialog.getByLabel("Group name", { exact: true }).fill("Quality");
  await dialog.getByRole("checkbox", { name: "Include aux_loss", exact: true }).check();
  await dialog.getByRole("button", { name: "Save group", exact: true }).click();
  await shared.getByRole("button", { name: "Show metric group Losses", exact: true }).click();
  assert.deepEqual(await chartNames(group(shared, "Losses")), ["train/loss", "val/loss"]);
  assert.deepEqual(await chartNames(group(shared, "Quality")), ["train/accuracy", "val/accuracy", "train/aux_loss"]);
  await shared.screenshot({ path: `${output}/desktop.png`, fullPage: true });

  await shared.setViewportSize({ width: 390, height: 844 });
  dialog = await openGroups(shared, "Losses");
  const bounds = await dialog.boundingBox();
  assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= 390);
  assert.equal(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth), true);
  const editor = await dialog.getByLabel("Group name", { exact: true }).boundingBox();
  const filter = await dialog.getByRole("searchbox", { name: "Filter metrics for group", exact: true }).boundingBox();
  assert.ok(filter.y >= editor.y + editor.height && filter.y < editor.y + editor.height + 30, "The filter follows the name field on mobile");
  await shared.screenshot({ path: `${output}/mobile-dialog.png`, fullPage: true });
  await shared.keyboard.press("Escape");
  assert.equal(await shared.getByRole("dialog").count(), 0);
  assert.equal(await shared.getByRole("button", { name: /^Metric groups/ }).evaluate(element => element === document.activeElement), true);
  dialog = await openGroups(shared, "Losses");
  await dialog.getByRole("button", { name: "Ungroup charts", exact: true }).click();
  assert.equal(await group(shared, "Losses").count(), 0);
  assert.deepEqual(await chartNames(shared.getByRole("region", { name: "Ungrouped metrics", exact: true })), ["train/loss", "val/loss", "train/lr"]);
  await shared.screenshot({ path: `${output}/mobile.png`, fullPage: true });
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ checks: ["create groups", "hide and restore groups", "individual visibility preserved", "reorder charts within a group", "move groups by button and drag", "reload persistence", "compare mode", "search group names", "fresh-browser saved view", "rename and transfer membership", "mobile and keyboard dialog", "ungroup without deleting charts"], output }));
} catch (error) {
  const failed = browser.contexts().at(-1)?.pages()[0];
  if (failed) await failed.screenshot({ path: `${output}/failure.png`, fullPage: true });
  throw error;
} finally {
  await browser.close();
  await server.close();
}
