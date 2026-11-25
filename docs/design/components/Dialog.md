A dialog interrupts to ask for a decision; it opens centred over a 60% black backdrop.

- Standard: `card` background, `line-strong` border, `radius-2xl`, `space-5` padding, `shadow-dialog`. Maximum width 460px.
- Warning: `warn-surface` background with a `warn` border and `warn-ink` body text. Use it when the action cannot be undone or carries a security risk.
- The title is `heading` and states the question or the subject. One line of explanation follows in `ink-muted`.
- Buttons are a single row at the bottom. In a standard dialog the primary button is on the right. In a warning dialog the safe choice comes first and the destructive fill is last.
- Clicking the backdrop or the close button cancels. A dialog never closes itself on a timer.

Technical values inside a dialog sit in a `ground` box in the mono family.
