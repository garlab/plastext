# Git status

Files in the navigation tree and tabs use the same cached Git status. Folders
summarize changes beneath them, including deleted files and changes in nested
repositories. A folder containing both staged and unstaged changes is purple;
conflicts take precedence. These summaries are built in the background, so
rendering a folder never scans its descendants.

| Default color | Meaning |
| --- | --- |
| Green | Staged changes, with no further working-tree changes |
| Amber | Unstaged working-tree changes |
| Purple | Staged changes plus further unstaged changes |
| Red | Unresolved merge conflict |
| Normal text | Clean, untracked, or outside the discovered repositories |

These colors describe files on disk and the Git index. The tab's unsaved dot
continues to describe the editor buffer. Colors follow saves, staging,
unstaging, commits and branch changes made outside Plastext. Plastext never
stages, commits or refreshes the on-disk index itself.

Repositories are discovered above the opened folder, listed child folders,
and the parents of saved files open in any pane. For example, opening a
`projects` directory probes the checkout folders listed beneath it before
they are expanded. Repositories containing open tabs are also tracked when
outside the opened folder.

Collapsing a folder retains its cached status. Reopening it shows the same
colors immediately, without requiring another status scan. Caches live for
the window's lifetime; when the 64-repository limit is reached, the least
recently used inactive repository is evicted first. Empty folderless windows
keep their cache but stop watching and scanning until a file is opened again.

Discovery does not recursively walk unopened descendants. Positive and
negative directory probes are cached for 60 seconds, with invalidation on
observed `.git` creation/removal. Multiple folders within one
repository share one status scan, while sibling checkouts, nested repositories,
expanded submodules and linked worktrees each get their own cache. The nearest
repository determines a file's color; a clean nested repository does not
inherit changes reported by its parent. Linked worktrees watch both their
private Git directory and the shared repository directory.

## Refresh cost

`plastext-git` uses `gix` with status support and no network backend. Git does
not need to be installed to run the application. Discovery, index loading,
status computation and snapshot comparison run off the UI thread. Rendering
only looks up cached status for visible tree rows and open tabs. Only changed
paths are retained between scans; temporary index memory still scales with
the tracked-file count. Files that need content comparison incur background
I/O and hashing; native content conversions can also need temporary buffers.

Filesystem events are coalesced for 300 ms and invalidate only affected
repositories, including linked worktrees sharing Git metadata. Tree expansion,
collapse and watcher registration do not invalidate fresh status. Cached
discovery results are reused, and unchanged snapshots do not rebuild folder
summaries.

Discovery and status scans run one at a time per window. Each completed
repository is published immediately. A batch stops after about 500 ms of
status work, finishing its current repository first. Both per-repository and
window-wide cooldowns are at least one second, rising to four times the work
duration (up to 60 seconds). This prevents many individually cheap repositories
from keeping storage continuously busy.

Watches cover expanded directories, open files, cached repository roots and
Git metadata. Fallback status checks run every 30 seconds for repositories
represented by listed folders or open tabs, and every two minutes for other
retained caches, subject to the scan pacing above. Thus changes deep inside
collapsed, unwatched folders can take longer to appear; their last known
colors remain available immediately.

The previous snapshot remains visible while a refresh runs. The status bar
shows refresh/unavailable state. Discovery and each repository scan have a
ten-second deadline that requests cooperative cancellation; the next scan
waits for the current one to finish. This is not
a hard process timeout: an individual filesystem read can take longer.
Discovery is capped at 4,096 directory probes and 64 active repositories.
Retained file results across the window are capped at 100,000 changed paths
and 16 MiB of path bytes. Folder summaries have a separate cap of 100,000
paths and 16 MiB of path bytes. A failed
repository scan retains its last complete result while other repositories
continue refreshing; partial scans never mark remaining files as clean.

To avoid expensive directory walks and similarity searches, this increment
does not enumerate untracked files, match renames by content, or recursively
inspect unopened submodules. A staged rename still colors its destination as staged.
Executable clean filters and external diff drivers are disabled in memory;
files that require a custom clean filter can appear modified when their raw
contents differ from the index. Native line-ending conversions remain active.
Repository configuration is not changed.

## Theme colors

`[theme.light]` and `[theme.dark]` accept `git_staged`, `git_modified`,
`git_mixed` and `git_conflict`. Imported VS Code themes supply staged/added,
modified and conflicting resource colors when available; explicit TOML
colors take precedence.

## Validation

Automated coverage includes staged/unstaged/mixed changes, unstage/commit,
unborn HEAD, deleted files, staged renames, intent-to-add, conflicts,
linked worktrees, cancellation and result limits. UI tests cover shared
tree/tab snapshots, folderless windows, parent folders containing sibling
repositories, immediate cached colors with zero scans during collapse/reopen,
targeted event refreshes, folder summaries, LRU eviction, and independent
recovery after a failed refresh.

Run the disposable 100,000-file diagnostic separately from UI benchmarks:

```sh
cargo test --release --locked -p plastext-git warm_status_100k_files -- --ignored --nocapture
```

It creates a temporary repository, warms status once, then reports five
scans with one modified file. This isolates status cost; it does not certify
frame latency, slow/network filesystems or M4's full exit criteria. Existing
M1/M2 performance baselines remain unchanged.

On an Apple M2 Pro with 16 GiB RAM, the release diagnostic's five warm scans
on 2026-11-07 took 220–253 ms (median 241 ms), below the specification's
300 ms target for warm status in a 100k-file repository. This is a local
measurement, not certification on the reference M1 Air.

The separate folder-summary diagnostic uses 100,000 changed files across
1,000 directories, rebuilding the summaries five times:

```sh
cargo test --release --locked -p plastext-git folder_summary_100k_changes -- --ignored --nocapture
```

On the same machine and date, these rebuilds took 97–99 ms (median 98 ms).
This measures aggregation after status has completed, not total refresh time.
Unchanged snapshots skip this work; folder rendering uses cached lookups.
