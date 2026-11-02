# Graphene IDE

A desktop app for writing, viewing, and running Graphene programs. The text editor and an Obsidian-style graph view show the same program, and the text is always the source of truth.

## Using it

| Do this | How |
|---|---|
| Open a project | **Files → Open folder…** (`Ctrl+Shift+O`), then click a `.grph` file |
| Inspect a node | Click it. `Ctrl`-click to select several. The editor highlights its lines. |
| Add a step after a node | Right-click it, or use the inspector's step buttons: `f`, `⚙ fx`, `⊗ fx~`, `? match`, `out`, `fail`, and (once the program has subgraphs) `↳ call`, `∀ map`, `⊂ filter` |
| Add a subgraph | Right-click the canvas → **Add subgraph…** (needs `#!graphene 0.3`; the palette can change the header) |
| Open a subgraph | Double-click a `call`/`map`/`filter` node, or **Open …** in the inspector. **◂ Program** (or `Esc`) goes back. |
| Connect two nodes | `Shift`-drag from one node to the other. Drop on empty space to pick a new step instead. |
| Join nodes | Select two or more, then right-click → **Join** |
| Rename or delete | Inspector or right-click. Rename updates every use. |
| Edit a statement directly | Select it and edit its line in the inspector, or type in the editor |
| Fix a problem | **Problems** panel, or hover the squiggle in the editor → quick fix |
| Run | **Run** panel: fill in the inputs (lists as JSON), pick a mock, then **▶ Run** (`Ctrl+Enter`). **Replay** steps through the run. |
| Use real effects | **Run** panel → **Effects: Host config**, pick a `hosts.json`. Tick **record** to save the answers as `<program>.recorded.json`, a mock that replays the run. |
| See one `map` element | After a run, **Show run of** picks an element; the graph opens its subgraph with that element's values. |
| Test | **Tests** panel → **Run all cases**. **Save as test case** in the Run panel adds one. |
| Find untested paths | **Tests** panel → **Coverage**: lists what no case reaches and dashes it on the graph. Click again to hide. |
| Everything else | Command palette, `Ctrl+P` |

**Graph settings** (⚙ above the graph):
- **Forces:** center, repel, link strength and link distance.
- **Filters:** sinks, error paths, results, and expression reads.
- **Coloring:** by kind, active condition, or type.
- **Local graph:** shows only the selection's neighborhood.

**Arranging the graph:**
- Drag a node to place it. It stays where you drop it; **Unpin** in the inspector or the right-click menu hands it back to the automatic layout.
- Each open tab keeps its whole arrangement (every node position, plus zoom and pan) while you switch between tabs.
- Placed nodes are also saved next to the program, in `<file>.grph.layout.json`, so they come back when you reopen the file.

**Shortcuts:**

| Keys | Action |
|---|---|
| `Ctrl+S` | Save (formats on save; turn that off in the palette) |
| `Ctrl+N` | New program |
| `Ctrl+O` | Open file |
| `F` | Fit the graph |
| `Delete` | Remove the selected nodes |
| `Esc` | Clear the selection |
| `Ctrl+Z` | Undo, for typing and graph actions alike |

## Developing

Prerequisites:
- Node 20+;
- Rust with the `stable-x86_64-pc-windows-msvc` toolchain;
- Visual Studio Build Tools with the C++ workload;
- WebView2, which comes with Windows 11.

`src-tauri/rust-toolchain.toml` selects the MSVC toolchain automatically. Run these from `ide/`:

```sh
npm install
npm run tauri dev      # app with hot reload
npm run tauri build    # release build + MSI/NSIS installers in src-tauri/target/release/bundle/
npm test               # TypeScript unit tests (vitest)
npm run check          # typecheck
```

The language logic and its tests live in `crates/graphene-ide-core`. Run `cargo test -p graphene-ide-core` from the repo root. It includes a property test that random graph-command sequences on every example keep the text parseable, with untouched lines byte-identical.

### End-to-end smoke test

```sh
cargo run -p graphene-ide-core --example bridge -- 1421   # serves ide/dist with the Rust commands over HTTP
npm run build && npm run smoke                            # drives the UI in headless Edge, saves screenshots to e2e/shots/
```

## Layout

```
src/
  main.ts       window layout, menus, palette, shortcuts
  app.ts        controller: tabs, files, analysis loop, edit commands
  editor.ts     CodeMirror 6 + Graphene language mode, diagnostics, fixes
  graph.ts      force-directed canvas graph (d3-force)
  sidebar.ts    Files, Program, Problems, Tests panels
  runpanel.ts   Run panel: inputs, mocks, results, replay, record, save case
  inspector.ts  node details and actions
  api.ts        typed wrappers over the Tauri commands
  store.ts      app state and events
src-tauri/      Tauri 2 shell; every command wraps graphene_ide_core::shell
e2e/            headless smoke test
```
