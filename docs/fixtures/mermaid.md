# Mermaid diagrams

Flowcharts use native boxes, labels, and arrows:

```mermaid
flowchart LR
    A[Read Markdown] --> B{Diagram?}
    B -->|Yes| C[Render]
    B -->|No| D[Keep text]
    C --> E[Preview]
    D --> E
```

## Sequence

```mermaid
sequenceDiagram
    participant Reader
    participant Viewer
    Reader->>Viewer: Open file
    activate Viewer
    Viewer-->>Reader: Show document
    deactivate Viewer
```

## Class

```mermaid
classDiagram
    class Document {
        +String source
        +render()
    }
    Document --> Renderer
```

## State

```mermaid
stateDiagram-v2
    [*] --> Reading
    Reading --> Rendering
    Rendering --> [*]
```

## Relationships

```mermaid
erDiagram
    DOCUMENT ||--o{ BLOCK : contains
```

## Grouping

> ```mermaid
> flowchart TD
>     subgraph Input
>     A[Markdown] --> B[Parser]
>     end
>     subgraph Output
>     B --> C[Text]
>     B --> D[Images]
>     end
> ```

## Unicode labels

```mermaid
flowchart LR
    A[Café déjà vu] --> B[Ελληνικά Русский]
```

## Attribute tables

```mermaid
erDiagram
    BOOK {
        int identifier PK,FK,UK
        varchar title "display name"
        decimal price
    }
    BOOK ||--o{ PAGE : contains
    PAGE {
        int identifier PK
        int book_id FK
    }
```

## Pie chart

```mermaid
pie showData
    title Contents
    "Prose" : 60
    "Code" : 25
    "Math" : 15
```

## Mind map

```mermaid
mindmap
  root((Reading))
    Notes
      Ideas
    Diagrams
```

## Timeline

```mermaid
timeline
    title History
    2020 : Launch : Learn
    2021 : Grow
```

## User journey

```mermaid
journey
    title Reading
    section Start
    Open file: 5: Reader
    Read: 4: Reader, Editor
```

## Quadrants

```mermaid
quadrantChart
    title Work
    x-axis Low effort --> High effort
    y-axis Low value --> High value
    Task A: [0.2, 0.8]
    Task B: [0.8, 0.3]
```

## Source fallback

Unsupported families and unavailable glyphs retain complete source:

```mermaid
gantt
    title Release
    Work: 2026-01-01, 1d
```

```mermaid
flowchart LR
    A[中文] --> B[Text fallback]
```
