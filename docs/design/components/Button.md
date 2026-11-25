A button starts an action; five kinds cover every case in the app.

- **Primary**: `accent` fill, `on-accent` text, weight 600. One per view: the action the screen exists for.
- **Secondary**: 1px `line-strong` border, `ink` text. Any other action.
- **Quiet**: no border, `ink-muted` text. Cancel and dismiss.
- **Destructive outline**: `warn-line` border, `warn` text. The entry point to a destructive action, such as "Delete host".
- **Destructive fill**: `warn` fill, `on-warn` text. Only as the confirming button inside a warning dialog.

Height is `control`; use `control-compact` inside toolbars and pane headers. Radius is `radius-md`, or `radius-lg` for the primary action in a page header. Labels are sentence case and name the action. A link styled as a button uses the same classes.
