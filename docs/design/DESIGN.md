---
name: Pantau
description: A macOS menu bar panel that names what is using your laptop and lets you act on it.
---

# Design System: Pantau

## Overview

**Creative North Star: "The Control Center Sibling"**

Pantau should read as a panel macOS could have shipped: opened for a few seconds, read at a glance, then gone. The direction is called "Kaca macOS" (macOS glass).

The surface is calm until something needs attention. Glass and neutrals carry the panel. Colour appears only for state, so a colour on screen always means something.

**Key Characteristics:**

- Translucent panel over the desktop, following the macOS light or dark appearance live.
- Dense but readable rows, with numbers as the main content.
- One destructive colour, used only for kill.
- Real app icons, never decorative illustration.

## Colors

A Restrained strategy: neutrals plus state colours. Light muted inks and the banner text are darkened to reach 4.5:1 for 11px text on the light glass.

### Primary

- **System Blue:** CPU graph, disk bar, and focus. `#2f6fe0` in light, `#7fb0ff` in dark.

### Secondary

- **Warning Amber:** RAM graph and memory pressure banner. Graph `#d9861a` in light, `#f0b24a` in dark. Banner text `#6b3a00` in light, `#f5bd62` in dark.
- **Kill Red:** kill buttons only. `#d6372c` in light, `#c43a2a` in dark.

### Neutral

- **Ink:** primary text. `#1c1e24` in light, `#f2f3f6` in dark.
- **Muted Ink:** labels and sublines. `#373c48` and `#3a3f4b` in light, `#bcc0ca` and `#a6abb6` in dark.
- **Hairline and Hover:** dividers and row hover. Ink at 9% and 6% in light, white at 10% and 8% in dark.

### Named Rules

**The Colour Means State Rule.** Colour marks a metric or a state that needs attention. It never decorates.

**The Red Is Kill Rule.** Red appears only on an action that ends a process. Cleanup actions stay neutral.

## Typography

**Font:** the macOS system stack (`-apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif`). No web font: it is the native face, and it costs nothing to load in a webview that is created on every open.

**Character:** native and quiet. Hierarchy comes from size and weight.

### Hierarchy

- **Metric** (600, 20px): the CPU, RAM, and Disk percentages.
- **Row title** (550, 13px): process group or storage item names.
- **Body** (400 to 500, 12.5 to 13px): amounts, buttons, and tabs.
- **Subline** (400, 11px): counts, projects, paths, and timestamps. 11px is the floor.

### Named Rules

**The Tabular Numbers Rule.** Every number uses `font-variant-numeric: tabular-nums`, because the values update every 2 seconds and must not jitter.

## Layout

A single column 360px wide, top to bottom: pressure banner (only when needed), three equal meters, Proses and Penyimpanan tabs, tab content, footer. The list scrolls inside the panel; banner, meters, tabs, and footer stay fixed. Horizontal padding is 14px, with list rows inset 6px so their hover shape sits slightly wider than the text column.

## Elevation & Depth

One lifted surface, the panel itself, over a live desktop. Inside it, depth comes from tonal fills rather than shadows. When the system asks for reduced transparency, the panel becomes solid at the same tone.

## Shapes

Gently curved, following macOS: panel 16px, rows and inset cards 8 to 9px, buttons and segmented controls 6 to 7px, meter tracks fully round.

## Do's and Don'ts

### Do:

- **Do** follow the macOS appearance live, without a restart.
- **Do** use real app icons from the bundle for app rows.
- **Do** honour reduced motion and reduced transparency.
- **Do** make every action reachable by keyboard, with a visible focus ring.

### Don't:

- **Don't** use coloured letter tiles in place of real app icons.
- **Don't** add a theme switch inside Pantau. It follows the system.
- **Don't** word a guess as a fact, in the UI or in a label such as "downloaded by AI".
