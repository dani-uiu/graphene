// The text editor (CodeMirror 6): Graphene highlighting, live diagnostics
// with quick fixes, autocomplete, and two-way selection with the graph.
// Graph actions arrive here as text edits, so one undo history covers both.

import { autocompletion, type CompletionContext } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab, toggleComment } from "@codemirror/commands";
import {
  HighlightStyle,
  StreamLanguage,
  bracketMatching,
  syntaxHighlighting,
  type StringStream,
} from "@codemirror/language";
import { lintGutter, setDiagnostics, type Diagnostic } from "@codemirror/lint";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { EditorState, StateEffect, StateField, type Extension } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  drawSelection,
  highlightActiveLine,
  keymap,
  lineNumbers,
  type DecorationSet,
} from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import type { Analysis, DiagView, TextEdit } from "./api";

const KEYWORDS = new Set([
  "in", "out", "fail", "sink", "extern", "f", "match", "assert", "err", "true", "false",
  // v0.3 (contextual) and v0.3.1
  "graph", "call", "map", "filter", "each", "const", "if", "then", "else",
  // v0.4 (contextual)
  "type", "enum", "import",
]);

/** Built-in functions, offered by autocomplete with their argument shape. */
const FUNCTIONS: [string, string][] = [
  ["len", "len(xs)"], ["sum", "sum(xs)"], ["min", "min(xs)"], ["max", "max(xs)"], ["avg", "avg(xs)"],
  ["all", "all(xs)"], ["any", "any(xs)"], ["contains", "contains(xs, v)"],
  ["first", "first(xs)"], ["last", "last(xs)"], ["sort", "sort(xs)"], ["reverse", "reverse(xs)"],
  ["round", "round(x, 2)"], ["floor", "floor(x, 2)"], ["ceil", "ceil(x, 2)"],
  ["abs", "abs(x)"], ["pow", "pow(x, 2)"], ["str", "str(x)"],
  ["upper", "upper(s)"], ["lower", "lower(s)"], ["starts_with", "starts_with(s, p)"], ["ends_with", "ends_with(s, p)"],
  // v0.4 dates
  ["date", "date(\"2026-11-02\")"], ["days_between", "days_between(a, b)"], ["add_days", "add_days(d, n)"],
  ["year", "year(d)"], ["month", "month(d)"], ["day", "day(d)"], ["weekday", "weekday(d)"],
];

/** Line tokenizer for Graphene; also accepts the Unicode aliases. */
export const graphene = StreamLanguage.define<{ header: boolean }>({
  name: "graphene",
  startState: () => ({ header: true }),
  token(stream: StringStream, state) {
    if (state.header && stream.sol() && stream.match(/^#!graphene.*/)) {
      state.header = false;
      return "meta";
    }
    state.header = false;
    if (stream.eatSpace()) return null;
    if (stream.match("//")) {
      stream.skipToEnd();
      return "comment";
    }
    if (stream.match("@intent")) return "meta";
    if (stream.match(/^"(?:[^"\\]|\\.)*"?/)) return "string";
    if (stream.match(/^\d+(\.\d+)?/)) return "number";
    if (stream.match("fx~") || stream.match(/^fx\b/)) return "keyword";
    if (stream.match(/^[➔⚙⊗×⫫]/u) || stream.match("𝑓")) return "keyword";
    if (stream.match(/^(->|=>|\|\||&&|==|!=|<=|>=|\+\+|[|!<>+\-*/%])/)) return "operator";
    if (stream.match(/^[A-Za-z_][A-Za-z0-9_]*/)) {
      const w = stream.current();
      if (KEYWORDS.has(w)) return "keyword";
      if (w === "total" || w === "indep") return "keyword";
      if (stream.peek() === "(") return "function";
      if (/^(u64|i64|dec|bool|str|date)$/.test(w)) return "typeName";
      return /^[A-Z]/.test(w) ? "variableName.definition" : "variableName";
    }
    stream.next();
    return null;
  },
  languageData: { commentTokens: { line: "//" } },
});

const highlight = HighlightStyle.define([
  { tag: t.keyword, color: "var(--syn-keyword)" },
  { tag: t.meta, color: "var(--syn-meta)", fontStyle: "italic" },
  { tag: t.comment, color: "var(--syn-comment)", fontStyle: "italic" },
  { tag: t.string, color: "var(--syn-string)" },
  { tag: t.number, color: "var(--syn-number)" },
  { tag: t.operator, color: "var(--syn-operator)" },
  { tag: t.typeName, color: "var(--syn-type)" },
  { tag: t.function(t.variableName), color: "var(--syn-function)" },
  { tag: t.definition(t.variableName), color: "var(--syn-node)" },
  { tag: t.variableName, color: "var(--syn-name)" },
]);

// Lines belonging to the graph selection get a background.
const setMarked = StateEffect.define<number[]>();
const markedLines = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    deco = deco.map(tr.changes);
    for (const e of tr.effects) {
      if (!e.is(setMarked)) continue;
      const lines = e.value.filter((l) => l >= 1 && l <= tr.state.doc.lines);
      const ranges = [...new Set(lines)]
        .sort((a, b) => a - b)
        .map((l) => Decoration.line({ class: "cm-graph-selected" }).range(tr.state.doc.line(l).from));
      deco = Decoration.set(ranges);
    }
    return deco;
  },
  provide: (f) => EditorView.decorations.from(f),
});

export interface EditorHooks {
  onChange: (text: string) => void;
  onCursorLine: (line: number) => void;
  completions: () => string[];
  /** F12: go to the definition of the name at this UTF-16 offset. */
  onDefinition?: (offset: number) => void;
}

export class Editor {
  view: EditorView;
  private quiet = false;
  private extensions: Extension[];

  constructor(parent: HTMLElement, hooks: EditorHooks) {
    const complete = (ctx: CompletionContext) => {
      const word = ctx.matchBefore(/[A-Za-z_][A-Za-z0-9_]*/);
      if (!word || (word.from === word.to && !ctx.explicit)) return null;
      const options = [...KEYWORDS, "fx", "fx~", "total", "indep", "u64", "i64", "dec", "bool", "str", "date"]
        .map((label) => ({ label, type: "keyword" }))
        .concat(FUNCTIONS.map(([label, detail]) => ({ label, type: "function", detail })))
        .concat(hooks.completions().map((label) => ({ label, type: "variable" })));
      return { from: word.from, options };
    };
    const extensions: Extension[] = [
      lineNumbers(),
      history(),
      drawSelection(),
      highlightActiveLine(),
      bracketMatching(),
      highlightSelectionMatches(),
      graphene,
      syntaxHighlighting(highlight),
      lintGutter(),
      markedLines,
      autocompletion({ override: [complete] }),
      keymap.of([
        ...defaultKeymap,
        ...historyKeymap,
        ...searchKeymap,
        indentWithTab,
        { key: "Mod-/", run: toggleComment },
        { key: "F12", run: (v) => (hooks.onDefinition?.(v.state.selection.main.head), true) },
      ]),
      EditorView.updateListener.of((u) => {
        if (u.docChanged && !this.quiet) hooks.onChange(u.state.doc.toString());
        if (u.selectionSet && u.view.hasFocus) {
          hooks.onCursorLine(u.state.doc.lineAt(u.state.selection.main.head).number);
        }
      }),
      EditorView.theme({}, { dark: true }),
    ];
    this.extensions = extensions;
    this.view = new EditorView({ parent, state: this.fresh("") });
  }

  get text(): string {
    return this.view.state.doc.toString();
  }

  /** A new editor state for `text` with this editor's configuration. */
  fresh(text: string): EditorState {
    return EditorState.create({ doc: text, extensions: this.extensions });
  }

  /** Show a stored state (switching tabs keeps each tab's undo history). */
  load(state: EditorState) {
    this.quiet = true;
    this.view.setState(state);
    this.quiet = false;
  }

  /** Replace the text, e.g. after the file changed on disk (undoable). */
  replaceAll(text: string) {
    this.view.dispatch({ changes: { from: 0, to: this.view.state.doc.length, insert: text } });
  }

  /** Current state, to stash per tab (keeps each tab's undo history). */
  snapshot(): EditorState {
    return this.view.state;
  }

  /** Apply edits from a graph action as one undoable transaction. */
  apply(edits: TextEdit[]) {
    if (edits.length === 0) return;
    const len = this.view.state.doc.length;
    const changes = edits.map((e) => ({ from: Math.min(e.from, len), to: Math.min(e.to, len), insert: e.insert }));
    this.view.dispatch({ changes, userEvent: "graph.edit", scrollIntoView: false });
  }

  /** Show diagnostics from the latest analysis (only if the text matches).
   *  A problem inside an imported library is shown on the first `import`
   *  line, naming the library and its line. */
  showDiagnostics(a: Analysis, forText: string, onFix: (edits: TextEdit[]) => void) {
    if (forText !== this.text) return;
    const len = this.view.state.doc.length;
    const imp = /^[ \t]*import[ \t]+"/m.exec(forText);
    const importAt = imp ? imp.index : 0;
    const importEnd = imp ? forText.indexOf("\n", importAt) : 0;
    const diags: Diagnostic[] = a.diags.map((d: DiagView) => {
      const elsewhere = d.file !== null;
      const from = elsewhere ? importAt : d.from;
      const to = elsewhere ? (importEnd < 0 ? len : importEnd) : d.to;
      return {
        from: Math.min(from, len),
        to: Math.min(Math.max(to, from), len),
        severity: "error",
        source: d.code,
        message: `${elsewhere ? `${d.file}:${d.line}: ` : ""}${d.message}${d.label ? ` — ${d.label}` : ""}${d.help ? `\nhelp: ${d.help}` : ""}`,
        actions: d.fixes.map((f) => ({ name: f.label, apply: () => onFix(f.edits) })),
      };
    });
    this.view.dispatch(setDiagnostics(this.view.state, diags));
  }

  /** Highlight `lines` and optionally scroll the first into view. */
  mark(lines: number[], scroll: boolean) {
    const effects: StateEffect<unknown>[] = [setMarked.of(lines)];
    const first = lines.filter((l) => l >= 1 && l <= this.view.state.doc.lines).sort((a, b) => a - b)[0];
    if (scroll && first !== undefined) {
      effects.push(EditorView.scrollIntoView(this.view.state.doc.line(first).from, { y: "center" }));
    }
    this.view.dispatch({ effects });
  }

  /** Select a UTF-16 range and focus the editor. */
  reveal(from: number, to: number) {
    const len = this.view.state.doc.length;
    this.view.dispatch({
      selection: { anchor: Math.min(from, len), head: Math.min(to, len) },
      scrollIntoView: true,
    });
    this.view.focus();
  }

  /** First and last line of the selection (for extract into subgraph). */
  selectedLines(): [number, number] {
    const { from, to } = this.view.state.selection.main;
    const doc = this.view.state.doc;
    const last = doc.lineAt(to > from && doc.lineAt(to).from === to ? to - 1 : to).number;
    return [doc.lineAt(from).number, last];
  }

  lineText(line: number): string {
    if (line < 1 || line > this.view.state.doc.lines) return "";
    return this.view.state.doc.line(line).text;
  }
}
