# apt-kwin-overlay: remaining gaps vs. real Awakened Poe Trade

Audit of `main/src/*` in the vendored, unmodified `awakened-poe-trade` submodule
against what `apt-kwin-overlay`'s Rust backend actually implements. The renderer
(Vue UI) is 100% the real, unmodified app — everything below is about the
`main/` process we replaced.

Ordered roughly by how much it'd actually change day-to-day use, not by file.

## 1. Hotkeys are hardcoded, not config-driven — DONE, but landed differently than planned

Implemented: parses `CLIENT->MAIN::update-host-config`'s `cfg.shortcuts:
ShortcutAction[]` and dynamically binds via `ashpd`'s `GlobalShortcuts`
portal (`host_config.rs`, `shortcuts.rs`), scoped to the `copy-item` and
`toggle-overlay` action types — the ones our `price_check()`/
`toggle_click_through()` already implemented; the actual gap was that the
*set of bound shortcuts* was fixed at 3.

Key design departure from a straightforward port of `Shortcuts.ts`:
confirmed empirically (checking `~/.config/kglobalshortcutsrc` live) that
the portal has no way to silently rebind an already-granted id's trigger —
`bind_shortcuts` only lets a brand-new id claim a trigger; calling it again
for a known id with a different `preferred_trigger` does nothing, and
KDE's kglobalaccel also replaces an app's *entire* grant set with whatever
the latest call supplies rather than adding to it (the full accumulated
list has to be resupplied every call, not just the delta). So shortcut ids
are content-stable (action type + target + `focus_overlay`, never the
trigger key itself) — which means the renderer's own hotkey editing no
longer has any effect once an id is first granted. **KDE's own Global
Shortcuts (System Settings, or `ConfigureShortcuts`) is now the one place a
user actually assigns keys**, matching how the portal model expects
shortcut management to work in general, not just for us. The renderer's
Hotkeys and Item Info tabs (real, unmodified upstream Vue UI) stay
reachable, but their content is replaced with an explanatory banner via
injected JS (`main.rs`'s `HOTKEY_UI_INFO_JS`, `WebKitUserContentManager`)
rather than forking the submodule.

All `copy-item` targets the original UI could ever configure (price-check
unlocked/locked, item-check, wiki/PoEDB/Craft of Exile/find-in-stash) are
pre-registered at startup (`host_config::extra_registerable_actions`) with
no default trigger, purely so they show up as nameable, bindable entries in
KDE's Shortcuts KCM — otherwise there'd be no way to ever discover or
enable them, since the renderer UI that used to expose them no longer does.

Still not implemented (parsed as `ActionKind::Unsupported`, inert):
- `paste-in-chat`, `trigger-event`, `stash-search` — need EIS text-injection
  + clipboard-write primitives that don't exist in `remote_input.rs` yet.
- `ocr-text` (heist gems) — Windows-only upstream, correctly out of scope
  regardless of platform (see §12).

## 2. Stash scroll-wheel navigation

Real: Ctrl+scroll while hovering the stash tab bar (not over the sidebar) taps
Left/Right arrow to change tabs (`Shortcuts.ts`'s `uIOhook.on('wheel', ...)` +
`isStashArea`). Needs real PoE window bounds (`GameWindow.bounds`,
`uiSidebarWidth`) to know where "the stash tab bar" actually is on screen.

We have no scroll-event capture at all (libei/EIS only ever *emits* input for
us, doesn't capture the user's real mouse wheel), and no window-bounds-based
region logic. Not attempted.

## 3. Clipboard read is fragile compared to `HostClipboard.ts`

Real `readItemText()`:
- Polls every 48ms up to 500ms (we: fixed single 150ms wait, one read, no
  retry — if PoE is slow to update the clipboard, we return empty and give up).
- Validates the result actually looks like an item via a first-line check
  against 10 language signatures before accepting it (we: accept whatever's
  on the clipboard, no validation — a stale clipboard value would silently
  "work").
- Optional clipboard restore-after-copy (`restoreClipboard` setting) so
  copying an item doesn't clobber whatever the user had copied before. We
  don't implement the setting at all (always leaves the item text sitting in
  the clipboard).
- KDE-specific "prevent empty clipboard" and Proton 10+ workarounds (writes a
  throwaway marker string before triggering the copy) — noted as *not*
  directly applicable to us in the original phase plan since our capture path
  is portal-based, not Electron clipboard; worth re-checking if stale reads
  ever show up in testing.

Real fix shape: port the poll-with-timeout + language-signature-validation
loop into `remote_input.rs::read_clipboard_text`, add a `restoreClipboard`
config flag — §1's config plumbing (`host_config.rs`) exists now, so this
is unblocked; just needs its own pass.

## 4. No hover-to-interact / hold-modifier-to-pin for the price-check popup

Real `WidgetAreaTracker.ts` continuously tracks the mouse (`uiohook`
`mousemove`/`mousedown`) against a screen-space rectangle sent by the
renderer (`OVERLAY->MAIN::track-area`, includes a `holdKey`): moving the
mouse *into* that rectangle auto-activates the overlay for interaction
(`assertOverlayActive`) without needing the locked hotkey at all, and holding
a configured modifier key pins the popup open past the normal distance-close
threshold.

We: only the distance-based auto-close is ported (`spawn_auto_close` in
`shortcuts.rs`); hovering back into the popup doesn't re-activate anything,
and there's no hold-to-pin since we don't have live modifier-key state
outside of what `remote_input.rs`'s EIS events happen to report for our own
injected keys (not the user's real keyboard). This was flagged as an explicit
known simplification back in the original phase plan.

Real fix shape: we already receive `OVERLAY->MAIN::track-area` payloads
nowhere (no listener) — would need a live cursor-position stream (not the
current one-shot KWin query-per-poll) to react promptly, which is a bigger
lift than it sounds given our cursor position only comes from on-demand KWin
scripting round-trips.

## 5. No Alt-hold-to-hide-overlay (`OverlayVisibility.ts`)

Real: holding Alt (no other modifiers) makes the whole overlay invisible
after 85ms (if currently interactable) or 275ms (if not) — lets you glance at
the game without fully closing widgets. Releasing Alt or moving the mouse
without Alt held restores visibility. We don't implement this at all — no
`MAIN->OVERLAY::visibility` event is ever sent from our side.

Needs real-time modifier-key state (same missing piece as §4's hold-to-pin) —
`remote_input.rs` currently only sees modifier events tied to *our own*
injected key presses, not a live feed of the user's actual keyboard, since
EIS is a Sender-only context for us (see `remote_input.rs` doc comments).
Getting a live modifier-key feed would likely need a second EIS/portal
capability we haven't set up (or KWin D-Bus scripting's own key-event hooks,
unverified whether that's exposed).

## 6. Escape/Ctrl+W: real APT does this locally, not as a global shortcut

Worth re-visiting now that keyboard-mode toggling is fixed. Real
`OverlayWindow.ts`'s Escape/Ctrl+W handler is a **local**
`webContents.on('before-input-event')` listener — it only fires when the
overlay window itself has input focus, which Electron's overlay trick allows
even mid-interaction. We dropped Escape entirely (see recent commits) because
our layer-shell surface never held keyboard focus at all, so the only path
was an unconditional global portal grab — which is fundamentally
unforwardable (a synthetic Escape we inject just re-triggers our own grab)
and, when made dynamic per-popup-open, triggered a fresh KDE consent dialog
on every single price-check open (unacceptably disruptive, see commit
`df5aec9`).

**But**: we since fixed `apply_click_through_to_surface` to flip
`KeyboardMode::OnDemand` whenever click-through is off (commit `d749d98`),
specifically so typing in the UI works. That means our surface *can* now
genuinely hold local keyboard focus while a widget is interactive — the same
precondition real APT relies on for its local Escape handler. A GTK
`EventControllerKey` on the window/webview, active only while it holds
focus, could plausibly replicate real APT's exact approach without needing
any global portal grab at all. Not attempted yet; flagging because the
previous blocker (no local focus, ever) no longer applies.

## 7. No game-log-driven features (`GameLogWatcher.ts`)

Whisper/trade/zone-change notifications from tailing `Client.txt`. Explicitly
deferred in the original phase plan as "real feature, not required for hover
item → see price." Genuinely separate, sizeable feature — would need a file
watcher on the log path (`cfg.clientLog` from config) and its own event
stream into `MAIN->CLIENT::game-log`.

## 8. `GameConfig.ts` — actually fine to keep skipping

Reads PoE's `production_Config.ini` to find the player's "advanced item
description" keybind. Deferred originally, and confirmed here: as of PoE
3.29, `Shortcuts.ts` hardcodes `showModsKey = 'Ctrl'` and ignores whatever
`GameConfig` found anyway (`pressKeysToCopyItemText`'s `showModsKey`
parameter is overwritten unconditionally). This file is close to dead code
in the real app too now — not worth porting.

## 9. In-app Logger is underused — Settings → Log tab is mostly empty for us — DONE

Wired `logger.write(...)` alongside the existing `eprintln!`/`println!` at
every genuine-failure site the original audit called out: price-check's
cursor-query/kwin-not-ready/remote-input-not-ready/clipboard-read failures
and the shortcut-reapply failure (`shortcuts.rs`), reserved-hotkey-skip and
bad-payload parsing (`host_config.rs`), config save failure
(`config_store.rs`), proxy/GGG request failure (`proxy.rs`), the
keyboard-device-not-ready warning (`remote_input.rs`), and host-app-
registration/remote-input-setup failures (`main.rs`). Pure debug/lifecycle
noise (connection open/close, raw EIS event dumps, per-frame geometry logs)
was deliberately left as terminal-only — feeding those into the renderer's
Log view would just be spam.

Found `Logger::write` was actually dead code (`#[allow(dead_code)]`, never
called) and, separately, that it only ever fed the history buffer replayed
to a client on connect — nothing broadcast a log entry live to clients
already connected. Fixed both: `Logger` now holds an `Arc<EventBus>` (built
before `Logger` in `server::spawn()` so it can be handed in) and `write()`
broadcasts `MAIN->CLIENT::log-entry` immediately, in addition to appending
to history. `Arc<Logger>` is threaded alongside the existing `Arc<EventBus>`
param through the same call chains §1's shortcut/config plumbing already
established, rather than being stored as a field everywhere (`ConfigStore`,
`proxy::handle` take it as a plain parameter; `RemoteInput` stores it since
`press_keys` has no other way to reach it from `&self`).

## 10. PoE losing OS focus doesn't reset our overlay state

Real `OverlayWindow.ts`'s `handlePoeWindowActiveChange` forces
`isInteractable = false` and re-broadcasts `focus-change` whenever the game
window's own OS-level active/inactive state changes — e.g. Alt-tabbing away
from PoE to a browser forces the overlay back to non-interactive. We track
PoE activation via `kwin_tracker` (for monitor-follow and the price-check
`is_poe` gate) but never react to PoE losing focus by resetting
`click_through`/closing widgets. In practice: if you Alt-Tab away with the
locked popup open, our overlay just stays exactly as it was instead of
snapping back to click-through-on like real APT would.

## 11. No tray icon (`AppTray.ts`)

Real APT: a system tray icon (tooltip shows version) with a right-click menu —
"Settings/League" (message box pointing at the in-game overlay-key hint,
since Settings only exists inside the overlay), "Open in Browser" (opens
`http://localhost:{port}` — our exact equivalent of `--no-overlay` mode),
"Open config folder", "Quit". Also listens for `CLIENT->MAIN::user-action`
with `action: 'quit'` so a quit button inside the renderer's own UI works.
We have none of this — closing the overlay window (there isn't even a
window-close affordance right now) or Ctrl+C in the terminal are the only
ways to exit.

**GTK4-specific wrinkle**: GTK4 removed `GtkStatusIcon` outright — there is
no built-in tray/status-icon widget anymore (GTK3 had one; it's gone). On
Linux the modern mechanism is the freedesktop **StatusNotifierItem** D-Bus
interface (what KDE Plasma's systray actually implements; menus go over the
companion "dbusmenu" protocol), not anything GTK ships directly. Two viable
paths:
- **`ksni`** crate — a Rust StatusNotifierItem implementation (built on top
  of `zbus`, which we already depend on). Least new surface area, handles
  the dbusmenu plumbing for you. Likely the pragmatic choice.
- Hand-roll the SNI + dbusmenu D-Bus interfaces ourselves via our existing
  `zbus` dependency, matching the "everything through zbus" pattern already
  used by `kwin_tracker.rs`. More code, no new dependency, but dbusmenu
  specifically is fiddly to get right (nested menu items, hover/activate
  signals) — `ksni` earns its keep here specifically to avoid that part.

Menu items map straightforwardly to what we already have: "Open in Browser"
→ open `http://127.0.0.1:{port}` (`server::spawn()` already returns the
port), "Open config folder" → open `~/.local/share/apt-kwin-overlay`, "Quit"
→ `app.quit()`/exit the process (also wire the existing
`CLIENT->MAIN::user-action` `quit` case in `server.rs`'s dispatch, which we
don't currently handle either). "Settings/League" doesn't map cleanly since
we don't have real APT's in-overlay Settings-access hint flow — could just
open the browser UI instead, or drop the item.

## 12. Not applicable / correctly out of scope, no action needed

- **`AppUpdater.ts`** (real electron-updater auto-update flow) — we already
  stub this correctly (always `update-not-available`). Self-updating doesn't
  make sense for a locally-built dev binary; leave as-is permanently, not a
  gap to close.
- **`vision/*`** (OCR for heist gems) — Windows-only upstream already (uses a
  Windows-specific screenshot path), correctly out of scope regardless of our
  platform.
- **`HostClipboard.ts`'s KDE/Proton-specific empty-clipboard workarounds** —
  noted in the original phase plan as likely not directly applicable since
  our capture path is portal-based, not Electron's clipboard API. Revisit
  only if stale/empty clipboard reads actually show up in testing (haven't
  so far).

## Suggested order if picked back up

1. ~~§9 (logger wiring)~~ — done, and turned up two extra fixes along the
   way (dead-code `write()`, no live broadcast — see §9).
2. §3 (clipboard retry/validation) — small, fixes a real reliability gap in
   the one feature that matters most (price-check).
3. §10 (PoE-focus-loss resets state) — small, matches an actual UX
   expectation (Alt-tab should always hand control back to the game).
4. §6 (local Escape via focus, not global grab) — medium, now unblocked,
   directly un-regresses a feature we had to rip out.
5. §11 (tray icon, likely via `ksni`) — medium, self-contained (one new
   module), and the only real way to quit/reopen the app without a terminal —
   worth doing before this gets used outside active dev/testing.
6. ~~§1 (config-driven hotkeys)~~ — done, though it landed as "centralized
   in KDE Global Shortcuts" rather than a literal port of `Shortcuts.ts`
   (see §1). Doesn't unlock paste-in-chat/stash-search "for free" the way
   originally hoped — those still need their own EIS text-injection +
   clipboard-write primitives, tracked in §1's own remaining list.
7. §4/§5 (hover-to-interact, hold-to-pin, Alt-hold-hide) — blocked on having
   a live modifier-key/cursor stream we don't currently have; needs its own
   spike to figure out whether KWin scripting or another portal capability
   can provide one before committing to an approach.
8. §7 (game log watcher) — biggest standalone feature, independent of
   everything else; tackle whenever, in its own pass.

## Productionalization: non-feature gaps

Separate axis from everything above — not feature parity with upstream
APT, just packaging/robustness of this repo's own Rust backend. Audited
2026-08-03.

**Done**: PKGBUILD (`makepkg -si` local install on Arch/CachyOS — not
published to AUR, registration's closed to new accounts); GitHub Actions
`ci.yml` (build check every push/PR), `bump-submodule.yml` (daily poll +
validated PR for upstream awakened-poe-trade commits), `release.yml`
(tag push → build → GitHub Release with a tarball); `server.rs`'s renderer
asset path now resolves at runtime (`APT_KWIN_OVERLAY_DIST` env var →
`/usr/share/apt-kwin-overlay/dist` → dev fallback via `.cargo/config.toml`)
instead of baking in a build-time path that broke once packaged; crash
resilience (`server::spawn()` failure in `main.rs` now shows a
`gtk4::AlertDialog` and quits cleanly instead of panicking); XDG basedir
compliance (`xdg::data_dir()` respects `$XDG_DATA_HOME`, used by
`config_store.rs`, `uploads.rs`, `remote_input.rs`); branding
(`APP_ID` is `io.github.thwonghin.AptKwinOverlay`, window title is
"apt-kwin-overlay"); `.desktop` file (`data/io.github.thwonghin.AptKwinOverlay.desktop`,
installed to `/usr/share/applications` by `PKGBUILD`'s `package()`, reuses
the real APT icon already vendored at `renderer/dist/icon.png`) — needed
for `ashpd::register_host_app` to succeed on KDE, which looks up an
installed app matching `APP_ID` via the portal's Registry interface.

**Remaining**: nothing — all non-feature gaps closed as of 2026-08-03.
