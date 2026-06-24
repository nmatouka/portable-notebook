import { chromium } from 'playwright';
const BASE='http://localhost:8123/';
const marker = process.argv[2] || 'OFFLINE-COWSAY-OK';
const offline = process.argv[3] === 'offline';
const b = await chromium.launch();
const p = await (await b.newContext()).newPage();
const reqs=[], blocked=[];
if (offline) await p.route('**/*', r => { const u=r.request().url(); (u.startsWith(BASE)||u.startsWith('data:')||u.startsWith('blob:')) ? r.continue() : (blocked.push(u), r.abort()); });
p.on('request', r => reqs.push(r.url()));
let ok=false, err=null, errText=null;
try {
  await p.goto(BASE, { waitUntil:'domcontentloaded', timeout:30000 });
  await p.waitForFunction(m => document.body.innerText.includes(m) || /Traceback|ModuleNotFound|Error loading|could not|failed/i.test(document.body.innerText), marker, { timeout:120000 });
  const body = await p.evaluate(() => document.body.innerText);
  ok = body.includes(marker);
  if (!ok) errText = (body.match(/[^\n]*(Traceback|ModuleNotFound|Error|failed|could not)[^\n]*/i)||[])[0];
} catch(e){ err = e.message; }
const ext = [...new Set(reqs.filter(u => !u.startsWith(BASE) && !u.startsWith('data:') && !u.startsWith('blob:')))];
console.log('marker_found:', ok);
if (errText) console.log('error_text:', errText);
if (err) console.log('timeout_err:', err);
console.log('external_requests('+ext.length+'):'); ext.forEach(u => console.log('  ', u));
if (offline) console.log('blocked('+blocked.length+'):'), blocked.slice(0,10).forEach(u=>console.log('   x', u));
await b.close();
process.exit(ok?0:1);
