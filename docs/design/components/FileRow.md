A file row is one file or folder in an SFTP pane.

- Columns: checkbox, icon and name, size, modified, and on the remote pane permissions. The header row uses the `label` style.
- Icons show the type: folder in `accent`, symlink in `group-violet`, scripts and code in `group-mint`, everything else in `ink-muted`.
- Rows are `control` high with a `line-soft` divider. A selected row is `card-selected`; hover is `card`.
- Clicking anywhere on the row toggles its checkbox. Rows can be dragged to the other pane, which then shows a dashed `accent` drop target.
- Selecting one or more rows shows an action bar above the list with the count and the actions that apply.

Size and permissions use `mono-small`. Names truncate with an ellipsis.
