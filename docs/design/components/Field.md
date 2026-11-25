A field is a labelled input, select or checkbox in a form.

- The label sits above the control in `meta`, `ink-muted`. Mark a required field in the label text: "Primary address (required)".
- Controls are `control` high on `card`, with a `line` border and `radius-md`. Focus changes the border to `accent`.
- Fields that hold an address, port, path or command use the mono family.
- A checkbox has its label to the right in `body`, and the whole row is at least `control` high.
- Help and validation text goes below the control in `meta`; a problem uses `warn`.

Lay fields out in an auto-fit grid with `space-4` gaps so they stack on a narrow window.
