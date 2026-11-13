import { chromium } from '../aeroboard-web/node_modules/playwright/index.mjs';
import { performance } from 'node:perf_hooks';
const [base,run]=process.argv.slice(2);
const browser=await chromium.launch({headless:true});
try{
  const page=await browser.newPage();const errors=[];page.on('pageerror',error=>errors.push(String(error)));
  const start=performance.now();
  await page.goto(`${base}/?runs=${encodeURIComponent(run)}`);
  await page.locator('.chart-card .u-over').first().waitFor({timeout:120000});
  console.log(JSON.stringify({navigation_to_chart_ms:performance.now()-start,errors}));
  if(errors.length)process.exitCode=1;
}finally{await browser.close();}
