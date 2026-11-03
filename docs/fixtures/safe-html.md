# Native HTML in Markdown

Normal **Markdown** with <b>bold <i>and italic</i></b> text,
H<sub>2</sub>O, x<sup>n + 1</sup>, <kbd>⌘C</kbd>, and
<mark>marked text that stays still when selected</mark>.

<a href="#disclosures" title="Jump within this document">Jump to disclosures</a>.

<p align="center">A centered paragraph<br>with a second line and &amp; entities.</p>

<div align="right">

Markdown inside an aligned container, including **strong text**.

</div>

## Disclosures

<details>
<summary>Open this <strong>section</strong></summary>

This body is ordinary Markdown. Find **hidden needle** to reveal it.

<details open>
<summary>A nested section, initially open</summary>

Nested content with H<sub>2</sub>O and <mark>highlighting</mark>.

</details>

The outer body continues here.

</details>

<details open>
<summary>An initially expanded section</summary>

Drag to select the body and copy it. Collapsing it omits its body from copy.

</details>

## Local images

<picture>
<source media="(prefers-color-scheme: dark)" srcset="sample.svg">
<source media="(prefers-color-scheme: light)" srcset="sample.svg">
<img src="sample.svg" width="240" align="center" alt="A local sample graphic">
</picture>

<hr>

## Inert source and blocked resources

The following elements stay visible as source; no scripts, styles, forms or
embedded pages run. The remote image shows a blocked placeholder.

<script>alert('This never runs');</script>

<style>@import url('https://example.invalid/style.css');</style>

<iframe src="https://example.invalid/frame"></iframe>

<img src="https://example.invalid/image.png" alt="Remote image">

<!-- This comment should not be visible in Preview. -->

End of fixture.
