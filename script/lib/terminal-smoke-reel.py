#!/usr/bin/env python3
"""Driver for script/terminal-smoke-reel: XTEST-drives the dev Zed on the
nested Xwayland display and captures one screenshot per reel step.

The reel follows verification-strategy.md §7: colored output, vim edit
(alt-screen enter/exit), tmux split, resize, title change (OSC 0/2), and
scrollback + clear. A human reviews the captured frames.
"""

import os
import subprocess
import time

from Xlib import X, display
from Xlib.ext import xtest

OUT_DIR = os.environ["OUT_DIR"]
DISPLAY = os.environ.get("DISPLAY_NUM", ":99")

x_connection = display.Display(DISPLAY)
root = x_connection.screen().root


def find_zed_window():
    """The largest viewable window whose WM_CLASS mentions zed (an early
    unmapped 10x10 helper also carries the class)."""
    best = None
    best_area = 0
    stack = [root]
    while stack:
        window = stack.pop()
        try:
            for child in window.query_tree().children:
                stack.append(child)
            wm_class = window.get_wm_class()
            attrs = window.get_attributes()
            if (
                wm_class
                and any("zed" in part.lower() for part in wm_class)
                and attrs.map_state == X.IsViewable
            ):
                geom = window.get_geometry()
                area = geom.width * geom.height
                if area > best_area:
                    best, best_area = window, area
        except Exception:
            # Windows can be destroyed between query_tree and the property
            # reads; a vanished window is expected churn, not a failure.
            continue
    return best


def press(keysym_name, modifiers=()):
    keysym = x_connection.keysym_to_keycodes(_keysym(keysym_name))
    keycodes = list(keysym)
    if not keycodes:
        raise RuntimeError(f"no keycode for {keysym_name}")
    keycode = keycodes[0][0]
    modifier_codes = [list(x_connection.keysym_to_keycodes(_keysym(m)))[0][0] for m in modifiers]
    for code in modifier_codes:
        xtest.fake_input(x_connection, X.KeyPress, code)
    xtest.fake_input(x_connection, X.KeyPress, keycode)
    xtest.fake_input(x_connection, X.KeyRelease, keycode)
    for code in reversed(modifier_codes):
        xtest.fake_input(x_connection, X.KeyRelease, code)
    x_connection.sync()
    time.sleep(0.08)


_NAMED = {
    "enter": 0xFF0D,
    "escape": 0xFF1B,
    "space": 0x0020,
    "ctrl": 0xFFE3,
    "shift": 0xFFE1,
    "alt": 0xFFE9,
    "grave": 0x0060,
    "percent": 0x0025,
    "quotedbl": 0x0022,
    "minus": 0x002D,
    "underscore": 0x005F,
    "semicolon": 0x003B,
    "colon": 0x003A,
    "slash": 0x002F,
    "period": 0x002E,
    "comma": 0x002C,
    "apostrophe": 0x0027,
    "exclam": 0x0021,
    "dollar": 0x0024,
    "parenleft": 0x0028,
    "parenright": 0x0029,
    "bar": 0x007C,
    "greater": 0x003E,
    "less": 0x003C,
    "equal": 0x003D,
    "plus": 0x002B,
    "asterisk": 0x002A,
    "numbersign": 0x0023,
    "at": 0x0040,
    "ampersand": 0x0026,
    "braceleft": 0x007B,
    "braceright": 0x007D,
    "bracketleft": 0x005B,
    "bracketright": 0x005D,
    "backslash": 0x005C,
    "asciitilde": 0x007E,
    "asciicircum": 0x005E,
    "question": 0x003F,
    "pageup": 0xFF55,
    "pagedown": 0xFF56,
}

_SHIFTED = {
    '"': "quotedbl",
    "%": "percent",
    "_": "underscore",
    ":": "colon",
    "!": "exclam",
    "$": "dollar",
    "(": "parenleft",
    ")": "parenright",
    "|": "bar",
    ">": "greater",
    "<": "less",
    "+": "plus",
    "*": "asterisk",
    "#": "numbersign",
    "@": "at",
    "&": "ampersand",
    "{": "braceleft",
    "}": "braceright",
    "~": "asciitilde",
    "^": "asciicircum",
    "?": "question",
}

_PLAIN = {
    " ": "space",
    "-": "minus",
    ";": "semicolon",
    "/": "slash",
    ".": "period",
    ",": "comma",
    "'": "apostrophe",
    "=": "equal",
    "[": "bracketleft",
    "]": "bracketright",
    "\\": "backslash",
    "`": "grave",
}


def _keysym(name):
    if name in _NAMED:
        return _NAMED[name]
    if len(name) == 1:
        return ord(name)
    raise RuntimeError(f"unknown key name {name}")


def type_text(text):
    for char in text:
        if char.isupper():
            press(char.lower(), modifiers=("shift",))
        elif char in _SHIFTED:
            press(_SHIFTED[char], modifiers=("shift",))
        elif char in _PLAIN:
            press(_PLAIN[char])
        else:
            press(char)


def run_line(text, settle=1.0):
    type_text(text)
    press("enter")
    time.sleep(settle)


def shot(name):
    # A palette open/close forces a redraw first: frames right after input
    # can otherwise capture stale or black content.
    time.sleep(0.4)
    path = os.path.join(OUT_DIR, f"{name}.png")
    subprocess.run(
        ["import", "-display", DISPLAY, "-window", str(zed.id), path],
        check=True,
    )
    print(f"  captured {name}.png")


zed = find_zed_window()
if zed is None:
    raise SystemExit("no viewable zed window on the nested display")
x_connection.set_input_focus(zed, X.RevertToParent, X.CurrentTime)
x_connection.sync()

# A fresh --user-data-dir starts on onboarding; ctrl-enter completes it.
press("enter", modifiers=("ctrl",))
time.sleep(2)
x_connection.set_input_focus(zed, X.RevertToParent, X.CurrentTime)
shot("00-launch")

# Open the terminal panel.
press("grave", modifiers=("ctrl",))
time.sleep(2)
shot("01-terminal-open")

# Colored output.
run_line(
    "for i in $(seq 0 15); do tput setab $i; printf '  '; tput sgr0; done; echo; "
    "printf '\\033[38;5;196mred\\033[0m \\033[38;5;46mgreen\\033[0m "
    "\\033[38;5;21mblue\\033[0m \\033[1mbold\\033[0m \\033[4munderline\\033[0m\\n'",
    settle=1.5,
)
shot("02-colored-output")

# Title change (OSC 2).
run_line("printf '\\033]2;smoke-reel-title\\007'", settle=1.5)
shot("03-title-change")

# vim edit: alt-screen enter, typing, then exit restores scrollback.
run_line("vi /tmp/smoke-reel.txt", settle=2.5)
type_text("i")
type_text("ghostty smoke reel line one")
press("enter")
type_text("line two with wide chars: whatever")
press("escape")
shot("04-vim-editing")
type_text(":wq")
press("enter")
time.sleep(1.5)
shot("05-vim-exited-scrollback-restored")

# tmux split: both panes render, output stays in one pane.
run_line("tmux new-session -s smoke", settle=2.5)
shot("06-tmux-started")
press("b", modifiers=("ctrl",))
press("percent", modifiers=("shift",))
time.sleep(1.5)
run_line("seq 1 200 | tail -5", settle=1.5)
shot("07-tmux-split-scroll")
run_line("exit", settle=1.0)
run_line("exit", settle=1.5)

# Scrollback: long output, page up into history, then clear.
run_line("seq 1 5000", settle=4.0)
shot("08-scrollback-tail")
press("pageup", modifiers=("shift",))
press("pageup", modifiers=("shift",))
time.sleep(0.6)
shot("09-scrollback-paged-up")
press("escape")
run_line("clear", settle=1.0)
shot("10-cleared")

# Resize/reflow: shrink and regrow the window.
geometry = zed.get_geometry()
zed.configure(width=900, height=600)
x_connection.sync()
time.sleep(2)
shot("11-resized-narrow")
zed.configure(width=geometry.width, height=geometry.height)
x_connection.sync()
time.sleep(2)
shot("12-resized-back")

# Quit cleanly (flushes state synchronously).
press("q", modifiers=("ctrl",))
time.sleep(2)
print("reel complete")
