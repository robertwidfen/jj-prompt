## What it does

Calls `jj log` and creates a prompt from the info.

Shows:
- working copy ID (@) state and stats
- offset and ID of conflicts: first (to @), previous/next to @ and last
- offset and ID w.r.to @-branch of last immutable (main) and last
- number of branches, merges and bookmarks

<img src="screenshot.png" alt="jj-prompt screenshot" width="400">

## Install

Add it to your starship config.

```toml
format = "[$directory${custom.jj-prompt}]($style)$character"

[custom.jj-prompt]
when = "jj root --ignore-working-copy --quiet"
format = "$output "
shell = "jj-prompt"
ignore_timeout = true
```
