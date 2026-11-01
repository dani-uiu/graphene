// End-to-end smoke test: drives the real UI in headless Edge (or Chrome)
// against the dev bridge, which runs the same Rust commands as the app.
//
//   cargo run -p graphene-ide-core --example bridge -- 1421   (in one shell)
//   npm run build && node e2e/smoke.mjs                       (in another)
//
// Env: BRIDGE (default http://127.0.0.1:1421), BROWSER (path to msedge or
// chrome), SHOTS (folder for screenshots, default e2e/shots).

import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import puppeteer from "puppeteer-core";

const BRIDGE = process.env.BRIDGE ?? "http://127.0.0.1:1421";
const BROWSER =
  process.env.BROWSER ?? "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const SHOTS = resolve(process.env.SHOTS ?? "e2e/shots");
const REPO = resolve("..");
mkdirSync(SHOTS, { recursive: true });

let failures = 0;
function check(name, ok, detail = "") {
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${ok || !detail ? "" : `: ${detail}`}`);
  if (!ok) failures++;
}

const browser = await puppeteer.launch({ executablePath: BROWSER, headless: true, args: ["--no-sandbox"] });
const page = await browser.newPage();
await page.setViewport({ width: 1440, height: 900 });
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
await page.evaluateOnNewDocument((folder) => {
  localStorage.setItem("graphene.folder", folder);
  localStorage.setItem("graphene.layout", "split");
}, REPO);
await page.goto(BRIDGE, { waitUntil: "networkidle0" });

const g = (fn, ...args) => page.evaluate(fn, ...args);
const settle = (ms = 700) => new Promise((r) => setTimeout(r, ms));
const shot = (name) => page.screenshot({ path: join(SHOTS, `${name}.png`) });

async function open(rel) {
  await g((p) => window.__graphene.app.openFile(p), join(REPO, rel));
  await page.waitForFunction(() => window.__graphene.store.analysis !== null);
  await settle(1500);
}

// 1. Open a program from the file tree; it checks and draws.
await page.waitForSelector(".tree-file");
check("file tree lists examples", (await page.$$eval(".tree-file", (els) => els.map((e) => e.textContent))).includes("pricing.grph"));
await open("examples/valid/pricing.grph");
let a = await g(() => window.__graphene.store.analysis);
check("pricing checks", a.checked === true, JSON.stringify(a.diags));
check("graph has value, op, and terminal nodes", ["value", "op", "match", "out", "fail", "sink", "input"].every((k) => a.nodes.some((n) => n.kind === k)));
await shot("1-pricing");

// 2. Select a node: editor marks its lines.
await g(() => window.__graphene.app.selectNodes(["v:B"], false));
await settle(300);
check("selecting B marks editor lines", (await page.$$(".cm-graph-selected")).length >= 3);
await shot("2-selected");

// 3. Graph action -> text edit -> undo.
await g(() => window.__graphene.app.command({ cmd: "rename", old: "B", new: "USER" }));
await settle(600);
let text = await g(() => window.__graphene.app.editor.text);
check("rename edits the text", text.includes("USER.role") && !/\bB\b/.test(text));
check("renamed program still checks", (await g(() => window.__graphene.store.analysis.checked)) === true);
await page.click(".cm-content");
await page.keyboard.down("Control");
await page.keyboard.press("z");
await page.keyboard.up("Control");
await settle(600);
text = await g(() => window.__graphene.app.editor.text);
check("one undo reverts the graph action", text.includes("B.role") && !text.includes("USER"));

// 4. Add a step from the graph; it lands in the text and the graph.
await g(() => window.__graphene.app.command({ cmd: "add_step", from: "D", step: "fx~", effect: null }));
await settle(700);
a = await g(() => window.__graphene.store.analysis);
check("added fx~ step appears as a node", a.nodes.some((n) => n.kind === "sink" && n.name === "S1"));
await g(() => window.__graphene.app.editor.view.dispatch({ changes: { from: 0, to: window.__graphene.app.editor.view.state.doc.length, insert: window.__graphene.store.tab.saved } }));
await settle(600);

// 5. Run with inputs and a mock; values and the taken branch show up.
await page.click(".side-tabs button:nth-child(4)");
await settle(200);
await page.type(".run-panel input[type=text]", "1");
await page.select(".run-panel select", join(REPO, "fixtures", "users.json"));
await page.click(".run-panel button.primary");
await page.waitForFunction(() => window.__graphene.store.run !== null);
await settle(800);
const run = await g(() => window.__graphene.store.run);
check("run returns 80.00", run.output?.result?.ok?.final_price === "80.00", JSON.stringify(run.output ?? run.error));
check("taken branch active, other inactive", run.nodes.find((n) => n.id === "v:C1")?.status === "active" && run.nodes.find((n) => n.id === "v:C2")?.status === "inactive");
await shot("3-run");

// 6. Quick fix from the Problems panel.
await open("examples/invalid/unhandled_err.grph");
a = await g(() => window.__graphene.store.analysis);
check("invalid program shows G005", a.diags.some((d) => d.code === "G005"));
await page.click(".side-tabs button:nth-child(3)");
await settle(300);
await shot("4-problems");
await page.click(".problem .row-actions button.primary");
await settle(900);
a = await g(() => window.__graphene.store.analysis);
check("quick fix removes G005", !a.diags.some((d) => d.code === "G005"), JSON.stringify(a.diags.map((d) => d.code)));

// 7. Test cases for a v0.2 program, all passing (also in parallel).
await open("examples/valid/checkout_v2.grph");
const cases = await g((p) => window.__graphene.api.runCases(window.__graphene.app.editor.text, p, true), join(REPO, "examples/valid/checkout_v2.grph"));
check("checkout_v2 cases all pass", cases.length === 7 && cases.every((c) => c.pass), JSON.stringify(cases.filter((c) => !c.pass)));
await page.click(".side-tabs button:nth-child(5)");
await page.click(".panel .row-actions button.primary");
await settle(900);
await shot("5-tests");

// 8. Local graph, settings, light theme, palette.
await g(() => window.__graphene.app.selectNodes(["v:RATE"], false));
await page.click(".graph-toolbar button:last-child");
await settle(200);
await shot("6-settings");
await page.click(".top-actions button:last-child");
await settle(500);
await shot("7-light");
await page.keyboard.down("Control");
await page.keyboard.press("p");
await page.keyboard.up("Control");
await settle(200);
check("command palette opens", (await page.$(".palette")) !== null);
await shot("8-palette");

check("no page errors", errors.length === 0, errors.join(" | "));
await browser.close();
console.log(failures ? `${failures} check(s) failed` : "all checks passed");
process.exit(failures ? 1 : 0);
