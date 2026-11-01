// The controller: owns tabs and files, runs analysis after every change,
// keeps the editor, graph, and panels in sync, and turns every graph
// gesture into an edit command that Rust converts into text edits.

import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import type { EditorState } from "@codemirror/state";
import { api, type Command, type NodeView, type TextEdit } from "./api";
import { Editor } from "./editor";
import { GraphView } from "./graph";
import { store, type Tab } from "./store";
import { baseName, debounce, toast } from "./util";

const NEW_PROGRAM = `#!graphene 0.2
@intent "Describe what this program does"

in x: u64 -> A
A -> f(x * 2) -> B
B -> out result: u64

assert total
`;

export class App {
  editor!: Editor;
  graph!: GraphView;
  private states = new Map<Tab, EditorState>();
  private analyzedText = "";
  private analyzing = false;
  private pending = false;

  attach(editor: Editor, graph: GraphView) {
    this.editor = editor;
    this.graph = graph;
    window.setInterval(() => void this.checkDisk(), 2000);
  }

  // ------------------------------------------------------------ analysis

  /** Called by the editor on every change. */
  readonly changed = debounce((text: string) => {
    const tab = store.tab;
    if (tab) {
      tab.text = text;
      store.emit("tabs", undefined);
    }
    void this.analyze();
  }, 60);

  /** Analyze the current text; coalesces bursts of changes. */
  async analyze() {
    if (this.analyzing) {
      this.pending = true;
      return;
    }
    this.analyzing = true;
    try {
      do {
        this.pending = false;
        const text = this.editor.text;
        if (!this.isGraphene()) {
          store.analysis = null;
          break;
        }
        const a = await api.analyze(text);
        this.analyzedText = text;
        store.analysis = a;
        this.graph.update(a);
        this.editor.showDiagnostics(a, text, (edits) => this.editor.apply(edits));
        store.emit("analysis", a);
      } while (this.pending);
    } catch (e) {
      toast(`analysis failed: ${e}`, "error");
    } finally {
      this.analyzing = false;
    }
  }

  isGraphene(): boolean {
    const t = store.tab;
    return !!t && (t.path === null || t.path.endsWith(".grph"));
  }

  // ------------------------------------------------------------ edits

  /** Run an edit command against the current text (one undo step). */
  async command(cmd: Command): Promise<boolean> {
    try {
      const edits = await api.apply(this.editor.text, cmd);
      this.applyEdits(edits);
      return true;
    } catch (e) {
      toast(String(e), "error");
      return false;
    }
  }

  applyEdits(edits: TextEdit[]) {
    this.editor.apply(edits);
  }

  /** Name used by edit commands for a node (statement nodes have none). */
  nameOf(n: NodeView): string | null {
    return n.id.startsWith("v:") ? n.name : null;
  }

  // ------------------------------------------------------------ selection

  selectNodes(ids: string[], additive: boolean, scroll = true) {
    const next = additive ? new Set(store.selection) : new Set<string>();
    for (const id of ids) {
      if (additive && next.has(id)) next.delete(id);
      else next.add(id);
    }
    store.select(next);
    this.graph.setSelection(store.selection);
    this.editor.mark(this.linesOf(next), scroll);
  }

  /** Editor cursor moved: select the graph nodes defined on that line. */
  cursorLine(line: number) {
    const a = store.analysis;
    if (!a) return;
    const ids = a.nodes.filter((n) => n.line === line && (n.id.startsWith("s:") || n.defined)).map((n) => n.id);
    store.select(ids);
    this.graph.setSelection(store.selection);
    this.editor.mark([], false);
  }

  linesOf(ids: Iterable<string>): number[] {
    const a = store.analysis;
    if (!a) return [];
    const out: number[] = [];
    for (const id of ids) {
      const n = a.nodes.find((x) => x.id === id);
      if (n) out.push(...(n.id.startsWith("s:") ? [n.line] : n.lines));
    }
    return out;
  }

  // ------------------------------------------------------------ tabs and files

  newTab(text = NEW_PROGRAM) {
    this.openTab({ path: null, name: "untitled.grph", text, saved: "", mtime: null });
  }

  async openFile(path: string) {
    const existing = store.tabs.findIndex((t) => t.path === path);
    if (existing >= 0) return this.switchTo(existing);
    try {
      const text = await api.readFile(path);
      const mtime = await api.mtime(path);
      this.openTab({ path, name: baseName(path), text, saved: text, mtime });
    } catch (e) {
      toast(String(e), "error");
    }
  }

  private openTab(tab: Tab) {
    this.stash();
    store.tabs.push(tab);
    this.states.set(tab, this.editor.fresh(tab.text));
    void this.show(store.tabs.length - 1);
  }

  /** Remember the editor state (with undo history) of the current tab. */
  private stash() {
    const t = store.tab;
    if (t) {
      this.states.set(t, this.editor.snapshot());
      t.text = this.editor.text;
    }
  }

  switchTo(i: number) {
    if (i === store.active) return;
    this.stash();
    void this.show(i);
  }

  private async show(i: number) {
    store.active = i;
    const tab = store.tab;
    store.run = null;
    store.replay = null;
    this.graph.setRun(null);
    this.graph.setReplay(null);
    store.emit("run", null);
    this.graph.clear();
    if (tab) {
      this.editor.load(this.states.get(tab) ?? this.editor.fresh(tab.text));
      await this.loadLayout(tab);
    }
    store.emit("tabs", undefined);
    await this.analyze();
    this.graph.fit();
  }

  closeTab(i: number) {
    const tab = store.tabs[i];
    if (tab && tab.text !== tab.saved && !confirm(`${tab.name} has unsaved changes. Close anyway?`)) return;
    store.tabs.splice(i, 1);
    if (tab) this.states.delete(tab);
    if (!store.tabs.length) {
      store.active = -1;
      this.newTab();
      return;
    }
    store.active = -1;
    void this.show(Math.min(i, store.tabs.length - 1));
  }

  async save(as = false) {
    const tab = store.tab;
    if (!tab) return;
    let path = tab.path;
    if (!path || as) {
      const chosen = await saveDialog({ filters: [{ name: "Graphene", extensions: ["grph"] }], defaultPath: tab.name });
      if (!chosen) return;
      path = chosen;
    }
    let text = this.editor.text;
    if (localStorage.getItem("graphene.formatOnSave") !== "false" && path.endsWith(".grph")) {
      try {
        const formatted = await api.format(text, store.analysis?.unicode ?? false);
        if (formatted !== text) {
          this.editor.replaceAll(formatted);
          text = formatted;
        }
      } catch {
        /* does not parse: save as written */
      }
    }
    try {
      await api.writeFile(path, text);
      tab.path = path;
      tab.name = baseName(path);
      tab.saved = text;
      tab.text = text;
      tab.mtime = await api.mtime(path);
      store.emit("tabs", undefined);
      await this.saveLayout();
      if (store.folder && path.startsWith(store.folder.path)) await this.refreshFolder();
    } catch (e) {
      toast(String(e), "error");
    }
  }

  async openFolderDialog() {
    const dir = await openDialog({ directory: true });
    if (typeof dir === "string") await this.openFolder(dir);
  }

  async openFileDialog() {
    const f = await openDialog({ filters: [{ name: "Graphene", extensions: ["grph", "json"] }] });
    if (typeof f === "string") await this.openFile(f);
  }

  async openFolder(path: string) {
    try {
      store.folder = await api.listDir(path);
      localStorage.setItem("graphene.folder", path);
      store.emit("folder", store.folder);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  async refreshFolder() {
    if (store.folder) await this.openFolder(store.folder.path);
  }

  /** Reload files changed on disk; warn when there are local edits too. */
  private async checkDisk() {
    const tab = store.tab;
    if (!tab?.path) return;
    const m = await api.mtime(tab.path).catch(() => null);
    if (m === null || tab.mtime === null || m === tab.mtime) return;
    tab.mtime = m;
    const disk = await api.readFile(tab.path).catch(() => null);
    if (disk === null || disk === tab.saved) return;
    if (this.editor.text === tab.saved) {
      this.editor.replaceAll(disk);
      tab.saved = disk;
      toast(`${tab.name} changed on disk and was reloaded`);
    } else {
      toast(`${tab.name} changed on disk; you have unsaved edits (save to overwrite)`, "error");
    }
  }

  // ------------------------------------------------------------ layout sidecar

  private layoutPath(tab: Tab): string | null {
    return tab.path && tab.path.endsWith(".grph") ? `${tab.path}.layout.json` : null;
  }

  private async loadLayout(tab: Tab) {
    this.graph.pins.clear();
    const p = this.layoutPath(tab);
    if (!p || (await api.mtime(p)) === null) return;
    try {
      const data = JSON.parse(await api.readFile(p)) as { pinned?: Record<string, [number, number]> };
      for (const [id, xy] of Object.entries(data.pinned ?? {})) this.graph.pins.set(id, xy);
    } catch {
      /* a broken sidecar only loses pins */
    }
  }

  readonly saveLayout = debounce(async () => {
    const tab = store.tab;
    const p = tab && this.layoutPath(tab);
    if (!p) return;
    const pinned = Object.fromEntries(this.graph.pins);
    if (!Object.keys(pinned).length && (await api.mtime(p)) === null) return;
    await api.writeFile(p, JSON.stringify({ pinned }, null, 2) + "\n").catch(() => undefined);
  }, 400);

  /** Text of the program as last analyzed (what the graph shows). */
  get shownText() {
    return this.analyzedText;
  }
}

export const app = new App();
