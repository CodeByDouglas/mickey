# What's new in Mickey

What changed for you in each version, newest first. Mickey shows the part
for its own version once, the first time it starts after an update.

Writing an entry: one line per change people will notice, in plain words
("Taps on the touchpad click on the other PC"), under `## Unreleased`. The
release renames that heading to the version. Internal work (refactors, CI,
dependencies) stays out unless it fixes something people saw. Plain text
only: no arrows or emoji, which Mickey's font may not have.

## Unreleased

- Taps and two-finger scrolling on a laptop's touchpad now work while it
  controls another PC, following GNOME's, KDE's or Hyprland's touchpad
  settings. Settings > This PC sets tap to click and natural scrolling by
  hand. Thanks to @bigptlg.
- Mickey speaks your language: English, Brazilian Portuguese and European
  Portuguese, following the system or picked in Settings > General.
- Screen edges work on COSMIC (Pop!_OS 24.04) in both directions, and on
  other desktops without the InputCapture portal.
- The clipboard can be kept on each PC: Settings > General > Share the
  clipboard > Never.
- Pairing says why it did not work, and a PC is remembered only once the
  other one accepts it.
- A PC that stops answering no longer holds the keyboard and mouse on the
  PC it was controlling.
- On Windows, the installer says so when Defender removes Mickey right
  after installing it.
