# Divergence ledger

Adjudication record for the `alacritty_terminal` → libghostty-vt migration
(prescribed by [verification-strategy.md §3.4](verification-strategy.md#34-divergence-ledger);
operative spec: [SPEC.md §7](SPEC.md#7-verification)). Every divergence — a
differential mismatch or a ported-test expectation change — gets an entry
here: the triggering input, both behaviors, and the adjudication (**fixed**
in the seam/core, or **accepted** with rationale). Accepted entries are the
only permitted deltas at gate time.

The ledger's full seeding happens at P7 with the differential harness; it is
opened early because P2's acceptance criteria require entries for the two
retired `Arc`-sharing round-trip tests (SPEC.md §6, P2 row).

Entry ID format: `<phase>-<sequence>`.

---

## P2-001 — `terminal_hyperlink_from_alacritty_keeps_alacritty_storage` retired

- **Phase / change**: P2 (Zed-owned domain types, ticket #43). `Hyperlink`
  dropped its `Alacritty` storage variant and became a fully owned
  `{ id: Option<Arc<str>>, uri: Arc<str> }` struct (SPEC.md §4.2, S2).
- **Triggering input**: `crates/terminal/src/alacritty.rs` test
  `terminal_hyperlink_from_alacritty_keeps_alacritty_storage`, which asserted
  `matches!(&hyperlink.data, HyperlinkData::Alacritty(_))` — i.e. that
  conversion kept the alacritty `Arc` storage alive inside the Zed type.
- **Old behavior**: `terminal_hyperlink_from_alacritty` wrapped the alacritty
  `Hyperlink` handle; the Zed value shared alacritty's allocation.
- **New behavior**: conversion copies `id`/`uri` into Zed-owned storage at
  snapshot build; no alacritty allocation outlives the seam.
- **Adjudication**: **accepted**. Storage sharing was an implementation
  detail of the wrapper era, unobservable through the public accessor
  surface; ghostty render cells are transient FFI handles, so owned
  materialization is forced (S2 rationale). The semantic half of the
  round-trip survives as
  `terminal_hyperlink_from_alacritty_preserves_id_and_uri`
  (id and uri preserved verbatim).

## P2-002 — `terminal_cell_from_alacritty_shares_extra_storage` retired

- **Phase / change**: P2 (Zed-owned domain types, ticket #43). `Cell` became
  fully Zed-owned (`c`/`fg`/`bg`/`flags` + rare data behind
  `Option<Arc<CellExtra>>`), converted at snapshot build (SPEC.md §4.2, S2).
- **Triggering input**: `crates/terminal/src/alacritty.rs` test
  `terminal_cell_from_alacritty_shares_extra_storage`, which asserted
  `Arc::ptr_eq` between the alacritty cell's `extra` and the converted Zed
  cell's `extra`.
- **Old behavior**: `terminal_cell_from_alacritty` cloned the alacritty cell
  wholesale, so both sides pointed at the same `CellExtra` allocation.
- **New behavior**: conversion materializes a Zed-owned `CellExtra`
  (grapheme tail + hyperlink) per snapshot; clones of the *owned* cell still
  share that one allocation.
- **Adjudication**: **accepted**, same rationale as P2-001. The semantic
  half survives as `terminal_cell_from_alacritty_preserves_zerowidth`.
  Accompanying note: the `terminal.rs` domain test
  `terminal_cell_clone_shares_extra_storage` reaches into the cell's private
  representation (`cell.cell.extra`); its field path was mechanically
  re-pointed to the owned field (`cell.extra`). Its expectation —
  `Arc::ptr_eq` across `Cell::clone` — is unchanged and still passes; no
  behavioral delta.
