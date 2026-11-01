# Graphene IDE — plan

A desktop app for writing, viewing and running Graphene programs as a graph of connected nodes, styled after Obsidian's graph view.

Status: **draft, awaiting approval.** No code has been written yet.

---

## 1. The core rule: the text is the program

The IDE adds a picture of the program. It does not add a second way of storing it.

- The `.grph` file is the **only source of truth**. The graph is always computed from the text, never stored as its own model.
- Every graph action, such as adding a node, connecting two nodes, renaming or deleting, becomes a **text edit**. Anything you can do in the graph, you can also do by typing, and anything you can type shows up in the graph.
- Graph edits change only the statements they touch. Comments, blank lines and your own formatting everywhere else stay as they are.
- Undo and redo work on the text, so one history covers both views. Undoing a graph action undoes its text edit.
- A file saved by the IDE is a normal `.grph` file. The `graphene` CLI, `git diff` and any text editor all keep working.
- The only extra file is an optional layout sidecar for pinned node positions (§5.4). It has no effect on what the program means.

Full parity with plain text means this: **any valid or invalid `.grph` file opens, shows as a graph as far as it parses, and can be edited from either view without losing anything.**

## 2. What it looks like

Three resizable panes, any of which can be hidden:

```
┌──────────────┬───────────────────────────────────┬──────────────────────┐
│ Files        │            Graph view             │ Inspector            │
│  pricing     │                                   │  B  (value node)     │
│  checkout ●  │        ◯ A                        │  type {role, bal}    │
│  order_total │         │ ⚙ DbFetch               │  active when         │
│              │        ◉ B ───⊗──── ◌ S1          │   ok(DbFetch)        │
│ Externs      │       ╱ ? ╲                       │  deps [usr_id,       │
│  fx DbFetch  │     ◯ C1   ◯ C2                   │        DbFetch]      │
│  fx~ Log…    │       ╲ f   ╱ f                   │  last run: {prem,    │
│              │        ◉ D                        │            100.00}   │
│ Problems (0) │         │                         ├──────────────────────┤
│ Run          │        ⬢ out final_price          │ Text (CodeMirror)    │
│              │                                   │  B -> match B.role…  │
└──────────────┴───────────────────────────────────┴──────────────────────┘
```

The default layout puts the text editor below the inspector. It can also be docked beside the graph or opened full-screen, so the IDE works as a plain text editor too.

### 2.1 Obsidian graph-view style

- Dark canvas by default with a light theme option. Nodes are soft circles and edges are thin, low-contrast lines.
- **Force-directed layout** that keeps moving gently and settles. Dragging a node pulls its neighbors along.
- **Hover a node:** it and its direct neighbors light up, and everything else fades.
- **Labels fade in with zoom.** Zoomed out you see only the shape of the program. Zoomed in you see names, operations and expressions.
- Node size grows with the number of connections.
- **Graph settings panel**, as in Obsidian:
  - forces: center, repel, link strength, link distance;
  - filters: show or hide sinks, error paths, terminals and expression references;
  - color groups by kind, by active condition, or by a search query;
  - arrow and label toggles.
- **Local graph mode:** show only the selected node and its neighbors up to N steps away. This keeps large programs readable.
- **Search** highlights matching nodes and dims everything else.

### 2.2 How Graphene maps onto the picture

| Graphene | Drawn as |
|---|---|
| input node (`A`) | ring ◯ with the input name, such as `usr_id` |
| value node (`B`, `D`) | filled circle ◉, sized by connections |
| merge node (several writers) | filled circle with a small ring marking the merge |
| sink (`S1`) | dashed hollow circle ◌ |
| `out` / `fail` | hexagon ⬢, green for `out` and red for `fail` |
| `f(...)` | small glyph `f` sitting on the edge |
| `fx` / `fx~` | glyph `⚙` / `⊗` on the edge. `fx~` edges are dashed. |
| `\| err ->` | red edge |
| `match` | glyph `?` with one labeled edge per arm |
| join `(X, Y) -> ...` | small hub dot the members feed into |
| expression reference (`B.bal` used in `f`) | faint dotted line, hidden by default |
| `extern` declarations, `@intent`, `assert` | listed in the side panel, since they aren't part of the flow |

Operation glyphs are small, but you can click them and they have their own inspector. That gives full control over every statement without cluttering the Obsidian look.

## 3. Feature list

### 3.1 Editing in the graph (each one becomes a text edit)

| Action | Text it produces |
|---|---|
| Add input | `in name: type -> Node` |
| Drag from a node to empty space and pick `f` / `fx` / `fx~` / `match` / `out` / `fail` | a new flow, match, `out` or `fail` statement with a placeholder that you fill in |
| Drag from a node to an existing node | a flow into that node. If it already has a writer, this makes a merge. |
| Select several nodes, then "Join into…" | `(X, Y) -> f(...) -> T` |
| Add or remove a match arm | edits the arm list |
| Add an error edge | adds `\| err -> Node` |
| Edit an expression, effect arguments or a type | inline mini-editor in the inspector, with highlighting and autocomplete |
| Rename a node | renames it everywhere it's defined, written to and referenced |
| Delete a node | deletes its statements. It asks first if other statements still reference it. |
| Delete an edge | deletes that statement |
| Edit `extern`, `@intent`, `assert` | side-panel forms or text |
| Reorder statements | drag in an outline list. Order doesn't change what the program means. |

### 3.2 Text editor

- CodeMirror 6, the editor Obsidian uses, with a Graphene language mode:
  - syntax highlighting;
  - bracket matching;
  - comment toggling;
  - autocomplete for names, effects, parameters and field names;
  - Unicode-alias input (typing `->` can optionally show `➔`).
- Diagnostics appear as squiggles, with the full message and `help:` text on hover.
- Format on save, using the existing `graphene fmt`, can be turned off.

### 3.3 The two views stay linked

- Clicking a node selects its defining statement in the text, and the reverse.
- Moving the text cursor highlights the matching node and edge.
- Edits in either view update the other within about 100 ms.
- **Broken programs still draw.** If the text doesn't parse, the graph keeps everything up to the parse error and greys out the rest. Checker errors draw normally, with error badges on the affected nodes.

### 3.4 Checking and quick fixes

- The checker runs on every change. Errors show in three places: on nodes, in the text, and in the Problems panel.
- Quick fixes come from each diagnostic's `help:` line. For example:
  - G005: "add error edge"
  - G004: "add `_` arm"
  - G011: "remove extra `out`"
  - G002: "use input name `usr_id`"
- Hovering a node shows when it's active (for example `ok(DbFetch) ∧ arm "prem"`), its type and its dependencies, which the analysis already computes.
- For G020, the counterexample path is highlighted in the graph.

### 3.5 Running programs

- The **Run panel** builds an input form from the `in` statements. Each field is checked against its type.
- **Mocks:**
  - Pick a fixture JSON, or edit one in a form generated from the `extern` declarations: one row per case, with arguments and an `ok` or `err` answer.
  - "Record missing" mode: when a call has no matching case, the IDE asks for the answer and saves it to the fixture.
- **Sequential or parallel** toggle.
- **Results:**
  - the result and the effect log, as in the CLI;
  - each node shows the value it got;
  - nodes that never became active are dimmed;
  - the path taken through each `match` and error edge lights up.
- **Replay** steps through the run in its fixed run order, one node at a time, with a timeline scrubber. This is possible because runs are deterministic.
- **Test cases:**
  - run every case in a `.cases.json` file, with pass/fail per case and a diff when one fails;
  - "Save this run as a case".
- Runtime faults (R001–R005) point at the node that faulted.

### 3.6 Other tools

- Format, token count (ASCII vs Unicode), export as DOT, SVG or PNG, and export the effect log.
- Open a folder as a workspace with a file tree and multiple tabs. Files changed outside the IDE reload automatically.
- A command palette (Ctrl+P) for every action, plus keyboard shortcuts.

## 4. Architecture

```
graphene/                      (Cargo workspace)
├─ crates/graphene/            the existing compiler and runtime library (moved here)
├─ crates/graphene-cli/        the existing CLI (src/main.rs moved here)
├─ crates/graphene-ide-core/   NEW: analysis JSON, edit commands, traced runs (pure Rust, tested)
└─ ide/
   ├─ src-tauri/               NEW: Tauri 2 desktop shell, a thin layer over ide-core
   └─ src/                     NEW: frontend (Svelte + TypeScript)
       ├─ graph/               PixiJS (WebGL) renderer + d3-force simulation
       ├─ editor/              CodeMirror 6 + Graphene language mode
       └─ panels/              inspector, run, problems, files, settings
```

### 4.1 Why this stack

| Choice | Why |
|---|---|
| **Tauri 2** (Rust backend + system web view) | The backend calls the existing Rust library directly, with no second implementation of the parser or checker. Installers are small (~10 MB) because it uses Windows' built-in WebView2, which is already installed here. |
| **PixiJS + d3-force** | The same approach Obsidian's graph view uses, a WebGL canvas with a force simulation. It stays smooth with thousands of nodes. |
| **CodeMirror 6** | Obsidian's editor. It has an extensible language mode, diagnostics, and a transaction-based history that graph edits can feed into. |
| **Svelte** | Small and fast, with little boilerplate for panel UIs. React would also work. |

The alternative is a pure-Rust UI (egui). It's simpler to build, but matching the Obsidian look and a good text editor is much harder, so I don't recommend it.

### 4.2 `graphene-ide-core`: where the logic lives

All language-aware logic stays in Rust, where it is tested, never in the frontend:

- **`analyze(text) -> Analysis`**, serialized to JSON for the frontend. It contains:
  - the syntax tree with source positions;
  - the graph, with stable IDs (value nodes keyed by name, statements by index);
  - types, active conditions, dependencies and run order;
  - diagnostics.
  - It accepts partial results, so a file that doesn't fully parse still draws.
- **Edit commands:** `apply(text, Command) -> Vec<TextEdit>`. There is one command per row of §3.1, for example `AddFlow`, `Connect`, `Rename`, `DeleteNode`, `SetExpr` and `AddArm`. Each command:
  - returns minimal span-based edits, so the frontend applies them as one CodeMirror transaction (one undo step);
  - never re-formats untouched lines, which keeps comments;
  - is tested to (a) produce text that parses, (b) leave every other statement byte-for-byte unchanged, and (c) apply cleanly when undone and redone.
- **Quick fixes:** `fixes(text, Diag) -> Vec<(label, Vec<TextEdit>)>`.
- **Traced runs:** `run_traced(...)` returns the normal result and effect log, plus each node's state (active, inactive or faulted), its value, and the step at which it ran. This needs a small, opt-in hook added to the runtime's step function.

The library rules from the v0.1 brief still apply:
- `syntax` and `check` never depend on `runtime`;
- no `unsafe`;
- no panics on user input.

The no-tokio rule applies to the library crates. Tauri uses an async runtime internally, which is acceptable in the app shell only, and will be recorded as a decision.

### 4.3 Data flow

```
keystroke / graph action
      │
      ▼
CodeMirror document (truth) ──debounce 50ms──► ide-core::analyze ──► Analysis JSON
      ▲                                                                   │
      │ TextEdits (one undo step)                                         ▼
ide-core::apply(Command) ◄── graph action        graph view + inspector + problems update
```

Runs execute on a background thread and can be cancelled. The UI never blocks.

## 5. Open design points (decided in Phase 0)

1. **Tauri on the GNU toolchain.** Tauri officially expects Visual Studio's C++ Build Tools, which aren't installed. Phase 0 includes a short trial build with the current GNU + MinGW setup. If that fails, the fallback is installing the VS Build Tools (~5 GB), which I'd ask you about first.
2. **Operations as edge glyphs or as full nodes.** The plan uses glyphs because they match the Obsidian look. A setting could draw operations as full nodes for people who prefer a classic node-editor look.
3. **New code from the graph.** Placeholder names (`N1`, `N2`, …) and placeholder expressions (`f(_)`) are invalid until filled in. The checker flags them, so the graph never hides an incomplete program.
4. **Layout persistence.** Positions come from the force simulation by default. Pinned nodes are saved in `name.grph.layout.json` next to the program. That file is optional, safe to delete and ignored by the CLI.
5. **Order relative to language v0.2.** v0.2 changes the grammar: record literals, separate edges per error, and inputs usable everywhere. The IDE's edit commands mirror the grammar, so I recommend **writing the v0.2 spec first**, then building the IDE against it. Otherwise the edit commands and their tests get written twice.

## 6. Phases

Each phase ends with tests passing, a commit and a 5-line report, as in v0.1.

| Phase | Delivers | Done when |
|---|---|---|
| **0. Design** | This plan approved. Stack trial (Tauri builds and opens a window). Edit-command catalogue. UI mock-ups. Decision log entries. | You approve; a blank Tauri window runs on this PC |
| **1. Core library** | Workspace split, `graphene-ide-core`: `analyze` with partial results, all edit commands, quick fixes, `run_traced` | Property tests: random command sequences on all examples always give parseable text, untouched lines byte-identical |
| **2. Text IDE** | Tauri shell, file tree, tabs, CodeMirror with Graphene mode, live diagnostics, format, Problems panel | Every example can be opened, edited, checked and saved in the app |
| **3. Graph view (read-only)** | PixiJS + d3-force renderer in Obsidian style, hover highlight, zoom-faded labels, settings panel, local graph, search, two-way selection sync | All examples render; 1,000-node generated graph stays at 60 fps |
| **4. Graph editing** | Every action in §3.1, inspector with inline editors, quick-fix buttons, layout pinning | Every example can be rebuilt from an empty file using only the graph, and gives byte-identical output after `fmt` |
| **5. Running** | Run panel, input form, mock editor, "record missing", node values, active path, replay timeline, `.cases.json` runner | All example cases pass from inside the app; replay matches the effect log |
| **6. Polish and release** | Themes, keyboard shortcuts, command palette, export, settings persistence, Windows installer (MSI), end-to-end tests | Installer works on a clean Windows user account |

Testing:
- Rust unit and property tests for `ide-core`, the important layer;
- Vitest for frontend logic;
- a few end-to-end smoke tests through `tauri-driver`.

## 7. Out of scope for the first version

- Real effect hosts (HTTP, databases). Runs use mocks, as in the CLI.
- Collaboration or cloud sync.
- macOS and Linux builds. Tauri supports them, but they won't be tested at first.
- An LSP for other editors. `ide-core` is shaped so an LSP could reuse it later.
