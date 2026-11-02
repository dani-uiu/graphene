// Application state and a tiny event bus. The editor text is the program;
// everything else here (analysis, selection, run results) is derived or UI
// state.

import type { Analysis, Entry, RunView } from "./api";

export interface Tab {
  /** File on disk, or null for an unsaved scratch program. */
  path: string | null;
  name: string;
  text: string;
  /** Text as last saved/loaded, to show the "modified" dot. */
  saved: string;
  mtime: number | null;
}

export interface GraphSettings {
  centerForce: number;
  repelForce: number;
  linkForce: number;
  linkDistance: number;
  showSinks: boolean;
  showErrors: boolean;
  showTerminals: boolean;
  showRefs: boolean;
  showArrows: boolean;
  showLabels: boolean;
  colorBy: "kind" | "guard" | "type";
  localDepth: number;
  local: boolean;
}

export const defaultSettings: GraphSettings = {
  centerForce: 0.08,
  repelForce: 260,
  linkForce: 0.7,
  linkDistance: 46,
  showSinks: true,
  showErrors: true,
  showTerminals: true,
  showRefs: false,
  showArrows: true,
  showLabels: true,
  colorBy: "kind",
  localDepth: 2,
  local: false,
};

type Events = {
  tabs: void;
  analysis: Analysis;
  selection: void;
  run: RunView | null;
  replay: number | null;
  settings: void;
  folder: Entry | null;
  layout: void;
  search: string;
};

type Handler<T> = (v: T) => void;

class Store {
  tabs: Tab[] = [];
  active = -1;
  folder: Entry | null = null;
  analysis: Analysis | null = null;
  /** Selected node ids (`v:NAME` / `s:LINE`). */
  selection = new Set<string>();
  run: RunView | null = null;
  /** Replay position in run order, or null to show the whole run. */
  replay: number | null = null;
  search = "";
  settings: GraphSettings = loadSettings();
  layoutMode: "split" | "graph" | "text" = "split";
  private handlers = new Map<keyof Events, Handler<never>[]>();

  on<K extends keyof Events>(k: K, f: Handler<Events[K]>) {
    const list = this.handlers.get(k) ?? [];
    list.push(f as Handler<never>);
    this.handlers.set(k, list);
  }

  emit<K extends keyof Events>(k: K, v: Events[K]) {
    for (const f of this.handlers.get(k) ?? []) (f as Handler<Events[K]>)(v);
  }

  get tab(): Tab | null {
    return this.tabs[this.active] ?? null;
  }

  /** The active file's path and every other tab's unsaved text, for
   *  resolving imports (v0.4). */
  workspace(): { path: string | null; buffers: Record<string, string> } {
    const buffers: Record<string, string> = {};
    this.tabs.forEach((t, i) => {
      if (i !== this.active && t.path && t.text !== t.saved) buffers[t.path] = t.text;
    });
    return { path: this.tab?.path ?? null, buffers };
  }

  /** True for a node drawn from an imported library (read-only here). */
  imported(group: string | null): boolean {
    return !!group && !!this.analysis?.groups.some((g) => g.name === group && g.file);
  }

  select(ids: Iterable<string>) {
    this.selection = new Set(ids);
    this.emit("selection", undefined);
  }

  saveSettings() {
    try {
      localStorage.setItem("graphene.settings", JSON.stringify(this.settings));
    } catch {
      /* storage unavailable: settings last for this session only */
    }
    this.emit("settings", undefined);
  }
}

function loadSettings(): GraphSettings {
  try {
    const raw = localStorage.getItem("graphene.settings");
    if (raw) return { ...defaultSettings, ...JSON.parse(raw) };
  } catch {
    /* fall back to defaults */
  }
  return { ...defaultSettings };
}

export const store = new Store();
