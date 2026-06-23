// Usage: node capture.mjs <engine> <mode>
//   engine: chromium | webkit
//   mode:   online  -> allow all requests, log them
//           offline -> abort every non-localhost request (true offline test)
import { chromium, webkit } from 'playwright';

const engineName = process.argv[2] || 'chromium';
const mode = process.argv[3] || 'online';
const BASE = 'http://localhost:8123/';
const engines = { chromium, webkit };
const engine = engines[engineName];
if (!engine) { console.error('unknown engine', engineName); process.exit(2); }

const browser = await engine.launch();
const ctx = await browser.newContext();
const page = await ctx.newPage();

const requests = [];      // all requests
const blocked = [];       // requests we aborted in offline mode
const failed = [];        // requests that errored

const isLocal = (url) => url.startsWith(BASE) || url.startsWith('http://localhost:8123');

if (mode === 'offline') {
  await page.route('**/*', (route) => {
    const url = route.request().url();
    if (isLocal(url) || url.startsWith('data:') || url.startsWith('blob:')) {
      route.continue();
    } else {
      blocked.push(url);
      route.abort();
    }
  });
}

page.on('request', (r) => requests.push({ url: r.url(), type: r.resourceType() }));
page.on('requestfailed', (r) => failed.push({ url: r.url(), err: r.failure()?.errorText }));
page.on('console', (m) => { if (m.type() === 'error') console.log('  [console.error]', m.text().slice(0, 200)); });

let ok = false, computed = null, computed2 = null, err = null;
try {
  await page.goto(BASE, { waitUntil: 'domcontentloaded', timeout: 30000 });
  // wait for the numpy-computed markdown to appear
  await page.waitForFunction(
    () => document.body.innerText.includes('grows to'),
    { timeout: 150000 }
  );
  ok = true;
  computed = await page.evaluate(() => {
    const m = document.body.innerText.match(/grows to[^\n]*/);
    return m ? m[0] : null;
  });

  // prove interactivity: change the slider, expect the number to change.
  // marimo's slider thumb is a span[role=slider] inside the <marimo-slider>
  // web component's (open) shadow DOM — Playwright locators pierce it.
  try {
    const slider = page.locator('[role="slider"]').first();
    await slider.focus();
    for (let i = 0; i < 8; i++) await page.keyboard.press('ArrowRight');
    await page.waitForTimeout(2500);
    computed2 = await page.evaluate(() => {
      const m = document.body.innerText.match(/grows to[^\n]*/);
      return m ? m[0] : null;
    });
  } catch (e) { computed2 = 'INTERACTION_ERROR: ' + e.message; }

  await page.screenshot({ path: `shot-${engineName}-${mode}.png`, fullPage: true });
} catch (e) {
  err = e.message;
  try { await page.screenshot({ path: `shot-${engineName}-${mode}-FAIL.png`, fullPage: true }); } catch {}
}

const external = [...new Set(requests.map(r => r.url).filter(u => !isLocal(u)))];
const externalHosts = [...new Set(external.map(u => { try { return new URL(u).host; } catch { return u; } }))];

console.log('\n=== RESULT', engineName, mode, '===');
console.log('loaded_ok:', ok);
console.log('computed_initial:', computed);
console.log('computed_after_slider:', computed2);
console.log('changed_on_interaction:', computed && computed2 && computed !== computed2);
if (err) console.log('error:', err);
console.log('total_requests:', requests.length);
console.log('external_request_count:', external.length);
console.log('external_hosts:', externalHosts);
if (mode === 'offline') console.log('blocked_nonlocal_count:', blocked.length, '(first few):', blocked.slice(0, 8));
if (failed.length) console.log('failed_requests:', failed.slice(0, 12));
console.log('\n--- all external URLs ---');
for (const u of external) console.log(' ', u);

await browser.close();
process.exit(ok ? 0 : 1);
