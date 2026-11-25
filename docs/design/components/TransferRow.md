A transfer row is one upload or download in the SFTP queue.

- Left: the file name, with the direction below it in `mono-small` ("This PC → nas-01").
- Middle: a 4px progress bar on a `line` track, then one status line in `meta`: percent, speed and time left.
- Bar color: `accent` for uploads, `group-violet` for downloads, `ink-faint` when paused or queued.
- A failed transfer has an empty bar and its reason in `warn`, with a Retry button.
- Right: one compact button for the state (Pause, Resume or Retry) and a remove button.

Rows sit in a `card` container with `radius-lg`. Finished transfers collapse behind "Show finished".
