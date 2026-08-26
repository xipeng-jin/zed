# Input encoding: keys, mouse, paste, focus (v2)

**Status: resolved for v2** — wayfinder ticket
[#32](https://github.com/xipeng-jin/zed/issues/32), map
[#27](https://github.com/xipeng-jin/zed/issues/27). Re-validated 2026-08-26
against `main` @ `38c5dd7c98`, ghostty `8867c37c5`, libghostty-rs `de9fd9b`.
v1 resolution (grilling 2026-07-15, landed as P3 `6dbce83c20` and finished at
P8 `f6e5104b9f` on `migration/libghostty`) is the starting text; every section
states whether it is re-confirmed or amended. v1 had no record doc for this
ticket (SPEC v1 §4.3 cites the resolution comment); this file is the v2 record.

## 1. Full adoption and the policy/encoding split — re-confirmed, exception restated

Ghostty owns input **encoding**; Zed owns input **policy**. The load-bearing
finding stands: ghostty answers the kitty-keyboard query unconditionally
(`ghostty:src/terminal/stream_terminal.zig`, re-confirmed by the parity matrix
§3.5), so keeping `to_esc_str` would advertise a capability the encoder cannot
deliver. Vi-mode interception stays upstream of encoding.

Seam rule: **Zed never hand-writes escape sequences to the PTY.** The one
bounded exception, restated to cover the v2 rows: `legacy_fill` may emit C0
bytes and the alt ESC prefix for keys ghostty's legacy table deliberately
reserves (`ctrl-[`/`_`/`?`, `ctrl-i`/`ctrl-m`, now also `ctrl-alt-i`/`ctrl-alt-m`)
and the F13–F20 xterm codes, **while kitty flags are inactive** (ledger P3-001,
P3-002, P8-001). Nothing wider.

## 2. The `keys.rs` contract suite as oracle — re-confirmed, new rows pre-adjudicated

The 7-test suite on `main` (`test_plain_inputs`, `test_application_mode`,
`test_ctrl_codes`, `alt_is_meta`, `test_ctrl_alt_codes`,
`test_shift_enter_newline`, `test_modifier_code_calc`) is ported as the
permanent contract module, byte-for-byte, as in v1. #62891 (`2bf9e26473`)
added the rows below; traced against `ghostty:src/input/key_encode.zig`:

| Row (Zed `keys.rs:185,213-230`) | Zed bytes | Ghostty legacy encoder | Adjudication |
|---|---|---|---|
| `ctrl-alt-{a..z}` except `i`, `m` | `\x1b` + C0 | `ctrlSeq` drops alt before the ctrl-only check and the caller writes `0x1B` + byte when alt is held (not gated on DEC 1036) → identical | **Native.** Requires a v2 *mapping-layer* change: v1's `ghostty_encode_keystroke` returned `None` for ctrl+alt (`!modifiers.alt` guard); v2 passes `CTRL\|ALT`. No ledger entry — contract parity. |
| `ctrl-alt-i`, `ctrl-alt-m` | `\x1b\x09`, `\x1b\x0d` | `ctrlSeq` reserves `i`/`m` (commented out); falls to `legacyAltPrefix` → `\x1bi`/`\x1bm` | **Seam fill — ledger P3-002 extended** (same root cause). `legacy_fill`'s `ctrl-i`/`ctrl-m` rows accept alt and prefix `\x1b`. |
| `alt-shift-{letter}` → `\x1b` + UPPER | `\x1bA` | `legacyAltPrefix` with `utf8 = "A"` → `\x1bA` | **Native, already pinned** by v1's contract `alt_is_meta`. #62891 restructured the branch; bytes unchanged. Its ordering (`shift` beats `ctrl` for `ctrl-alt-shift-letter` → `\x1bUPPER`) is preserved by v1's alt+shift path, which drops ctrl. |
| modified `f5` (`"F5"`→`"f5"`) | `\x1b[15;N~` | `\x1b[15;N~` | **Native — ledger P3-004 amended** to modified-`delete` only (still absent from the table). |

All other P3-001…P3-010 adjudications carry forward `unverified` (salvage
policy) until the v2 harness re-fires them; the implementing phase confirms
this table against the real encoder rather than re-triaging.

## 3. Paste — v1 re-confirmed: `paste::encode`, not `ghostty_terminal_paste`

Side by side (`vt/paste.h`, `ghostty:src/terminal/paste.zig`,
`ghostty:src/input/paste.zig`):

| | v1 `mappings/paste.rs` on `paste::encode` | `ghostty_terminal_paste` |
|---|---|---|
| Mode gate | Zed policy (`BRACKETED_PASTE`) | live terminal |
| Sanitization | 16-byte xterm strip table → spaces, both modes (P3-007) | **identical** (`encodeWriter`) |
| `\r\n` unbracketed | Zed pre-normalizes `\r\n`→`\n` → `\r` | `\r\r`; Zed would pre-normalize before the reader — same fix |
| Unsafe rule | none | `isSafeWith`: bracketed → reject on `\x1b[201~`; unbracketed → reject on **any `\n`** → `GHOSTTY_REJECTED` unless `allow_unsafe` |
| Output | `Vec<u8>` → `Terminal::input()` (`keyboard_input_sent`, init-command handshake, `write_input`) | chunks through `WRITE_PTY`, bypassing `input()` |
| Mode 5522 events | n/a | only with `CLIPBOARD_READ` installed — NULL by #31, never fires |
| Binding | in libghostty-rs `de9fd9b` | **absent** (no `GhosttyPaste`/`ghostty_terminal_paste`, not even in `bindings.rs` — the #33 lag) |

**Decision:** keep v1's design unchanged — `paste::encode` behind Zed's gate
with the `\r\n`→`\n` pre-normalization and the `\r\n`→`\r` collapse contract
test. The two encode the same bytes; `terminal_paste` would cost a hand-written
binding, a write-path detour around `input()`, and (without `allow_unsafe`
always on) silently rejected multi-line unbracketed pastes. Parity matrix row
P5 resolves to "Zed-side kept"; ledger P3-007 stands.

**Unsafe-paste confirmation UX stays out of scope** for this map (map
Out-of-scope section): a product decision with a dialog, beyond parity. Hooks
if it ever comes in: `paste::is_safe` (conservative) or `isSafeWith`'s
state-aware rule, with either paste API.

## 4. Mouse and focus — re-confirmed verbatim

Mouse: ghostty `mouse::Encoder` owns wire formats (X10/UTF-8/SGR/SGR-Pixels);
Zed keeps `mappings/mouse.rs` policy (mode gating on the snapshot `Modes`
per P8-002, grid math, scroll-report repeats, `alt_scroll` synthesis, #60880
shift+drag). P3-009 and P8-002 stand. Focus: ghostty focus helper replaces the
`\x1b[I`/`\x1b[O` literals (`terminal.rs:2373,2379`).

## 5. Mode sync — encode-time from the live terminal, re-confirmed; **shim phase dropped**

v1 landed the encoders pre-swap behind a temporary `Modes`→options shim (P3)
and deleted it at the swap (P8), where P8-001 proved live
`set_options_from_terminal` and the contract harness was re-plumbed to drive a
live terminal (`mappings::test_support::backend_with_modes`).

**Amendment:** v2 has no shim phase. The encoders land **with the core swap**,
calling `set_options_from_terminal` from day one; the salvage takes P8's
final `keys.rs`/`mouse.rs`/`paste.rs`/`focus.rs`/`test_support` by file, not
P3's shimmed versions. The shim's reasons — unproven live sync, isolating
keystroke regressions — are settled by P8-001 and by a contract suite that is
byte-identical on either path and still bisects regressions. Consequence for
the phase plan: the encoder work is no longer an independently landable
pre-swap phase; it is a sub-step of the swap phase, and the vendored binding
must build before it (already implied by vendoring).

**Policy gates read live too:** the paste and focus gates read the live
terminal at input time (a stale bracketed-paste bit is exactly the case that
injects a multi-line command), replacing `last_content.mode` there. Mouse
*gating* stays on the snapshot per P8-002.

## Consequences for other tickets

- **Seam/phase plan (#36):** the "vendor → encoder phase (shim) → swap (shim
  deleted)" edge collapses to "vendor → swap (encoders + live sync)"; the
  paste/focus gates move to live reads at the swap.
- **Verification (#37):** the contract module gains the §2 rows; P3-002 and
  P3-004 ledger text amended as above; P3-007 stands.
- **Vendoring (#33):** no `terminal_paste` binding needed.
- **Parity matrix (#29):** P5 → "Zed-side kept".
