# Holonet

Holonet is an SSH and SFTP client for Windows on ARM, written in Rust. This system records how its screens are drawn so that new screens, and the app itself, use one set of values.

It was built from the Holonet design canvas (17 screens). Values are the ones used there. No logo file exists yet: the mark is a globe drawn as a 2px stroke icon in `accent`, next to the name in the display face.

## Voice

- Plain and specific. Say what happened and what the person can do next: "pi-cluster-1 did not answer" and "Try Tailscale", not "Connection error".
- Sentence case everywhere, including buttons and titles. Uppercase is only the `label` style.
- A button says what it does: "Save and connect", "Trust and connect", "Delete 2 items". Never "OK" or "Submit".
- Security prompts explain the risk in one sentence and make the safe choice the main button.
- Technical values (addresses, paths, fingerprints, commands) are shown exactly, in the mono family.

## Surfaces

The app is dark and neutral gray, with no tint. Depth comes from five steps of gray, not from borders alone.

| Level | Token | Used for |
| --- | --- | --- |
| Lowest | `sunken` | Gutters around panes, code boxes |
| Page | `ground` | Page background, active tab |
| Panel | `panel` | Navigation, detail panels, pane headers |
| Card | `card` | Cards, fields, dialogs, inactive tabs |
| Top | `raised` | Title bar, active navigation item |

The title bar is the lightest surface so that tabs read as cut into it. Borders are `line`; use `line-strong` only where a control needs to stand out on `card`. Right-hand panels cast `shadow-panel`; dialogs sit on a 60% black backdrop with `shadow-dialog`.

## Color

- `accent` is the one interactive color: primary buttons, selected borders, links, focus.
- Group colors (`accent`, `group-violet`, `group-mint`, `group-gray`) identify which group a host belongs to. They appear as an 8px square beside a host name, a tinted icon tile, a 2px line on the top of a tab, and a 2px top edge on terminal and SFTP panes. A group color is never the only signal: the group name is shown wherever there is room.
- `warn` is reserved for warnings and destructive actions. Do not use it for decoration, categories or progress.
- Connected state is a solid `ink-soft` dot; connecting is a hollow ring. State is not shown with a group color.

## Type

Three families, all loaded from Google Fonts: Space Grotesk for titles (`display`), Inter for everything else (`sans`), JetBrains Mono for terminal text and technical values (`mono`). The terminal itself defaults to JetBrainsMono Nerd Font, which the app bundles; the Nerd Font files are not part of this system.

Use `title` once per screen. Section labels and table headers use `label`, uppercase. Secondary lines use `meta` in `ink-muted`.

## Layout

- Controls are `control` (44px) high. Inside the title bar, toolbars and pane headers they are `control-compact` (36px).
- Page content has `space-6` side padding and `space-5` between sections. Cards have `space-4` padding and `space-3` between them.
- A management screen is three columns: navigation, content, and a detail panel about 320px wide. On a narrow window the columns stack in that order.
- Session screens (terminal, SFTP) replace the navigation with a 56px icon rail to give the panes the width.

## Icons

Icons are inline stroke drawings on a 24px grid, 1.7 to 2px stroke, round caps and joins, colored with `currentColor`. Hosts use role icons (server, database, bastion, storage, single-board computer, cloud server) on a tile tinted with the group color at 14%. Distribution and vendor logos are not used.

## Components

Previews and guidelines are in this system for Button, Tab, Host card, Field, Tag, Dialog, Terminal text, File row, Transfer row, Command palette and Setting row. They are reference drawings in plain HTML and CSS: there is no component bundle to load, so build them from the tokens as each guideline describes.

## Using this system on the design canvas

The Holonet canvas carries a copy of `tokens.json` and a `tokens.css` generated from it, and every screen reads its colors from those variables. After changing a token here, the canvas copy has to be refreshed before the screens change. Tints of a group color (the 14% icon tile, the 45% inactive tab line) are written as fixed `rgba()` values and do not follow a token change.
