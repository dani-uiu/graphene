// Small DOM and timing helpers shared by every panel.

type Attrs = Record<string, string | number | boolean | EventListener | undefined>;

/** Create an element: `h("button", { class: "x", onclick: f }, "Label")`. */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: (Node | string | null | undefined | false)[]
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === false) continue;
    if (k.startsWith("on") && typeof v === "function") {
      e.addEventListener(k.slice(2), v as EventListener);
    } else if (v === true) {
      e.setAttribute(k, "");
    } else {
      e.setAttribute(k, String(v));
    }
  }
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    e.append(typeof c === "string" ? document.createTextNode(c) : c);
  }
  return e;
}

/** Replace all children of `parent`. */
export function mount(parent: Element, ...children: (Node | string | null | false)[]) {
  parent.replaceChildren(...children.filter((c): c is Node | string => c !== null && c !== false));
}

export function debounce<A extends unknown[]>(fn: (...a: A) => void, ms: number) {
  let t: number | undefined;
  return (...a: A) => {
    window.clearTimeout(t);
    t = window.setTimeout(() => fn(...a), ms);
  };
}

/** Compact one-line JSON for values shown on nodes and in tables. */
export function short(v: unknown, max = 40): string {
  const s = typeof v === "string" ? v : JSON.stringify(v);
  if (s === undefined) return "";
  return s.length > max ? s.slice(0, max - 1) + "…" : s;
}

/** A run value for display: lists lead with their length, `[3] [...]`. */
export function valueText(v: unknown, max = 40): string {
  return Array.isArray(v) ? `[${v.length}] ${short(v, Math.max(8, max - 6))}` : short(v, max);
}

/** Display name of a path (last segment). */
export function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** Folder part of a path. */
export function dirName(path: string): string {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return i < 0 ? "" : path.slice(0, i);
}

/** Small modal prompt; resolves to the entered text, or null if cancelled. */
export function ask(title: string, initial = "", placeholder = ""): Promise<string | null> {
  return new Promise((resolve) => {
    const input = h("input", { type: "text", value: initial, placeholder, spellcheck: "false" });
    const close = (v: string | null) => {
      overlay.remove();
      resolve(v);
    };
    const overlay = h(
      "div",
      { class: "modal-overlay", onmousedown: (e: Event) => e.target === overlay && close(null) },
      h(
        "div",
        { class: "modal" },
        h("div", { class: "modal-title" }, title),
        input,
        h(
          "div",
          { class: "modal-actions" },
          h("button", { onclick: () => close(null) }, "Cancel"),
          h("button", { class: "primary", onclick: () => close(input.value.trim()) }, "OK"),
        ),
      ),
    );
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") close(input.value.trim());
      if (e.key === "Escape") close(null);
    });
    document.body.append(overlay);
    input.focus();
    input.select();
  });
}

/** Transient message in the corner. */
export function toast(text: string, kind: "info" | "error" = "info") {
  const t = h("div", { class: `toast ${kind}` }, text);
  document.body.append(t);
  window.setTimeout(() => t.classList.add("gone"), 2600);
  window.setTimeout(() => t.remove(), 3100);
}

/** Context menu at screen position; items with `null` action are headers. */
export function menu(x: number, y: number, items: ({ label: string; action: (() => void) | null; danger?: boolean } | "-")[]) {
  document.querySelectorAll(".context-menu").forEach((m) => m.remove());
  const m = h("div", { class: "context-menu" });
  for (const it of items) {
    if (it === "-") {
      m.append(h("div", { class: "menu-sep" }));
      continue;
    }
    const row = h("div", { class: `menu-item${it.action ? "" : " header"}${it.danger ? " danger" : ""}` }, it.label);
    if (it.action) {
      const act = it.action;
      row.addEventListener("click", () => {
        m.remove();
        act();
      });
    }
    m.append(row);
  }
  document.body.append(m);
  const r = m.getBoundingClientRect();
  m.style.left = `${Math.min(x, window.innerWidth - r.width - 8)}px`;
  m.style.top = `${Math.min(y, window.innerHeight - r.height - 8)}px`;
  const away = (e: MouseEvent) => {
    if (!m.contains(e.target as Node)) {
      m.remove();
      window.removeEventListener("mousedown", away, true);
    }
  };
  window.setTimeout(() => window.addEventListener("mousedown", away, true));
}

/** True when the letters of `q` appear in `text` in order (palette filter). */
export function fuzzy(text: string, q: string): boolean {
  let i = 0;
  for (const ch of text) if (ch === q[i]) i++;
  return i >= q.length;
}
