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

## Source fallback

Diagram types outside the initial native subset retain their complete code:

```mermaid
pie title Contents
    "Prose" : 60
    "Code" : 25
    "Math" : 15
```

ER attribute tables also retain their source in this version:

```mermaid
erDiagram
    BLOCK {
        string kind
        int position
    }
```

Use **Source** or **Preview** above a diagram to switch modes. **Copy** copies the original fence. Search for a label with `⌘F` to reveal its source.
