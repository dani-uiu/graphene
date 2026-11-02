// Left sidebar: Files, Program (header, intent, externs, inputs, asserts),
// Problems (diagnostics with quick fixes), and Tests (.cases.json).

import { api, type Analysis, type CaseResult, type CoverItem, type Entry } from "./api";
import { app } from "./app";
import { runPanel } from "./runpanel";
import { store } from "./store";
import { ask, h, mount, short, toast } from "./util";

type Section = "files" | "program" | "problems" | "run" | "tests";

export function sidebar(root: HTMLElement) {
  const body = h("div", { class: "side-body" });
  let section: Section = "files";
  const tabs: Record<Section, HTMLElement> = {
    files: h("button", { title: "Files" }, "Files"),
    program: h("button", { title: "Program" }, "Program"),
    problems: h("button", { title: "Problems" }, "Problems"),
    run: h("button", { title: "Run" }, "Run"),
    tests: h("button", { title: "Tests" }, "Tests"),
  };
  const render = () => {
    for (const [k, b] of Object.entries(tabs)) b.classList.toggle("active", k === section);
    const n = store.analysis?.diags.length ?? 0;
    tabs.problems.textContent = n ? `Problems (${n})` : "Problems";
    if (section === "files") mount(body, filesView());
    if (section === "program") mount(body, programView(store.analysis));
    if (section === "problems") mount(body, problemsView(store.analysis));
    if (section === "run") mount(body, runPanel.element);
    if (section === "tests") mount(body, testsView());
  };
  for (const [k, b] of Object.entries(tabs)) {
    b.addEventListener("click", () => {
      section = k as Section;
      render();
    });
  }
  mount(root, h("div", { class: "side-tabs" }, ...Object.values(tabs)), body);
  store.on("folder", () => section === "files" && render());
  store.on("tabs", () => section === "files" && render());
  store.on("analysis", () => {
    tabs.problems.textContent = store.analysis?.diags.length ? `Problems (${store.analysis.diags.length})` : "Problems";
    if (section === "program" || section === "problems") render();
  });
  render();
  return {
    show(s: Section) {
      section = s;
      render();
    },
  };
}

// ---------------------------------------------------------------- files

function filesView(): HTMLElement {
  const actions = h(
    "div",
    { class: "row-actions" },
    h("button", { onclick: () => app.openFolderDialog() }, "Open folder…"),
    h("button", { onclick: () => app.newTab() }, "New"),
    h("button", { onclick: () => app.openFileDialog() }, "Open file…"),
  );
  const tree = store.folder
    ? h("div", { class: "tree" }, treeNode(store.folder, 0, true))
    : h("p", { class: "hint" }, "Open a folder to browse its .grph programs, fixtures, and test cases.");
  return h("div", { class: "panel" }, actions, tree);
}

function treeNode(e: Entry, depth: number, open: boolean): HTMLElement {
  const pad = `padding-left:${8 + depth * 12}px`;
  if (!e.dir) {
    const active = store.tab?.path === e.path;
    const kind = e.name.endsWith(".grph") ? "grph" : "json";
    return h(
      "div",
      { class: `tree-file ${kind}${active ? " active" : ""}`, style: pad, title: e.path, onclick: () => app.openFile(e.path) },
      e.name,
    );
  }
  const kids = h("div", { class: "tree-kids" }, ...e.children.map((c) => treeNode(c, depth + 1, depth < 1)));
  kids.hidden = !open;
  const head = h(
    "div",
    {
      class: "tree-dir",
      style: pad,
      onclick: () => {
        kids.hidden = !kids.hidden;
        head.classList.toggle("open", !kids.hidden);
      },
    },
    e.name,
  );
  head.classList.toggle("open", open);
  return h("div", {}, head, kids);
}

// ---------------------------------------------------------------- program

function programView(a: Analysis | null): HTMLElement {
  if (!a) return h("p", { class: "hint panel" }, "Open a .grph program.");
  const version = h(
    "select",
    { onchange: (e: Event) => app.command({ cmd: "set_header", version: (e.target as HTMLSelectElement).value }) },
    h("option", { value: "0.2", selected: a.version === "0.2" }, "0.2"),
    h("option", { value: "0.1", selected: a.version === "0.1" }, "0.1"),
  );
  const intentLine = findLine(/^@intent\b/);
  const intent = h("input", {
    type: "text",
    value: a.intent ?? "",
    placeholder: "What does this program do?",
    onchange: (e: Event) => {
      const text = `@intent "${(e.target as HTMLInputElement).value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
      void app.command(intentLine ? { cmd: "set_line", line: intentLine, text } : { cmd: "insert_line", after: 1, text });
    },
  });
  return h(
    "div",
    { class: "panel" },
    h("label", { class: "field" }, h("span", {}, "Version"), version),
    h("label", { class: "field" }, h("span", {}, "Intent"), intent),
    section("Effects", a.externs.map((e) => item(
      `${e.is_async ? "fx~" : "fx"} ${e.name}(${e.params.map(([n, t]) => `${n}: ${t}`).join(", ")})${e.ret ? ` -> ${e.ret}` : ""}${e.errors.length ? ` ! ${e.errors.join(" | ")}` : ""}`,
      e.line,
    )), async () => {
      const t = await ask("New effect", "extern fx Name(arg: u64) -> u64 ! Failed", "extern fx Name(p: type) -> type ! Err");
      if (t) await app.command({ cmd: "add_extern", text: t });
    }),
    section("Inputs", a.inputs.map((i) => item(`${i.name}: ${i.ty}${i.node ? ` -> ${i.node}` : ""}`, i.line)), async () => {
      const t = await ask("New input", "name: u64 -> Node", "name: type, optionally -> Node");
      const m = t?.match(/^\s*([A-Za-z_]\w*)\s*:\s*([^-]+?)\s*(?:->\s*([A-Za-z_]\w*))?\s*$/);
      if (t && !m) return toast("write it as `name: type` or `name: type -> Node`", "error");
      if (m) await app.command({ cmd: "add_input", name: m[1], ty: m[2], node: m[3] ?? null });
    }),
    section("Assertions", a.asserts.map((s) => item(s.text, s.line)), async () => {
      const t = await ask("New assertion", "assert total", "assert total, or assert indep(Sink, out)");
      if (t) await app.command({ cmd: "add_assert", text: t });
    }),
  );
}

function findLine(re: RegExp): number | null {
  const lines = app.editor.text.split("\n");
  const i = lines.findIndex((l) => re.test(l.trim()));
  return i < 0 ? null : i + 1;
}

function section(title: string, items: HTMLElement[], onAdd: () => void): HTMLElement {
  return h(
    "div",
    { class: "section" },
    h("div", { class: "section-head" }, title, h("button", { class: "icon", title: `Add ${title.toLowerCase()}`, onclick: onAdd }, "+")),
    items.length ? h("div", {}, ...items) : h("p", { class: "hint" }, "None yet."),
  );
}

function item(text: string, line: number): HTMLElement {
  return h(
    "div",
    {
      class: "item mono",
      title: `line ${line}`,
      onclick: () => {
        const nodes = store.analysis?.nodes.filter((n) => n.line === line).map((n) => n.id) ?? [];
        app.selectNodes(nodes, false);
        app.editor.mark([line], true);
      },
    },
    text,
  );
}

// ---------------------------------------------------------------- problems

function problemsView(a: Analysis | null): HTMLElement {
  if (!a) return h("p", { class: "hint panel" }, "No program open.");
  if (!a.diags.length) {
    return h(
      "div",
      { class: "panel" },
      h("p", { class: "ok-line" }, a.checked ? "✓ No problems. Every check passed." : "No problems."),
    );
  }
  return h(
    "div",
    { class: "panel" },
    ...a.diags.map((d) =>
      h(
        "div",
        { class: "problem" },
        h(
          "div",
          { class: "problem-head", onclick: () => (d.path ? app.openAt(d.path, d.from, d.to) : app.editor.reveal(d.from, d.to)) },
          h("span", { class: "code" }, d.code),
          h("span", { class: "msg" }, d.message),
          h("span", { class: "line" }, `line ${d.line}`),
        ),
        d.label ? h("div", { class: "problem-label" }, d.label) : null,
        d.help ? h("div", { class: "problem-help" }, `help: ${d.help}`) : null,
        d.fixes.length
          ? h("div", { class: "row-actions" }, ...d.fixes.map((f) => h("button", { class: "primary", onclick: () => app.applyEdits(f.edits) }, f.label)))
          : null,
      ),
    ),
  );
}

// ---------------------------------------------------------------- tests

let lastCases: CaseResult[] | null = null;
let lastCoverage: CoverItem[] | null = null;

/** Totals per kind plus the items no case reaches. */
function coverageView(items: CoverItem[]): HTMLElement {
  const kinds: [CoverItem["kind"], string][] = [
    ["outcome", "outcomes"],
    ["branch", "branches"],
    ["effect", "effects"],
  ];
  const totals = kinds
    .map(([k, label]) => {
      const all = items.filter((i) => i.kind === k);
      return all.length ? `${all.filter((i) => i.covered).length}/${all.length} ${label}` : null;
    })
    .filter((x) => x !== null);
  const missing = items.filter((i) => !i.covered);
  return h(
    "div",
    { class: "coverage" },
    h("p", { class: missing.length ? "bad-line" : "ok-line" }, `Coverage: ${totals.join(", ") || "nothing to cover"}`),
    ...missing.map((i) =>
      h(
        "div",
        { class: "case fail", title: "Not reached by any case. Click to show the line.", onclick: () => app.editor.mark([i.line], true) },
        h("div", {}, `${i.kind} ${i.graph ? `${i.graph} › ` : ""}${i.label}`),
        h("div", { class: "case-detail mono" }, `line ${i.line}`),
      ),
    ),
  );
}

function testsView(): HTMLElement {
  const tab = store.tab;
  const out = h("div", {});
  const parallel = h("input", { type: "checkbox" });
  const cov = h("div", {});
  const showCoverage = () => mount(cov, lastCoverage ? coverageView(lastCoverage) : h("span", {}));
  const render = () => {
    showCoverage();
    if (!lastCases) return mount(out, h("p", { class: "hint" }, "Runs every case in the program's .cases.json against the text in the editor."));
    if (!lastCases.length) return mount(out, h("p", { class: "hint" }, "No .cases.json next to this program. Use “Save as test case” in the Run panel to create one."));
    const passed = lastCases.filter((c) => c.pass).length;
    mount(
      out,
      h("p", { class: passed === lastCases.length ? "ok-line" : "bad-line" }, `${passed} passed, ${lastCases.length - passed} failed`),
      ...lastCases.map((c) =>
        h(
          "div",
          { class: `case ${c.pass ? "pass" : "fail"}`, title: "Click to load this case into the Run panel" , onclick: () => runPanel.loadCase(c) },
          h("div", {}, `${c.pass ? "PASS" : "FAIL"} ${c.name}`),
          c.detail ? h("div", { class: "case-detail mono" }, c.detail) : null,
          !c.pass && c.actual ? h("div", { class: "case-detail mono" }, `got ${short(c.actual, 200)}`) : null,
        ),
      ),
    );
  };
  const runAll = async () => {
    if (!tab?.path) return toast("save the program first: cases live next to the .grph file", "error");
    try {
      lastCases = await api.runCases(app.editor.text, tab.path, parallel.checked);
    } catch (e) {
      toast(String(e), "error");
    }
    render();
  };
  const coverage = async () => {
    if (!tab?.path) return toast("save the program first: cases live next to the .grph file", "error");
    if (lastCoverage) {
      lastCoverage = null;
      app.graph.setCoverage(null);
      return showCoverage();
    }
    try {
      lastCoverage = await api.coverage(app.editor.text, tab.path);
      app.graph.setCoverage(lastCoverage);
    } catch (e) {
      toast(String(e), "error");
    }
    showCoverage();
  };
  render();
  return h(
    "div",
    { class: "panel" },
    h(
      "div",
      { class: "row-actions" },
      h("button", { class: "primary", onclick: runAll }, "Run all cases"),
      h("button", { onclick: coverage, title: "Which outcomes and branches no case reaches (shown dashed on the graph)" }, "Coverage"),
      h("label", { class: "check" }, parallel, " also in parallel"),
    ),
    cov,
    out,
  );
}
