// Graph view in the style of Obsidian's graph: a gently moving force
// layout on a canvas, hover highlighting of neighbors, labels that fade in
// with zoom, a local-graph mode, and run overlays. The view never edits
// the program itself; it reports gestures through `GraphHooks`, and the
// app turns them into text edits.

import {
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type Simulation,
  type SimulationLinkDatum,
  type SimulationNodeDatum,
} from "d3-force";
import type { Analysis, CoverItem, EdgeView, ElementView, NodeState, NodeView, RunView } from "./api";
import type { GraphSettings } from "./store";
import { short, valueText } from "./util";

export interface GNode extends SimulationNodeDatum {
  id: string;
  data: NodeView;
  r: number;
  degree: number;
  pinned: boolean;
}

interface GLink extends SimulationLinkDatum<GNode> {
  data: EdgeView;
  source: GNode;
  target: GNode;
}

/** A tab's graph as the user left it: positions, pins, and camera. */
export interface GraphSnapshot {
  positions: Map<string, [number, number]>;
  pins: Map<string, [number, number]>;
  view: { x: number; y: number; k: number };
}

export interface GraphHooks {
  onSelect: (ids: string[], additive: boolean) => void;
  onOpen: (node: NodeView) => void;
  onContext: (node: NodeView | null, x: number, y: number) => void;
  /** Shift-drag from `from`: released on `to`, or on empty space. */
  onConnect: (from: NodeView, to: NodeView | null, x: number, y: number) => void;
  onPinsChanged: () => void;
}

type Drag =
  | { kind: "node"; node: GNode; moved: boolean; sx: number; sy: number; additive: boolean }
  | { kind: "pan"; sx: number; sy: number; ox: number; oy: number; moved: boolean }
  | { kind: "connect"; from: GNode; x: number; y: number };

const GLYPH: Record<string, string> = { f: "f", fx: "⚙", "fx~": "⊗", call: "↳", map: "∀", filter: "⊂" };

export class GraphView {
  readonly canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private sim: Simulation<GNode, GLink>;
  private all = new Map<string, GNode>();
  private links: GLink[] = [];
  private visible: GNode[] = [];
  private visibleLinks: GLink[] = [];
  private selection = new Set<string>();
  private states = new Map<string, NodeState>();
  /** States of one nested run (a `map` element) laid over its group. */
  private element: ElementView | null = null;
  /** Only this subgraph's body is shown (drill-in), or everything. */
  focusGroup: string | null = null;
  /** Coverage: statement ids and `op->target` edges no case reaches. */
  private uncovered: Set<string> | null = null;
  private faultNode: string | null = null;
  private replay: number | null = null;
  private search = "";
  private hover: GNode | null = null;
  private drag: Drag | null = null;
  private view = { x: 0, y: 0, k: 1 };
  private fitted = false;
  private restoring: GraphSnapshot | null = null;
  private dirty = true;
  private colors: Record<string, string> = {};
  /** Pinned positions loaded from the layout sidecar, by node id. */
  pins = new Map<string, [number, number]>();

  constructor(parent: HTMLElement, private settings: GraphSettings, private hooks: GraphHooks) {
    this.canvas = document.createElement("canvas");
    this.canvas.className = "graph-canvas";
    this.canvas.tabIndex = 0;
    parent.append(this.canvas);
    const ctx = this.canvas.getContext("2d");
    if (!ctx) throw new Error("canvas 2D is not available");
    this.ctx = ctx;
    this.sim = forceSimulation<GNode, GLink>()
      .alphaDecay(0.03)
      .velocityDecay(0.35)
      .on("tick", () => (this.dirty = true));
    this.applyForces();
    this.readColors();
    new ResizeObserver(() => this.resize()).observe(parent);
    this.bindInput();
    const loop = () => {
      if (this.dirty) {
        this.dirty = false;
        this.draw();
      }
      requestAnimationFrame(loop);
    };
    requestAnimationFrame(loop);
  }

  // ---------------------------------------------------------------- data

  /** v0.4: the library each imported group comes from. */
  private groupFiles = new Map<string, string>();

  update(a: Analysis) {
    this.groupFiles = new Map(a.groups.filter((g) => g.file).map((g) => [g.name, g.file!]));
    const old = this.all;
    const bySig = new Map<string, GNode>();
    for (const n of old.values()) bySig.set(signature(n.data, a), n);
    const next = new Map<string, GNode>();
    const used = new Set<GNode>();
    for (const d of a.nodes) {
      // Reuse the same object (position, velocity, drag state); statement
      // nodes whose line moved are found by their signature instead.
      let prev = old.get(d.id);
      if (!prev && d.id.startsWith("s:")) prev = bySig.get(signature(d, a));
      if (prev && used.has(prev)) prev = undefined;
      const n: GNode = prev ?? { id: d.id, data: d, r: 5, degree: 0, pinned: false };
      used.add(n);
      n.id = d.id;
      n.data = d;
      const pin = this.pins.get(d.id);
      n.pinned = !!pin;
      if (pin) [n.fx, n.fy] = pin;
      else if (!(this.drag?.kind === "node" && this.drag.node.id === d.id)) n.fx = n.fy = null;
      next.set(d.id, n);
    }
    this.links = a.edges
      .filter((e) => next.has(e.from) && next.has(e.to))
      .map((e) => ({ data: e, source: next.get(e.from)!, target: next.get(e.to)! }));
    for (const n of next.values()) n.degree = 0;
    for (const l of this.links) {
      if (l.data.kind === "ref") continue;
      l.source.degree++;
      l.target.degree++;
    }
    for (const n of next.values()) {
      n.r = radius(n);
      if (n.x === undefined) this.place(n, next);
    }
    this.all = next;
    const restoring = this.restoring;
    this.restoring = null;
    if (restoring) {
      // Coming back to a tab: put every node exactly where it was and
      // keep the camera; only nodes that are new since then move.
      for (const n of next.values()) {
        const p = restoring.positions.get(n.id);
        if (p) [n.x, n.y] = p;
        n.vx = n.vy = 0;
      }
      for (const n of next.values()) if (n.x === undefined) this.place(n, next);
      this.view = { ...restoring.view };
      this.fitted = true;
      this.rebuild(false);
      return;
    }
    this.rebuild();
    if (!this.fitted && this.visible.length) {
      // Let the layout settle a little before the first fit.
      this.sim.tick(300);
      this.fit();
      this.fitted = true;
    }
  }

  /** Put a new node next to an already-placed neighbor. */
  private place(n: GNode, nodes: Map<string, GNode>) {
    for (const l of this.links) {
      const other = l.source.id === n.id ? l.target : l.target.id === n.id ? l.source : null;
      const o = other && nodes.get(other.id);
      if (o && o.x !== undefined && o.y !== undefined) {
        n.x = o.x + (Math.random() - 0.5) * 40;
        n.y = o.y + 30 + Math.random() * 20;
        return;
      }
    }
  }

  /** Forget positions and fit again (switching to another program). */
  clear() {
    this.all.clear();
    this.links = [];
    this.fitted = false;
    this.hover = null;
    this.restoring = null;
  }

  /** Everything needed to show this graph again exactly as it is now. */
  snapshot(): GraphSnapshot {
    return { positions: this.positions(), pins: new Map(this.pins), view: { ...this.view } };
  }

  /** Restore a snapshot; applied by the next `update` for this program. */
  restore(s: GraphSnapshot) {
    this.clear();
    this.pins = new Map(s.pins);
    this.restoring = s;
  }

  /** Current position of a node in graph coordinates. */
  positionOf(id: string): [number, number] | null {
    const n = this.all.get(id);
    return n && n.x !== undefined && n.y !== undefined ? [n.x, n.y] : null;
  }

  /** Where a node is drawn, in canvas pixels (for tests and tooltips). */
  screenOf(id: string): [number, number] | null {
    const p = this.positionOf(id);
    return p && [p[0] * this.view.k + this.view.x, p[1] * this.view.k + this.view.y];
  }

  setSelection(ids: Set<string>) {
    this.selection = ids;
    if (this.settings.local) this.rebuild();
    this.dirty = true;
  }

  setRun(run: RunView | null) {
    this.states = new Map((run?.nodes ?? []).map((s) => [s.id, s]));
    this.faultNode = run?.fault_node ?? null;
    this.element = null;
    this.dirty = true;
  }

  /** Show one nested run's states on its subgraph's nodes. */
  setElement(el: ElementView | null) {
    this.element = el;
    this.dirty = true;
  }

  /** Show only `group`'s body (drill into a subgraph), or everything. */
  setFocusGroup(group: string | null) {
    this.focusGroup = group;
    this.rebuild();
    this.fitted = false;
    // Fit once the layout has had time to settle around the new set.
    window.setTimeout(() => this.fit(), 50);
    window.setTimeout(() => this.fit(), 600);
  }

  /** Mark what no test case reaches (dashed), or clear with null. */
  setCoverage(items: CoverItem[] | null) {
    if (!items) {
      this.uncovered = null;
    } else {
      const ids = new Set<string>();
      for (const i of items.filter((x) => !x.covered)) {
        if (i.kind === "outcome") ids.add(`s:${i.line}`);
        if (i.kind === "branch" && i.target) {
          ids.add(`s:${i.line}->${i.graph ? `v:${i.graph}/${i.target}` : `v:${i.target}`}`);
        }
      }
      this.uncovered = ids;
    }
    this.dirty = true;
  }

  /** The state shown for a node: the picked element's, else the run's. */
  private stateOf(id: string): NodeState | undefined {
    const el = this.element;
    if (el) {
      const n = this.all.get(id);
      if (n?.data.group === el.group) return el.nodes.find((x) => x.id === id);
    }
    return this.states.get(id);
  }

  setReplay(step: number | null) {
    this.replay = step;
    this.dirty = true;
  }

  setSearch(q: string) {
    this.search = q.trim().toLowerCase();
    this.dirty = true;
  }

  setSettings(s: GraphSettings) {
    this.settings = s;
    this.applyForces();
    this.rebuild();
  }

  /** Re-read theme colors (after switching light/dark). */
  readColors() {
    const css = getComputedStyle(document.documentElement);
    for (const k of [
      "bg", "edge", "edge-err", "edge-ref", "node", "node-op", "node-input", "node-match",
      "node-out", "node-fail", "node-sink", "accent", "text", "text-dim", "error", "value",
    ]) {
      this.colors[k] = css.getPropertyValue(`--g-${k}`).trim() || "#888";
    }
    this.dirty = true;
  }

  positions(): Map<string, [number, number]> {
    const m = new Map<string, [number, number]>();
    for (const n of this.all.values()) if (n.x !== undefined && n.y !== undefined) m.set(n.id, [n.x, n.y]);
    return m;
  }

  togglePin(id: string) {
    const n = this.all.get(id);
    if (!n) return;
    n.pinned = !n.pinned;
    if (n.pinned && n.x !== undefined && n.y !== undefined) {
      n.fx = n.x;
      n.fy = n.y;
      this.pins.set(id, [n.x, n.y]);
    } else {
      n.fx = n.fy = null;
      this.pins.delete(id);
      this.sim.alpha(0.3).restart();
    }
    this.hooks.onPinsChanged();
    this.dirty = true;
  }

  isPinned(id: string) {
    return this.all.get(id)?.pinned ?? false;
  }

  /** Recompute which nodes are shown and restart the layout on them.
   *  `reheat: false` keeps the current arrangement still. */
  private rebuild(reheat = true) {
    const s = this.settings;
    const shown = (n: GNode) => {
      const k = n.data.kind;
      if (k === "sink" && !s.showSinks) return false;
      if ((k === "out" || k === "fail") && !s.showTerminals) return false;
      if (!s.showErrors && this.onErrorPath(n)) return false;
      return true;
    };
    let vis = [...this.all.values()].filter(shown);
    if (this.focusGroup !== null) vis = vis.filter((n) => n.data.group === this.focusGroup);
    if (s.local && this.selection.size) vis = this.neighborhood(vis, s.localDepth);
    const ids = new Set(vis.map((n) => n.id));
    this.visible = vis;
    this.visibleLinks = this.links.filter(
      (l) => ids.has(l.source.id) && ids.has(l.target.id) && (s.showRefs || l.data.kind !== "ref"),
    );
    this.sim.nodes(vis);
    // Parameters have no flow edges; their reads pull them gently toward
    // the steps that use them, so they do not drift off on their own.
    const flow = this.links.filter(
      (l) => ids.has(l.source.id) && ids.has(l.target.id) && (l.data.kind !== "ref" || l.source.data.kind === "param"),
    );
    (this.sim.force("link") as ReturnType<typeof forceLink<GNode, GLink>>).links(flow);
    if (reheat) this.sim.alpha(Math.max(this.sim.alpha(), 0.35)).restart();
    else this.sim.alpha(0).stop();
    this.dirty = true;
  }

  /** A node reached only through error edges (an error handler). */
  private onErrorPath(n: GNode): boolean {
    return this.links.some((l) => l.target.id === n.id && l.data.kind === "err");
  }

  private neighborhood(nodes: GNode[], depth: number): GNode[] {
    const allowed = new Set(nodes.map((n) => n.id));
    const seen = new Set([...this.selection].filter((id) => allowed.has(id)));
    let frontier = [...seen];
    for (let d = 0; d < depth; d++) {
      const next: string[] = [];
      for (const l of this.links) {
        if (l.data.kind === "ref") continue;
        for (const [a, b] of [[l.source.id, l.target.id], [l.target.id, l.source.id]]) {
          if (frontier.includes(a) && allowed.has(b) && !seen.has(b)) {
            seen.add(b);
            next.push(b);
          }
        }
      }
      frontier = next;
    }
    return nodes.filter((n) => seen.has(n.id));
  }

  private applyForces() {
    const s = this.settings;
    this.sim
      .force("charge", forceManyBody<GNode>().strength((n) => -s.repelForce * (n.data.id.startsWith("s:") ? 0.45 : 1)).distanceMax(600))
      .force(
        "link",
        forceLink<GNode, GLink>(this.visibleLinks.filter((l) => l.data.kind !== "ref"))
          .id((n) => n.id)
          .distance((l) => s.linkDistance * (l.source.id.startsWith("s:") || l.target.id.startsWith("s:") ? 0.6 : 1))
          .strength((l) => (l.data.kind === "ref" ? s.linkForce * 0.15 : s.linkForce)),
      )
      .force("x", forceX<GNode>(0).strength(s.centerForce))
      .force("y", forceY<GNode>(0).strength(s.centerForce))
      .force("collide", forceCollide<GNode>((n) => n.r + 6));
    this.sim.alpha(0.5).restart();
  }

  // ---------------------------------------------------------------- view

  private resize() {
    const r = this.canvas.parentElement!.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    this.canvas.width = Math.max(1, Math.round(r.width * dpr));
    this.canvas.height = Math.max(1, Math.round(r.height * dpr));
    this.canvas.style.width = `${r.width}px`;
    this.canvas.style.height = `${r.height}px`;
    this.dirty = true;
  }

  /** Zoom and pan so every visible node fits. */
  fit() {
    const ns = this.visible.filter((n) => n.x !== undefined);
    const w = this.canvas.clientWidth || 800;
    const hgt = this.canvas.clientHeight || 600;
    if (!ns.length) {
      this.view = { x: w / 2, y: hgt / 2, k: 1 };
      return;
    }
    const xs = ns.map((n) => n.x!);
    const ys = ns.map((n) => n.y!);
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    const k = Math.min(1.6, Math.max(0.12, Math.min(w / (x1 - x0 + 160), hgt / (y1 - y0 + 160))));
    this.view = { k, x: w / 2 - ((x0 + x1) / 2) * k, y: hgt / 2 - ((y0 + y1) / 2) * k };
    this.dirty = true;
  }

  /** Center the view on a node without changing zoom. */
  focus(id: string) {
    const n = this.all.get(id);
    if (!n || n.x === undefined || n.y === undefined) return;
    this.view.x = this.canvas.clientWidth / 2 - n.x * this.view.k;
    this.view.y = this.canvas.clientHeight / 2 - n.y * this.view.k;
    this.dirty = true;
  }

  private toWorld(px: number, py: number): [number, number] {
    return [(px - this.view.x) / this.view.k, (py - this.view.y) / this.view.k];
  }

  private hit(px: number, py: number): GNode | null {
    const [x, y] = this.toWorld(px, py);
    let best: GNode | null = null;
    let bestD = Infinity;
    for (const n of this.visible) {
      if (n.x === undefined || n.y === undefined) continue;
      const d = Math.hypot(n.x - x, n.y - y);
      const reach = n.r + 5 / this.view.k;
      if (d < reach && d < bestD) {
        best = n;
        bestD = d;
      }
    }
    return best;
  }

  // ---------------------------------------------------------------- input

  private bindInput() {
    const c = this.canvas;
    const pos = (e: MouseEvent): [number, number] => {
      const r = c.getBoundingClientRect();
      return [e.clientX - r.left, e.clientY - r.top];
    };
    c.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      c.setPointerCapture(e.pointerId);
      const [px, py] = pos(e);
      const n = this.hit(px, py);
      if (n && e.shiftKey) {
        this.drag = { kind: "connect", from: n, x: px, y: py };
      } else if (n) {
        this.drag = { kind: "node", node: n, moved: false, sx: px, sy: py, additive: e.ctrlKey || e.metaKey };
      } else {
        this.drag = { kind: "pan", sx: px, sy: py, ox: this.view.x, oy: this.view.y, moved: false };
      }
    });
    c.addEventListener("pointermove", (e) => this.onMove(...pos(e)));
    c.addEventListener("pointerup", (e) => this.onUp(e, ...pos(e)));
    c.addEventListener("pointerleave", () => {
      if (!this.drag && this.hover) {
        this.hover = null;
        this.dirty = true;
      }
    });
    c.addEventListener("dblclick", (e) => {
      const n = this.hit(...pos(e));
      if (n) this.hooks.onOpen(n.data);
    });
    c.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      const n = this.hit(...pos(e));
      this.hooks.onContext(n?.data ?? null, e.clientX, e.clientY);
    });
    c.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const [px, py] = pos(e);
        const k = Math.min(6, Math.max(0.08, this.view.k * Math.exp(-e.deltaY * 0.0015)));
        this.view.x = px - ((px - this.view.x) * k) / this.view.k;
        this.view.y = py - ((py - this.view.y) * k) / this.view.k;
        this.view.k = k;
        this.dirty = true;
      },
      { passive: false },
    );
  }

  private onMove(px: number, py: number) {
    const d = this.drag;
    if (!d) {
      const n = this.hit(px, py);
      if (n !== this.hover) {
        this.hover = n;
        this.canvas.style.cursor = n ? "pointer" : "default";
        this.dirty = true;
      }
      return;
    }
    if (d.kind === "pan") {
      d.moved ||= Math.hypot(px - d.sx, py - d.sy) > 3;
      this.view.x = d.ox + (px - d.sx);
      this.view.y = d.oy + (py - d.sy);
    } else if (d.kind === "node") {
      d.moved ||= Math.hypot(px - d.sx, py - d.sy) > 3;
      if (d.moved) {
        [d.node.fx, d.node.fy] = this.toWorld(px, py);
        this.sim.alphaTarget(0.25).restart();
      }
    } else {
      d.x = px;
      d.y = py;
      this.hover = this.hit(px, py);
    }
    this.dirty = true;
  }

  private onUp(e: PointerEvent, px: number, py: number) {
    const d = this.drag;
    this.drag = null;
    if (!d) return;
    if (d.kind === "node") {
      this.sim.alphaTarget(0);
      if (!d.moved) {
        this.hooks.onSelect([d.node.id], d.additive);
      } else {
        // A dropped node stays where the user put it (a manual
        // arrangement); "Unpin" hands it back to the layout.
        d.node.pinned = true;
        this.pins.set(d.node.id, [d.node.fx ?? 0, d.node.fy ?? 0]);
        this.hooks.onPinsChanged();
      }
    } else if (d.kind === "pan" && !d.moved) {
      this.hooks.onSelect([], false);
    } else if (d.kind === "connect") {
      const to = this.hit(px, py);
      this.hooks.onConnect(d.from.data, to && to !== d.from ? to.data : null, e.clientX, e.clientY);
    }
    this.dirty = true;
  }

  // ---------------------------------------------------------------- draw

  private draw() {
    const { ctx, canvas } = this;
    const dpr = window.devicePixelRatio || 1;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = this.colors.bg;
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.setTransform(dpr * this.view.k, 0, 0, dpr * this.view.k, dpr * this.view.x, dpr * this.view.y);
    const focus = this.focusSet();
    this.drawGroups();
    for (const l of this.visibleLinks) this.drawLink(l, focus);
    for (const n of this.visible) this.drawNode(n, focus);
    for (const n of this.visible) this.drawLabel(n, focus);
    const d = this.drag;
    if (d?.kind === "connect" && d.from.x !== undefined && d.from.y !== undefined) {
      const [x, y] = this.toWorld(d.x, d.y);
      ctx.strokeStyle = this.colors.accent;
      ctx.lineWidth = 1.5 / this.view.k;
      ctx.setLineDash([6 / this.view.k, 4 / this.view.k]);
      ctx.beginPath();
      ctx.moveTo(d.from.x, d.from.y);
      ctx.lineTo(x, y);
      ctx.stroke();
      ctx.setLineDash([]);
    }
  }

  /** Soft hulls around each subgraph's body, labeled with its name. */
  private drawGroups() {
    const { ctx } = this;
    const groups = new Map<string, GNode[]>();
    for (const n of this.visible) {
      if (n.data.group && n.x !== undefined) {
        const list = groups.get(n.data.group) ?? [];
        list.push(n);
        groups.set(n.data.group, list);
      }
    }
    for (const [name, ns] of groups) {
      const cx = ns.reduce((a, n) => a + n.x!, 0) / ns.length;
      const cy = ns.reduce((a, n) => a + n.y!, 0) / ns.length;
      const r = Math.max(30, ...ns.map((n) => Math.hypot(n.x! - cx, n.y! - cy) + n.r + 18));
      const color = paletteColor(name);
      ctx.globalAlpha = 0.07;
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(cx, cy, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.globalAlpha = 0.45;
      ctx.strokeStyle = color;
      ctx.lineWidth = 1 / this.view.k;
      ctx.setLineDash([5 / this.view.k, 4 / this.view.k]);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = 0.85;
      ctx.fillStyle = color;
      ctx.textAlign = "center";
      ctx.font = `600 ${12 / this.view.k}px Inter, "Segoe UI", sans-serif`;
      const file = this.groupFiles.get(name);
      ctx.fillText(file ? `${name} · ${file} (read-only)` : name, cx, cy - r - 6 / this.view.k);
      ctx.globalAlpha = 1;
    }
  }

  /** Nodes to keep bright: the hovered node and its neighbors. */
  private focusSet(): Set<string> | null {
    const h = this.hover;
    if (!h) return null;
    const s = new Set([h.id]);
    for (const l of this.visibleLinks) {
      if (l.source.id === h.id) s.add(l.target.id);
      if (l.target.id === h.id) s.add(l.source.id);
    }
    return s;
  }

  /** 0..1 brightness of a node from hover, search, run, and replay state. */
  private alphaOf(n: GNode, focus: Set<string> | null): number {
    let a = 1;
    if (focus && !focus.has(n.id)) a = 0.12;
    if (this.search && !this.matches(n)) a = Math.min(a, 0.12);
    const st = this.stateOf(n.id);
    if (this.states.size && (!st || st.status === "inactive")) a = Math.min(a, 0.22);
    if (this.replay !== null && st && st.step > this.replay) a = Math.min(a, 0.22);
    return a;
  }

  private matches(n: GNode): boolean {
    const q = this.search;
    return n.data.name.toLowerCase().includes(q) || n.data.detail.toLowerCase().includes(q);
  }

  private drawLink(l: GLink, focus: Set<string> | null) {
    const { ctx } = this;
    const s = l.source;
    const t = l.target;
    if (s.x === undefined || s.y === undefined || t.x === undefined || t.y === undefined) return;
    const a = Math.min(this.alphaOf(s, focus), this.alphaOf(t, focus));
    const hot = focus && focus.has(s.id) && focus.has(t.id) && (s === this.hover || t === this.hover);
    const kind = l.data.kind;
    ctx.globalAlpha = hot ? 0.95 : 0.55 * a;
    ctx.strokeStyle = hot ? this.colors.accent : kind === "err" ? this.colors["edge-err"] : kind === "ref" ? this.colors["edge-ref"] : this.colors.edge;
    ctx.lineWidth = (hot ? 1.6 : 1) / Math.sqrt(this.view.k);
    if (kind === "ref" || this.uncovered?.has(`${s.id}->${t.id}`)) ctx.setLineDash([3 / this.view.k, 3 / this.view.k]);
    const ang = Math.atan2(t.y - s.y, t.x - s.x);
    const ex = t.x - Math.cos(ang) * (t.r + 1.5);
    const ey = t.y - Math.sin(ang) * (t.r + 1.5);
    ctx.beginPath();
    ctx.moveTo(s.x, s.y);
    ctx.lineTo(ex, ey);
    ctx.stroke();
    ctx.setLineDash([]);
    if (this.settings.showArrows && this.view.k > 0.5 && kind !== "ref") {
      const sz = 5 / Math.sqrt(this.view.k);
      ctx.fillStyle = ctx.strokeStyle;
      ctx.beginPath();
      ctx.moveTo(ex, ey);
      ctx.lineTo(ex - sz * Math.cos(ang - 0.4), ey - sz * Math.sin(ang - 0.4));
      ctx.lineTo(ex - sz * Math.cos(ang + 0.4), ey - sz * Math.sin(ang + 0.4));
      ctx.closePath();
      ctx.fill();
    }
    if (l.data.label && this.settings.showLabels && (this.view.k > 1.5 || hot)) {
      ctx.globalAlpha = hot ? 1 : Math.min(1, (this.view.k - 1.5) / 0.4) * a;
      ctx.fillStyle = kind === "err" ? this.colors["edge-err"] : this.colors["text-dim"];
      ctx.font = `${10 / this.view.k}px Inter, "Segoe UI", sans-serif`;
      ctx.textAlign = "center";
      ctx.fillText(l.data.label, (s.x + t.x) / 2, (s.y + t.y) / 2 - 3 / this.view.k);
    }
    ctx.globalAlpha = 1;
  }

  private colorOf(n: GNode): string {
    const c = this.colors;
    const by = this.settings.colorBy;
    if (by !== "kind" && (n.data.kind === "value" || n.data.kind === "input" || n.data.kind === "param")) {
      const key = by === "guard" ? n.data.guard : n.data.ty;
      if (key) return paletteColor(key);
    }
    switch (n.data.kind) {
      case "input":
      case "param":
        return c["node-input"];
      case "op":
        return c["node-op"];
      case "match":
        return c["node-match"];
      case "out":
        return c["node-out"];
      case "fail":
        return c["node-fail"];
      case "sink":
        return c["node-sink"];
      default:
        return c.node;
    }
  }

  private drawNode(n: GNode, focus: Set<string> | null) {
    const { ctx } = this;
    if (n.x === undefined || n.y === undefined) return;
    const a = this.alphaOf(n, focus);
    const color = this.hover === n || this.selection.has(n.id) ? this.colors.accent : this.colorOf(n);
    ctx.globalAlpha = a;
    ctx.fillStyle = color;
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.6 / Math.sqrt(this.view.k);
    const k = n.data.kind;
    ctx.beginPath();
    if (k === "out" || k === "fail") polygon(ctx, n.x, n.y, n.r, 6, Math.PI / 6);
    else if (k === "match") polygon(ctx, n.x, n.y, n.r * 1.25, 4, 0);
    else ctx.arc(n.x, n.y, n.r, 0, Math.PI * 2);
    if (k === "sink" || k === "param") {
      ctx.setLineDash(k === "sink" ? [2.5, 2] : [1.2, 1.6]);
      ctx.stroke();
      ctx.setLineDash([]);
    } else if (k === "input") {
      ctx.lineWidth = 2.4;
      ctx.stroke();
      ctx.globalAlpha = a * 0.35;
      ctx.fill();
    } else {
      ctx.fill();
    }
    ctx.globalAlpha = a;
    if (n.data.writers > 1) {
      ctx.setLineDash([2, 2]);
      ring(ctx, n.x, n.y, n.r + 2.5, this.colorOf(n), 0.8);
      ctx.setLineDash([]);
    }
    if (n.data.error || this.faultNode === n.id) ring(ctx, n.x, n.y, n.r + 2.5, this.colors.error, 1.8);
    if (this.uncovered?.has(n.id)) {
      ctx.setLineDash([2, 2]);
      ring(ctx, n.x, n.y, n.r + 4, this.colors["text-dim"], 1);
      ctx.setLineDash([]);
    }
    if (this.selection.has(n.id)) ring(ctx, n.x, n.y, n.r + 5, this.colors.accent, 1.4);
    if (n.pinned) {
      ctx.fillStyle = this.colors["text-dim"];
      ctx.beginPath();
      ctx.arc(n.x + n.r * 0.8, n.y - n.r * 0.8, 1.6, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
  }

  private drawLabel(n: GNode, focus: Set<string> | null) {
    if (!this.settings.showLabels || n.x === undefined || n.y === undefined) return;
    const { ctx } = this;
    const k = this.view.k;
    const isOp = n.data.kind === "op" || n.data.kind === "match";
    const near = focus?.has(n.id) || this.selection.has(n.id);
    // Labels keep a constant size on screen and fade in with zoom.
    const fade = Math.min(1, Math.max(0, (k - (isOp ? 1.2 : 0.5)) / 0.4));
    const a = (near ? 1 : fade) * this.alphaOf(n, focus);
    if (a <= 0.02) return;
    const px = (size: number) => size / k;
    ctx.globalAlpha = a;
    ctx.textAlign = "center";
    ctx.fillStyle = n.data.error ? this.colors.error : isOp ? this.colors["text-dim"] : this.colors.text;
    ctx.font = `${px(isOp ? 10 : 12)}px Inter, "Segoe UI", sans-serif`;
    ctx.fillText(labelOf(n.data), n.x, n.y + n.r + px(13));
    const showDetail = near && n.data.detail && (n.data.kind === "op" || n.data.kind === "match");
    if (showDetail) {
      ctx.fillStyle = this.colors["text-dim"];
      ctx.font = `${px(10.5)}px "JetBrains Mono", Consolas, monospace`;
      ctx.fillText(short(n.data.detail, 56), n.x, n.y + n.r + px(26));
    }
    const st = this.stateOf(n.id);
    if (isOp && st?.count !== null && st?.count !== undefined && (k > 0.8 || near)) {
      ctx.fillStyle = this.colors.value;
      ctx.font = `${px(10)}px Inter, "Segoe UI", sans-serif`;
      ctx.fillText(`×${st.count}`, n.x, n.y - n.r - px(5));
    }
    const shown = st && st.status !== "inactive" && st.value !== null && st.value !== undefined;
    if (shown && !isOp && (k > 0.8 || near) && (this.replay === null || st.step <= this.replay)) {
      ctx.globalAlpha = Math.max(a, near ? 1 : 0);
      ctx.fillStyle = this.colors.value;
      ctx.font = `${px(10.5)}px "JetBrains Mono", Consolas, monospace`;
      ctx.fillText(`= ${valueText(st.value, near ? 60 : 28)}`, n.x, n.y - n.r - px(6));
    }
    ctx.globalAlpha = 1;
  }
}

function labelOf(d: NodeView): string {
  if (d.kind === "op" && d.calls) return `${d.op} ${d.name}`;
  if (d.kind === "op") return d.op === "f" ? "f" : `${GLYPH[d.op ?? "f"]} ${d.name}`;
  if (d.kind === "match") return "?";
  return d.name;
}

function radius(n: GNode): number {
  switch (n.data.kind) {
    case "op":
      return 3.5;
    case "match":
      return 4.5;
    case "out":
    case "fail":
      return 7;
    case "input":
    case "param":
      return 7 + Math.sqrt(n.degree);
    default:
      return 4.5 + Math.sqrt(n.degree) * 1.6;
  }
}

/** Identity of a statement node that survives line shifts. */
function signature(d: NodeView, a: Analysis): string {
  if (!d.id.startsWith("s:")) return d.id;
  const around = a.edges
    .filter((e) => e.from === d.id || e.to === d.id)
    .map((e) => (e.from === d.id ? `>${e.to}` : `<${e.from}`))
    .sort()
    .join(",");
  return `${d.kind}|${d.detail}|${around}`;
}

function polygon(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, sides: number, rot: number) {
  for (let i = 0; i < sides; i++) {
    const t = rot + (i * 2 * Math.PI) / sides;
    const px = x + r * Math.cos(t);
    const py = y + r * Math.sin(t);
    if (i === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.closePath();
}

function ring(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, color: string, w: number) {
  ctx.strokeStyle = color;
  ctx.lineWidth = w;
  ctx.beginPath();
  ctx.arc(x, y, r, 0, Math.PI * 2);
  ctx.stroke();
}

const PALETTE = ["#7f6df2", "#5ad19a", "#e0b354", "#4fb3d9", "#e5616d", "#c678dd", "#98c379", "#d19a66"];

/** Stable color for a string (color groups by guard or type). */
export function paletteColor(key: string): string {
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
  return PALETTE[h % PALETTE.length];
}
