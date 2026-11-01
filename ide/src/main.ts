// Entry point: builds the window layout and wires the editor, graph,
// panels, menus, command palette, and keyboard shortcuts together.

import "./style.css";
import { api, type NodeView } from "./api";
import { app } from "./app";
import { Editor } from "./editor";
import { GraphView } from "./graph";
import { connect, inspector, join, rename } from "./inspector";
import { runPanel } from "./runpanel";
import { sidebar } from "./sidebar";
import { defaultSettings, store, type GraphSettings } from "./store";
import { ask, fuzzy, h, menu, mount, toast } from "./util";

const STEP_ITEMS: [string, string][] = [
  ["f", "f(…) compute"],
  ["fx", "⚙ effect (fx)"],
  ["fx~", "⊗ fire & forget (fx~)"],
  ["match", "? match"],
  ["out", "out (result)"],
  ["fail", "fail"],
];

function build() {
  const tabsBar = h("div", { class: "tabs" });
  const side = h("aside", { class: "side" });
  const graphHost = h("div", { class: "graph-host" });
  const graphPane = h("section", { class: "graph-pane" }, graphHost);
  const insp = h("div", { class: "inspector" });
  const editorHost = h("div", { class: "editor-host" });
  const right = h("section", { class: "right" }, insp, h("div", { class: "split-handle", title: "Drag to resize" }), editorHost);
  const status = h("footer", { class: "status" });
  const main = h("main", { class: "main" }, side, h("div", { class: "v-handle" }), graphPane, h("div", { class: "v-handle" }), right);
  mount(document.getElementById("app")!, h("header", { class: "top" }, logo(), tabsBar, topActions()), main, status);

  const editor = new Editor(editorHost, {
    onChange: (text) => app.changed(text),
    onCursorLine: (line) => app.cursorLine(line),
    completions: () => {
      const a = store.analysis;
      if (!a) return [];
      return [...new Set([...a.nodes.filter((n) => n.defined).map((n) => n.name), ...a.inputs.map((i) => i.name), ...a.externs.map((e) => e.name)])];
    },
  });
  const graph = new GraphView(graphHost, store.settings, {
    onSelect: (ids, additive) => app.selectNodes(ids, additive),
    onOpen: (n) => app.editor.mark([n.line], true),
    onContext: (n, x, y) => (n ? nodeMenu(n, x, y) : canvasMenu(x, y)),
    onConnect: (from, to, x, y) => connectGesture(from, to, x, y),
    onPinsChanged: () => app.saveLayout(),
  });
  app.attach(editor, graph);
  graphPane.append(graphToolbar(), settingsPanel());
  sidebar(side);
  inspector(insp);
  tabs(tabsBar);
  statusBar(status);
  resizers(main, right);
  applyLayoutMode();
}

function logo(): HTMLElement {
  return h("div", { class: "logo", title: "Graphene IDE" }, h("span", { class: "dot" }), "Graphene");
}

function topActions(): HTMLElement {
  const mode = (m: typeof store.layoutMode, label: string) =>
    h("button", { class: "mode", "data-mode": m, title: `${label} view`, onclick: () => setLayoutMode(m) }, label);
  return h(
    "div",
    { class: "top-actions" },
    mode("graph", "Graph"),
    mode("split", "Split"),
    mode("text", "Text"),
    h("button", { title: "Command palette (Ctrl+P)", onclick: () => palette() }, "⌘ Commands"),
    h("button", { title: "Toggle light/dark theme", onclick: toggleTheme }, "◐"),
  );
}

// ---------------------------------------------------------------- tabs

function tabs(bar: HTMLElement) {
  const render = () => {
    mount(
      bar,
      ...store.tabs.map((t, i) =>
        h(
          "div",
          { class: `tab${i === store.active ? " active" : ""}`, title: t.path ?? "unsaved", onclick: () => app.switchTo(i) },
          h("span", {}, t.name),
          t.text !== t.saved ? h("span", { class: "modified" }, "●") : null,
          h("span", { class: "close", onclick: (e: Event) => (e.stopPropagation(), app.closeTab(i)) }, "×"),
        ),
      ),
    );
  };
  store.on("tabs", render);
  render();
}

// ---------------------------------------------------------------- graph chrome

function graphToolbar(): HTMLElement {
  const search = h("input", {
    type: "search",
    placeholder: "Search nodes…",
    oninput: (e: Event) => {
      store.search = (e.target as HTMLInputElement).value;
      app.graph.setSearch(store.search);
    },
  });
  return h(
    "div",
    { class: "graph-toolbar" },
    search,
    h("button", { title: "Fit the graph to the view (F)", onclick: () => app.graph.fit() }, "Fit"),
    h("button", { title: "Graph settings", onclick: () => document.querySelector(".graph-settings")?.classList.toggle("open") }, "⚙"),
  );
}

function settingsPanel(): HTMLElement {
  const panel = h("div", { class: "graph-settings" });
  const s = store.settings;
  const update = (patch: Partial<GraphSettings>) => {
    Object.assign(store.settings, patch);
    store.saveSettings();
    app.graph.setSettings(store.settings);
  };
  const slider = (label: string, key: keyof GraphSettings, min: number, max: number, step: number) =>
    h("label", { class: "slider" }, h("span", {}, label), h("input", { type: "range", min, max, step, value: s[key] as number, oninput: (e: Event) => update({ [key]: Number((e.target as HTMLInputElement).value) }) }));
  const toggle = (label: string, key: keyof GraphSettings) =>
    h("label", { class: "check" }, h("input", { type: "checkbox", checked: s[key] as boolean, onchange: (e: Event) => update({ [key]: (e.target as HTMLInputElement).checked }) }), ` ${label}`);
  const colorBy = h(
    "select",
    { onchange: (e: Event) => update({ colorBy: (e.target as HTMLSelectElement).value as GraphSettings["colorBy"] }) },
    ...(["kind", "guard", "type"] as const).map((v) => h("option", { value: v, selected: s.colorBy === v }, v === "guard" ? "active condition" : v)),
  );
  mount(
    panel,
    h("div", { class: "settings-head" }, "Filters"),
    toggle("Sinks (fx~ targets)", "showSinks"),
    toggle("Error paths", "showErrors"),
    toggle("Results (out / fail)", "showTerminals"),
    toggle("Expression reads (dotted)", "showRefs"),
    h("div", { class: "settings-head" }, "Display"),
    toggle("Arrows", "showArrows"),
    toggle("Labels", "showLabels"),
    h("label", { class: "slider" }, h("span", {}, "Color by"), colorBy),
    h("div", { class: "settings-head" }, "Local graph"),
    toggle("Only the selection's neighborhood", "local"),
    slider("Depth", "localDepth", 1, 5, 1),
    h("div", { class: "settings-head" }, "Forces"),
    slider("Center force", "centerForce", 0, 0.3, 0.01),
    slider("Repel force", "repelForce", 20, 800, 10),
    slider("Link force", "linkForce", 0.05, 1, 0.05),
    slider("Link distance", "linkDistance", 15, 160, 1),
    h("div", { class: "row-actions" }, h("button", { onclick: () => { store.settings = { ...defaultSettings }; store.saveSettings(); app.graph.setSettings(store.settings); panel.replaceWith(settingsPanel()); document.querySelector(".graph-settings")?.classList.add("open"); } }, "Reset")),
  );
  return panel;
}

// ---------------------------------------------------------------- menus

function nodeMenu(n: NodeView, x: number, y: number) {
  const name = app.nameOf(n);
  const items: Parameters<typeof menu>[2] = [{ label: n.detail || n.name, action: null }];
  if (name && n.kind !== "sink") {
    for (const [step, label] of STEP_ITEMS) items.push({ label: `Add ${label}`, action: () => app.command({ cmd: "add_step", from: name, step, effect: null }) });
    items.push("-", { label: "Connect to…", action: () => connect(name) });
  }
  const sel = [...store.selection].filter((id) => id.startsWith("v:")).map((id) => id.slice(2));
  if (name && sel.length >= 2 && sel.includes(name)) items.push({ label: `Join ${sel.length} selected…`, action: () => join(sel) });
  if (name) items.push({ label: "Rename…", action: () => rename(name) });
  items.push(
    { label: app.graph.isPinned(n.id) ? "Unpin" : "Pin in place", action: () => app.graph.togglePin(n.id) },
    { label: "Show local graph", action: () => { app.selectNodes([n.id], false); store.settings.local = true; store.saveSettings(); app.graph.setSettings(store.settings); } },
    { label: `Go to line ${n.line}`, action: () => app.editor.mark([n.line], true) },
    "-",
    { label: name ? "Delete node" : "Delete statement", danger: true, action: () => (name ? app.command({ cmd: "delete_node", name }) : app.command({ cmd: "delete_line", line: n.line })) },
  );
  menu(x, y, items);
}

function canvasMenu(x: number, y: number) {
  menu(x, y, [
    { label: "Add input…", action: () => addInput() },
    { label: "Add effect (extern)…", action: async () => { const t = await ask("New effect", "extern fx Name(arg: u64) -> u64 ! Failed"); if (t) await app.command({ cmd: "add_extern", text: t }); } },
    { label: "Add assert total", action: () => app.command({ cmd: "add_assert", text: "assert total" }) },
    "-",
    { label: "Fit view", action: () => app.graph.fit() },
    { label: store.settings.local ? "Show whole graph" : "Local graph of selection", action: () => { store.settings.local = !store.settings.local; store.saveSettings(); app.graph.setSettings(store.settings); } },
  ]);
}

async function addInput() {
  const t = await ask("New input", "name: u64 -> Node", "name: type, optionally -> Node");
  const m = t?.match(/^\s*([A-Za-z_]\w*)\s*:\s*([^-]+?)\s*(?:->\s*([A-Za-z_]\w*))?\s*$/);
  if (t && !m) return toast("write it as `name: type` or `name: type -> Node`", "error");
  if (m) await app.command({ cmd: "add_input", name: m[1], ty: m[2], node: m[3] ?? null });
}

/** Shift-drag result: connect two nodes, or pick a step to add. */
function connectGesture(from: NodeView, to: NodeView | null, x: number, y: number) {
  const name = app.nameOf(from);
  if (!name) return toast("start a connection from a value node", "error");
  if (to) {
    const target = app.nameOf(to);
    if (!target) return toast("connect to a value node", "error");
    void app.command({ cmd: "connect", from: name, to: target });
    return;
  }
  menu(x, y, [{ label: `New step after ${name}`, action: null }, ...STEP_ITEMS.map(([step, label]) => ({ label, action: () => void app.command({ cmd: "add_step", from: name, step, effect: null }) }))]);
}

// ---------------------------------------------------------------- palette

interface Cmd {
  label: string;
  key?: string;
  run: () => unknown;
}

function commands(): Cmd[] {
  const sel = [...store.selection].filter((id) => id.startsWith("v:")).map((id) => id.slice(2));
  const list: Cmd[] = [
    { label: "New program", key: "Ctrl+N", run: () => app.newTab() },
    { label: "Open folder…", key: "Ctrl+Shift+O", run: () => app.openFolderDialog() },
    { label: "Open file…", key: "Ctrl+O", run: () => app.openFileDialog() },
    { label: "Save", key: "Ctrl+S", run: () => app.save() },
    { label: "Save as…", key: "Ctrl+Shift+S", run: () => app.save(true) },
    { label: "Run program", key: "Ctrl+Enter", run: () => runPanel.run() },
    { label: "Format document (ASCII)", run: () => formatDoc(false) },
    { label: "Format document (Unicode)", run: () => formatDoc(true) },
    { label: "Fit graph", key: "F", run: () => app.graph.fit() },
    { label: "Toggle local graph", run: () => { store.settings.local = !store.settings.local; store.saveSettings(); app.graph.setSettings(store.settings); } },
    { label: "Add input…", run: () => addInput() },
    { label: "Token count", run: async () => { try { const t = await api.tokens(app.editor.text); toast(`ASCII ${t.ascii} tokens, Unicode ${t.unicode} tokens (${t.method})`); } catch (e) { toast(String(e), "error"); } } },
    { label: "Export graph as DOT…", run: exportDot },
    { label: "Copy graph image", run: copyImage },
    { label: "View: graph only", run: () => setLayoutMode("graph") },
    { label: "View: split", run: () => setLayoutMode("split") },
    { label: "View: text only", run: () => setLayoutMode("text") },
    { label: "Toggle light/dark theme", run: toggleTheme },
    { label: `Format on save: ${localStorage.getItem("graphene.formatOnSave") === "false" ? "off" : "on"} (toggle)`, run: () => localStorage.setItem("graphene.formatOnSave", localStorage.getItem("graphene.formatOnSave") === "false" ? "true" : "false") },
    { label: "Header: #!graphene 0.2", run: () => app.command({ cmd: "set_header", version: "0.2" }) },
  ];
  if (sel.length === 1) {
    list.push({ label: `Rename ${sel[0]}…`, run: () => rename(sel[0]) });
    for (const [step, label] of STEP_ITEMS) list.push({ label: `Add ${label} after ${sel[0]}`, run: () => app.command({ cmd: "add_step", from: sel[0], step, effect: null }) });
  }
  if (sel.length >= 2) list.push({ label: `Join ${sel.join(", ")}…`, run: () => join(sel) });
  return list;
}

function palette() {
  document.querySelector(".palette-overlay")?.remove();
  const input = h("input", { type: "text", placeholder: "Type a command…", spellcheck: "false" }) as HTMLInputElement;
  const list = h("div", { class: "palette-list" });
  let shown: Cmd[] = [];
  let index = 0;
  const close = () => overlay.remove();
  const render = () => {
    const q = input.value.toLowerCase();
    shown = commands().filter((c) => fuzzy(c.label.toLowerCase(), q));
    index = Math.min(index, Math.max(0, shown.length - 1));
    mount(list, ...shown.map((c, i) => h("div", { class: `palette-item${i === index ? " active" : ""}`, onclick: () => (close(), c.run()) }, h("span", {}, c.label), c.key ? h("kbd", {}, c.key) : null)));
  };
  input.addEventListener("input", () => ((index = 0), render()));
  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") index = Math.min(index + 1, shown.length - 1);
    else if (e.key === "ArrowUp") index = Math.max(index - 1, 0);
    else if (e.key === "Enter") {
      const c = shown[index];
      close();
      if (c) void c.run();
      return;
    } else if (e.key === "Escape") return close();
    else return;
    e.preventDefault();
    render();
  });
  const overlay = h("div", { class: "palette-overlay", onmousedown: (e: Event) => e.target === overlay && close() }, h("div", { class: "palette" }, input, list));
  document.body.append(overlay);
  render();
  input.focus();
}

async function formatDoc(unicode: boolean) {
  try {
    const out = await api.format(app.editor.text, unicode);
    app.editor.replaceAll(out);
  } catch (e) {
    toast(String(e), "error");
  }
}

async function exportDot() {
  try {
    const dot = await api.exportDot(app.editor.text);
    const { save } = await import("@tauri-apps/plugin-dialog");
    const path = await save({ filters: [{ name: "Graphviz", extensions: ["dot"] }], defaultPath: (store.tab?.name ?? "graph").replace(/\.grph$/, ".dot") });
    if (path) {
      await api.writeFile(path, dot);
      toast(`exported ${path}`);
    }
  } catch (e) {
    toast(String(e), "error");
  }
}

async function copyImage() {
  app.graph.canvas.toBlob(async (blob) => {
    if (!blob) return toast("could not render the graph", "error");
    try {
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      toast("graph image copied to the clipboard");
    } catch (e) {
      toast(`clipboard: ${e}`, "error");
    }
  });
}

// ---------------------------------------------------------------- layout

function setLayoutMode(m: typeof store.layoutMode) {
  store.layoutMode = m;
  localStorage.setItem("graphene.layout", m);
  applyLayoutMode();
}

function applyLayoutMode() {
  const saved = localStorage.getItem("graphene.layout") as typeof store.layoutMode | null;
  if (saved) store.layoutMode = saved;
  document.body.dataset.layout = store.layoutMode;
  document.querySelectorAll<HTMLElement>(".mode").forEach((b) => b.classList.toggle("active", b.dataset.mode === store.layoutMode));
}

function toggleTheme() {
  const next = document.documentElement.dataset.theme === "light" ? "dark" : "light";
  document.documentElement.dataset.theme = next;
  localStorage.setItem("graphene.theme", next);
  app.graph.readColors();
}

/** Drag handles between panes. */
function resizers(main: HTMLElement, right: HTMLElement) {
  const [left, mid] = main.querySelectorAll<HTMLElement>(".v-handle");
  const drag = (handle: HTMLElement, move: (e: PointerEvent) => void) => {
    handle.addEventListener("pointerdown", (e) => {
      handle.setPointerCapture(e.pointerId);
      const onMove = (ev: PointerEvent) => move(ev);
      handle.addEventListener("pointermove", onMove);
      handle.addEventListener("pointerup", () => handle.removeEventListener("pointermove", onMove), { once: true });
    });
  };
  drag(left, (e) => main.style.setProperty("--side-w", `${Math.max(180, Math.min(520, e.clientX))}px`));
  drag(mid, (e) => main.style.setProperty("--right-w", `${Math.max(280, Math.min(900, window.innerWidth - e.clientX))}px`));
  const split = right.querySelector<HTMLElement>(".split-handle")!;
  drag(split, (e) => {
    const r = right.getBoundingClientRect();
    right.style.setProperty("--insp-h", `${Math.max(120, Math.min(r.height - 120, e.clientY - r.top))}px`);
  });
}

// ---------------------------------------------------------------- status

function statusBar(root: HTMLElement) {
  const render = () => {
    const a = store.analysis;
    const t = store.tab;
    mount(
      root,
      h("span", {}, t ? (t.path ?? "unsaved") : ""),
      a ? h("span", {}, `Graphene ${a.version}`) : null,
      a ? h("span", { class: a.checked ? "good" : "bad" }, a.checked ? "✓ checks pass" : `${a.diags.length} problem(s)`) : null,
      a?.unicode ? h("span", {}, "Unicode") : null,
      store.run?.output ? h("span", {}, store.run.output.fault ? `last run: fault ${store.run.output.fault.code}` : store.run.output.result?.err !== undefined ? `last run: fail ${store.run.output.result.err}` : "last run: ok") : null,
    );
  };
  store.on("analysis", render);
  store.on("tabs", render);
  store.on("run", render);
  render();
}

// ---------------------------------------------------------------- keys

function keys() {
  window.addEventListener("keydown", (e) => {
    const mod = e.ctrlKey || e.metaKey;
    const inField = (e.target as HTMLElement).closest("input, textarea, .cm-editor, select");
    if (mod && e.key.toLowerCase() === "p") return e.preventDefault(), palette();
    if (mod && e.key.toLowerCase() === "s") return e.preventDefault(), app.save(e.shiftKey);
    if (mod && e.key.toLowerCase() === "n") return e.preventDefault(), app.newTab();
    if (mod && e.key.toLowerCase() === "o") return e.preventDefault(), e.shiftKey ? app.openFolderDialog() : app.openFileDialog();
    if (mod && e.key === "Enter") return e.preventDefault(), runPanel.run();
    if (inField) return;
    if (e.key === "f" || e.key === "F") app.graph.fit();
    if (e.key === "Delete" || e.key === "Backspace") void deleteSelection();
    if (e.key === "Escape") app.selectNodes([], false);
  });
}

async function deleteSelection() {
  const a = store.analysis;
  if (!a || !store.selection.size) return;
  const sel = a.nodes.filter((n) => store.selection.has(n.id));
  // Delete statements bottom-up so earlier line numbers stay valid.
  for (const n of sel.filter((n) => n.id.startsWith("s:")).sort((x, y) => y.line - x.line)) {
    await app.command({ cmd: "delete_line", line: n.line });
  }
  for (const n of sel.filter((n) => n.id.startsWith("v:"))) await app.command({ cmd: "delete_node", name: n.name });
  app.selectNodes([], false);
}

// ---------------------------------------------------------------- start

async function start() {
  // Handle for automated UI checks (e2e/smoke.mjs); harmless otherwise.
  (window as unknown as { __graphene: unknown }).__graphene = { app, store, runPanel, api };
  const theme = localStorage.getItem("graphene.theme");
  if (theme) document.documentElement.dataset.theme = theme;
  build();
  keys();
  const folder = localStorage.getItem("graphene.folder");
  if (folder) await app.openFolder(folder);
  app.newTab();
}

void start();
