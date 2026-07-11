# Fork divergence management: additive crates, enumerated touch list, merge-based tracking

All fork functionality lives in new ("additive") crates; edits to upstream files are
restricted to an explicitly enumerated touch list (see `docs/glass/migration-plan.md`
§6.3) of small, mechanical registration hooks; upstream is tracked by periodically
merging zed `main` into the long-lived `glass` branch (never rebasing published
history), while the fork's `main` stays a clean upstream mirror.

This is a direct lesson from the original Glass repository, whose in-place rewrites
of `workspace.rs`, `dock.rs`, and the title bar produced an ever-growing merge tax
visible in its history as a chain of increasingly painful "Merge upstream through …"
commits. The explicit no: we do not carry broad rewrites of upstream crates, and when
an extension point is missing upstream, the preferred move is to contribute the hook
to upstream Zed rather than patch it locally.

## Consequences

Some Glass behaviors that required invasive workspace changes (native sidebar, mode
switching in the title bar) are deferred or dropped rather than ported as-is. Any
implementation step wanting to edit an upstream file not on the touch list must first
justify expanding the list.
