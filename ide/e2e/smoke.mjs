// End-to-end smoke test: drives the real UI in headless Edge (or Chrome)
// against the dev bridge, which runs the same Rust commands as the app.
//
//   cargo run -p graphene-ide-core --example bridge -- 1421   (in one shell)
//   npm run build && node e2e/smoke.mjs                       (in another)
//
// Env: BRIDGE (default http://127.0.0.1:1421), BROWSER (path to msedge or
// chrome), SHOTS (folder for screenshots, default e2e/shots).

import { mkdirSync, rmSync } from "node:fs";
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
await open("examples/learn/pricing.grph");
let a = await g(() => window.__graphene.store.analysis);
check("pricing checks", a.checked === true, JSON.stringify(a.diags));
check("graph has value, op, and terminal nodes", ["value", "op", "match", "out", "fail", "sink", "input"].every((k) => a.nodes.some((n) => n.kind === k)));
await shot("1-pricing");

// 2. Select a node: editor marks its lines.
await g(() => window.__graphene.app.selectNodes(["v:B"], false));
await settle(300);
check("selecting B marks editor lines", (await page.$$(".cm-graph-selected")).length >= 3);
await shot("2-selected");

// 2b. Manual arrangement survives switching tabs.
{
  const canvas = await page.$(".graph-canvas");
  const box = await canvas.boundingBox();
  const [sx, sy] = await g(() => window.__graphene.app.graph.screenOf("v:B"));
  await page.mouse.move(box.x + sx, box.y + sy);
  await page.mouse.down();
  for (let i = 1; i <= 10; i++) await page.mouse.move(box.x + sx + i * 12, box.y + sy + i * 8);
  await page.mouse.up();
  await settle(900);
  const placed = await g(() => window.__graphene.app.graph.positionOf("v:B"));
  const other = await g(() => window.__graphene.app.graph.positionOf("v:D"));
  const view = await g(() => window.__graphene.app.graph.snapshot().view);
  check("dropped node stays pinned", await g(() => window.__graphene.app.graph.isPinned("v:B")));
  await open("examples/learn/order_total.grph");
  await g((p) => window.__graphene.app.openFile(p), join(REPO, "examples/learn/pricing.grph"));
  await settle(1200);
  const back = await g(() => window.__graphene.app.graph.positionOf("v:B"));
  const otherBack = await g(() => window.__graphene.app.graph.positionOf("v:D"));
  const viewBack = await g(() => window.__graphene.app.graph.snapshot().view);
  const same = (a, b) => a && b && Math.abs(a[0] - b[0]) < 0.5 && Math.abs(a[1] - b[1]) < 0.5;
  check("dragged node is where it was dropped after switching tabs", same(placed, back), `${placed} vs ${back}`);
  check("other nodes keep their positions too", same(other, otherBack), `${other} vs ${otherBack}`);
  check("zoom and pan are restored", Math.abs(view.k - viewBack.k) < 1e-9 && Math.abs(view.x - viewBack.x) < 0.5, `${JSON.stringify(view)} vs ${JSON.stringify(viewBack)}`);
  await g(() => window.__graphene.app.graph.togglePin("v:B"));
}

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
await page.select(".run-panel select.mock-select", join(REPO, "examples", "mocks", "users.json"));
await page.click(".run-panel button.primary");
await page.waitForFunction(() => window.__graphene.store.run !== null);
await settle(800);
const run = await g(() => window.__graphene.store.run);
check("run returns 80.00", run.output?.result?.ok?.final_price === "80.00", JSON.stringify(run.output ?? run.error));
check("taken branch active, other inactive", run.nodes.find((n) => n.id === "v:C1")?.status === "active" && run.nodes.find((n) => n.id === "v:C2")?.status === "inactive");
await shot("3-run");

// 6. Quick fix from the Problems panel.
await open("examples/errors/unhandled_err.grph");
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
await open("examples/apps/checkout_v2.grph");
const cases = await g((p) => window.__graphene.api.runCases(window.__graphene.app.editor.text, p, true), join(REPO, "examples/apps/checkout_v2.grph"));
check("checkout_v2 cases all pass", cases.length === 7 && cases.every((c) => c.pass), JSON.stringify(cases.filter((c) => !c.pass)));
await page.click(".side-tabs button:nth-child(5)");
await page.click(".panel .row-actions button.primary");
await settle(900);
await shot("5-tests");

// 7b. v0.3: subgraphs, a map run with a list input, elements, coverage.
await open("examples/apps/cart_checkout.grph");
a = await g(() => window.__graphene.store.analysis);
check("cart_checkout checks", a.checked === true, JSON.stringify(a.diags));
check("subgraphs become groups", a.groups.length >= 1 && a.nodes.some((n) => n.group !== null));
const mapNode = a.nodes.find((n) => n.calls && n.op === "map");
check("map step names its subgraph", !!mapNode, JSON.stringify(a.nodes.filter((n) => n.calls)));
await shot("9-groups");
const cartPath = join(REPO, "examples/apps/cart_checkout.grph");
const cartCases = await g((p) => window.__graphene.api.runCases(window.__graphene.app.editor.text, p, true), cartPath);
check("cart_checkout cases all pass", cartCases.length > 0 && cartCases.every((c) => c.pass), JSON.stringify(cartCases.filter((c) => !c.pass)));
await page.click(".side-tabs button:nth-child(4)");
await settle(200);
await g((c) => (window.__graphene.store.run = null, window.__graphene.runPanel.loadCase(c)), cartCases[0]);
await page.waitForFunction(() => window.__graphene.store.run !== null);
await settle(800);
const cartRun = await g(() => window.__graphene.store.run);
check("list input run succeeds", cartRun.output?.result?.ok !== undefined, JSON.stringify(cartRun.output ?? cartRun.error));
check("each map ran once per cart line", cartRun.elements.length > 0 && [...new Set(cartRun.elements.map((e) => e.group))].every((grp) => cartRun.elements.filter((e) => e.group === grp).length === 2), JSON.stringify(cartRun.elements.map((e) => e.at)));
check("map node shows a count", cartRun.nodes.some((n) => n.count === 2), JSON.stringify(cartRun.nodes.filter((n) => n.count !== null)));
const picker = await page.$$(".run-panel select");
await picker[picker.length - 1].select("1");
await settle(700);
check("picking an element drills into its subgraph", (await g(() => window.__graphene.app.graph.focusGroup)) === cartRun.elements[1].group);
check("breadcrumb shows while drilled in", await g(() => !document.querySelector(".graph-toolbar .crumb").hidden));
await shot("10-element");
await page.click(".graph-toolbar .crumb button");
await settle(400);
check("breadcrumb returns to the whole program", (await g(() => window.__graphene.app.graph.focusGroup)) === null);
await page.click(".side-tabs button:nth-child(5)");
await settle(200);
await page.click(".panel .row-actions button:nth-child(2)");
await page.waitForSelector(".coverage");
await settle(600);
check("coverage report shows totals", /Coverage: \d+\/\d+ outcomes/.test(await page.$eval(".coverage", (e) => e.textContent)));
await shot("11-coverage");

// 7c. v0.3.1 syntax (if, %, ++, indexing, new functions) checks in the IDE.
await open("examples/apps/invoice_split.grph");
a = await g(() => window.__graphene.store.analysis);
check("invoice_split (v0.3.1) checks", a.checked === true, JSON.stringify(a.diags));

// 7d. Each view mode leaves the right panes visible (Text once showed nothing).
const area = (sel) => g((s) => { const r = document.querySelector(s)?.getBoundingClientRect(); return r ? r.width * r.height : 0; }, sel);
for (const [mode, editor, graph] of [["text", true, false], ["graph", false, true], ["split", true, true]]) {
  await page.click(`.mode[data-mode="${mode}"]`);
  await settle(400);
  check(`${mode} view shows the ${[editor && "editor", graph && "graph"].filter(Boolean).join(" and ")}`,
    (await area(".cm-editor") > 0) === editor && (await area(".graph-canvas") > 0) === graph);
}

// 7e. v0.4: a program with imports, enum and date inputs, extract.
await open("examples/apps/late_invoices.grph");
a = await g(() => window.__graphene.store.analysis);
check("late_invoices (imports) checks", a.checked === true, JSON.stringify(a.diags));
const lineTotal = a.groups.find((x) => x.name === "LineTotal");
check("imported subgraph is a group marked with its library", lineTotal?.file === "../lib/billing.grph", JSON.stringify(a.groups));
await page.click(".side-tabs button:nth-child(4)");
await settle(300);
check("enum input is a dropdown of its variants", (await page.$$eval(".run-panel select option", (os) => os.map((o) => o.value))).join(",").includes("Standard,Gold"));
check("date inputs get a date picker", (await page.$$(".run-panel input[type=date]")).length === 2);
const invoicePath = join(REPO, "examples/apps/late_invoices.grph");
const invoiceCases = await g((p) => window.__graphene.api.runCases(window.__graphene.app.editor.text, p, true), invoicePath);
check("late_invoices cases all pass in the IDE", invoiceCases.length === 4 && invoiceCases.every((c) => c.pass), JSON.stringify(invoiceCases.filter((c) => !c.pass)));
await g((c) => (window.__graphene.store.run = null, window.__graphene.runPanel.loadCase(c)), invoiceCases[1]);
await page.waitForFunction(() => window.__graphene.store.run !== null);
await settle(800);
const invoiceRun = await g(() => window.__graphene.store.run);
check("run with an enum input (Gold) waives the fee", invoiceRun.output?.result?.ok?.notice?.fee === "0.00", JSON.stringify(invoiceRun.output ?? invoiceRun.error));
check("the imported subgraph's run is painted", invoiceRun.elements.some((e) => e.group === "LineTotal"));
await shot("12-imports");
await g(() => window.__graphene.app.newTab("#!graphene 0.4\nin price: dec -> P\nP -> f(price * 1.21) -> GROSS\nGROSS -> f(round(GROSS, 2)) -> R\nR -> out total: dec\n"));
await settle(900);
await g(() => {
  const v = window.__graphene.app.editor.view;
  v.dispatch({ selection: { anchor: v.state.doc.line(3).from, head: v.state.doc.line(4).to } });
});
await g(() => window.__graphene.app.extractSubgraph("Tax"));
await settle(900);
a = await g(() => window.__graphene.store.analysis);
text = await g(() => window.__graphene.app.editor.text);
check("extract into subgraph adds a group and a call", a.groups.some((x) => x.name === "Tax") && text.includes("P -> call Tax(price: price) -> R"), text);
check("the extracted program checks", a.checked === true, JSON.stringify(a.diags));
await shot("13-extract");

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
// The drag test pins a node, which writes a layout sidecar; remove it.
rmSync(join(REPO, "examples/learn/pricing.grph.layout.json"), { force: true });
console.log(failures ? `${failures} check(s) failed` : "all checks passed");
process.exit(failures ? 1 : 0);
