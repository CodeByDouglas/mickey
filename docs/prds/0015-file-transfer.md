# 0015 — Big files and folders

> Status: **in progress** (works end to end; gaps in #502) · Depends on: [0002](0002-secure-link.md), [0011](0011-shared-clipboard.md) · Vision: [0000](0000-vision.md) · Issue: #431

## Goals

Copy a file or a whole folder on one PC, move the keyboard to another,
press Ctrl+V anywhere: the Explorer, Dolphin, the desktop, a browser's
upload box. No size limit, no file-count limit, and what arrives is
exactly what was copied. Decided with the owner on 2026-11-18:

- **Copy and paste is the trigger**, not a "send" button. Copying is the
  intent; the transfer follows the keyboard.
- **Reliable first**: free space is checked before and during the
  transfer, every file is verified, and a broken transfer resumes where
  it stopped instead of starting over.
- **Fast, and honest about it**: a progress window with the real stage,
  the speed and the time left.
- **Built on the clipboard Mickey already has** (`arboard`, PRD 0011), so
  it works on every system Mickey supports today, X11 included.

Done means: copy a 5 GB folder of 10 000 files on the Windows PC, move the
keyboard to the laptop, say *Bring*, pull the network cable halfway,
plug it back, and paste the complete, verified folder into Dolphin.

Today the clipboard carries files up to 60 MB and 1 000 files, read whole
into memory, and drops folders (`clipboard/wire.rs`). This PRD replaces
that path.

## How it works

1. **Copy on PC A.** Nothing leaves PC A. When the keyboard crosses to
   PC B, PC A sends an **offer**: the tree of files (relative paths and
   sizes), the total, and an offer ID. No content.
2. **PC B decides.** Below the threshold (1 GB by default, configurable)
   it fetches at once. Above it, a window shows the tree and the total
   size with **Bring / Ignore** (Enter / Esc) and the warning "don't
   change this folder on PC A until it finishes".
3. **PC B fetches** over a connection of its own (below), streaming each
   file from PC A's disk to its own, in fixed-size pieces, never whole in
   memory.
4. **When every file is verified**, PC B puts the paths on its clipboard
   with `arboard`, as today. Ctrl+V then works in any app, because to the
   app these are ordinary files on disk.
5. **Before that**, Ctrl+V pastes whatever was on the clipboard before:
   nothing hangs, and the progress window is on screen.
6. **If something newer is copied** before it finishes, the transfer still
   finishes to disk but leaves the clipboard alone; the "done" notice says
   where the files are.

### Where files land

- **Automatic fetches** (below the threshold) go to Mickey's cache
  folder, as today (`~/.cache/mickey/received`,
  `%LOCALAPPDATA%\mickey\received`), which cleans itself. Copies that
  cross and are never pasted do not pile up.
- **Brought on purpose** (above the threshold) goes to
  `Downloads/Mickey`, which Mickey never empties.
- The "done" notice always says where the files are, with **Open folder**.
- A name that already exists becomes `name (2)`.

### Progress and the queue

- A small window in the bottom-right corner lists the transfers: the
  stage (*Listing*, *Checking space*, *Transferring 2.1 of 4.2 GB · 85
  MB/s · ~25 s*, *Verifying*, *Done*), with **Cancel**. It can be
  minimized.
- Only real stages are shown: there is no packing step (nothing is
  compressed) and encryption happens piece by piece while sending.
- The time left comes from the speed averaged over the last seconds, not
  the instant speed.
- It appears only when a transfer takes more than about two seconds.
- **One transfer at a time per PC**; another one waits in the queue,
  shown as *Queued*.
- At the end, a summary: "10 523 files, 4.2 GB, verified", plus what was
  skipped and why (below). File names appear only in this window, never
  in the log above `trace`.

### Reliability

- **Free space** is checked before starting: if the total does not fit,
  PC B says how much is missing and writes nothing. It is checked again
  while writing (every 64 MB or every second, whichever comes first), and
  the transfer stops while a safety margin is still free (1 GB or 5% of
  the disk, whichever is larger), so the disk never fills even if another
  program writes at the same time. The partial file is kept for resuming.
- **Every file is verified**: PC A hashes each file with BLAKE3 while
  reading it, PC B hashes what it writes, and the file gets its real name
  only when both hashes match. Until then it is `name.part`.
- **Resume at the exact byte**: after a disconnect, a sleeping PC or a
  full disk, fetching again re-hashes the `.part` already on disk and asks
  only for the rest. (Re-hashing reads the partial file again: about 2 s
  for 4 GB on an SSD. Accepted for v1; a hash per block only if a
  measurement shows it hurts.)
- **A file that changes on PC A** during the transfer (size or
  modification time differ before and after reading it) is sent again; a
  file deleted on PC A is listed in the summary as not sent.
- **No `fsync` per file**: it makes 10 000 small files very slow,
  especially on Windows. Surviving a power cut mid-transfer is a
  non-goal; a verified rename is the guarantee.

### What is not an ordinary file

- **Empty folders** are recreated.
- **Symlinks and Windows junctions are never followed.** That protects
  against sending something outside the copied folder (a link to
  `~/.ssh` would otherwise send the keys). They are listed in the summary
  as skipped.
- **Names Windows refuses** (`a:b.txt`, `CON`), names that differ only in
  case (`Foto.jpg` and `foto.jpg` are the same file on Windows) and paths
  longer than Windows allows are skipped and listed.
- **Permissions** (such as the executable bit) are not carried in v1.

### On Windows: where it came from

Received files keep the Mark of the Web (`Zone.Identifier`), now with
`HostUrl=mickey://<PC name>` so it says they came from Mickey. The zone
is **Local intranet** by default: Defender still scans every file, but
SmartScreen and Office's Protected View do not warn about the owner's own
files. A setting switches to **Internet** for those warnings. The mark is
written before the `.part` is renamed, so a file never exists without it.
This also applies to small files that cross with the clipboard
(*hypothesis*: that zone 1 silences SmartScreen and Protected View is
from memory; the first PR that writes it checks it on Windows 10).

### Cleaning up

- When a transfer starts and `Downloads/Mickey` holds more than 10 GB, or
  the transfer would leave less than 10% of the disk free, Mickey
  suggests cleaning: "Downloads/Mickey holds 23 GB · fotos/ arrived 47
  days ago and was never changed · Open folder". Both numbers are
  configurable. It shows when Mickey received each item and when it was
  last modified (last-access times are usually not kept by the systems).
- **Mickey never deletes anything the owner brought.** It only deletes
  abandoned `.part` files, after 7 days without resuming, and lists
  incomplete transfers in the suggestion.

## Approach

### Modules

The transfer is its own module; the clipboard only triggers it, so #425
(text and images on paste) can reuse it without copying it.

```
crates/mickey/src/
  net/secure.rs        stream primitive; send_bulk becomes a thin call to it
  fs/names.rs          safe names and relative paths (moved from clipboard/received.rs)
  fs/mark.rs           the Mark of the Web (moved from clipboard/received.rs)
  platform/space.rs    free disk space: statvfs / GetDiskFreeSpaceExW
  transfer/
    mod.rs             public API: offer, fetch, cancel
    offer.rs           walk a copied tree into an offer (pure)
    wire.rs            protocol messages, encode / decode (pure)
    send.rs            PC A: serve the files of an offer
    receive.rs         PC B: .part, hash, space, rename
    resume.rs          where to resume from (pure)
    progress.rs        stages, average speed, time left (pure)
    queue.rs           one transfer at a time per PC
```

Pure logic (is this path safe, does it fit, where to resume, how long is
left) lives in functions with no disk or network, tested with made-up
data; the disk and the network stay at the edges and stay thin. Every
helper exists once: `safe_name` and the Mark of the Web move to `fs/`
and both the clipboard and the transfer use them from there.

### The connection

- **One connection per transfer**, opened by PC B to PC A's transfer
  port (the input port + 2, TCP 4244 by default; see *Discoveries*), with
  the same Noise `XX` handshake and the same check that only a paired PC
  may open one. The clipboard keeps its own data link, so a 4 GB transfer
  never holds a small copy behind it.
- At most **2 transfer connections per PC**; more are refused.
- A **read timeout and a periodic "still here" message**: a PC that went
  to sleep turns the transfer into *paused, resumable* instead of a
  connection hanging forever.
- The handshake's cost is measured in the PR that adds this (rule 9). If
  it matters, transfers share one connection instead.

### The stream

- `Link::send_stream(impl Read)` / `Link::recv_stream(impl Write)` in
  `net/secure.rs` move bytes in Noise messages of up to 64 KB (Noise's
  limit). `send_bulk` / `recv_bulk` become calls to them: one stream code
  path, not two.
- Memory stays fixed (a few 64 KB buffers), within the vision's 20 MB.
- A reading thread with a small queue (4 pieces) overlaps disk and
  network **only if a measurement shows it helps**; otherwise one thread.
- A `BufWriter` on the transfer connection cuts the two writes per piece
  `write_noise` does today; measured before and after.

### The protocol

- PC B asks for **an offer ID and a file's index in that offer**, never
  a path. PC A serves only files of its current offer, and an offer
  expires when PC A's clipboard changes. A paired PC can never ask for
  `~/.ssh/id_rsa`.
- The offer ID is a hash of the paths, sizes and modification times, not
  of the content: hashing the content would read 4 GB just because
  something was copied. Content hashes are computed while sending.
- Every byte from the peer is untrusted:
  - each path component goes through `safe_name`; absolute paths and `..`
    are refused, depth and length are capped;
  - the offer's file count and size are capped (PC B refuses beyond);
  - PC B never writes more than the size announced for a file;
  - a `.part` belongs to one PC's key and one offer: no other PC can
    append to it, and it must be a plain file, not a link.
- **Versions**: when two PCs connect they say whether they know
  transfers. A PC that does not falls back to today's 60 MB path, which
  is removed one release later.
- PC A opens files without following links (`O_NOFOLLOW` on Linux,
  `FILE_FLAG_OPEN_REPARSE_POINT` on Windows) and checks again when
  opening, not only when listing, so a file swapped for a link meanwhile
  is not followed.

### The window

`mickey-ui` today answers yes or no through its exit code. The queue
needs live updates, so the daemon gets a channel to it: one message per
line on the child's stdin, at most 10 updates a second.

### New code outside the standard library

- **`blake3`**: hashing files. Mickey has BLAKE2s only inside `snow`,
  not exposed for files; `blake3` is the reference implementation by the
  algorithm's authors, fast, and widely used.
- **Free disk space** has no standard-library API: `statvfs` and
  `GetDiskFreeSpaceExW` called directly in `platform/`, with `// SAFETY:`
  comments, instead of a crate.

## Plan

Each step is one PR of about 400 lines of logic or less, useful and
testable on its own. Each step has its issue, all part of #431.

0. **Spike: paste on demand** (#469, in parallel, not on the path below). A
   throwaway prototype, under 100 lines and one day, that advertises
   clipboard content without the data and logs who asks for it and when,
   run on Windows (Win+V) and on the laptop (cliphist). It answers #425:
   do clipboard managers fetch everything at once? If paste on demand
   works, it is rewritten properly in its own PR and PRD 0011 is updated.
1. **Move `safe_name` and the Mark of the Web to `fs/`.** (#470) Refactor only,
   no change in behaviour.
2. **The stream primitive** (#471), with `send_bulk` rewritten on top of it.
3. **A connection per transfer** (#472): the limit per PC, the timeout, the
   handshake's cost measured.
4. **Offers, safe paths and the protocol** (#473, `offer` and `wire`), with the
   version check.
5. **Send and receive** (#474): hashing, `.part`, free space and the margin.
6. **Resume.** (#475)
7. **The daemon → window channel.** (#476)
8. **The queue and progress window.** (#477)
9. **The clipboard trigger and Bring / Ignore** (#478); today's file path
   becomes the fallback for old versions.
10. **The cleaning suggestion** (#479) and removing abandoned `.part` files.

## How it is verified

On two real PCs (the Windows PC and the laptop), never with the owner's
own files:

- Test data is generated in a scratch folder: nested folders, an empty
  folder, thousands of small files, a few big ones, a fake symlink and a
  junction, names Windows refuses, names differing only in case.
- A disconnect is simulated with a firewall rule dropping the data port
  in the middle of a transfer, then removed.
- A full disk is simulated on a small file system (a 200 MB image mounted
  on a loop device), never on a real disk.
- Speed is compared with a plain copy over the same network (`iperf3`)
  and reported with the numbers.

## Requirements

- [x] Folders and files of any size cross with copy and paste (at most 100 000 entries per copy).
- [x] Bring / Ignore above a configurable threshold. *(`[interface] bring_over_gb`, 1 by default, in Settings > General next to where the question appears; decimal gigabytes, as the windows show sizes.)*
- [x] Automatic fetches in the cache; brought ones in `Downloads/Mickey`.
- [x] Free space checked before and during; the disk never fills.
- [ ] Every file verified with BLAKE3; summary with counts and skips. *(Verified, and the window shows counts; skipped files are only counted in the log, #502.)*
- [ ] Resume at the exact byte. *(Built and tested in `receive`, #475; the trigger starts every fetch afresh, so it is not used yet, #502.)*
- [ ] Changed and deleted source files handled and reported. *(Handled: skipped, never sent half right; not shown to the owner yet, #502.)*
- [x] Links never followed; unsafe names skipped. *(Listing them to the owner: #502.)*
- [ ] Progress window with real stages, a queue, Cancel, minimizable. *(Stages, Cancel and the queue work; a queued transfer shows nothing while it waits, and the window is not minimizable, #502.)*
- [x] The clipboard keeps working during a transfer.
- [x] Mark of the Web from Mickey, Local intranet by default, configurable.
- [ ] Cleaning suggestion; abandoned `.part` removed after 7 days. *(Removed after 7 days. The Bring question says how much `Downloads/Mickey` holds, with Open folder, when it is over 10 GB or the disk would drop under 10% free. Not yet: each item's age, and making the 10 GB and 10% configurable.)*
- [x] Older Mickey versions fall back to today's path.

## Non-Goals

- **Paste straight into the Explorer folder** without a stop in a Mickey
  folder (owning the clipboard per system, *virtual files* on Windows):
  later, if the spike shows paste on demand is worth it.
- **Dragging files between screens**: it collides with the rule that a
  held mouse button never switches screens. Later.
- **An explicit "send to PC" button**: it would reuse all of this with a
  different trigger. Later.
- **Compression**: photos, videos and archives are already compressed,
  and on a LAN the CPU becomes the bottleneck. Only if a measurement
  shows a gain without costing reliability.
- **Surviving a power cut** mid-transfer.
- **Paste on demand on X11.**
- **Text, images and "secret" marks**: #425; the secret marks are
  reserved for their proposer.

## Credits

Paste on demand and announcing before sending came from @wendelmax in
#425, and the failing test for resent content in #424. Commits that use
them carry `Co-authored-by`.

## Discoveries

- **The version check moves to the clipboard trigger** (#478, not #473):
  it belongs where two PCs decide to use transfers, and it touches the
  input link's protocol, which the offer code leaves alone.
- **At most 100 000 entries per offer** (#473). With paths of up to 200
  bytes the encoded offer stays under the 64 MiB bulk limit; a bigger
  tree (a `node_modules`) is refused. The upgrade path is sending the
  offer in pages.
- **The offer id** is a hash of paths, sizes and modification times made
  with the standard library's hasher: only the PC that made the offer
  computes it, so it need not match across versions.
- **Names that are not UTF-8** (possible on Linux) are skipped as bad
  names: the other PC could not write them the same way.
- **The daemon → window channel** (#476) is built with the queue window
  (#477), its first user, instead of on its own.
- **Transfers have a port of their own** (#472, decided 2026-11-19): the
  input port + 2, TCP 4244 by default. The data port keeps one link per
  PC and a new one closes the older (#452), so a transfer there would
  drop the clipboard. Windows' firewall rule is per program; the Linux
  installers open TCP 4244 in the PR that starts the listener (#474).
- **A handshake costs 2 ms, with `TCP_NODELAY`** (#472, localhost,
  release build, median of 200): 44 ms without it, the peer's delayed
  acknowledgement holding each short handshake message back. Transfer
  connections set it, since every file also starts with a short request;
  one connection per transfer stays. The clipboard's data link has the
  same delay on its short messages: #485.
- **The "still here" message** moves to #474, where the transfer's
  session loop is; #472 sets the read timeout (15 s).
- **A file that changed is skipped, not sent again** (#474): the offer
  fixed its size, and the receiving PC never writes past it, so sending
  the new version would need a new offer. It is listed as *changed* in the
  summary; copying again makes a new offer. A file that changes while it
  is sent is checked again after its bytes and skipped the same way.
- **The hash covers the whole file**, also when resuming: the sending PC
  reads (without sending) the bytes the other PC already has.
- **#474 is split in two**: sending and receiving (hashes, `.part`, `name
  (2)`), then free space and its margin. The "still here" message moves
  to #475: only re-hashing a big `.part` keeps a side silent for long.
  Starting the listener, and opening TCP 4244 in the installers, moves to
  #478, where there is an offer to serve.
- **Free space** (#474): `platform/space.rs` asks the system
  (`statvfs`, `GetDiskFreeSpaceExW`). A transfer starts only if all its
  files fit with the margin to spare; while writing, the disk is looked at
  every 64 MB or every second and writing stops while the margin is still
  free. The stopped file's `.part` is kept, to resume.
- **Resuming lives as long as Mickey runs** (#475): what a transfer has
  done (the names its top-level entries got, finished files, the `.part`
  files it wrote) is kept in memory, so a transfer resumes after the
  network goes, a PC sleeps or the disk fills, but starts over after
  Mickey restarts; its old `.part` files are then left for the cleaning
  (#479). Only a `.part` this transfer wrote is ever continued: one found
  on disk is never trusted.
- **`StillHere`** (#475): while a side re-reads what it has of a big file
  to hash it, it sends one every 5 s, a third of `TRANSFER_SILENT`.
- **Offer ids are random, not a hash** (#478): an id is what lets a PC
  fetch, and only the PC the clipboard was carried to learns it, over the
  encrypted link; another paired PC cannot fetch the copied files at any
  time, which would get around #185. Only the latest offer is served.
- **Every PC listens for transfers**, controlled or not: the PC with the
  keyboard is where things are copied most. A PC updated by Mickey itself
  keeps its firewall as it was (the update runs without `sudo`): with
  firewalld or ufw on, TCP 4244 must be let in by hand once. A fresh
  install opens it.
- **Testing by hand found what tests did not** (2026-11-19, Windows 10 ↔
  Hyprland):
  - arboard (3.7) sets a file list on Windows without emptying the
    clipboard first, so over files the Explorer copied the write was
    ignored with no error: emptied first now;
  - received files were offered back to the PC they came from on every
    crossing, and the overlapping transfers made a paste come out as an
    older file: each PC remembers where received files came from, and an
    offer carries that origin (`FileOffer::origin`, #424);
  - every crossing made a new offer of the same files, which cut a
    transfer under way: the same files keep their offer.
- **Bring / Ignore** is a small window in the bottom-right corner, like a
  notice, sized to what it lists; where it appears is
  `[interface] bring_position`, in Settings > General too. Enter brings,
  Esc ignores, only there, never in pairing.
- **Progress** shows after two seconds, in the same corner: bar, how much
  of how much, speed and time left averaged over 5 s, the stage, Cancel;
  then Done with Open folder. The daemon writes a line ten times a second
  on the window's input (the channel #476 asked for).
- **The last four offers are served**, not the last one only: copying
  something else while files are still coming must not cut them short.
  Transfers from one PC take turns.
- **Windows' mark** says `HostUrl=mickey://<PC>` and `ZoneId=1`; the
  clipboard's own small files say `mickey://` alone, since the data link
  does not name the PC.
- **The margin is capped at 5 GB** (2026-11-19): "1 GB or 5% of the disk"
  meant 21 GB on the owner's 422 GB `/home`, and a 1.2 GB transfer was
  refused with 18 GB free. Now 1 GB or 5%, at most 5 GB. And a transfer
  that fails before its window opens (no room, the other PC out of reach)
  still shows the window, with why: it went to the log only.
