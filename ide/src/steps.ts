// Steps that can follow a node, shared by the inspector, menus and the
// palette, plus drilling into a subgraph (v0.3).

import { app } from "./app";
import { store } from "./store";
import { ask, toast } from "./util";

const BASE: [string, string][] = [
  ["f", "f(…) compute"],
  ["fx", "⚙ effect (fx)"],
  ["fx~", "⊗ fire & forget (fx~)"],
  ["match", "? match"],
  ["out", "out (result)"],
  ["fail", "fail"],
];

const CALLS: [string, string][] = [
  ["call", "↳ call subgraph"],
  ["map", "∀ map subgraph over a list"],
  ["filter", "⊂ filter a list with a subgraph"],
];

/** The steps on offer: subgraph calls only once the program has subgraphs. */
export function stepItems(): [string, string][] {
  return store.analysis?.groups.length ? [...BASE, ...CALLS] : BASE;
}

/** Add `step` after `from`, asking which subgraph when there are several. */
export async function addStep(from: string, step: string) {
  let effect: string | null = null;
  if (step === "call" || step === "map" || step === "filter") {
    const groups = store.analysis?.groups ?? [];
    if (!groups.length) return toast("declare a subgraph first (Add subgraph…)", "error");
    effect = groups[0].name;
    if (groups.length > 1) {
      effect = await ask(`Which subgraph? (${groups.map((g) => g.name).join(", ")})`, groups[0].name);
      if (!effect) return;
    }
  }
  await app.command({ cmd: "add_step", from, step, effect });
}

/** Ask for a name and type, then add an empty `graph` block. */
export async function addSubgraph() {
  const t = await ask("New subgraph", "Name: u64", "Name: parameter type");
  const m = t?.match(/^\s*([A-Za-z_]\w*)\s*:\s*(.+?)\s*$/);
  if (t && !m) return toast("write it as `Name: type`", "error");
  if (m) await app.command({ cmd: "add_subgraph", name: m[1], ty: m[2] });
}

const listeners: ((group: string | null) => void)[] = [];

/** Show only one subgraph's body (drill in), or the whole program. */
export function focusGroup(group: string | null) {
  app.graph.setFocusGroup(group);
  for (const f of listeners) f(group);
}

export function onFocusGroup(f: (group: string | null) => void) {
  listeners.push(f);
}
