A tab is one open session; the tab strip sits in the title bar on `raised`.

- The first tab is pinned and icon-only. It opens the vault screens and is separated from session tabs by a divider.
- A session tab shows a type icon (terminal or folder), a state dot, the host name and a close button. Names truncate.
- The 2px top line is the host's group color: full strength on the active tab, 45% on the others.
- The active tab is `ground`, so it joins the page below. Inactive tabs are `card` and darken on hover.
- State dot: solid for connected, hollow ring for connecting, `warn` for a failed session.

Tabs are 38px high with `radius-sm` top corners. The close button is `control-compact`.
