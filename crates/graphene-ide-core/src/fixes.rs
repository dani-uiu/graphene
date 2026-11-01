//! Quick fixes: concrete edits for common diagnostics, built from what
//! each diagnostic's `help:` line suggests (docs/ide-plan.md §3.4).
//!
//! Depends on: `edit` (shared parsing context and line regeneration),
//! `graphene::diag`, `text`.
//!
//! Invariants: a fix never touches lines other than the ones it names;
//! a diagnostic with no safe mechanical fix simply gets none.

use std::collections::BTreeSet;

use graphene::diag::{Code, Diag};
use graphene::syntax::ast::*;
use serde::Serialize;

use crate::edit::Ctx;
use crate::text::{self, ByteEdit, TextEdit};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fix {
    pub label: String,
    pub edits: Vec<TextEdit>,
}

/// Every quick fix available for `d`.
pub fn fixes_for(src: &str, d: &Diag) -> Vec<Fix> {
    let ctx = Ctx::new(src);
    let line = d.line(src);
    let stmt = ctx
        .stmts
        .iter()
        .find(|(l, _)| *l == line)
        .map(|(_, s)| s.clone());
    let found = match (d.code, stmt) {
        (Code::P006, _) => header(&ctx),
        (Code::G009, _) if d.help.contains("#!graphene 0.2") => header(&ctx),
        (Code::G005, Some(Stmt::Flow(f))) => err_edges(&ctx, d, line, f),
        (Code::G004, Some(Stmt::Match(m))) => wildcard_arm(&ctx, line, m),
        (Code::G014, Some(s)) => redundant(&ctx, d, line, s),
        (Code::G011, _) => Some(("Remove this `out`".into(), vec![ctx.delete_stmt(line)])),
        (Code::G008, Some(Stmt::Input(_))) if d.message.ends_with("is never used") => Some((
            "Remove the unused input".into(),
            vec![ctx.delete_stmt(line)],
        )),
        (Code::G002, _) => use_input_name(src, d),
        (Code::G010, _) if d.label == "not a sink" => Some((
            "Make the target a sink".into(),
            vec![ByteEdit {
                from: d.span.start,
                to: d.span.start,
                insert: "sink ".into(),
            }],
        )),
        (Code::G003, Some(Stmt::Fail(f))) if f.name.is_none() => {
            let mut f = f.clone();
            f.name = Some(ident("Failed"));
            Some((
                "Use a named error: `fail Failed`".into(),
                vec![ctx.regenerate(line, &Stmt::Fail(f))],
            ))
        }
        _ => None,
    };
    found
        .map(|(label, edits)| Fix {
            label,
            edits: text::to_utf16(src, edits),
        })
        .into_iter()
        .collect()
}

/// G004: add a `_ => N` arm to a non-exhaustive match.
fn wildcard_arm(ctx: &Ctx, line: usize, mut m: MatchStmt) -> Found {
    let mut taken = BTreeSet::new();
    let n = ctx.fresh("N", &mut taken);
    m.arms.push(Arm {
        pat: Pat::Wild(Pos::default()),
        target: ident(&n),
    });
    Some((
        format!("Add a `_ => {n}` arm"),
        vec![ctx.regenerate(line, &Stmt::Match(m))],
    ))
}

fn ident(name: &str) -> Ident {
    Ident {
        name: name.to_string(),
        pos: Pos::default(),
    }
}

type Found = Option<(String, Vec<ByteEdit>)>;

fn header(ctx: &Ctx) -> Found {
    Some((
        "Change the header to `#!graphene 0.2`".into(),
        vec![text::replace_line(ctx.src, 1, "#!graphene 0.2")],
    ))
}

/// G005: add a catch-all error edge to a new `fail` node, or remove error
/// edges from an effect that cannot fail.
fn err_edges(ctx: &Ctx, d: &Diag, line: usize, mut f: Flow) -> Found {
    if d.message.contains("can never fire") {
        f.errs.clear();
        return Some((
            "Remove the error edge".into(),
            vec![ctx.regenerate(line, &Stmt::Flow(f))],
        ));
    }
    let mut taken = BTreeSet::new();
    let e = ctx.fresh("E", &mut taken);
    f.errs.push(ErrEdge {
        variant: None,
        target: ident(&e),
    });
    let fail = ctx.render(&format!("{e} -> fail")).ok()?;
    Some((
        format!("Handle errors: `| err -> {e}` and `{e} -> fail`"),
        vec![
            ctx.regenerate(line, &Stmt::Flow(f)),
            text::insert_after(ctx.src, ctx.span_of(line).1, &fail),
        ],
    ))
}

/// G014: drop the redundant match arm or error edge the diagnostic marks.
fn redundant(ctx: &Ctx, d: &Diag, line: usize, s: Stmt) -> Found {
    let kept = match s {
        Stmt::Match(mut m) => {
            let before = m.arms.len();
            m.arms.retain(|a| a.pat.span() != d.span);
            (m.arms.len() < before).then_some(Stmt::Match(m))
        }
        Stmt::Flow(mut f) => {
            let before = f.errs.len();
            f.errs.retain(|e| e.target.span() != d.span);
            (f.errs.len() < before).then_some(Stmt::Flow(f))
        }
        _ => None,
    }?;
    Some((
        "Remove the redundant part".into(),
        vec![ctx.regenerate(line, &kept)],
    ))
}

/// G002 for an input node used in an expression: the label says
/// "use `name` here".
fn use_input_name(src: &str, d: &Diag) -> Found {
    let name = d.label.strip_prefix("use `")?.strip_suffix("` here")?;
    let _ = src;
    Some((
        format!("Use the input name `{name}`"),
        vec![ByteEdit {
            from: d.span.start,
            to: d.span.end,
            insert: name.to_string(),
        }],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::apply_edits;

    fn fixed(path: &str) -> String {
        let src = std::fs::read_to_string(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR")))
            .expect("example");
        let diags = graphene::check(&src).expect_err("invalid example");
        let fixes = fixes_for(&src, &diags[0]);
        let fix = fixes.first().unwrap_or_else(|| panic!("{path}: no fix"));
        apply_edits(&src, &fix.edits)
    }

    #[test]
    fn fixes_resolve_their_diagnostic() {
        for (path, gone) in [
            ("examples/invalid/unhandled_err.grph", "G005"),
            ("examples/invalid/uncovered_variant.grph", "G005"),
            ("examples/invalid/redundant_arm.grph", "G014"),
            ("examples/invalid/redundant_catchall.grph", "G014"),
            ("examples/invalid/two_outs.grph", "G011"),
            ("examples/invalid/input_node_ref.grph", "G002"),
            ("examples/invalid/unused_param.grph", "G008"),
            ("examples/invalid/v01_feature.grph", "P006"),
            ("examples/invalid/v01_input_scope.grph", "G009"),
            ("examples/invalid/nonexhaustive.grph", "G004"),
        ] {
            let out = fixed(path);
            let codes: Vec<String> = match graphene::check(&out) {
                Ok(_) => Vec::new(),
                Err(d) => d.iter().map(|d| d.code.to_string()).collect(),
            };
            assert!(
                !codes.contains(&gone.to_string()),
                "{path}: still {codes:?}\n{out}"
            );
        }
    }
}
