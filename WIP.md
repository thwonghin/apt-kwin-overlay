# apt-kwin-overlay: remaining gaps vs. real Awakened Poe Trade

Audit of `main/src/*` in the vendored, unmodified `awakened-poe-trade` submodule
against what `apt-kwin-overlay`'s Rust backend actually implements. The renderer
(Vue UI) is 100% the real, unmodified app — everything below is about the
`main/` process we replaced.

Re-audited 2026-08-05: read every file under `main/src/*` directly (including
several no prior pass had opened — `text-box.ts`, `Shortcuts.ts`,
`OverlayWindow.ts`, `RemoteLogger.ts`, `ConfigStore.ts`, `file-uploads.ts`,
`proxy.ts`, `server.ts`, `main.ts`, `GameWindow.ts`) against the current
`src/*.rs`. §§2-4, 6-9, 11, 12 below still match; §10 has been corrected (the
mechanism it described was backwards); §1, §5, §6, §9, §11 got new detail;
§13 is new, covering two small differences found in files not previously
audited (`proxy.ts`, `main.ts`).

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
- `paste-in-chat`, `trigger-event`, `stash-search` — closer than the
  Rust-side gap suggests. Read `main/src/shortcuts/text-box.ts` directly:
  `typeInChat`/`stashSearch` are just `uiohook`-style key-tap sequences
  (`V`/`Ctrl+V`, `Enter`, `Home`, `Delete`, `Ctrl+A`, `Ctrl+F`, `ArrowUp`,
  `Escape`) wrapped in a clipboard-save/restore. On our side:
  `remote_input.rs::press_keys` is already a generic
  `&[(keycode, KeyState)]` injector (`press_ctrl_c` just calls it with two
  keys) — extending it to these sequences is mostly adding evdev keycode
  constants and a public multi-key-tap helper, not new plumbing. Clipboard
  *write* doesn't exist yet (`RemoteInput` only exposes
  `read_clipboard_text`), but `ashpd::desktop::clipboard::Clipboard`
  (already a dependency, checked `ashpd-0.13.13`'s
  `src/desktop/clipboard.rs` directly) has `set_selection`/
  `selection_write`/`selection_write_done` on the same session we already
  hold — no new portal capability needed, just unused API surface. So the
  real remaining work is: evdev keycodes for the extra keys, a generic
  multi-tap helper, and a clipboard-write wrapper — not the "need EIS
  text-injection + clipboard-write primitives that don't exist" blocker
  this used to describe.
- `ocr-text` (heist gems) — Windows-only upstream, correctly out of scope
  regardless of platform (see §12).

Also not modeled at all: `logKeys` (see §5) and `stashScroll`/`keepModKeys`
config fields. `keepModKeys` (`Shortcuts.ts`'s `register()`/
`pressKeysToCopyItemText`) only matters for deciding whether to reuse a
still-physically-held modifier key vs. synthesizing the whole combo fresh —
moot for us since we can't observe the user's physical key state either way
(same root limitation as §4/§5) and already always synthesize a full
Ctrl+C, matching real APT's own `keepModKeys:false` code path.

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

(Separately, `HostClipboard.ts` also has a `restoreShortly`/`RESTORE_AFTER`
120ms throttle-and-restore path used only by `typeInChat`/`stashSearch` —
not `readItemText` — so it's relevant to §1's paste-in-chat/stash-search
work, not this one.)

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

Same root limitation blocks a smaller, separate real feature: `Shortcuts.ts`
also has a `logKeys` config flag (off some renderer Settings toggle) that,
when on, logs every raw keydown/keyup and which action type fired to the
Log tab (`this.logger.write('debug [Shortcuts] Keydown ...')` etc.) — a
live debugging aid for hotkey issues. `host_config.rs` doesn't parse
`logKeys` at all; even if it did, we have nothing to log from, since we
never observe the user's real keyboard either.

## 6. Escape/Ctrl+W: real APT does this locally, not as a global shortcut

Worth re-visiting now that keyboard-mode toggling is fixed. Real
`OverlayWindow.ts`'s `handleExtraCommands` is a **local**
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

Read `handleExtraCommands` directly: the same local handler also matches
`this.overlayKey` (default "Shift + Space") and calls `toggleActiveState`,
*in addition to* the global `electron.globalShortcut` registration
`Shortcuts.ts` sets up for the identical action. It's a deliberate
redundant fallback — global shortcuts can behave oddly when the same app
already has native OS keyboard focus on some platforms, so the overlay's
own toggle key still works via the local path in that state. If §6 is
picked up, binding the same local `EventControllerKey` to the
`toggle-overlay` action (not just Escape/Ctrl+W) would be a cheap way to
close this same redundancy gap, not just Escape.

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
parameter is overwritten unconditionally, confirmed reading `Shortcuts.ts`
directly). This file is close to dead code in the real app too now — not
worth porting.

## 9. In-app Logger is underused — Settings → Log tab is mostly empty for us — DONE, one loose end

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

Loose end found this pass: real `RemoteLogger.ts`'s `write()` prefixes every
line with a wall-clock timestamp (`` `[${new Date().toLocaleTimeString()}] message` ``)
before storing/broadcasting it; our `logger.rs::write()` stores/broadcasts
the raw message with no timestamp at all. Cosmetic (the Log tab just loses
per-entry timestamps), one-line fix if picked up — format the timestamp
prefix in `Logger::write` before touching history/broadcast.

## 10. PoE *regaining* OS focus doesn't reset our overlay state — DONE

Previous passes described this as "PoE losing focus doesn't reset state."
Reading `OverlayWindow.ts::handlePoeWindowActiveChange` directly this pass
shows the opposite direction:

```ts
private handlePoeWindowActiveChange = (isActive: boolean) => {
  if (isActive && this.isInteractable) {
    this.isInteractable = false
  }
  this.server.sendEventTo('broadcast', {
    name: 'MAIN->OVERLAY::focus-change',
    payload: { game: isActive, overlay: this.isInteractable, usingHotkey: this.isOverlayKeyUsed }
  })
  this.isOverlayKeyUsed = false
}
```

`isInteractable` only gets forced back to `false` when `isActive` is
**true** — i.e. when PoE *regains* OS focus (e.g. you click back into the
game after having the locked price-check popup open while alt-tabbed to a
browser), not when it loses focus. Losing focus only re-broadcasts
`focus-change` with the unchanged `isInteractable` value; renderer-side
hide-on-blur widgets react to that broadcast on their own. We currently
never react to either transition: `click_through`/widget state just sits
however it was left, regardless of which way PoE's focus changes.

This turns out to be **cheaper to close than previously scoped**: KWin
already gives us the exact "PoE window activated" signal this needs.
`kwin_tracker.rs`'s `TrackerEvent::Activated` fires precisely on
`workspace.windowActivated`, and `main.rs`'s receiver loop already special-
cases `w.is_path_of_exile()` on that event (for monitor-follow). Adding "if
click-through is currently off, force it back on and broadcast
`MAIN->OVERLAY::focus-change`" to that same arm — reusing
`set_click_through`/`shortcuts::close_all_ui`'s existing logic — replicates
real APT's actual behavior without needing any new event source.

Implemented: `main.rs`'s `TrackerEvent::Activated` arm now calls
`shortcuts::close_all_ui` whenever `w.is_path_of_exile()` and click-through
is currently off. `price_check_open`/`price_check_locked` (previously
locals created inside `shortcuts::spawn`) were hoisted up to `build_ui` and
threaded into `shortcuts::spawn` as parameters so the tracker-event handler
can share them and call `close_all_ui` directly — this resets click-through
*and* the price-check popup's open/locked bookkeeping together, not just
click-through, which matters because a stale `price_check_open` would make
the next price-check hotkey press try to *close* an already-hidden popup
instead of opening a fresh one. `close_all_ui` is now `pub(crate)` (was
module-private). No new event source or channel needed — both call sites
already run on the same GTK main-loop thread.

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

**Implementation caveat found this pass**: once "Open in Browser" exists,
`server.rs`'s WS server can have two concurrent clients (the overlay WebView
+ a plain browser tab), and `EventBus`'s `last_active`/`mark_active`
tracking doesn't distinguish which one is "the overlay" — it's just
whichever client sent something most recently. Real APT tracks this
explicitly: `OverlayWindow.ts` keeps its own `wasUsedRecently` flag, set
from a dedicated `CLIENT->MAIN::used-recently` payload's `isOverlay: true`
field, and `Shortcuts.ts`'s `copy-item` handler only calls
`assertOverlayActive()` for a `focusOverlay` action when
`this.overlay.wasUsedRecently` is true — i.e. a locked price-check should
only force the *overlay* interactive, not steal focus onto it because a
separate browser tab happened to be the last thing that touched the
WebSocket. Our `shortcuts.rs::price_check` unconditionally calls
`set_click_through(false, ...)` for any `focus_overlay:true` action today,
which is harmless while the overlay is the only possible client but would
misbehave the moment §11 ships a second one. Worth carrying this
`isOverlay`-style distinction into `EventBus` at the same time as the tray,
not after.

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

## 13. Two small differences found this pass, not previously covered

Found while reading `proxy.ts` and `main.ts` directly (neither had been
read line-by-line in a prior audit pass) against `proxy.rs`/`main.rs`.
Neither looks urgent; noting them so they don't get re-discovered from
scratch later.

- **Proxy doesn't forward `Set-Cookie` / has no outbound cookie jar.** Real
  `proxy.ts` uses Electron's `net.request` with `useSessionCookies: true` —
  the app's persistent session cookie jar is attached to outbound requests
  to `pathofexile.com`/etc. and updated from responses automatically,
  and `proxy.ts` additionally strips the `Partitioned` attribute from
  `Set-Cookie` response headers so Chromium's cookie store will actually
  accept cross-context Cloudflare cookies. Our `proxy.rs` forwards only
  `content-type` and `x-rate-limit-*` response headers
  (`write_response_with_headers` call in `proxy::handle`) — `Set-Cookie` is
  silently dropped, and there's no cookie jar on the `ureq::Agent` side to
  attach anything back on the next request either way. Checked the renderer
  for any cookie/credential dependency (`grep -rn "POESESSID\|document.cookie\|withCredentials"
  renderer/src`) and found none, so this likely doesn't affect price-check
  or trade-search today — but it's a genuine behavioral difference from
  real APT's proxy, worth remembering if GGG-side requests ever start
  behaving inconsistently (e.g. increasingly aggressive Cloudflare
  challenges from what looks like a cookie-less client on every request).
- **No single-instance lock.** Real `main.ts` calls
  `app.requestSingleInstanceLock()` and exits immediately if another
  instance already holds it. `main.rs` has no equivalent — running the
  binary twice spawns two overlay windows, two local HTTP/WS servers, and
  two competing `ashpd` `GlobalShortcuts`/`RemoteDesktop` sessions with no
  guard against it. Minor (accidental double-launch, not a real usage
  path), but a one-time `flock`-style lock file (e.g. under
  `xdg::data_dir()`) would close it cheaply if it ever bites someone.

## Suggested order if picked back up

1. ~~§9 (logger wiring)~~ — done, and turned up two extra fixes along the
   way (dead-code `write()`, no live broadcast — see §9). One loose end
   left (missing timestamp prefix), trivial if ever picked up.
2. ~~§10 (PoE-focus-regain resets state)~~ — done, landed as scoped: a small
   addition to `main.rs`'s existing `TrackerEvent::Activated` handler
   calling `shortcuts::close_all_ui`, no new event source.
3. §3 (clipboard retry/validation) — small, fixes a real reliability gap in
   the one feature that matters most (price-check).
4. §6 (local Escape via focus, not global grab) — medium, unblocked since
   commit `d749d98`, directly un-regresses a feature we had to rip out;
   worth binding the overlay toggle key locally too while in there (see §6).
5. §11 (tray icon, likely via `ksni`) — medium, self-contained (one new
   module), and the only real way to quit/reopen the app without a terminal —
   worth doing before this gets used outside active dev/testing. Carry the
   `wasUsedRecently`/`isOverlay` distinction (see §11) into `EventBus` at
   the same time, not as a follow-up.
6. ~~§1 (config-driven hotkeys)~~ — done, though it landed as "centralized
   in KDE Global Shortcuts" rather than a literal port of `Shortcuts.ts`
   (see §1). paste-in-chat/stash-search turned out more tractable than
   originally scoped (see §1's re-audit) but still their own pass — needs
   new evdev keycode constants, a generic multi-tap helper, and a clipboard-
   write wrapper around `ashpd`'s already-present `Clipboard` portal.
7. §4/§5 (hover-to-interact, hold-to-pin, Alt-hold-hide, `logKeys` debug
   view) — blocked on having a live modifier-key/cursor stream we don't
   currently have; needs its own spike to figure out whether KWin scripting
   or another portal capability can provide one before committing to an
   approach.
8. §7 (game log watcher) — biggest standalone feature, independent of
   everything else; tackle whenever, in its own pass.
9. §13 (proxy cookie handling, single-instance lock) — low priority, no
   confirmed real-world symptom for either; pick up opportunistically.

## Productionalization: non-feature gaps

Separate axis from everything above — not feature parity with upstream
APT, just packaging/robustness of this repo's own Rust backend. Audited
2026-08-03, re-checked 2026-08-05.

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

**Remaining**: nothing structural from the 2026-08-03 pass. Two small,
low-priority items surfaced 2026-08-05 while re-auditing files not
previously read closely — see §13 above (no single-instance lock; proxy
doesn't forward `Set-Cookie`/has no outbound cookie jar). Neither has a
confirmed real-world symptom.
