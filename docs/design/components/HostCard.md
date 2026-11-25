A host card is one saved host in the Hosts grid; selecting it fills the detail panel.

- Icon tile: a role icon in the group color on a 14% tint of that color, `radius-lg`.
- Name in `body-strong` at weight 600, then `user@address:port` in `mono-small`, truncated.
- Footer: the group (8px square and name) on the left; on the right "Connected" in `ink` with a solid dot, or the last connection date in `ink-muted`.
- A host with a Tailscale address shows a small "Tailscale" tag beside its name.
- Selected: `card-selected` background and an `accent` border. Hover: 1px lift with `shadow-hover`.

The card is a button. Minimum width 270px in an auto-fill grid with `space-3` gaps.
