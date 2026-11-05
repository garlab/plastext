# Go records in Markdown

An `sgf` fence shows the first game's final main-line position, including setup
stones and annotations:

```sgf
(;FF[4]GM[1]SZ[9]GN[Corner study]
AB[cc][dc]AW[cd]
;B[dd];W[ed]
(;B[ec]TR[ec]LB[dd:A])
(;B[ce]))
```

## Earlier position

```sgf move=1
(;FF[4]GM[1]SZ[9]AB[cc][dc]AW[cd];B[dd];W[ed];B[ec])
```

## Setup and annotations

> ```sgf move=0
> (;FF[4]GM[1]SZ[9]AB[cc][dd]AW[cd][dc]TR[cc]SQ[dc]LB[dd:A]AR[aa:cc])
> ```

## Collections

```sgf game=2 move=last
(;FF[4]GM[1]SZ[9];B[cc])
(;FF[4]GM[1]SZ[9];B[dd];W[ff])
```

## Source fallback

Out-of-range selectors and invalid records retain their source:

```sgf move=99
(;FF[4]GM[1]SZ[9];B[cc])
```

```sgf
This is not an SGF record.
```

Each fence has **Source / Preview** and **Copy** controls. Find also reveals
matching source. Open an `.sgf` file for interactive move and variation navigation.
