# Git status

Filenames in the navigation tree and tabs use the same cached Git status:

| Default color | Meaning |
| --- | --- |
| Green | Staged changes, with no further working-tree changes |
| Amber | Unstaged working-tree changes |
| Purple | Staged changes plus further unstaged changes |
| Red | Unresolved merge conflict |
| Normal text | Clean, untracked, or outside the current repository |

These colors describe files on disk and the Git index. The tab's unsaved dot
continues to describe the editor buffer. Colors follow saves, staging,
unstaging, commits and branch changes made outside Plastext. Plastext never
stages, commits or refreshes the on-disk index itself.

The repository is discovered above the opened folder. A folderless window
uses the active saved file's repository. Linked worktrees watch both their
private Git directory and the shared repository directory. This increment
tracks one repository per window; nested repositories and submodules are not
aggregated, and tabs outside that repository stay neutral.

## Refresh cost

`plastext-git` uses `gix` with status support and no network backend. Git does
not need to be installed to run the application. Discovery, index loading,
status computation and snapshot comparison run off the UI thread. Rendering
only looks up cached status for visible tree rows and open tabs. Only changed
paths are retained between scans; temporary index memory still scales with
the tracked-file count. Files that need content comparison incur background
I/O and hashing; native content conversions can also need temporary buffers.

Refreshes are coalesced for 300 ms and run one at a time. After each scan there
is a cooldown of at least one second, rising to four times the scan duration
(up to 60 seconds) for slow repositories. Existing watches cover expanded
directories, open files and Git metadata. A fallback scan ten seconds after
the cooldown catches changes in collapsed directories and missed events.

The previous snapshot remains visible while a refresh runs. The status bar
shows refresh/unavailable state. A ten-second deadline requests cooperative
cancellation; the next scan waits for the current one to finish. This is not
a hard process timeout: an individual filesystem read can take longer.
Results are capped at 100,000 changed paths and 16 MiB of path bytes. Errors,
cancellation and exceeded limits retain the last complete result; partial
scans never mark the remaining files as clean.

To avoid expensive directory walks and similarity searches, this increment
does not enumerate untracked files, match renames by content, or inspect
submodule contents. A staged rename still colors its destination as staged.
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
tree/tab snapshots, folderless windows and retention after a failed refresh.

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
