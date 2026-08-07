# apt-kwin-overlay: remaining gaps vs. real Awakened Poe Trade

Audit of `main/src/*` in the vendored, unmodified `awakened-poe-trade`
submodule against what `apt-kwin-overlay`'s Rust backend actually
implements. The renderer (Vue UI) is 100% the real, unmodified app —
everything below is about the `main/` process we replaced.

Items that were open gaps here and have since been closed — config-driven
hotkeys (via KDE's own Global Shortcuts), clipboard read reliability +
restore-on-copy, local Escape/Ctrl+W handling, in-app logger wiring, and
resetting overlay state when PoE regains OS focus — have been removed from
this list to keep it focused on what's actually still open. See git history
for how each landed; the reasoning behind non-obvious decisions (e.g. why
hotkeys are now configured in KDE System Settings instead of APT's own UI)
is preserved as code comments at the relevant call sites (`host_config.rs`,
`main.rs`), not duplicated here.

Ordered roughly by how much it'd actually change day-to-day use, not by file.

## 1. Stash scroll-wheel navigation

Real: Ctrl+scroll while hovering the stash tab bar (not over the sidebar) taps
Left/Right arrow to change tabs (`Shortcuts.ts`'s `uIOhook.on('wheel', ...)` +
`isStashArea`). Needs real PoE window bounds (`GameWindow.bounds`,
`uiSidebarWidth`) to know where "the stash tab bar" actually is on screen.

We have no scroll-event capture at all (libei/EIS only ever *emits* input for
us, doesn't capture the user's real mouse wheel), and no window-bounds-based
region logic. Not attempted. The `stashScroll` config field isn't parsed
either — relevant if this is ever picked up.

## 2. Hotkey actions not yet bound: paste-in-chat, trigger-event, stash-search

Parsed as `ActionKind::Unsupported` (inert) in `host_config.rs`. Closer than
it looks: `main/src/shortcuts/text-box.ts`'s `typeInChat`/`stashSearch` are
just `uiohook`-style key-tap sequences (`V`/`Ctrl+V`, `Enter`, `Home`,
`Delete`, `Ctrl+A`, `Ctrl+F`, `ArrowUp`, `Escape`) wrapped in a
clipboard-save/restore. `remote_input.rs::press_keys` is already a generic
`&[(keycode, KeyState)]` injector (`press_ctrl_c` just calls it with two
keys), and the clipboard-write wrapper these sequences need
(`RemoteInput::write_clipboard_text`, via `ashpd`'s `Clipboard` portal's
`set_selection`/`selection_write`/`selection_write_done`) already exists,
built for the clipboard-restore work. Remaining work is just evdev keycode
constants for the extra keys and a generic multi-key-tap helper — no new
portal capability needed.

## 3. No hover-to-interact / hold-modifier-to-pin for the price-check popup

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
injected keys (not the user's real keyboard).

Real fix shape: we already receive `OVERLAY->MAIN::track-area` payloads
nowhere (no listener) — would need a live cursor-position stream (not the
current one-shot KWin query-per-poll) to react promptly, which is a bigger
lift than it sounds given our cursor position only comes from on-demand KWin
scripting round-trips.

## 4. No Alt-hold-to-hide-overlay (`OverlayVisibility.ts`)

Real: holding Alt (no other modifiers) makes the whole overlay invisible
after 85ms (if currently interactable) or 275ms (if not) — lets you glance at
the game without fully closing widgets. Releasing Alt or moving the mouse
without Alt held restores visibility. We don't implement this at all — no
`MAIN->OVERLAY::visibility` event is ever sent from our side.

Needs real-time modifier-key state (same missing piece as §3's hold-to-pin) —
`remote_input.rs` currently only sees modifier events tied to *our own*
injected key presses, not a live feed of the user's actual keyboard, since
EIS is a Sender-only context for us (see `remote_input.rs` doc comments).
Getting a live modifier-key feed would likely need a second EIS/portal
capability we haven't set up (or KWin D-Bus scripting's own key-event hooks,
unverified whether that's exposed).

Same root limitation blocks a smaller, separate real feature: `Shortcuts.ts`
also has a `logKeys` config flag (off some renderer Settings toggle) that,
when on, logs every raw keydown/keyup and which action type fired to the
Log tab — a live debugging aid for hotkey issues. `host_config.rs` doesn't
parse `logKeys` at all; even if it did, we have nothing to log from, since we
never observe the user's real keyboard either.

## 5. No game-log-driven features (`GameLogWatcher.ts`)

Whisper/trade/zone-change notifications from tailing `Client.txt`. Genuinely
separate, sizeable feature — would need a file watcher on the log path
(`cfg.clientLog` from config) and its own event stream into
`MAIN->CLIENT::game-log`.

## 6. No tray icon (`AppTray.ts`)

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

**Implementation caveat**: once "Open in Browser" exists, `server.rs`'s WS
server can have two concurrent clients (the overlay WebView + a plain
browser tab), and `EventBus`'s `last_active`/`mark_active` tracking doesn't
distinguish which one is "the overlay" — it's just whichever client sent
something most recently. Real APT tracks this explicitly: `OverlayWindow.ts`
keeps its own `wasUsedRecently` flag, set from a dedicated
`CLIENT->MAIN::used-recently` payload's `isOverlay: true` field, and
`Shortcuts.ts`'s `copy-item` handler only calls `assertOverlayActive()` for a
`focusOverlay` action when `this.overlay.wasUsedRecently` is true — i.e. a
locked price-check should only force the *overlay* interactive, not steal
focus onto it because a separate browser tab happened to be the last thing
that touched the WebSocket. Our `shortcuts.rs::price_check` unconditionally
calls `set_click_through(false, ...)` for any `focus_overlay:true` action
today, which is harmless while the overlay is the only possible client but
would misbehave the moment this ships a second one. Worth carrying this
`isOverlay`-style distinction into `EventBus` at the same time as the tray,
not after.

## 7. Not applicable / correctly out of scope, no action needed

- **`AppUpdater.ts`** (real electron-updater auto-update flow) — we already
  stub this correctly (always `update-not-available`). Self-updating doesn't
  make sense for a locally-built dev binary; leave as-is permanently.
- **`vision/*`** (OCR for heist gems, incl. the `ocr-text` copy-item hotkey
  action) — Windows-only upstream already (uses a Windows-specific
  screenshot path), correctly out of scope regardless of our platform.
- **`HostClipboard.ts`'s KDE/Proton-specific empty-clipboard workarounds** —
  likely not directly applicable since our capture path is portal-based, not
  Electron's clipboard API. Revisit only if stale/empty clipboard reads
  actually show up in testing (haven't so far).
- **`GameConfig.ts`** (reads PoE's `production_Config.ini` for the player's
  "advanced item description" keybind) — as of PoE 3.29, `Shortcuts.ts`
  hardcodes `showModsKey = 'Ctrl'` and ignores whatever `GameConfig` found
  anyway. Close to dead code in the real app too now; not worth porting.
- **`keepModKeys` config field** — only matters for deciding whether to
  reuse a still-physically-held modifier key vs. synthesizing the whole
  combo fresh; moot for us since we can't observe the user's physical key
  state either way (same root limitation as §3/§4) and we already always
  synthesize a full Ctrl+C, matching real APT's own `keepModKeys:false` code
  path.

## 8. Two small differences: proxy cookie handling, no single-instance lock

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

1. §6 (tray icon, likely via `ksni`) — medium, self-contained (one new
   module), and the only real way to quit/reopen the app without a
   terminal — worth doing before this gets used outside active dev/testing.
   Carry the `wasUsedRecently`/`isOverlay` distinction (see §6) into
   `EventBus` at the same time, not as a follow-up.
2. §2 (paste-in-chat/stash-search hotkey actions) — needs new evdev keycode
   constants and a generic multi-tap helper; the clipboard-write wrapper it
   also needs already exists (`RemoteInput::write_clipboard_text`) and can
   be reused as-is.
3. §3/§4 (hover-to-interact, hold-to-pin, Alt-hold-hide, `logKeys` debug
   view) — blocked on having a live modifier-key/cursor stream we don't
   currently have; needs its own spike to figure out whether KWin scripting
   or another portal capability can provide one before committing to an
   approach.
4. §5 (game log watcher) — biggest standalone feature, independent of
   everything else; tackle whenever, in its own pass.
5. §8 (proxy cookie handling, single-instance lock) — low priority, no
   confirmed real-world symptom for either; pick up opportunistically.

(§1, stash scroll-wheel navigation, is deliberately left off this list —
needs real window-bounds tracking plus scroll-event capture we don't have
today, a bigger lift than anything above for a lower-traffic feature.)

## Productionalization: non-feature gaps

Separate axis from everything above — not feature parity with upstream
APT, just packaging/robustness of this repo's own Rust backend.

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

**Remaining**: nothing structural. Two small, low-priority items — see §8
above (no single-instance lock; proxy doesn't forward `Set-Cookie`/has no
outbound cookie jar). Neither has a confirmed real-world symptom.
