# Refactoring TODO

Audit baseline: `b42c018` (2026-10-01, after #55 / PR #60).

Use small, behavior-preserving stacked PRs. Each PR targets the previous branch,
so its diff contains only that step. Merge from the bottom of the stack upward,
retargeting/rebasing later PRs onto main as needed. Update this checklist in the
PR that completes the work.
Future design candidates are conditional; they are not a request to build new
abstractions ahead of the related features.

## 1. Shared definitions and duplicated protocol handling

- [x] Centralize application-owned UI colors and the syntax theme name (#33).
  Preserve every existing color, including the focused-row background
  `rgb(84, 84, 84)`, diff backgrounds, syntax colors, and selection contrast.
  Keep configuration and theme switching out of scope.
- [x] Share the Git patch options used by snapshots and context expansion.
  Preserve revision/path validation, rename behavior, batched reads, and per-file
  fallbacks. This prepares the code for #23 without implementing rename support.
- [x] Reuse the daemon WebSocket response loop for initialization and thread reads.
  Preserve the shared deadline, notification handling, response-ID checks, and
  disconnect/error behavior.

These items are implemented in the first refactoring PR. Later PRs build on its
branch until it is merged.

## 2. List construction and input handling

- [x] Separate file-list construction from rebuilding all sidebar lists.
  `App::rebuild_lists` currently also rebuilds comments and commits. Keep file
  filtering, compact directory chains, folds, and selection restoration together.
  Check callers before reducing which lists an operation rebuilds.
- [ ] Keep each list's display rows and source-item mapping together in its
  construction path. Preserve working-tree rows, filtered commit selection,
  history references, and file selection through filter/fold changes.
- [x] Split `App::handle` into focused mouse, editor, modal, and key handlers.
  Share sidebar activation directly between mouse and keyboard handling.
  Session-picker key synthesis remains part of the next step.
  Preserve event ordering, press/release semantics, and pending clicks during drags.

## 3. Session picker state

- [ ] Group the search input, options, and active control shared by Loading and
  Sessions; keep cancellation/worker ownership explicit.
- [ ] Share session filtering between rendering, mouse handling, and key handling.
  Preserve new-session and clipboard entries and their selection positions.
- [ ] Derive filter layout and hit areas from the labels that are rendered (#54).
  Check both loading and loaded states at narrow terminal widths.
- [ ] Consider representing preview destinations as an enum instead of an ID,
  clipboard flag, archived flag, and display label that must agree.

Investigate #54 separately before changing database-to-rollout fallback behavior
or claiming a loading-time fix.

## 4. Diff coordinates and related state

- [ ] Centralize restoration of a visual position after wrapping/layout changes
  in `FileView`; the cursor, selection anchors, drag anchor, and editor currently
  repeat the same row/fragment conversion.
- [ ] Share screen-to-diff hit testing with the layout used for drawing (#49, #51).
  Preserve unified/split views, scrolling, wrapping, comments, and editor layout.
- [ ] Give omitted-context rows enough identity to target the clicked gap (#49),
  rather than relying only on nearest-gap expansion. Treat enabling the new
  interaction as a separate feature change.
- [ ] Reuse the hunk-navigation row calculation across expanded and normal views
  where the mappings permit it; preserve original comment coordinates.
- [ ] Group expanded context and its visibility/source mappings if this makes
  their invariants explicit. Keep `FileView`'s useful derived-data caches.
- [ ] Group related `App` state only after the handler/layout boundaries are clear.
  The audit counted 49 `App` fields and 21 `FileView` fields; field count alone
  does not justify splitting a structure.
- [ ] Consider `Pane` and `Side` enums where they remove ambiguous numeric indices.
  Avoid introducing dedicated types for every integer without a concrete benefit.

## 5. Revisit with feature work

- [ ] #45: model comments outside diff hunks with stable source coordinates.
  Plan compatibility with saved reviews and history before changing `Comment`.
- [ ] #23: model old/new paths for renames, including pure renames, saved comments,
  history, context expansion, and batched Git metadata parsing.
- [ ] #10: revisit delivery state per recipient when multi-model sending is built.
- [ ] #11: separate configuration and editor launching from Codex-specific code
  when supporting configurable agent commands. Start with concrete commands.
- [ ] #12 / #13: separate Git acquisition from review persistence when branch
  comparisons or non-Git reviews actually require it. `Review` and `SessionClient`
  themselves have coherent responsibilities and do not need a size-based split.
- [ ] Split `tests/core.rs` by behavior if ongoing changes make its size an obstacle;
  retain shared helpers and existing cross-feature regression coverage.

## Guardrails and validation

- Keep atomic saves, state locking, validation, rollback, and Git fallbacks.
- Keep Unicode wrapping, IME cursor placement, bounded caching, incremental
  highlighting, and selection contrast checks.
- #56 and #57 are labeled invalid and are not confirmed bug fixes in this plan.
  #52 is labeled wontfix. Do not reopen the rejected #58 color change.
- No dependency removal or new dependency was justified by the audit.
- Run formatting, Clippy, Rust tests, CLI/terminal checks, release-workflow checks,
  and license validation for behavior-preserving code PRs, following CI.
