# Gantt charts

## Release plan

```mermaid
gantt
    title Release plan
    dateFormat YYYY-MM-DD
    axisFormat %d %b
    excludes weekends
    todayMarker off
    section Preparation
        Design       :done, design, 2026-11-02, 3d
        Documentation:docs, 2026-11-03, 1w
    section Delivery
        Implementation:crit, active, build, after design, 1w
        Review        :review, after build docs, 2d
        Release       :milestone, after review, 0d
```

## Calendar dates and accented labels

> ```mermaid
> gantt
>     title Équipe — calendrier
>     axisFormat %d %b
>     section Préparation
>         Recherche :2024-02-27, 2024-03-01
>         Révision  :2d
>     section Livraison
>         Café      :crit, done, 2024-02-29, 1M
>         Prêt      :milestone, 0d
> ```

## Holidays and parallel work

```mermaid
gantt
    title Short working week
    excludes weekends, 2026-12-25
    excludes 2026-12-28
    axisFormat %d %b
    Draft  :Draft, 2026-12-23, 2d
    Check  :Check, 2026-12-24, 1d
    Finish :after Draft Check, 1d
```

## Source fallbacks

Unsupported timezone formats retain their complete source:

```mermaid
gantt
    dateFormat YYYY-MM-DDTHH:mm:ssZ
    Meeting :2026-11-05T09:00:00Z, 30m
```

Unknown dependencies also retain source:

```mermaid
gantt
    Review :after missing, 2d
```
