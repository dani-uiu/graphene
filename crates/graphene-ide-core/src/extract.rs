//! Extract into subgraph (v0.4 IDE, deferred since v0.3): selected
//! top-level statements become a `graph` block plus one `call`.
//!
//! Depends on: `edit::Ctx`, `graphene` (fmt, check through `Workspace`).
//!
//! Invariants:
//! - it works only when exactly one value flows into the selection and
//!   exactly one value it produces is used afterwards; otherwise the
//!   error names the flows in the way;
//! - the rewritten program is checked before any edit is returned, so an
//!   extraction never leaves a program that used to check broken;
//! - lines outside the selection are untouched, apart from the inserted
//!   block.

use std::collections::BTreeSet;

use graphene::fmt::format_stmt_wrapped;
use graphene::syntax::ast::*;

use crate::edit::Ctx;
use crate::text::{self, ByteEdit};
use crate::view::collect_names;

impl Ctx<'_> {
    /// Extract the top-level statements starting on lines `first..=last`
    /// into subgraph `name`.
    pub(crate) fn extract(
        &self,
        first: usize,
        last: usize,
        name: &str,
    ) -> Result<Vec<ByteEdit>, String> {
        self.valid_name(name)?;
        if self.words.contains(name) {
            return Err(format!("`{name}` is already used"));
        }
        if self.program.version < (0, 3) {
            return Err("subgraphs need `#!graphene 0.3`; change the header first".into());
        }
        let top = self.top_level_lines();
        let selected: Vec<&(usize, Stmt)> = self
            .stmts
            .iter()
            .filter(|(l, _)| (first..=last).contains(l))
            .collect();
        if selected.is_empty() {
            return Err("select the statements to extract".into());
        }
        for (l, s) in &selected {
            if !top.contains(l) {
                return Err(format!(
                    "line {l} is inside a subgraph; extract top-level statements"
                ));
            }
            if !matches!(s, Stmt::Flow(_) | Stmt::Match(_)) {
                return Err(format!(
                    "line {l} is not a flow or a `match`; only steps between an input and a result can be extracted"
                ));
            }
        }
        let values = self.value_names();
        let canon = |n: &str| self.canonical(n);
        let mut defined = BTreeSet::new();
        let mut used = BTreeSet::new();
        for (_, s) in &selected {
            defined.extend(defs(s).into_iter().map(|n| canon(&n)));
            used.extend(
                uses(s)
                    .into_iter()
                    .filter(|n| values.contains(n))
                    .map(|n| canon(&n)),
            );
        }
        let incoming: Vec<String> = used.difference(&defined).cloned().collect();
        let entry = match incoming.as_slice() {
            [one] => one.clone(),
            [] => return Err("nothing flows into the selection".into()),
            many => {
                return Err(format!(
                    "{} all flow into the selection; extract works when exactly one flow comes in",
                    names(many)
                ))
            }
        };
        let mut after = BTreeSet::new();
        for (l, s) in &self.stmts {
            if selected.iter().any(|(sl, _)| sl == l) {
                continue;
            }
            after.extend(uses(s).into_iter().map(|n| canon(&n)));
            if let Stmt::AssertIndep(sink, _) = s {
                after.insert(sink.name.clone());
            }
        }
        let outgoing: Vec<String> = defined.intersection(&after).cloned().collect();
        let result = match outgoing.as_slice() {
            [one] => one.clone(),
            [] => return Err("nothing the selection produces is used afterwards".into()),
            many => return Err(format!(
                "{} are all used after the selection; extract works when exactly one flow goes out",
                names(many)
            )),
        };
        // The parameter: an input keeps its name and node; a node `N`
        // becomes the parameter's node, read in expressions as `n`.
        let input = self.stmts.iter().find_map(|(_, s)| match s {
            Stmt::Input(i)
                if i.node.as_ref().is_some_and(|n| n.name == entry) || i.name.name == entry =>
            {
                Some(i.clone())
            }
            _ => None,
        });
        let (pname, pnode, rename) = match &input {
            Some(i) => match &i.node {
                Some(n) => (i.name.name.clone(), n.name.clone(), None),
                None => {
                    return Err(format!(
                        "`{entry}` has no node to flow from; give it one first: `in {entry}: <type> -> Node`"
                    ))
                }
            },
            None => {
                let mut taken = BTreeSet::new();
                let lower = entry.to_lowercase();
                let p = if self.words.contains(&lower) { self.fresh(&lower, &mut taken) } else { lower };
                (p.clone(), entry.clone(), Some((entry.clone(), p)))
            }
        };
        let pty = self
            .type_of(&pnode)
            .ok_or("the program must check before extracting (the types come from the checker)")?;
        let rty = self
            .type_of(&result)
            .ok_or("the program must check before extracting (the types come from the checker)")?;
        let nl = text::newline(self.src);
        let mut block = format!("graph {name}({pname}: {pty} -> {pnode}) -> {rty} {{");
        for (l, s) in &selected {
            let mut s = s.clone();
            if let Some((from, to)) = &rename {
                rename_reads(&mut s, from, to);
            }
            block.push_str(&format!(
                "{nl}  {}",
                format_stmt_wrapped(&s, self.style, "  ")
            ));
            if let Some(c) = self.comment(*l) {
                block.push_str(&format!("  //{c}"));
            }
        }
        block.push_str(&format!("{nl}  {result} -> out{nl}}}"));
        let call = self.render(&format!(
            "{pnode} -> call {name}({pname}: {}) -> {result}",
            self.value_ref(&pnode)
        ))?;
        let first_line = selected[0].0;
        let mut edits = Vec::new();
        for (i, (l, _)) in selected.iter().enumerate() {
            let (a, b) = self.span_of(*l);
            edits.push(if i == 0 {
                text::replace_lines(self.src, a, b, &call)
            } else {
                text::delete_lines(self.src, a, b)
            });
        }
        let at = self
            .stmts
            .iter()
            .filter(|(l, s)| {
                *l < first_line
                    && matches!(
                        s,
                        Stmt::Extern(_)
                            | Stmt::Const(_)
                            | Stmt::Intent(..)
                            | Stmt::GraphEnd(_)
                            | Stmt::TypeAlias(_)
                            | Stmt::Enum(_)
                            | Stmt::Import(_)
                    )
            })
            .map(|(l, _)| self.span_of(*l).1)
            .next_back()
            .unwrap_or(1);
        edits.push(text::insert_after(self.src, at, &format!("{nl}{block}")));
        let out = text::apply_edits(self.src, &text::to_utf16(self.src, edits.clone()));
        if let Err(f) = self.ws.check(&out) {
            let why = f
                .diags
                .first()
                .map_or(String::new(), |d| format!(": [{}] {}", d.code, d.message));
            return Err(format!("the extracted program would not check{why}"));
        }
        Ok(edits)
    }

    /// First lines of statements outside every `graph` body.
    fn top_level_lines(&self) -> BTreeSet<usize> {
        let mut depth = 0usize;
        let mut out = BTreeSet::new();
        for (l, s) in &self.stmts {
            match s {
                Stmt::GraphHeader(_) => depth += 1,
                Stmt::GraphEnd(_) => depth = depth.saturating_sub(1),
                _ if depth == 0 => {
                    out.insert(*l);
                }
                _ => {}
            }
        }
        out
    }

    /// Names that hold values at the top level: inputs (both names) and
    /// nodes, but not effects, constants or types.
    fn value_names(&self) -> BTreeSet<String> {
        let externs: BTreeSet<String> = self
            .stmts
            .iter()
            .filter_map(|(_, s)| match s {
                Stmt::Extern(e) => Some(e.name.name.clone()),
                _ => None,
            })
            .collect();
        self.defined().difference(&externs).cloned().collect()
    }

    /// An input's name and its node are one value: use the node.
    fn canonical(&self, name: &str) -> String {
        for (_, s) in &self.stmts {
            if let Stmt::Input(i) = s {
                if i.name.name == name {
                    return i.node.as_ref().map_or(name.to_string(), |n| n.name.clone());
                }
            }
        }
        name.to_string()
    }
}

fn names(list: &[String]) -> String {
    let quoted: Vec<String> = list.iter().map(|n| format!("`{n}`")).collect();
    match quoted.as_slice() {
        [a, b] => format!("{a} and {b}"),
        _ => quoted.join(", "),
    }
}

/// Values a statement defines.
fn defs(s: &Stmt) -> Vec<String> {
    match s {
        Stmt::Flow(f) => std::iter::once(f.target.ident().name.clone())
            .chain(f.errs.iter().map(|e| e.target.name.clone()))
            .collect(),
        Stmt::Match(m) => m.arms.iter().map(|a| a.target.name.clone()).collect(),
        _ => Vec::new(),
    }
}

/// Values a statement reads: its sources and the names in its expressions.
fn uses(s: &Stmt) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    match s {
        Stmt::Flow(f) => {
            out.extend(f.sources.iter().map(|i| i.name.clone()));
            match &f.op {
                Op::Pure(e) => collect_names(e, &mut out),
                Op::Fx { args, .. } | Op::FxAsync { args, .. } | Op::Call { args, .. } => {
                    for a in args {
                        collect_names(&a.value, &mut out);
                    }
                }
            }
        }
        Stmt::Match(m) => {
            out.insert(m.source.name.clone());
            collect_names(&m.scrutinee, &mut out);
        }
        Stmt::Out(o) => {
            out.insert(o.source.name.clone());
        }
        Stmt::Fail(f) => {
            out.insert(f.source.name.clone());
        }
        _ => {}
    }
    out
}

/// Rename the expression reads of `from` (not flow sources) to `to`.
fn rename_reads(s: &mut Stmt, from: &str, to: &str) {
    match s {
        Stmt::Flow(f) => match &mut f.op {
            Op::Pure(e) => rename_expr(e, from, to),
            Op::Fx { args, .. } | Op::FxAsync { args, .. } | Op::Call { args, .. } => {
                for a in args {
                    rename_expr(&mut a.value, from, to);
                }
            }
        },
        Stmt::Match(m) => rename_expr(&mut m.scrutinee, from, to),
        _ => {}
    }
}

fn rename_expr(e: &mut Expr, from: &str, to: &str) {
    match &mut e.kind {
        ExprKind::Name(i) if i.name == from => i.name = to.to_string(),
        ExprKind::Lit(_) | ExprKind::Name(_) => {}
        ExprKind::Field(b, _) | ExprKind::Unary(_, b) | ExprKind::Convert(_, b) => {
            rename_expr(b, from, to)
        }
        ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) => {
            rename_expr(l, from, to);
            rename_expr(r, from, to);
        }
        ExprKind::If(c, a, b) => {
            for x in [c, a, b] {
                rename_expr(x, from, to);
            }
        }
        ExprKind::Record(fields) => {
            for (_, x) in fields {
                rename_expr(x, from, to);
            }
        }
        ExprKind::List(xs) | ExprKind::Builtin(_, xs) => {
            for x in xs {
                rename_expr(x, from, to);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::edit::{apply, Command};
    use crate::text::apply_edits;

    const SRC: &str = "#!graphene 0.3\nin price: dec -> P\nin qty: u64\n\nP -> f(price * 1.21) -> GROSS\nGROSS -> f(round(GROSS, 2)) -> R  // rounded\nR -> f(R + 1.00) -> T\nT -> out total: dec\n";

    fn extract(src: &str, first: usize, last: usize) -> Result<String, String> {
        let cmd = Command::ExtractSubgraph {
            first,
            last,
            name: "Tax".into(),
        };
        apply(src, &cmd).map(|e| apply_edits(src, &e))
    }

    #[test]
    fn one_flow_in_and_one_out_becomes_a_call() {
        let src = SRC.replace("in qty: u64\n", "");
        let out = extract(&src, 4, 5).expect("extracts");
        assert!(out.contains("graph Tax(price: dec -> P) -> dec {"), "{out}");
        assert!(
            out.contains("  GROSS -> f(round(GROSS, 2)) -> R  // rounded"),
            "{out}"
        );
        assert!(out.contains("P -> call Tax(price: price) -> R\n"), "{out}");
        assert!(graphene::check(&out).is_ok(), "{out}");
    }

    #[test]
    fn a_node_entry_is_read_through_a_parameter() {
        let src = SRC.replace("in qty: u64\n", "");
        let out = extract(&src, 5, 6).expect("extracts");
        assert!(
            out.contains("graph Tax(gross: dec -> GROSS) -> dec {"),
            "{out}"
        );
        assert!(out.contains("GROSS -> f(round(gross, 2)) -> R"), "{out}");
        assert!(
            out.contains("GROSS -> call Tax(gross: GROSS) -> T"),
            "{out}"
        );
        assert!(graphene::check(&out).is_ok(), "{out}");
    }

    #[test]
    fn the_flow_in_the_way_is_named() {
        let src = SRC.replace("R -> f(R + 1.00) -> T", "R -> f(R + dec(qty)) -> T");
        let e = extract(&src, 6, 7).expect_err("two flows in");
        assert!(e.contains("`GROSS` and `qty`"), "{e}");
        let both = src.replace("R + dec(qty)", "R + GROSS + dec(qty)");
        let e = extract(&both, 5, 6).expect_err("two flows out");
        assert!(e.contains("`GROSS` and `R`"), "{e}");
        // `qty` is unused in SRC (G008), so the types are unknown.
        let e = extract(SRC, 5, 6).expect_err("program must check first");
        assert!(e.contains("must check"), "{e}");
    }
}
