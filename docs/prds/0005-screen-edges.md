# 0005 — Screens have edges

> Status: **draft** · Depends on: [0003](0003-several-pcs-and-switcher.md), [0008](0008-windows-drives-linux.md) · Vision: [0000](0000-vision.md)

## Goals

Step 4 of the plan (2026-10-21): move the mouse off the edge of one screen
and it appears on the PC placed there, at the same height, and back. The
hotkeys stay as the way home. The owner also wants the settings window to
show the PCs that are online, their monitors and how they are arranged,
and to arrange them by dragging.

Done means: the Windows PC placed to the right of the laptop; the cursor
leaves the laptop's right edge and appears on the Windows PC's left edge at
the same relative height; leaving the Windows PC's left edge brings it back.

## The Wayland barrier, and how we pass it

A program may not read the cursor position on Wayland. Mickey asks whoever
may:

| System | How Mickey knows the cursor reached an edge |
|---|---|
| Windows | `GetCursorPos` |
| Hyprland | its IPC socket, `cursorpos` (measured: 20 µs p50, 56 µs p99) |
| X11 | `XQueryPointer` (later) |
| Other Wayland (Sway, KDE, GNOME) | 1-pixel layer-shell surfaces on the edges, or the InputCapture portal (later) |

The position is read only while the mouse moves.

## Approach

1. **Monitors.** Each PC finds its monitors (position and logical size):
   `EnumDisplayMonitors` on Windows, Hyprland's `monitors` on Linux.
   `mickey monitors` prints them.
2. **Exchange.** After `Hello`, each side sends a `Monitors` message
   *(only the target does; see Discoveries, 2026-10-22)*; the
   source keeps every peer's, and `mickey status` and the settings window
   show them.
3. **Layout.** `config.toml` says where each peer's desktop sits next to
   this one (`position = [x, y]`, see *Discoveries*); the settings window
   draws the PCs and their monitors and lets the owner drag them.
4. **Edges.** The source polls the cursor while it moves; reaching the edge
   that has a peer switches focus to it, and the target places its cursor
   at the matching point (absolute positioning: `SetCursorPos` on Windows,
   an `EV_ABS` axis on the uinput device on Linux). On the remote side,
   motion is tracked from the deltas; reaching the edge facing home comes
   back.

## Requirements

- [x] `mickey monitors` lists this PC's monitors on Windows and Hyprland.
- [x] `Monitors` message (up to 8 monitors), protocol version 3.
- [x] `[[peer]] position = [x, y]`.
- [x] Edge crossing both ways, where the desktops touch; no crossing while a
      mouse button is held (dragging a window must not throw it to another
      PC).
- [x] Settings window: PCs online and their monitors, drawn; drag to
      place, snapping against the nearest monitor.

## Non-Goals

- Edges on compositors other than Hyprland, and on X11, until later.
- Different scaling per monitor is handled by using logical sizes only.

## Discoveries

- **The entry point wins when crossing an edge.** The target receives the
  entry coordinates before the focus notification. After placing the cursor
  there, it clears the saved parking position so the focus notification
  cannot restore the previous exit point. A hotkey switch has no entry
  coordinates and still restores the saved position.
- **A free position, not a side.** The first version put each peer on
  one of four sides of the whole desktop, centred (#96, closed). Trying
  it, the owner wanted the laptop next to one particular monitor, in its
  corner, as Windows' display settings allow. So a peer has a position:
  where its point 0,0 sits on this desktop. Dropped in the settings
  window, it snaps against the nearest monitor, touching without
  overlapping, and the cursor will cross wherever the two touch.
- **The settings window asks the running Mickey.** It is a separate
  process, so it runs `mickey peers` over the control channel (a Unix
  socket on Linux; on Windows TCP on localhost with a token in the user's
  own folder). It asks again every 2 s while Mickey is still starting.
- **Crossing, both ways.** The source crosses when the cursor pushes
  against the edge of its desktop onto a pixel that, shifted by a peer's
  position, is on one of the peer's monitors; it sends `Enter` with that
  point. The way back is noticed by the target, which knows its own
  cursor: when it pushes off the target's desktop, the target sends
  `Leave` with the pixel just off it, and the source comes home if that
  pixel lands on one of its monitors. A Leave is sent once per push.
- **Placing the cursor on Linux** uses Hyprland's `movecursor` instead of
  an `EV_ABS` axis on the uinput device: an absolute axis would make the
  virtual device look like a tablet, and Hyprland already uses the
  coordinates `monitors` reports.
- **Windows reads raw mouse motion while local too**, to see the cursor
  pushing against an edge: the mouse hook is installed only while the
  focus is remote (it slowed games down otherwise), and raw input costs
  nothing in the game's path.
- **Games and raw input (Windows).** Found while testing with
  Counter-Strike open: games read input with raw input, which hooks cannot
  block, and pin the cursor while aiming. While the focus is remote, an
  invisible Mickey window takes the foreground (games only get raw input
  in the foreground) and motion is read from raw input.

- **Monitors move while Mickey runs** (#365). Each PC read its monitors
  once, at start. After a monitor was moved in Windows' display settings,
  the source kept checking the target's `Leave` against the old layout,
  found no monitor there and ignored it silently: the way back through
  that monitor was gone until Mickey restarted. The source now reads them
  again once a second, off its main loop, on Windows and Hyprland. Not
  on GNOME and KDE: the portal reports monitors once per session, and
  reading through it every second could ask for permission again. A
  target does the same, with one reader for every source it serves, and
  sends `Monitors` again after the next `Ping` when they moved, so the
  source's layout and the target's own edges follow them too.

### Checked against the code (2026-10-22)

- **Only the target sends `Monitors`**, right after `Hello`
  (`transport::serve`). The source never sends its own: it reads them
  locally (`monitors::this_pc`) and is the one that needs the peer's to
  cross, while the way back is decided by the target with its own
  monitors. So "each side sends" above never happened; nothing needs it.
- **"No crossing while a mouse button is held" was enforced only on the
  way out.** The source refused to cross with a button down, but a
  target still sent `Leave` mid-drag, so dragging a window or file
  against a peer's edge brought the focus home with the button still held
  there. Fixed in PR #208: the target sends `Leave` only when no button is
  held.

### From one peer straight onto the next (2026-11-05)

- **The cursor only ever went home from a peer.** With two laptops placed
  side by side below the Windows PC, pushing off one laptop toward the
  other did nothing: the source checked a peer's `Leave` against its own
  monitors only (`edges::back`). Now `layout::landing` tries this PC
  first, then every other placed peer at the same point of the layout,
  and the focus hops there the way a pick in the switcher does (home,
  then the peer). The peers need no change: a target already sends
  `Leave` off any of its edges.
- **Fast moves bounced between the two peers.** Two stale edge answers
  undid a hop: on the source, an answer about its own parked cursor
  arriving after the hop (now dropped unless the focus is here, and a hop
  wins over it); on the target, the last push off an edge before the
  cursor left, read on the first motion back (now dropped on `Enter`).
  Motion summed by the 8 ms throttle but not yet checked did the same on
  both sides, so the throttle starts again when the focus moves. An
  answer still being worked out by the edge thread at that moment is not
  caught; it takes a few milliseconds, against a hop that takes longer.
  Slow moves never showed it: little motion falls in that window.

### Desktops without InputCapture (2026-11-14, #429)

- **COSMIC has no InputCapture portal**, so the portal path cannot see the
  pointer reach an edge there. Checked in a COSMIC session (the Pop!_OS
  24.04 ISO in the `pop` VM): `No such interface
  org.freedesktop.portal.InputCapture`; its current portal source does not
  declare it either. The ISO's portal has no RemoteDesktop either, so
  Mickey found no monitors of its own.
- **The layer-shell strips work there.** A spike with the same one-pixel
  overlay surface as the Sway strips (`edges/sway.rs`) mapped invisibly on
  COSMIC and reported `wl_pointer.enter` at the height the pointer was put
  (y 122.1 and 683.6 of 800 for 15% and 85%). So the strips can serve any
  desktop with layer-shell when InputCapture is missing, which would also
  give edges to river, labwc and Wayfire.
- **Monitors from xdg-output.** Where no portal or compositor IPC names
  the monitors, `desktop::outputs` asks Wayland itself
  (`zxdg_output_manager_v1`): each output's name, logical position and
  size, the same layout the portal and Sway's IPC give. Checked on
  Hyprland with its IPC hidden: one monitor, 1536x864 at 0,0, as Hyprland
  says.
- **Placing without a portal (#459).** A controlled PC with no
  RemoteDesktop portal (the same COSMIC ISO) could not put the cursor where
  it came in, nor follow it to see it pushed off an edge. A uinput
  absolute pointer (`ABS_X`/`ABS_Y`, like a drawing tablet) puts it at the
  exact pixel on any Linux desktop (checked: y 400.0 and 650.0 of 800), so
  `desktop::absolute` does what the portal path does: adds each motion and
  places the cursor. Round trip checked in the VM: out across an edge,
  back across the other PC's edge, and out again. Coming home onto a strip
  counts as "just placed", or the cursor bounced straight back.
