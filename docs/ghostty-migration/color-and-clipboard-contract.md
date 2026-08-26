# Color model, ANSI parser, and clipboard contract (v2)

**Status: resolved for v2** — wayfinder ticket
[#31](https://github.com/xipeng-jin/zed/issues/31), map
[#27](https://github.com/xipeng-jin/zed/issues/27). Re-validated 2026-08-26
against `main` @ `38c5dd7c98`, ghostty `8867c37c5`, libghostty-rs `de9fd9b`.
v1 resolution (2026-07-15, landed as P2 `e4c22055ae` on `migration/libghostty`)
is the starting text; every section states whether it is re-confirmed or amended.

## 1. vte survives as a utility dependency — re-confirmed

`vte 0.15` (`ansi` feature) stays in the workspace, used only by the `terminal`
crate, only for `strip_ansi_text` (consumers: `git_ui`, `git_ui_core`) and
`parse_ansi_text` (debugger console). The emulator-driving
`Processor<StdSyncHandler>` (`terminal.rs:53`, `:982`, `:1259`, `:1452`) dies
with `alacritty_terminal`; ghostty parses internally via `vt_write`.

Rejected alternative: drive a display-only ghostty `Terminal` and read rows back.
It turns two pure functions into FFI objects with allocator/lifetime baggage for
`git_ui` notifications.

After the swap, `vte` appears in exactly one file's imports (`terminal.rs`),
where a private `From<vte::ansi::Color> for Color` is the only vte → contract
bridge.

## 2. Zed-owned color contract — re-confirmed verbatim

`pub use vte::ansi::{Color, NamedColor, Rgb}` (`terminal.rs:54`) is replaced by
Zed-owned types of identical shape on the same paths:
`Color::{Named(NamedColor), Spec(Rgb), Indexed(u8)}`, the same 30 `NamedColor`
variants (including the `Foreground = 256` discriminant split), `Rgb{r,g,b}`.
`terminal_view::convert_color` and `debugger_ui`'s console compile unmodified.

The ten `Dim*` variants stay although the ghostty seam never produces them
(dim/bold/inverse are cell flags): removing them would break the
"downstream compiles unmodified" property for no runtime gain.

## 3. Seam mapping (ghostty → contract) — re-confirmed

Read the semantic color via `cell.style()` on the render iterator; the
flattened per-cell `fg_color`/`bg_color` resolves go unused. The sized
`RENDER_STATE_DATA_COLORS` struct (replacing `colors_get`) changes how a
*palette* is fetched, not what a *cell* says, so the mapping is untouched:

| ghostty `StyleColor` | Zed `terminal::Color` |
|---|---|
| `None` (fg position) | `Named(Foreground)` |
| `None` (bg position) | `Named(Background)` |
| `Palette(0–15)` | `Named(Black … BrightWhite)` |
| `Palette(16–255)` | `Indexed(i)` |
| `Rgb(rgb)` | `Spec(Rgb)` |

Accepted loss: SGR 31 and SGR 38;5;1 both become `Named(Red)` — unobservable in
Zed (`convert_color`, `get_color_at_index`, and `is_app_chosen_exact_color`
treat `Indexed(0–15)` and `Named` identically).

## 4. Theme → ghostty push; `ColorRequest` deleted — re-confirmed, one addition

ghostty answers OSC 4/10/11/12 internally from stored defaults, so Zed pushes
`OPT_COLOR_FOREGROUND` / `_BACKGROUND` / `_CURSOR` and the full 256-entry
`OPT_COLOR_PALETTE` on creation and again on every theme change (setters
preserve per-index OSC overrides). The `ColorRequest` event arm
(`terminal.rs:1622-1636`) and its lock-and-comment ordering dance are deleted;
query responses flow through `WRITE_PTY` inside `vt_write`, in order.
`mappings/colors.rs::to_vte_rgb` becomes a theme → `RgbColor` helper.

**Addition (v2):** palette entries 16–255 are generated in Zed exactly as
`get_color_at_index` computes them today, *not* via
`ghostty_color_palette_generate`, so the pushed palette and `convert_color`'s
render-time resolution can never disagree.

## 5. OSC 52 — premise corrected, read stays unwired, `ClipboardLoad` deleted

**Corrected premise.** Zed has no OSC 52 setting. PTY terminals run alacritty's
implicit `OnlyCopy` (write on, `?` reads silently ignored); display-only
terminals get `Osc52::Disabled` (`alacritty.rs:128`, pinned by
`test_display_only_write_output_ignores_osc52`). Write is on; read is impossible.

**Read.** `OPT_CLIPBOARD_READ` (opt 38) now exists, but is left NULL, which is
byte-for-byte the `OnlyCopy` behaviour. A user setting that enables reads is
**out of scope** for this map: letting programs read the clipboard is a new
capability with a consent question attached, and the callback contract is
synchronous (the VT stream waits inside `vt_write`), so any prompt-based UX
would freeze the terminal. Product ticket post-migration.

**`ClipboardLoad` variant: deleted (v1 re-confirmed; parity-matrix #29 "keep
dead in enum" superseded).** Under ghostty a read is a synchronous callback that
must reply before returning; it cannot be a `TerminalBackendEvent` handled later
on the foreground like today's arm (`terminal.rs:1588-1597`). The future hook is
a seam callback, not an event, so the variant's shape is wrong regardless.

## 6. Clipboard write reply ↔ Zed's event model — no change to the event model

`GhosttyClipboardWrite` must be answered via `write->reply` inside the callback
(returning without replying denies; OSC 52 discards the result, only Kitty
OSC 5522 observes it). The callback fires inside `vt_write` on the GPUI
foreground (#30: `!Send` core).

Seam behaviour:

- reply `SUCCESS` immediately, then emit `TerminalBackendEvent::ClipboardStore(String)`
  exactly as today; the existing arm performs `cx.write_to_clipboard` moments
  later. Reply semantics are "accepted", which is all OSC 52 can observe;
- content = the first `text/*` representation; `contents_len == 0` is ignored
  (Zed never clears the clipboard on a program's behalf — matches today);
- **all** locations (Standard / Selection / Primary) map to the system
  clipboard, matching today's `ClipboardStore(_, data)` which ignores
  alacritty's `ClipboardType`;
- `name` / `granted` / `can_remember` are ignored (Kitty-protocol only);
- the display-only profile registers no write callback (= `Osc52::Disabled`).

libghostty-rs `de9fd9b` binds the pre-reply ABI; the binding update is #33's.

## 7. `UNKNOWN_SEQUENCE` — not in the product seam

APC-only today, synchronous per sequence, with capture cost when
`UNKNOWN_MAX_BYTES` is nonzero. Not installed by Zed. The verification ticket
(#38) may install it in the differential harness as a triage aid; that is its
call, not part of the spec.

## Feeds into

- **Seam design (#36)**: home of the conversion functions, the theme re-push
  obligation, and the clipboard-write callback shape in §6.
- **libghostty-rs re-vendoring (#33)**: request/reply clipboard ABI.
- **Verification (#38)**: optional `UNKNOWN_SEQUENCE` in the harness.
- **SPEC v2 §4.2 / §6 P8**: handler arms deleted = `ColorRequest`,
  `ClipboardLoad`, `TextAreaSizeRequest` (unchanged list).
