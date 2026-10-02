# Hotkeys & text insertion (Windows) — `vt-platform`

Getting keystrokes in and text out of *any* Windows app is where dictation tools feel either
magical or broken. Everything here lives in `crates/platform/src/windows/` behind the traits in
`ARCHITECTURE.md` §5.

## 1. Global hotkeys

### 1.1 Mechanism

- `RegisterHotKey` can't do hold-to-talk because it only reports presses. We use
  **`SetWindowsHookExW(WH_KEYBOARD_LL)`** on a **dedicated thread** that runs its own
  `GetMessageW` loop.
- **The callback must return fast.** If it exceeds `LowLevelHooksTimeout` (~1 s; Windows 10+
  removes the hook *silently* after repeated timeouts), the hook is gone. The callback only:
  1. updates a small modifier bitmask,
  2. runs the chord matcher (pure, allocation-free),
  3. `try_send`s a `HotkeyEvent` on a bounded crossbeam channel, and
  4. returns `CallNextHookEx` or `1` (swallow).
- **Ignore our own injected input.** Every `SendInput` we issue sets `dwExtraInfo` to a magic
  tag (`0x4D55524D`, "MURM"). The hook ignores events carrying it. Events with `LLKHF_INJECTED`
  from other tools (AutoHotkey, remote desktop) are still processed.
- **Watchdog:** the pipeline pings the hook thread every 30 s with a posted message. If the hook
  thread stops responding, or `GetLastError` shows the hook was removed, it re-installs the hook
  and logs a warning.
- **Pause hotkeys** (tray menu) unhooks entirely, for games and remote sessions.

### 1.2 Chord semantics (defaults; all rebindable)

| Chord | Behaviour |
|---|---|
| Hold `Ctrl+Win` | `HotkeyDown(Hold)` as soon as both are down → record; release either → `HotkeyUp` |
| `Space` pressed while `Ctrl+Win` held | **Upgrade** the current recording to hands-free toggle; `Space` swallowed. The next `Ctrl+Win+Space` stops |
| `Alt` pressed while `Ctrl+Win` held | **Upgrade** to command mode (`Ctrl+Win+Alt`) |
| `Esc` while recording | Cancel (swallowed) |
| Any other key while chord held | **Abort**: discard the recording silently and let the key through, so Windows shortcuts like `Ctrl+Win+←/→` (virtual desktops), `Ctrl+Win+D`, `Ctrl+Win+O` keep working |
| `Alt+Shift+Z` | Paste last transcript |

Recording starts **immediately** on the chord (no debounce), so the first word isn't clipped.
The abort rule makes that safe. Short accidental presses are discarded by the < 250 ms rule.

Known chords to stay clear of: `Ctrl+Win+Enter` (Narrator), `Win+Space` (input-language
switch), `Ctrl+Win+Shift+B` (graphics driver reset).

### 1.3 Stopping Win/Alt side effects

- Releasing **Win** alone opens the Start menu; releasing **Alt** alone focuses the app's menu
  bar. We let modifier *downs* through untouched, so other shortcuts work. If our chord
  consumed the modifier, then **just before its key-up passes through** we inject a masking
  keystroke: an unassigned virtual key (`0xE8`) down+up, tagged as ours. Windows then sees
  "Win + something" and doesn't open Start. This is the same technique AutoHotkey uses
  (`#MenuMaskKey`).

## 2. Inserting text

### 2.1 Pre-flight (every insertion)

1. **Wait for physical modifier release.** Poll `GetAsyncKeyState` for Ctrl/Win/Alt/Shift every
   10 ms, up to 1 s. Pasting while Win is still down turns `Ctrl+V` into `Ctrl+Win+V`. If the
   modifiers are still held after 1 s, skip injection: put the text on the clipboard and show
   "Copied — press Ctrl+V".
2. **Identify the target** (`ForegroundApp`):
   - `GetForegroundWindow` → `GetWindowThreadProcessId` →
     `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` → `QueryFullProcessImageNameW` gives the
     exe name, used as the per-app key.
   - **Elevation check:** `OpenProcessToken` + `TokenElevation`. Access-denied counts as
     elevated.
3. **Elevated target and we're not elevated:** UIPI drops our input silently. Use the clipboard
   fallback ("Copied — press Ctrl+V; can't type into admin windows"). We never ask to run
   elevated.
4. **Look up the per-app override** (method, paste chord, restore delay, newline style).

### 2.2 Method A: clipboard paste (default)

```
snapshot = ClipboardGuard::snapshot()         // see 2.4
set clipboard:
    CF_UNICODETEXT = text
    "ExcludeClipboardContentFromMonitorProcessing" = (any)   // clipboard managers skip it
    "CanIncludeInClipboardHistory"  = DWORD 0                 // Win+V history skips it
    "CanUploadToCloudClipboard"     = DWORD 0                 // never syncs to cloud
seq = GetClipboardSequenceNumber()
SendInput: Ctrl↓ V↓ V↑ Ctrl↑        (tagged; paste chord overridable, e.g. Shift+Insert)
sleep(restore_delay)                 // default 150 ms; Electron apps may need 300 ms
if GetClipboardSequenceNumber() == seq:  restore(snapshot)
else: don't restore — something else (user/app) put new content there; never clobber it
```

- `OpenClipboard` can fail while another app holds it. Retry 10 × 10 ms, then fall back to
  method B.
- The guard is RAII. Restore runs on every exit path, including errors and panics
  (`catch_unwind` boundary in the worker).

### 2.3 Method B: SendInput Unicode typing (fallback / per-app)

- Each UTF-16 code unit is sent as `KEYEVENTF_UNICODE` down+up (surrogate pairs as two units).
- `\n` is sent as a real `VK_RETURN`, or **`Shift+Enter` for chat apps**, where plain Enter
  sends the message: Slack, Teams, Discord, WhatsApp (per-app `newline = "shift-enter"`; shipped
  defaults for these exes).
- Sent in batches of ~64 units with 1 ms gaps. Some apps drop input when it is flooded.
- It never touches the clipboard, but it is slow for long text (~1–2 ms/char) and can trigger
  autocomplete or auto-indent in editors.
- Automatic switch: text longer than 2,000 chars always uses paste unless the app is pinned to
  B.

### 2.4 Clipboard snapshot details

- Enumerate formats with `EnumClipboardFormats`. Copy `HGLOBAL`-backed formats byte for byte.
  - GDI-handle formats (`CF_BITMAP`, `CF_ENHMETAFILE`, `CF_PALETTE`) are skipped; Windows
    re-synthesises bitmaps from `CF_DIB`/`CF_DIBV5`.
  - Delayed-rendered formats may be slow (e.g. Excel ranges). Time-box the snapshot to 100 ms.
- **Size cap 32 MB.** If the clipboard holds more (a big image), or the snapshot times out,
  **use method B for this insertion** rather than risk losing the user's clipboard.

### 2.5 Per-app overrides (`settings.toml`)

```toml
[apps."slack.exe"]
method = "paste"          # paste | type
restore_delay_ms = 250
newline = "shift-enter"   # used by method B only

[apps."WindowsTerminal.exe"]
paste_chord = "ctrl+v"    # or "ctrl+shift+v" / "shift+insert"
```

Shipped defaults cover the matrix below. Users edit them in Settings → Apps.

## 3. Reading the selection (command mode)

`SelectionReader::read_selection()` is called **after the command chord is released**: it waits
for modifier release (§2.1 step 1) first, because a `Ctrl+C` sent while `Ctrl+Win+Alt` is still
held becomes a different shortcut. The spoken instruction is transcribed in parallel.
1. Snapshot the clipboard.
2. Record the sequence number.
3. Send tagged `Ctrl+C`.
4. Wait for the sequence number to change (poll 10 ms, up to 300 ms).
5. Read `CF_UNICODETEXT`.
6. Restore the snapshot.

If there was no change, there is no selection; return `None` and show "Select text first".

The replacement is inserted with method A. The selection is still active, so pasting replaces
it. Reading via UI Automation (`TextPattern.GetSelection`) is cleaner but inconsistent across
apps, so it's in the Later backlog.

## 4. App compatibility matrix (manual test before each release)

| App | Method | Notes / expected |
|---|---|---|
| Notepad (Win 11) | paste | baseline |
| Word / Outlook (desktop) | paste | keeps the destination formatting (plain text only) |
| Chrome & Edge: `<textarea>`, Gmail compose, Google Docs | paste | Docs works via paste only |
| ChatGPT / Claude web input | paste | contenteditable |
| Slack desktop | paste | restore_delay 250; newline = shift-enter if typed |
| Microsoft Teams (new) | paste | same as Slack |
| Discord, WhatsApp desktop | paste | newline = shift-enter if typed |
| VS Code editor | paste | typing would trigger autocomplete |
| VS Code integrated terminal | paste | multi-line paste warning is VS Code's; acceptable |
| Windows Terminal (PowerShell) | paste | |
| Classic conhost `cmd.exe` | paste | Ctrl+V needs Win10+ console; fallback `shift+insert` |
| Excel cell (editing) | paste | when not in edit mode the paste goes to the cell, which is fine |
| Explorer rename box, Start search box | paste | |
| Elevated app (Notepad as admin) | — | **expected fallback**: clipboard + notice |
| Password field | paste | works; history records it, so the "don't save history for password fields" detection is a Later item |
| Remote Desktop window | paste | goes through the RDP client; clipboard redirection must be on |

Results are recorded per release in `docs/progress.md`.

## 5. Things we deliberately don't do

- No DLL injection or hooking other processes, and no UI Automation writes. It is fragile, and
  antivirus flags it.
- No running elevated, and no service.
- No `keybd_event` (deprecated). `SendInput` only.
