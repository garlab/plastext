# Git status

Files in the navigation tree and tabs use the same cached Git status. Folders
summarize changes beneath them, including deleted files and changes in nested
repositories. A folder containing both staged and unstaged changes is purple;
conflicts take precedence. These summaries are built in the background, so
rendering a folder never scans its descendants.

Hovering a colored folder shows its changed-file count, deletion count and an
example path. A folder can correctly be colored while none of its listed
children are: the changes may be hidden by tree settings or deleted from disk.
Deleted files are included in Git summaries but are not inserted into the file tree.

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
colors immediately. Healthy watches keep these caches current without a scan
on expansion. If watch coverage is incomplete, selecting or expanding a
previously background repository queues revalidation when its last check is
older than five seconds. Caches live for
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
private Git directory and the shared repository directory. Positive discovery
results covered by healthy repository watches do not expire on a timer.

## Refresh cost

`plastext-git` uses `gix` with status support and no network backend. Git does
not need to be installed to run the application. Discovery, index loading,
status computation and snapshot comparison run off the UI thread. Rendering
only looks up cached status for visible tree rows and open tabs. Changed paths
and a bounded set of directories from Git's index are retained between scans;
the full index is released. Temporary index memory still scales with
the tracked-file count. Files that need content comparison incur background
I/O and hashing; native content conversions can also need temporary buffers.

Filesystem events are the primary update trigger. They are coalesced by
repository before reaching the UI, then batched for 300 ms. Only affected
repositories refresh, including linked worktrees sharing Git metadata. Tree expansion,
collapse and unchanged watch registration do not invalidate fresh status. Cached
discovery results are reused, and unchanged snapshots do not rebuild folder
summaries.

Events in directories absent from Git's index are filtered before delivery,
so activity deep in untracked build-output or dependency directories does not
trigger Git scans. Files alongside tracked files can still trigger a scan;
tracked files are never excluded just because their directory is named `target`
or `node_modules`. Git object writes, reflog writes and lock-file churn are
filtered too; index, HEAD, refs, configuration and attribute changes are retained.
The callback performs no filesystem inspection or Git work.

macOS and Windows use native recursive watches on repository roots and Git
metadata. Linux and other backends use nonrecursive watches on indexed
directories and a bounded traversal of Git refs, avoiding a recursive walk
through build outputs. Registration is batched on a worker and unchanged watches
are reused. Each window is limited to 4,096 native watch paths. The directory
filter is capped at 8,192 directories and 512 KiB of relative path bytes per
repository. Exceeding a limit or failing registration keeps fallback polling
enabled instead of claiming complete coverage.

Discovery and status scans run one at a time per window. Each completed
repository is published immediately. A batch stops after about 500 ms of
status work, finishing its current repository first. Both per-repository and
window-wide cooldowns are at least one second, rising to four times the work
duration (up to 60 seconds). This prevents many individually cheap repositories
from keeping storage continuously busy. Expanded or selected folders and open
files prioritize their owning repository. These scans keep a per-repository
cooldown capped at five seconds and bypass unrelated background cooldowns;
they run after the current scan finishes, before queued background work.

Healthy repositories have a ten-minute reconciliation check, with no routine
30-second/two-minute Git scan. Registration changes, dropped events, watcher
errors, window reactivation and detected sleep/resume gaps request reconciliation.
Registration is followed by a check to cover the gap after the initial status
snapshot. Inactive windows retain/coalesce events and defer Git work until
reactivated.

When watch coverage is incomplete or a status scan fails, fallback checks run every 30 seconds for
prioritized repositories and every two minutes for other retained caches,
subject to scan pacing. Watch setup is retried at most once a minute when idle.
Native watchers can silently miss events on network filesystems: the ten-minute
check and reactivation still reconcile these, and the existing open-document
metadata checks can request a Git refresh when they detect a change.

The previous snapshot remains visible while a refresh runs. The status bar
shows refresh state only after a batch has run for 250 ms, so short background
checks do not flash an indicator. Unavailable status is shown immediately.
Discovery and each repository scan have a ten-second deadline that requests
cooperative cancellation; the next scan
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

This follows the usual editor model of cached decorations, filesystem events,
debounced refreshes and bounded background work. VS Code's
[Git refresh implementation](https://github.com/microsoft/vscode/blob/main/extensions/git/src/repository.ts)
debounces filesystem events and throttles status; its
[repository model](https://github.com/microsoft/vscode/blob/main/extensions/git/src/model.ts)
also limits concurrent initial status requests. Zed's
[repository activation](https://zed.dev/docs/git#repository-activation) activates
repositories directly beneath a project container and defers deeper ones.
Notifications are treated as invalidations, not as a replacement for Git status:
each affected repository is rescanned in the background before publishing colors.

## Theme colors

`[theme.light]` and `[theme.dark]` accept `git_staged`, `git_modified`,
`git_mixed` and `git_conflict`. Imported VS Code themes supply staged/added,
modified and conflicting resource colors when available; explicit TOML
colors take precedence.

## Validation

### Diagnosing repeated refreshes

Quit the running application, then launch with Git tracing enabled:

```sh
PLASTEXT_GIT_TRACE=1 cargo run -p plastext-app -- ~/projects
```

The terminal prints the log path: `/tmp/plastext-git-<pid>-<timestamp>.log` on
Unix, or the system temporary directory on Windows. Restarting matters because
later launches normally forward their request to the already running process.
Logs record filesystem event kinds and paths, accepted/ignored events, watch
registration and failures, focus changes, scheduling causes, and each status
scan's duration and whether its result changed. Ignored events are counted and
sampled (three per second) so build output cannot crowd out scan diagnostics.
Use `PLASTEXT_GIT_TRACE=all` to include every raw event instead. Logs contain filenames and
repository paths, but no document contents or file diffs.

Logging is disabled by default. A background writer uses a bounded queue;
overflow is reported as `trace_dropped`, and each process stops logging at
8 MiB. The log stays outside the watched project and is never added to Git.
Each record starts with milliseconds since tracing began; the header includes
the process ID and Unix start time for correlating separate captures.

An independent, read-only event probe can observe a project container without
opening another application window:

```sh
PLASTEXT_GIT_TRACE=1 PLASTEXT_GIT_WATCH_ROOT=~/projects \
  PLASTEXT_GIT_WATCH_SECONDS=180 \
  cargo test --locked -p plastext-git watch_directory_events -- --ignored --nocapture
```

The probe reads repository indexes to configure the same event filter as the
application, watches for the requested period, and logs affected repositories.
It does not run status scans or change repository files. Native watcher
permissions are required, as for the integration tests below.

### Automated checks

Automated coverage includes staged/unstaged/mixed changes, unstage/commit,
unborn HEAD, deleted files, staged renames, intent-to-add, conflicts,
linked worktrees, cancellation and result limits. UI tests cover shared
tree/tab snapshots, folderless windows, parent folders containing sibling
repositories, immediate cached colors with zero scans during collapse/reopen,
targeted event refreshes, folder summaries, LRU eviction, and independent
recovery after a failed refresh.
Priority tests cover browsing ahead of background work, reuse of fresh caches,
and revalidation of older caches. Deletion tests cover a removed hidden directory
with no surviving child row, both before and after staging.
Native watcher tests cover collapsed descendants, atomic replacement, staging,
deletion and unavailable watches. Deterministic tests cover build-event filtering,
event storms/overflow, linked-worktree metadata, idle scan counts, inactive windows
and fallback deadlines. Native watcher tests require filesystem notification
permissions; run them outside a sandbox that blocks FSEvents.

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

After event-driven watching was added, the same diagnostic on 2026-11-08 took
224–240 ms (median 227 ms), including construction of the bounded directory
filter. This remains below 300 ms on this machine; it does not measure native
watch-registration latency or change the accepted M1/M2 baseline.

The separate folder-summary diagnostic uses 100,000 changed files across
1,000 directories, rebuilding the summaries five times:

```sh
cargo test --release --locked -p plastext-git folder_summary_100k_changes -- --ignored --nocapture
```

On the same machine and date, these rebuilds, including the tooltip counts and
example paths, took 115–117 ms (median 115 ms).
This measures aggregation after status has completed, not total refresh time.
Unchanged snapshots skip this work; folder rendering uses cached lookups.
