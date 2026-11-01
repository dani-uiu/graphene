// Inspector: details and actions for the selected node(s). Every action
// is an edit command, so it shows up in the text and in undo history.

import type { NodeView } from "./api";
import { app } from "./app";
import { store } from "./store";
import { ask, h, mount, short } from "./util";

const STEPS: [string, string][] = [
  ["f", "f(…) compute"],
  ["fx", "⚙ effect"],
  ["fx~", "⊗ fire & forget"],
  ["match", "? match"],
  ["out", "out"],
  ["fail", "fail"],
];

const KIND: Record<string, string> = {
  input: "input",
  param: "parameter",
  value: "value",
  sink: "sink",
  op: "operation",
  match: "match",
  out: "result",
  fail: "failure",
};

export function inspector(root: HTMLElement) {
  const render = () => {
    const a = store.analysis;
    const sel = a ? a.nodes.filter((n) => store.selection.has(n.id)) : [];
    if (!a) return mount(root, h("div", { class: "panel" }, h("p", { class: "hint" }, "No program open.")));
    if (sel.length === 0) return mount(root, overview());
    if (sel.length === 1) return mount(root, single(sel[0]));
    return mount(root, multi(sel));
  };
  store.on("selection", render);
  store.on("analysis", render);
  store.on("run", render);
  render();
}

function overview(): HTMLElement {
  const a = store.analysis!;
  const values = a.nodes.filter((n) => n.kind === "value").length;
  const ops = a.nodes.filter((n) => n.id.startsWith("s:")).length;
  return h(
    "div",
    { class: "panel" },
    h("div", { class: "insp-title" }, store.tab?.name ?? "Program"),
    a.intent ? h("p", { class: "intent" }, a.intent) : null,
    h(
      "div",
      { class: "kv" },
      h("span", {}, "Version"), h("span", {}, a.version),
      h("span", {}, "Status"), h("span", { class: a.checked ? "good" : "bad" }, a.checked ? "✓ all checks pass" : `${a.diags.length} problem(s)`),
      h("span", {}, "Graph"), h("span", {}, `${a.inputs.length} inputs, ${values} values, ${ops} statements`),
    ),
    h(
      "ul",
      { class: "hint tips" },
      h("li", {}, "Click a node to inspect it; Ctrl-click to select several."),
      h("li", {}, "Shift-drag from a node to connect it, or drop on empty space to add a step."),
      h("li", {}, "Right-click a node or the canvas for actions."),
      h("li", {}, "Every action edits the text, so Ctrl+Z undoes it."),
    ),
  );
}

function single(n: NodeView): HTMLElement {
  const st = store.run?.nodes.find((s) => s.id === n.id);
  const name = app.nameOf(n);
  const rows: (HTMLElement | string)[] = [];
  const kv = (k: string, v: string | null | undefined, cls = "") => {
    if (v) rows.push(h("span", {}, k), h("span", { class: cls }, v));
  };
  kv("Kind", n.op ? `${KIND[n.kind]} (${n.op})` : KIND[n.kind]);
  kv("Type", n.ty, "mono");
  kv("Active when", n.guard);
  kv("Depends on", n.deps.length ? n.deps.join(", ") : null, "mono");
  kv("Run order", n.order === null ? null : `#${n.order}`);
  kv("Writers", n.writers > 1 ? `${n.writers} (a merge)` : null);
  if (st) kv("Last run", st.status === "inactive" ? "did not run" : st.value === null ? st.status : short(st.value, 80), st.status === "fault" ? "bad" : "mono");
  if (!n.defined && n.kind === "value") kv("Problem", "used but never defined", "bad");
  return h(
    "div",
    { class: "panel" },
    h("div", { class: "insp-title" }, h("span", { class: `badge ${n.kind}` }, KIND[n.kind]), " ", n.name),
    n.detail ? h("div", { class: "mono detail" }, n.detail) : null,
    h("div", { class: "kv" }, ...rows),
    n.id.startsWith("s:") ? statementEditor(n) : null,
    name && n.kind !== "sink" ? stepButtons(name) : null,
    h(
      "div",
      { class: "row-actions" },
      name ? h("button", { onclick: () => rename(name) }, "Rename…") : null,
      name && n.kind !== "sink" ? h("button", { onclick: () => connect(name) }, "Connect to…") : null,
      h("button", { onclick: () => app.graph.togglePin(n.id) }, app.graph.isPinned(n.id) ? "Unpin" : "Pin"),
      h("button", { onclick: () => app.editor.mark([n.line], true) }, `Line ${n.line}`),
      h(
        "button",
        {
          class: "danger",
          onclick: () => (name ? app.command({ cmd: "delete_node", name }) : app.command({ cmd: "delete_line", line: n.line })),
        },
        "Delete",
      ),
    ),
  );
}

/** Edit the statement's source line directly (full control). */
function statementEditor(n: NodeView): HTMLElement {
  const input = h("input", { type: "text", class: "mono", value: app.editor.lineText(n.line), spellcheck: "false" }) as HTMLInputElement;
  const apply = () => app.command({ cmd: "set_line", line: n.line, text: input.value });
  input.addEventListener("keydown", (e) => e.key === "Enter" && apply());
  return h("div", { class: "stmt-edit" }, h("div", { class: "subhead" }, `Statement (line ${n.line})`), input, h("div", { class: "row-actions" }, h("button", { class: "primary", onclick: apply }, "Apply")));
}

function stepButtons(name: string): HTMLElement {
  return h(
    "div",
    {},
    h("div", { class: "subhead" }, "Add a step after this node"),
    h("div", { class: "steps" }, ...STEPS.map(([step, label]) => h("button", { onclick: () => app.command({ cmd: "add_step", from: name, step, effect: null }) }, label))),
  );
}

function multi(sel: NodeView[]): HTMLElement {
  const names = sel.map((n) => app.nameOf(n)).filter((x): x is string => !!x);
  return h(
    "div",
    { class: "panel" },
    h("div", { class: "insp-title" }, `${sel.length} nodes selected`),
    h("div", { class: "mono detail" }, sel.map((n) => n.name).join(", ")),
    h(
      "div",
      { class: "row-actions" },
      names.length >= 2 ? h("button", { class: "primary", onclick: () => join(names) }, "Join into a new node…") : null,
      h(
        "button",
        {
          class: "danger",
          onclick: async () => {
            for (const n of names) await app.command({ cmd: "delete_node", name: n });
          },
        },
        "Delete all",
      ),
    ),
  );
}

export async function rename(old: string) {
  const name = await ask(`Rename ${old}`, old);
  if (name && name !== old) {
    const ok = await app.command({ cmd: "rename", old, new: name });
    if (ok) app.selectNodes([`v:${name}`], false);
  }
}

export async function connect(from: string) {
  const to = await ask(`Connect ${from} to…`, "", "an existing node (makes a merge) or a new name");
  if (to) await app.command({ cmd: "connect", from, to });
}

export async function join(members: string[]) {
  const target = await ask(`Join ${members.join(", ")} into…`, "", "new node name (empty for automatic)");
  if (target === null) return;
  await app.command({ cmd: "join", members, target: target || null });
}
