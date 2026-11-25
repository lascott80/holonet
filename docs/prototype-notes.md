# Milestone 1: prototype notes

Results of the checks listed in `CLAUDE.md`. The working plan is `docs/milestone-1-plan.md`.

Machine: Snapdragon X2 Plus, Windows 11 ARM64, display scale 150%. Rust 1.100.0, `aarch64-pc-windows-msvc`.

## Spike 1: GPUI window on ARM64 (25 Nov 2026), passed

`crates/holonet-app` with `gpui` 0.2.2 from crates.io, default features.

| Check | Result |
| --- | --- |
| Builds natively for `aarch64-pc-windows-msvc` | Yes. First clean debug build about 2 minutes; incremental rebuild about 8 seconds. |
| Window opens and renders (DirectX 11) | Yes, with a native title bar and the design's dark surfaces. |
| Font discovery | `window.text_system().all_font_names()` finds the user-installed "JetBrainsMono Nerd Font Mono". |
| Cell width | 7.80px at 13px (logical pixels). |
| Nerd Font glyph widths | All probes measure exactly one cell: Latin, box drawing, Powerline arrows and rounded caps, folder, git branch, Linux and Rust icons. Expected for the "Mono" build. |
| Ligatures | **Not rendering.** `->`, `!=`, `===` and `<!--` draw as separate characters, although GPUI's DirectWrite backend enables `calt` by default (`platform/windows/direct_write.rs`). Open question for spike 3: the font build, a GPUI shaping detail, or both. The terminal grid draws cell by cell anyway, so ligatures there need runs to be shaped together. |

### GPUI API notes

- `Pixels` has a private inner field: use `f32::from(pixels)`.
- Font metrics: `text_system.resolve_font(&font(family))` gives a `FontId`; `advance(font_id, size, ch)` gives per-glyph widths. `resolve_font` panics only when no fallback resolves.
- No window title is set by default; set `WindowOptions.titlebar` later.
- Build warning from a dependency: `proc-macro-error2` 2.0.1 will be rejected by a future Rust version. It's harmless for now.
