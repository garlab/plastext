+++
title = "TOML front matter"
tags = ["native", "markdown"]
published = false

[author]
name = "Ada"
description = "A reader of café notes and 🦀 code."

[[links]]
label = "Markdown demo"
path = "m2.md"

[[links]]
label = "Specification"
path = "../specs.md"
+++

# TOML front matter

Expand **Front matter** to see each field in source order. Nested table keys use
paths such as `author.name`; array entries use `links[0].label`.

Find “café” and press Enter to reveal the matching field. Toggle Source with
`⇧⌘V` to inspect the original TOML, then toggle back to return to the same field.

[Back to the Markdown demo](m2.md).
