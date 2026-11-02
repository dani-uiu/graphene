//! Property test for edit commands (docs/ide-plan.md, Phase 1): random
//! command sequences on every example always give text that parses, and
//! every line a command did not touch survives byte-for-byte, in order.

use graphene_ide_core::text::{apply_edits, line_count, line_of, line_text};
use graphene_ide_core::{analyze, apply, Command, TextEdit};
use proptest::prelude::*;

fn examples() -> Vec<String> {
    let root = format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"));
    let mut out: Vec<String> = ["learn", "apps"]
        .iter()
        .flat_map(|d| std::fs::read_dir(format!("{root}/{d}")).expect("examples"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "grph"))
        .map(|p| std::fs::read_to_string(p).expect("read"))
        .collect();
    out.sort();
    out
}

/// A command shape; names are picked from the current program at run time.
#[derive(Debug, Clone)]
enum Shape {
    Step(usize, &'static str),
    Connect(usize, u8),
    Rename(usize, u8),
    Delete(usize),
    Join(usize, usize),
}

fn shape() -> impl Strategy<Value = Shape> {
    let step = prop::sample::select(vec!["f", "fx", "fx~", "match", "out", "fail"]);
    prop_oneof![
        (any::<usize>(), step).prop_map(|(i, s)| Shape::Step(i, s)),
        (any::<usize>(), any::<u8>()).prop_map(|(i, k)| Shape::Connect(i, k)),
        (any::<usize>(), any::<u8>()).prop_map(|(i, k)| Shape::Rename(i, k)),
        any::<usize>().prop_map(Shape::Delete),
        (any::<usize>(), any::<usize>()).prop_map(|(a, b)| Shape::Join(a, b)),
    ]
}

/// Names of value-like nodes that commands can start from.
fn names(src: &str) -> Vec<String> {
    analyze(src)
        .nodes
        .into_iter()
        .filter(|n| n.defined && matches!(n.kind, "value" | "input"))
        .map(|n| n.name)
        .collect()
}

fn command(src: &str, s: &Shape) -> Option<Command> {
    let ns = names(src);
    let pick = |i: usize| ns.get(i % ns.len().max(1)).cloned();
    Some(match s {
        Shape::Step(i, step) => Command::AddStep {
            from: pick(*i)?,
            step: step.to_string(),
            effect: None,
        },
        Shape::Connect(i, k) => Command::Connect {
            from: pick(*i)?,
            to: format!("Zc{k}"),
        },
        Shape::Rename(i, k) => Command::Rename {
            old: pick(*i)?,
            new: format!("Rn{k}"),
        },
        Shape::Delete(i) => Command::DeleteNode { name: pick(*i)? },
        Shape::Join(a, b) => Command::Join {
            members: vec![pick(*a)?, pick(*b)?],
            target: None,
        },
    })
}

/// Lines of `before` that no edit touches must appear in `after`, in order.
fn untouched_lines_survive(before: &str, edits: &[TextEdit], after: &str) {
    let utf8 = |u16_off: usize| {
        let mut units = 0;
        for (b, c) in before.char_indices() {
            if units >= u16_off {
                return b;
            }
            units += c.len_utf16();
        }
        before.len()
    };
    let mut touched = std::collections::BTreeSet::new();
    for e in edits {
        let (a, b) = (line_of(before, utf8(e.from)), line_of(before, utf8(e.to)));
        touched.extend(a..=b);
        touched.insert(a.saturating_sub(1).max(1));
    }
    let new_lines: Vec<&str> = after.lines().collect();
    let mut at = 0;
    for l in 1..=line_count(before) {
        if touched.contains(&l) {
            continue;
        }
        let text = line_text(before, l);
        let found = new_lines[at..].iter().position(|x| *x == text);
        assert!(found.is_some(), "line {l} `{text}` was lost:\n{after}");
        at += found.unwrap_or(0) + 1;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn random_commands_keep_text_parseable(which in any::<usize>(), shapes in prop::collection::vec(shape(), 1..6)) {
        let all = examples();
        let mut src = all[which % all.len()].clone();
        for s in &shapes {
            let Some(cmd) = command(&src, s) else { continue };
            let Ok(edits) = apply(&src, &cmd) else { continue };
            let next = apply_edits(&src, &edits);
            let (_, diags) = graphene::parse(&next);
            prop_assert!(diags.is_empty(), "{cmd:?} broke parsing: {diags:?}\n{next}");
            untouched_lines_survive(&src, &edits, &next);
            src = next;
        }
    }
}
