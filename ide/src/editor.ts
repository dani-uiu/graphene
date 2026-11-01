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

const KEYWORDS = new Set(["in", "out", "fail", "sink", "extern", "f", "match", "assert", "err", "true", "false"]);

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
    if (stream.match(/^(->|=>|\|\||&&|==|!=|<=|>=|[|!<>+\-*/])/)) return "operator";
    if (stream.match(/^[A-Za-z_][A-Za-z0-9_]*/)) {
      const w = stream.current();
      if (KEYWORDS.has(w)) return "keyword";
      if (w === "total" || w === "indep") return "keyword";
      if (/^(u64|i64|dec|bool|str)$/.test(w)) return "typeName";
      if (stream.peek() === "(") return "function";
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
}

export class Editor {
  view: EditorView;
  private quiet = false;
  private extensions: Extension[];

  constructor(parent: HTMLElement, hooks: EditorHooks) {
    const complete = (ctx: CompletionContext) => {
      const word = ctx.matchBefore(/[A-Za-z_][A-Za-z0-9_]*/);
      if (!word || (word.from === word.to && !ctx.explicit)) return null;
      const options = [...KEYWORDS, "fx", "fx~", "total", "indep", "u64", "i64", "dec", "bool", "str"]
        .map((label) => ({ label, type: "keyword" }))
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
      keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab, { key: "Mod-/", run: toggleComment }]),
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

  /** Show diagnostics from the latest analysis (only if the text matches). */
  showDiagnostics(a: Analysis, forText: string, onFix: (edits: TextEdit[]) => void) {
    if (forText !== this.text) return;
    const len = this.view.state.doc.length;
    const diags: Diagnostic[] = a.diags.map((d: DiagView) => ({
      from: Math.min(d.from, len),
      to: Math.min(Math.max(d.to, d.from), len),
      severity: "error",
      source: d.code,
      message: `${d.message}${d.label ? ` — ${d.label}` : ""}${d.help ? `\nhelp: ${d.help}` : ""}`,
      actions: d.fixes.map((f) => ({ name: f.label, apply: () => onFix(f.edits) })),
    }));
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

  lineText(line: number): string {
    if (line < 1 || line > this.view.state.doc.lines) return "";
    return this.view.state.doc.line(line).text;
  }
}
