//! `/help [vendor]` — the transient help buffer, rendered from the registry
//! (S4). Includes N/A-by-design rows WITH their documented reason (the parity
//! law's "documented, never silently absent") — the one place those rows
//! surface, since the typeahead excludes them.
//!
//! Output is plain markdown (the fork renders it through `crates/markdown` in
//! a transient buffer — RUST_PORT_NOTES §3). Pure — unit-tested directly.

use std::fmt::Write as _;

use super::types::{Classification, CommandEntry, CommandKind, Vendor};

/// Build the `/help` markdown. `vendor` scopes to one vendor's rows (plus the
/// orchestrator-native set, which is always shown as the product's own layer);
/// `None` = the full surface grouped by source.
pub fn render_help(rows: &[CommandEntry], vendor: Option<Vendor>) -> String {
    let mut out = String::new();
    match vendor {
        Some(v) => {
            let _ = writeln!(out, "# /{} commands\n", v.badge());
            section(&mut out, "Orchestrator", rows, |r| {
                r.kind == CommandKind::Orchestrator
            });
            section(&mut out, v.badge(), rows, |r| r.vendor == Some(v));
        }
        None => {
            let _ = writeln!(out, "# Command surface\n");
            let _ = writeln!(
                out,
                "Every command by source. Rows marked _(disabled)_ are real \
                 in principle but have no working trigger yet; _(N/A)_ rows are \
                 documented here but never appear in the `/` menu.\n"
            );
            section(&mut out, "Orchestrator", rows, |r| {
                r.kind == CommandKind::Orchestrator
            });
            section(&mut out, "Skills", rows, |r| r.kind == CommandKind::Skill);
            section(&mut out, "Custom commands", rows, |r| {
                r.kind == CommandKind::Custom
            });
            for v in [Vendor::Claude, Vendor::Codex, Vendor::Gemini, Vendor::Agy] {
                section(&mut out, v.badge(), rows, |r| {
                    r.vendor == Some(v) && matches!(r.kind, CommandKind::Builtin)
                });
            }
        }
    }
    out
}

/// Emit one `## Section` with the matching rows (skipped entirely when empty).
fn section(
    out: &mut String,
    title: &str,
    rows: &[CommandEntry],
    keep: impl Fn(&CommandEntry) -> bool,
) {
    let mut matched: Vec<&CommandEntry> = rows.iter().filter(|r| keep(r)).collect();
    if matched.is_empty() {
        return;
    }
    matched.sort_by(|a, b| a.name.cmp(&b.name));
    let _ = writeln!(out, "## {title}\n");
    for row in matched {
        let _ = writeln!(out, "- `{}` — {}{}", row.slash_name(), row.description, tail(row));
    }
    out.push('\n');
}

/// The trailing status marker: `_(N/A: reason)_` for by-design rows,
/// `_(disabled: reason)_` for not-yet-built ones, nothing for live rows.
fn tail(row: &CommandEntry) -> String {
    if let Classification::NaByDesign { reason } = &row.classification {
        return format!(" _(N/A: {reason})_");
    }
    if let Some(reason) = row.disabled_tooltip() {
        return format!(" _(disabled: {reason})_");
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::static_seed::static_entries;

    #[test]
    fn full_help_lists_every_source_and_flags_disabled() {
        let md = render_help(&static_entries(), None);
        assert!(md.contains("## Orchestrator"));
        assert!(md.contains("`/plan`"));
        // A disabled orchestrator row shows its tooltip reason.
        assert!(md.contains("`/compact`"));
        assert!(md.contains("_(disabled:"));
    }

    #[test]
    fn na_by_design_rows_appear_in_help_with_reason() {
        // The honesty clause: /login never appears in the menu but MUST be in
        // /help with its documented reason.
        let md = render_help(&static_entries(), Some(Vendor::Claude));
        assert!(md.contains("`/login`"));
        assert!(md.contains("_(N/A:"));
    }

    #[test]
    fn vendor_scoped_help_shows_orchestrator_plus_that_vendor() {
        let md = render_help(&static_entries(), Some(Vendor::Codex));
        assert!(md.contains("# /codex commands"));
        assert!(md.contains("## Orchestrator"));
        assert!(md.contains("## codex"));
        // Not claude's built-ins.
        assert!(!md.contains("`/security-review`"));
    }
}
