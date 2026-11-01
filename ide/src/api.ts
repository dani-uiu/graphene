// Typed wrappers over the Tauri commands in src-tauri/src/main.rs.
// Every language decision happens in Rust (graphene-ide-core); this file
// only moves data.

import { invoke } from "@tauri-apps/api/core";

export interface NodeView {
  id: string;
  kind: "input" | "param" | "value" | "sink" | "op" | "match" | "out" | "fail";
  op: "f" | "fx" | "fx~" | "call" | "map" | "filter" | null;
  name: string;
  detail: string;
  line: number;
  lines: number[];
  defined: boolean;
  writers: number;
  ty: string | null;
  guard: string | null;
  deps: string[];
  order: number | null;
  error: boolean;
  /** Subgraph whose body holds this node (null: main program). */
  group: string | null;
  /** For call/map/filter nodes: the subgraph they run. */
  calls: string | null;
}

export interface GroupView {
  name: string;
  line: number;
  end_line: number;
  params: [string, string][];
  ret: string;
  errors: string[];
}

export interface EdgeView {
  from: string;
  to: string;
  kind: "flow" | "ok" | "err" | "arm" | "ref";
  label: string | null;
  line: number;
}

export interface TextEdit {
  from: number;
  to: number;
  insert: string;
}

export interface Fix {
  label: string;
  edits: TextEdit[];
}

export interface DiagView {
  code: string;
  message: string;
  label: string;
  help: string;
  line: number;
  from: number;
  to: number;
  notes: [number, number, string][];
  fixes: Fix[];
}

export interface ExternView {
  name: string;
  is_async: boolean;
  params: [string, string][];
  ret: string | null;
  errors: string[];
  line: number;
}

export interface InputView {
  name: string;
  ty: string;
  node: string | null;
  line: number;
}

export interface Analysis {
  version: string;
  intent: string | null;
  nodes: NodeView[];
  edges: EdgeView[];
  externs: ExternView[];
  inputs: InputView[];
  asserts: { text: string; line: number }[];
  groups: GroupView[];
  diags: DiagView[];
  checked: boolean;
  unicode: boolean;
}

export type Command =
  | { cmd: "set_line"; line: number; text: string }
  | { cmd: "delete_line"; line: number }
  | { cmd: "insert_line"; after: number; text: string }
  | { cmd: "add_input"; name: string; ty: string; node: string | null }
  | { cmd: "add_extern"; text: string }
  | { cmd: "add_assert"; text: string }
  | { cmd: "add_step"; from: string; step: string; effect: string | null }
  | { cmd: "connect"; from: string; to: string }
  | { cmd: "join"; members: string[]; target: string | null }
  | { cmd: "rename"; old: string; new: string }
  | { cmd: "delete_node"; name: string }
  | { cmd: "set_header"; version: string }
  | { cmd: "add_subgraph"; name: string; ty: string };

export interface NodeState {
  id: string;
  status: "active" | "inactive" | "fault";
  value: unknown;
  step: number;
  count: number | null;
}

export interface ElementView {
  at: [string, number | null][];
  group: string;
  nodes: NodeState[];
}

export interface CoverItem {
  kind: "outcome" | "branch" | "effect";
  graph: string | null;
  line: number;
  label: string;
  target: string | null;
  covered: boolean;
}

export interface HostChoice {
  mockPath: string | null;
  hostPath: string | null;
  recordPath: string | null;
}

export interface RunView {
  output: any | null;
  error: string | null;
  nodes: NodeState[];
  fault_node: string | null;
  parallel_matches: boolean | null;
  elements: ElementView[];
}

export interface CaseResult {
  name: string;
  pass: boolean;
  detail: string | null;
  expect: unknown;
  actual: unknown;
  inputs: Record<string, unknown>;
  mock: string | null;
}

export interface Entry {
  name: string;
  path: string;
  dir: boolean;
  children: Entry[];
}

export const api = {
  analyze: (text: string) => invoke<Analysis>("analyze", { text }),
  apply: (text: string, command: Command) => invoke<TextEdit[]>("apply_command", { text, command }),
  format: (text: string, unicode: boolean) => invoke<string>("format_text", { text, unicode }),
  run: (text: string, args: [string, string][], hosts: HostChoice, parallel: boolean) =>
    invoke<RunView>("run_program", { text, args, hosts, parallel }),
  coverage: (text: string, programPath: string) => invoke<CoverItem[]>("coverage", { text, programPath }),
  runCases: (text: string, programPath: string, parallel: boolean) =>
    invoke<CaseResult[]>("run_cases", { text, programPath, parallel }),
  saveCase: (programPath: string, name: string, inputsJson: string, mock: string | null, expect: unknown) =>
    invoke<void>("save_case", { programPath, name, inputsJson, mock, expect }),
  tokens: (text: string) => invoke<{ method: string; ascii: number; unicode: number }>("tokens", { text }),
  exportDot: (text: string) => invoke<string>("export_dot", { text }),
  readFile: (path: string) => invoke<string>("read_file", { path }),
  writeFile: (path: string, text: string) => invoke<void>("write_file", { path, text }),
  mtime: (path: string) => invoke<number | null>("file_mtime", { path }),
  listDir: (path: string) => invoke<Entry>("list_dir", { path }),
};
