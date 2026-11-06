// Exercise the comparison grid in isolation with 5000 runs and 100 hyperparameters.
import assert from "node:assert/strict";
import { mkdir, readFile } from "node:fs/promises";
import { createServer } from "vite";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const server = await createServer({ server: { host: "127.0.0.1", port: 0 }, plugins: [{
  name: "comparison-grid-fixture",
  configureServer(vite) {
    vite.middlewares.use("/__comparison-grid", async (request, response) => {
      const html = await vite.transformIndexHtml(request.originalUrl,
        '<!doctype html><html data-theme="dark"><body style="margin:0"><div id="root"></div><script type="module" src="/tests/fixtures/comparisonGrid.tsx"></script></body></html>');
      response.setHeader("Content-Type", "text/html");response.end(html);
    });
  },
}] });
await server.listen();
const output = process.env.AEROBOARD_BROWSER_OUTPUT || "/tmp/aeroboard-grid-browser";
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true, ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
const errors = [];
page.on("pageerror", error => errors.push(String(error)));
page.on("console", message => {if (message.type() === "error" && message.text().includes("AG Grid")) errors.push(message.text());});
const table = page.getByRole("region", { name: "Run comparison table", exact: true });
const rows = table.locator(".ag-row[role='row']");
const viewport = table.locator(".ag-grid-viewport");
const rowIds = () => rows.evaluateAll(nodes => nodes.sort((a, b) => Number(a.getAttribute("row-index")) - Number(b.getAttribute("row-index"))).map(node => node.getAttribute("row-id")));
async function filterCount(count) {
  await page.waitForFunction(count => document.querySelector(".comparison-card-header p")?.textContent === `${count} / 5000 runs visible`, count);
}

try {
  await page.goto(`${server.resolvedUrls.local[0]}__comparison-grid`);
  await rows.first().waitFor();
  await filterCount(5000);
  assert.equal(await table.getByRole("grid").getAttribute("aria-colcount"), "103");
  const initialRows = await rows.count(), initialCells = await table.locator(".ag-cell").count();
  assert.ok(initialRows < 40, `Only ${initialRows} rows mounted for 5000 runs`);
  assert.ok(initialCells < 500, `Only ${initialCells} cells mounted for 515000 data cells`);
  assert.ok(await table.locator(".ag-header-cell[col-id]").count() < 25);

  await viewport.evaluate(element => {element.scrollTop = element.scrollHeight;element.scrollLeft = element.scrollWidth;});
  await table.locator('.ag-row[row-id="run-04999"]').waitFor();
  await table.locator('.ag-row[row-id="run-04999"] .comparison-run-name').waitFor();
  assert.ok(await rows.count() < 40);
  assert.ok(await table.locator(".ag-cell").count() < 500);
  await viewport.evaluate(element => {element.scrollTop = 0;element.scrollLeft = 0;});
  await rows.locator('.comparison-run-name[title="run-00000"]').waitFor();

  const header = table.locator('.ag-header-cell[col-id="hp:parameter_0"]').filter({ has: page.locator(".ag-header-cell-label") });
  await header.locator(".ag-header-cell-label").click();
  await page.waitForFunction(() => document.querySelector('.ag-row[row-index="0"]')?.getAttribute("row-id") === "run-00000");
  await header.locator(".ag-header-cell-label").click();
  await page.waitForFunction(() => document.querySelector('.ag-row[row-index="0"]')?.getAttribute("row-id") === "run-04999");
  assert.equal((await rowIds())[0], "run-04999", "Numeric sorting is descending");

  const width = (await header.boundingBox()).width;
  const grip = await header.locator(".ag-header-cell-resize").boundingBox();
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
  await page.mouse.down();
  await page.mouse.move(grip.x + grip.width / 2 + 60, grip.y + grip.height / 2, { steps: 10 });
  await page.mouse.up();
  assert.ok((await header.boundingBox()).width >= width + 50, "Columns resize without rendering the full dataset");

  const configHeader = table.locator('.ag-header-cell[col-id="hp:config"]').filter({ has: page.locator(".ag-header-cell-label") });
  const source = await header.locator(".ag-header-cell-label").boundingBox();
  const destination = await configHeader.boundingBox();
  await page.mouse.move(source.x + source.width / 2, source.y + source.height / 2);
  await page.mouse.down();
  await page.mouse.move(source.x + source.width / 2 - 10, source.y + source.height / 2, { steps: 3 });
  await page.mouse.move(destination.x + 20, source.y + source.height / 2, { steps: 20 });
  await page.mouse.up();
  await page.waitForFunction(() => {
    const numeric = document.querySelector('.ag-header-cell[col-id="hp:parameter_0"] .ag-header-cell-label');
    const config = document.querySelector('.ag-header-cell[col-id="hp:config"] .ag-header-cell-label');
    return numeric.getBoundingClientRect().x < config.getBoundingClientRect().x;
  });
  const numericFilter = table.getByRole("spinbutton", { name: "parameter_0 Filter Input", exact: true });
  await numericFilter.fill("12");
  await filterCount(1);
  assert.deepEqual(await rowIds(), ["run-00011"]);
  await page.getByRole("button", { name: "Clear filters", exact: true }).click();
  await filterCount(5000);
  await page.getByRole("searchbox", { name: "Filter comparison runs", exact: true }).fill("run-01234");
  await filterCount(1);
  assert.deepEqual(await rowIds(), ["run-01234"], "Quick filtering finds rows outside the viewport");
  const csv = page.waitForEvent("download");
  await page.getByRole("button", { name: "Export table CSV", exact: true }).click();
  await (await csv).saveAs(`${output}/filtered.csv`);
  const exported = await readFile(`${output}/filtered.csv`, "utf8");
  assert.equal(exported.split(/\r?\n/).filter(Boolean).length, 2);
  assert.match(exported, /run-01234/);
  assert.match(exported, /'checkpointing': True/);
  assert.ok(exported.indexOf('"parameter_0"') < exported.indexOf('"config"'), "CSV follows the moved column order");
  await page.getByRole("button", { name: "Clear filters", exact: true }).click();
  await filterCount(5000);
  await page.getByRole("combobox", { name: "Baseline run", exact: true }).selectOption("run-04999");
  await table.locator('.ag-row[row-id="run-04999"] .comparison-grid-run small').waitFor();
  assert.equal(await table.locator('.ag-row[row-id="run-04999"] .comparison-grid-run small').textContent(), "Baseline");
  assert.ok((await header.boundingBox()).width >= width + 50, "Resized columns survive data updates");
  assert.ok((await header.boundingBox()).x < (await configHeader.boundingBox()).x, "Moved columns survive data updates");
  await page.screenshot({ path: `${output}/desktop.png`, fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
  await viewport.evaluate(element => {element.scrollTop = 0;element.scrollLeft = 360;});
  await page.getByRole("button", { name: "Inspect config for run-04999", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "config for run-04999", exact: true });
  const bounds = await dialog.boundingBox();
  assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= 390);
  assert.match(await dialog.locator("pre").textContent(), /"checkpointing": true/);
  await page.screenshot({ path: `${output}/mobile-value.png`, fullPage: true });
  await page.keyboard.press("Escape");
  assert.equal(await dialog.count(), 0);
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ runs: 5000, columns: 103, initialRows, initialCells,
    checks: ["row and column virtualization", "scroll to last run and column", "pinned run names", "numeric sorting", "column resizing and moving", "numeric column filters", "cached quick filter", "filtered raw CSV export and column order", "mobile value inspection"], output }));
} catch (error) {
  await page.screenshot({ path: `${output}/failure.png`, fullPage: true });
  throw error;
} finally {
  await browser.close();await server.close();
}
