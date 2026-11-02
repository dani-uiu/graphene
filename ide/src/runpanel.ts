// Run panel (docs/ide-plan.md §3.5): an input form generated from the `in`
// statements, a mock fixture picker, results with the effect log, a
// replay timeline over the run order, "record missing" for mock responses,
// and saving a run as a test case.

import { api, type CaseResult, type ElementView, type Entry, type HostChoice, type RunView } from "./api";
import { app } from "./app";
import { focusGroup } from "./steps";
import { store } from "./store";
import { ask, baseName, dirName, h, mount, short, toast } from "./util";

class RunPanel {
  element = h("div", { class: "panel run-panel" });
  /** Input values per program (by path or name), as typed. */
  private values = new Map<string, Record<string, string>>();
  private mocks = new Map<string, string>();
  /** Per program: answer effects from a mock fixture or a host config. */
  private modes = new Map<string, "mock" | "hosts">();
  private record = false;
  private parallel = false;
  private timer: number | undefined;

  constructor() {
    store.on("analysis", () => this.render());
    store.on("tabs", () => this.render());
    store.on("folder", () => this.render());
    this.render();
  }

  private key(): string {
    return store.tab?.path ?? store.tab?.name ?? "";
  }

  private get inputs(): Record<string, string> {
    const k = this.key();
    if (!this.values.has(k)) this.values.set(k, {});
    return this.values.get(k)!;
  }

  get mock(): string {
    return this.mocks.get(this.key()) ?? this.guessMock();
  }

  /** A fixture named like the program, if the folder has one. */
  private guessMock(): string {
    const name = store.tab?.name.replace(/\.grph$/, "") ?? "";
    const all = jsonFiles(store.folder);
    return all.find((p) => baseName(p) === `${name}.json`) ?? "";
  }

  render() {
    const a = store.analysis;
    if (!a || !app.isGraphene()) {
      mount(this.element, h("p", { class: "hint" }, "Open a .grph program to run it."));
      return;
    }
    const form = h("div", { class: "form" });
    for (const i of a.inputs) {
      const set = (e: Event) => (this.inputs[i.name] = (e.target as HTMLInputElement | HTMLSelectElement).value);
      let input: HTMLElement;
      if (i.variants.length) {
        // v0.4 enum: a dropdown of its variants.
        if (!i.variants.includes(this.inputs[i.name] ?? "")) this.inputs[i.name] = i.variants[0];
        input = h(
          "select",
          { onchange: set },
          ...i.variants.map((v) => h("option", { value: v, selected: this.inputs[i.name] === v }, v)),
        );
      } else {
        input = h("input", {
          // v0.4 date: a date picker, which gives `YYYY-MM-DD`.
          type: i.ty === "date" ? "date" : "text",
          value: this.inputs[i.name] ?? "",
          placeholder: i.ty,
          spellcheck: "false",
          oninput: set,
          onkeydown: (e: Event) => (e as KeyboardEvent).key === "Enter" && this.run(),
        });
      }
      form.append(h("label", { class: "field" }, h("span", { class: "mono" }, `${i.name}: ${i.ty}`), input));
    }
    if (!a.inputs.length) form.append(h("p", { class: "hint" }, "This program has no inputs."));
    const mockSel = h("select", {
      class: "mock-select",
      onchange: (e: Event) => this.mocks.set(this.key(), (e.target as HTMLSelectElement).value),
    });
    mockSel.append(h("option", { value: "" }, "No mock (effects fail with R003)"));
    for (const p of jsonFiles(store.folder)) mockSel.append(h("option", { value: p, selected: p === this.mock }, relative(p)));
    if (this.mock && !jsonFiles(store.folder).includes(this.mock)) mockSel.append(h("option", { value: this.mock, selected: true }, baseName(this.mock)));
    const par = h("input", { type: "checkbox", checked: this.parallel, onchange: (e: Event) => (this.parallel = (e.target as HTMLInputElement).checked) });
    const mode = this.modes.get(this.key()) ?? "mock";
    const modeSel = h(
      "select",
      { onchange: (e: Event) => (this.modes.set(this.key(), (e.target as HTMLSelectElement).value as "mock" | "hosts"), this.render()) },
      h("option", { value: "mock", selected: mode === "mock" }, "Mock fixture"),
      h("option", { value: "hosts", selected: mode === "hosts" }, "Host config (real effects)"),
    );
    const rec = h("input", { type: "checkbox", checked: this.record, onchange: (e: Event) => (this.record = (e.target as HTMLInputElement).checked) });
    mount(
      this.element,
      form,
      h("label", { class: "field" }, h("span", {}, "Effects"), modeSel),
      h("label", { class: "field" }, h("span", {}, mode === "mock" ? "Mock" : "Config"), mockSel),
      h(
        "div",
        { class: "row-actions" },
        h("button", { class: "primary", onclick: () => this.run(), title: "Ctrl+Enter" }, "▶ Run"),
        h("label", { class: "check" }, par, " also run in parallel"),
        h("label", { class: "check", title: "Write every answer to <program>.recorded.json, a mock fixture that replays this run" }, rec, " record"),
        this.mock ? h("button", { onclick: () => app.openFile(this.mock) }, "Edit mock") : null,
      ),
      this.results(store.run),
    );
  }

  async run() {
    const a = store.analysis;
    if (!a) return;
    const args: [string, string][] = a.inputs.map((i) => [i.name, this.inputs[i.name] ?? ""]);
    try {
      const r = await api.run(app.editor.text, args, this.hosts(), this.parallel);
      this.show(r);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  /** The selected effect source, plus where to record, if enabled. */
  private hosts(): HostChoice {
    const file = this.mock || null;
    const prog = store.tab?.path;
    const recordPath = this.record && prog ? prog.replace(/\.grph$/, ".recorded.json") : null;
    return (this.modes.get(this.key()) ?? "mock") === "hosts"
      ? { mockPath: null, hostPath: file, recordPath }
      : { mockPath: file, hostPath: null, recordPath };
  }

  private show(r: RunView) {
    store.run = r;
    store.replay = null;
    app.graph.setRun(r.error ? null : r);
    app.graph.setReplay(null);
    store.emit("run", r);
    this.render();
  }

  /** Fill the form from a test case and run it. */
  loadCase(c: CaseResult) {
    for (const [k, v] of Object.entries(c.inputs)) this.inputs[k] = typeof v === "string" ? v : JSON.stringify(v);
    this.mocks.set(this.key(), c.mock ?? "");
    void this.run();
  }

  private results(r: RunView | null): HTMLElement | null {
    if (!r) return null;
    if (r.error) return h("div", { class: "result bad" }, r.error);
    const out = r.output ?? {};
    const status = out.fault
      ? h("div", { class: "result bad" }, `fault ${out.fault.code}: ${out.fault.message}`)
      : out.result?.err !== undefined
        ? h("div", { class: "result warn" }, `fail → ${out.result.err}`)
        : h("div", { class: "result good" }, `ok → ${short(out.result?.ok, 120)}`);
    const effects = (out.effects ?? []) as any[];
    const log = h(
      "table",
      { class: "log" },
      h("tr", {}, h("th", {}, "kind"), h("th", {}, "effect"), h("th", {}, "args"), h("th", {}, "result")),
      ...effects.map((e) => h("tr", {}, h("td", {}, e.kind), h("td", {}, e.effect), h("td", { class: "mono" }, short(e.args, 60)), h("td", { class: "mono" }, e.result ? short(e.result, 60) : "—"))),
    );
    return h(
      "div",
      {},
      status,
      r.parallel_matches === null ? null : h("div", { class: r.parallel_matches ? "ok-line" : "bad-line" }, r.parallel_matches ? "✓ parallel run identical" : "✗ parallel run differs"),
      this.replayBar(r),
      this.elementPicker(r.elements),
      out.fault?.code === "R003" ? this.recordMissing(out.fault.message) : null,
      h("div", { class: "subhead" }, `Effect log (${effects.length})`),
      effects.length ? log : h("p", { class: "hint" }, "No effects were called."),
      h("div", { class: "row-actions" }, h("button", { onclick: () => this.saveCase(out) }, "Save as test case")),
      h("details", {}, h("summary", {}, "Raw output"), h("pre", { class: "mono" }, JSON.stringify(out, null, 2))),
    );
  }

  /** Pick one nested subgraph run to see its states on the graph. */
  private elementPicker(elements: ElementView[]): HTMLElement | null {
    if (!elements.length) return null;
    const label = (el: ElementView) => el.at.map(([n, i]) => (i === null ? n : `${n}[${i}]`)).join(" › ");
    const sel = h(
      "select",
      {
        onchange: (e: Event) => {
          const i = Number((e.target as HTMLSelectElement).value);
          const el = elements[i] ?? null;
          app.graph.setElement(el);
          focusGroup(el ? el.group : null);
        },
      },
      h("option", { value: "-1" }, "main program"),
      ...elements.map((el, i) => h("option", { value: String(i) }, label(el))),
    );
    return h("label", { class: "field" }, h("span", {}, "Show run of"), sel);
  }

  private replayBar(r: RunView): HTMLElement {
    const max = Math.max(0, ...r.nodes.map((n) => n.step));
    const label = h("span", { class: "mono" }, store.replay === null ? "all" : `step ${store.replay}/${max}`);
    const slider = h("input", {
      type: "range",
      min: 0,
      max,
      value: store.replay ?? max,
      oninput: (e: Event) => this.setReplay(Number((e.target as HTMLInputElement).value), label, max),
    }) as HTMLInputElement;
    const play = h("button", {
      onclick: () => {
        window.clearInterval(this.timer);
        let s = 0;
        this.timer = window.setInterval(() => {
          this.setReplay(s, label, max);
          slider.value = String(s);
          if (++s > max) window.clearInterval(this.timer);
        }, 350);
      },
    }, "▶ Replay");
    const all = h("button", {
      onclick: () => {
        window.clearInterval(this.timer);
        store.replay = null;
        app.graph.setReplay(null);
        slider.value = String(max);
        label.textContent = "all";
      },
    }, "Show all");
    return h("div", { class: "replay" }, play, slider, label, all);
  }

  private setReplay(step: number, label: HTMLElement, max: number) {
    store.replay = step;
    app.graph.setReplay(step);
    label.textContent = `step ${step}/${max}`;
    const at = store.run?.nodes.filter((n) => n.step === step && n.status !== "inactive").map((n) => n.id) ?? [];
    if (at.length) app.selectNodes(at, false, true);
  }

  /** R003: offer to add the missing response to the mock and run again. */
  private recordMissing(message: string): HTMLElement | null {
    const m = message.match(/no mock response for `([^`]+)` with args (\{.*\})$/);
    if (!m) return null;
    const [, effect, argsText] = m;
    const ext = store.analysis?.externs.find((e) => e.name === effect);
    const value = h("input", { type: "text", placeholder: ext?.ret ? `ok value as JSON, e.g. for ${ext.ret}` : "ok value as JSON", spellcheck: "false" });
    const err = h("select", {}, h("option", { value: "" }, "ok →"), ...(ext?.errors ?? []).map((e) => h("option", { value: e }, `err ${e}`)));
    const add = async () => {
      let response: Record<string, unknown>;
      if ((err as HTMLSelectElement).value) response = { err: (err as HTMLSelectElement).value };
      else {
        try {
          response = { ok: JSON.parse((value as HTMLInputElement).value) };
        } catch {
          return toast("the ok value must be JSON, e.g. 42, \"text\", or {\"a\": \"1.00\"}", "error");
        }
      }
      await this.addResponse(effect, JSON.parse(argsText), response);
    };
    return h(
      "div",
      { class: "record" },
      h("div", { class: "subhead" }, `Record a response for ${effect}`),
      h("div", { class: "mono hint" }, argsText),
      h("div", { class: "row-actions" }, err, value, h("button", { class: "primary", onclick: add }, "Add & run again")),
    );
  }

  private async addResponse(effect: string, args: unknown, response: Record<string, unknown>) {
    let path = this.mock;
    if (!path) {
      const prog = store.tab?.path;
      if (!prog) return toast("save the program first", "error");
      path = prog.replace(/\.grph$/, ".mock.json");
      this.mocks.set(this.key(), path);
    }
    let fixture: any = { effects: {} };
    try {
      fixture = JSON.parse(await api.readFile(path));
    } catch {
      /* new fixture */
    }
    fixture.effects ??= {};
    fixture.effects[effect] ??= { cases: [] };
    fixture.effects[effect].cases ??= [];
    fixture.effects[effect].cases.push({ args, ...response });
    await api.writeFile(path, JSON.stringify(fixture, null, 2) + "\n");
    await app.refreshFolder();
    toast(`added a ${effect} response to ${baseName(path)}`);
    await this.run();
  }

  private async saveCase(out: any) {
    const prog = store.tab?.path;
    if (!prog) return toast("save the program first: cases live next to the .grph file", "error");
    const name = await ask("Name this test case", "", "what this case shows");
    if (!name) return;
    const a = store.analysis;
    // Typed inputs as JSON text, so large integers stay exact.
    const parts = (a?.inputs ?? []).map((i) => {
      const raw = this.inputs[i.name] ?? "";
      const literal = i.ty === "u64" || i.ty === "i64" || i.ty === "bool" ? raw : JSON.stringify(raw);
      return `${JSON.stringify(i.name)}: ${literal || "null"}`;
    });
    try {
      await api.saveCase(prog, name, `{${parts.join(", ")}}`, this.mock || null, out);
      await app.refreshFolder();
      toast(`saved “${name}” to ${baseName(prog).replace(/\.grph$/, ".cases.json")}`);
    } catch (e) {
      toast(String(e), "error");
    }
  }
}

/** JSON files in the folder that look like mock fixtures. */
function jsonFiles(e: Entry | null): string[] {
  if (!e) return [];
  if (!e.dir) {
    return e.name.endsWith(".json") && !e.name.endsWith(".cases.json") && !e.name.endsWith(".layout.json") ? [e.path] : [];
  }
  return e.children.flatMap(jsonFiles);
}

function relative(p: string): string {
  const root = store.folder?.path;
  return root && p.startsWith(root) ? p.slice(root.length + 1) : `${baseName(dirName(p))}/${baseName(p)}`;
}

export const runPanel = new RunPanel();
