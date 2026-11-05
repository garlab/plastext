# Gantt schedules with times

## Hours and minutes

```mermaid
gantt
    title Morning release
    dateFormat HH:mm
    axisFormat %H:%M
    section Preparation
        Build  :done, build, 09:00, 1.5h
        Notes  :active, notes, 09:15, 10:00
    section Delivery
        Review :crit, after build notes, 30m
        Ship   :milestone, 0ms
```

## Across midnight

> ```mermaid
> gantt
>     title Overnight run
>     dateFormat HH:mm
>     Prepare :a, 23:30, 45m
>     Check   :after a, 30m
>     Finish  :milestone, 0ms
> ```

## Dated timestamps and calendar exclusions

```mermaid
gantt
    title Weekend schedule
    dateFormat YYYY-MM-DD HH:mm
    axisFormat %d %b %H:%M
    excludes weekends
    Build :build, 2026-11-06 16:30, 25h
    Check :after build, 30m
    Ready :milestone, 0ms
```

## Millisecond intervals

```mermaid
gantt
    title Processing stages
    dateFormat HH:mm:ss.SSS
    axisFormat %S.%L
    Read    :read, 12:00:00.000, 10ms
    Decode  :decode, after read, 25ms
    Display :after decode, 15ms
```

## Source fallbacks

Timezone syntax and calendar-dependent clock-only schedules retain source:

```mermaid
gantt
    dateFormat YYYY-MM-DDTHH:mm:ssZ
    Zoned task :2026-11-05T09:00:00Z, 1h
```

```mermaid
gantt
    dateFormat HH:mm
    excludes weekends
    Undated calendar :09:00, 1h
```
