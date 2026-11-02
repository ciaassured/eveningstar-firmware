# Storage view: what is left

Status: the API is implemented; the web page is being redesigned as "The
storage page" below describes. The page as implemented before the redesign
(one scrolling page with a flash map, a region table, firmware cards and a boot
chain panel) is what `docs/architecture.md` describes ("Binary bodies",
"Downloads", "Checks and the storage service", "Downloads of public regions"),
with the user's review of it in the pull request that introduced it (#52). The endpoints are
documented in the OpenAPI document (`nix build .#openapi`). This file keeps
what those don't: the decisions made with the user, the design of the
redesigned page, the work that builds on the view, and what hasn't been
verified.

> **The split between sensitive and public handles is what keeps secrets on
> the device.** Every byte the API sends, and every digest it reports, comes
> from a handle that implements `Read`. Don't add a route from a
> `SensitiveRead`-only handle to a response, and don't relax a trust tier or a
> protected region, without asking the user.

## Decided with the user

- The storage API only reads, and needs the `read` scope. Wiping comes later
  (below).
- `/storage` is an index of the board's storage. The external NAND is listed
  as `not_supported`; later firmware may present it as several resources
  under `/storage/external`.
- Regions are named by their role this boot (`app_current`, `app_alternate`,
  `config_current`, `config_alternate`), with the slot reported beside them.
  `factory` is sensitive.
- A public region downloads whole or by HTTP range, and the web UI resumes
  broken downloads with ranges. The web UI offers no range download of its
  own: a part of a region is an API client's job.
- App images are verified and hashed in the background after boot, never on
  request; no region is hashed whole. A write makes results stale through the
  write generations in `InternalFlash`.
- Long jobs (checks, downloads, later uploads and wipes) each have a watchdog
  handle fed on progress, and yield often. The HTTP workers are left to
  picoserve's timeouts.
- The web UI verifies downloads only in a secure context, with
  `crypto.subtle`; there is no SHA-256 of its own. It queues downloads in an
  app-wide service and fetches each into memory before saving it.
- The storage page is a single page: a flash strip, a region list and a region
  inspector, with its Refresh button in the shell's header. The boot chain and the firmware cards of the earlier
  page, and the Boot chain and Updates pages of the design prototypes, are not
  part of it.

## The storage page

This section specifies the redesigned page completely. It was approved as an
HTML prototype; everything the prototype shows is restated here, so the page
can be built without it. Where the prototype and the API disagree, or the
prototype leaves a case open, the rule here is the one to build, and
"Decisions beyond the prototype" lists each such rule.

### Overview

The page shows the board's internal flash as a set of **regions** in flash
order: the bootloader, the partition table, each partition of the expected
layout, the unallocated gaps between them, and any partition the board's table
has that the expected layout doesn't. A gap is at least one whole 4 KiB flash
sector or crosses a sector boundary; space that lies inside one sector without
filling it (the 1 KiB from 0x008c00 to 0x009000 after the 0xC00-byte partition
table) is the remainder of the sector of the region beside it, and is neither
a list item nor a strip block. Every region answers one question first:
is it as the firmware expects? The answer is a status word, an icon and a
colour, with the reason one click away.

On a desktop the page is, top to bottom: the flash strip with
its readout line, then two panes side by side: the region list (master) on the
left and the region inspector (detail) on the right. On a phone the inspector
slides over the list.

The demo controls of the prototype (the yellow, rotated `demo ▾` chip in the
title bar and its popover with the scenario selector, the "Contents in the
flash map" selector, "Make the next download fail" and "Reset the demo") are
not part of the product. The Dashboard and System placeholder pages are not
either.

### Frame and scrolling

- The app is a fixed frame of the viewport height (`height: 100%` on `html`
  and `body`, never `100vh`). The body never scrolls (`overflow: hidden`).
- The frame is a column: the header (52 px tall on a desktop, 50 px on a
  phone, `--header-bg`, a 1 px `--header-line` bottom border, above everything
  but popovers and the tray), then the main area, then on a phone the bottom
  navigation bar. The header and the navigation never scroll.
- The header and navigation belong to the app shell. What this page needs of
  them: the header carries the Downloads button at its right end ("Downloads
  button and tray"); the navigation has Dashboard, System and Storage, with
  Storage current; on a phone the navigation is a bottom bar (icons of 20 px
  over 11 px labels, the current one `--fg` and 600) and the header hides the
  wordmark, the top navigation and the Downloads label.
- The storage page fills the main area as a column: the strip band and the
  panes. The strip band never scrolls. Only the panes scroll:
  the region list and the inspector each scroll on their own, vertically, with
  `overscroll-behavior: contain`. Nothing scrolls horizontally at any width;
  the otadata table alone may scroll sideways inside its own container.
- Side gutters are `max(16px, env(safe-area-inset-left/right))`, set on the
  strip band and the header. The list pane adds the left safe
  area inset, the inspector pane the right one.

Breakpoints:

| Width          | Layout                                                                                                                                                                          |
|----------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| ≥ 1100 px      | Panes in a grid of `minmax(280px, 340px)` (list) and `minmax(0, 1fr)` (inspector). The inspector's Expected state and Details panels sit side by side when both show.            |
| 760 to 1099 px | List column `minmax(250px, 290px)`. The inspector's panels stack in one column. The header hides the device's IP address.                                                       |
| ≤ 759 px       | Phone: one pane at a time, the inspector sliding over the list; the phone strip; the bottom navigation. The switch happens at 760 px and the page re-renders when it crosses it. |

### Title bar

The page has no bar of its own: the strip is the top of the page, directly
under the shell's header. The page's heading, "Storage", is an `h1` that is
visually hidden.

The Refresh button is in the shell's header while the Storage page is shown,
between the "Operational" badge and the Downloads button: a pill 34 px tall in
the style of the Downloads button (padding 5 px 12 px 5 px 10 px, a 1 px
`--header-line` border, `--header-fg` text, `--header-muted` border on hover),
holding the 14 px reload icon (two arrows on a circle, 1.8 px stroke) and the
label "Refresh" (13 px; hidden on a phone, where the pill is the icon alone).
While the reload runs, the label reads "Refreshing…", the button is disabled
with `--header-muted` text and the icon turns once every 0.9 s (still with
`prefers-reduced-motion`). Its accessible name is the label.

The refresh failure note is the last line of the region list's summary,
under the other lines: a warning icon and "Not refreshed: `<why>`. Values from
`<time>`." (see "Decisions beyond the prototype"). The prototype has neither the
button nor the note; its demo chip sat in the title bar.

### The flash strip

The strip band is `--surface` with a 1 px `--border` bottom border, padding
10 px top and 6 px bottom (8 px and 4 px on a phone), above the panes
(`z-index` 2). It holds the strip and, under it, the readout line.

#### Blocks

- The strip is one row of blocks (a flex row, gap 2 px, 42 px tall; on a phone
  30 px tall with a 1 px gap), one block per region and gap, in offset order
  ("Ordering" under "Overlaps and shifted entries" settles ties). It is a
  `role="group"` with the label "Flash map, 8 MiB, in flash order" (the size
  is the flash's, formatted as below).
- Each block is a button with the accessible name `<title>, <subtitle>.
  <start> to <end>, <size>. <status word>.`, for example "ota_0, Running app.
  0x010000 to 0x310000, 3 MiB. Expected." Clicking it selects the region.
- Width: each block's `flex-grow` is `6 + (100 − 6n) × size / flash size`,
  with `n` the number of blocks and `flex-basis` 0, `min-width` 0. Every block
  so gets the same minimum share and the rest is shared in proportion to size.
  When `6n` exceeds 60 (more than 10 blocks of minimum share would leave less
  than 40 % to proportion), the minimum becomes `60 / n` in place of 6. The
  weights are relative, so overlapping entries, whose sizes add up to more
  than the flash, still fill exactly the strip's width.
- Shape: radius 3 px, `overflow: hidden`, a size container
  (`container-type: inline-size`), opacity transition 0.12 s.
- Default look: background `--t` (the region's tint) and a 3 px bar of the
  region colour `--c` along the bottom edge (`inset 0 -3px 0 var(--c)`).

#### Code, name and icons

- The code (for example `A0`) sits at the top left (4 px, 4 px) in the
  monospace font, 700, 10 px, line height 1.25, colour `--ink`, with 2 px of
  bottom padding. On a phone it sits 3 px from the left, centred vertically,
  at 9.5 px.
- The name (the region's title, for example "ota_0") follows the code after a
  5 px gap, in the UI font, 500, 11 px, line height 1, colour `--fg`, cut with
  an ellipsis. It shows only when the block is at least 84 px wide (a
  container query on the block).
- The icons sit at the top right (3 px from the top and the right), 12 px
  each, 3 px apart, laid out right to left: the lock (sensitive regions,
  colour `--ink`) is the rightmost, the warning triangle (any status other
  than ok, colour `--x-warn`) to its left. A sensitive region that is not as
  expected so shows the warning, then the lock, at the right edge.
- The code and name never run under the icons: their box is at most
  `100% − 8px − (number of icons) × 15px` wide (on a phone, where the lock
  isn't drawn, `100% − 6px − (warning ? 15px : 0)`), and the name is cut
  first.

#### Usage bar

The product draws a region's used part as a half-height inset bar; the
prototype's other modes ("Full height, tinted" and "Off") are not part of the
product.

- The track is inset 4 px from the left and the right and 6 px from the bottom
  edge, 13 px tall, radius 2 px, filled with `color-mix(in oklab,
  var(--surface) 55%, transparent)` and ringed by an inset 1 px line of
  `color-mix(in oklab, var(--c) 55%, transparent)`.
- The used bar starts at the track's left edge, fills the used share of the
  region (to scale), radius 2 px, in solid `--c` with an inset 1 px
  `--surface` ring.
- Used share per region: an app slot, the image size over the slot size; the
  bootloader, the bootloader image size over the region size; the partition
  table, the entries plus the MD5 entry over the region size; the otadata,
  both sectors (100 %). Other regions, gaps, missing and extra regions, and
  regions with no image draw no track.
- Hovering a part row of the inspector's span table (for example "Image")
  rings this bar with an inset 2 px `--fg` line (see "Hover and focus
  emphasis").
- On a phone no usage bar is drawn.

#### Textures

The texture of a block, the list swatch and the inspector ruler is the same
for a region:

| Kind                                 | Look                                                                                                                                                                                                                         |
|--------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Public region                        | Flat tint `--t`, bottom bar `--c`.                                                                                                                                                                                           |
| Sensitive region (hatched)           | `repeating-linear-gradient(135deg, var(--t) 0 3px, var(--t2) 3px 5px)`, bottom bar `--c`, and the lock icon.                                                                                                                 |
| Reserved region (dotted)             | Dots: `radial-gradient(circle, var(--t2) 1.1px, transparent 1.5px)` on a 5 px grid over `--surface`. On a block: bottom bar `--c` plus an inset 1 px ring of `--c` at 30 %; elsewhere an inset 1 px ring of `--c` at 45 %. |
| Unallocated gap (dashed)             | Transparent, no shadow, a 1 px dashed `--faint` outline inset by 1 px. No code: the list and the inspector swatch show an en dash, in `--muted`.                                                                              |
| Missing region (dashed outline)      | Transparent, no shadow, a 1.5 px dashed `--x-warn` outline inset by 1.5 px. The code and name are struck through.                                                                                                          |
| Extra entry (tinted, with a plus)    | Background `color-mix(in oklab, var(--x-warn) 14%, var(--surface))`, an inset 1.5 px `--x-warn` ring, the code `+`. Its colour `--c` is `--x-warn`.                                                                       |

#### Selection, hover and dimming

- The selected block (`aria-current="true"`) has an inset 2 px `--fg` ring
  above its bottom bar and is raised (`z-index` 1). A selected gap shows a
  2 px solid `--fg` outline in place of its dashed one. A selected missing or
  extra block, whose own outline or ring replaces the shadow, gets a 2 px
  solid `--fg` outline inset by 2 px.
- While a region is hovered or focused anywhere on the page, every other block
  drops to opacity 0.35 and the highlighted block gets an inset 2 px ring of
  its own colour `--c` above its bottom bar.

#### Readout line

Under the strip, 5 px below it: a row 20 px tall at least, 11.5 px, `--muted`,
gap 8 px:

- left: `0x000000` in the monospace font with tabular figures;
- centre (fills the rest, centred, `--fg`, one line, cut with an ellipsis):
  `<b>title</b> subtitle: <mono>start</mono> to <mono>end</mono>, size`, for
  example "**ota_0** Running app: 0x010000 to 0x310000, 3 MiB". It names the
  hovered or focused region, else the selected one. On a phone the offsets
  are left out: "**ota_0** Running app, 3 MiB".
- the help button (`?`, label "How is the flash map drawn?");
- right: `0x800000, 8 MiB` (the flash's end and size) in the monospace font.

On a phone both ends are hidden; the readout and the help button stay.

#### Help popover

| Title         | Text                                                                                                                                                                                                                                                                                                             |
|---------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| The flash map | The flash map shows every region and gap in flash order, from offset 0x000000 to the end of the 8 MiB flash. Small regions are drawn wider than their size to remain visible. Within a region, the image is drawn to scale. The ruler in the inspector is drawn to scale. Overlapping entries are drawn side by side, in offset order. |

The last sentence is new ("Overlaps and shifted entries"). The flash size in
the text is the flash's.

#### Phone strip

On a phone the strip is 30 px tall with 1 px gaps, and shows codes only (names
appear only in a block 84 px wide or more, which a phone rarely gives): no
usage bars, no lock icons, the warning icon only, no offsets in the readout
and no end labels.

### The region list

The list pane is `--surface` with a 1 px `--border` right border.

#### Summary header

A block padded 14 px 16 px 10 px, with a 1 px `--border` bottom border, its
lines 6 px apart:

1. "8 MiB flash" (the flash size, formatted as below, then " flash"), 15 px,
   bold.
2. The status line, 13 px: an icon (16 px, 2 px from the top) and the text in
   `--fg`; the icon takes the level's colour. The wording:

| State                                                    | Level | Text                                                                                                                         |
|----------------------------------------------------------|-------|------------------------------------------------------------------------------------------------------------------------------|
| Every region as expected, full tier                      | ok    | Everything matches the expected layout.                                                                                      |
| The same, and an update is activated in the other slot   | ok    | Everything matches the expected layout. An OTA update in `<label>` is selected for the next boot.                          |
| Every region as expected, tier below full                | warn  | Everything matches the expected layout. `<consequence>`                                                                     |
| One region not as expected                               | warn  | 1 item is not as expected: `<title>`. `<consequence>`                                                                       |
| Several                                                  | warn  | `<n>` items are not as expected: `<title>`, `<title>`, `<title>`. `<consequence>`                                           |
| The board's table is not known                           | warn  | The partition table the bootloader read is not known. `<search sentence>` `<consequence>`                                   |

   `<label>` is the selected slot's label (for example "ota_1"). An update is
   activated when the slot the otadata selects isn't the running one and the
   selecting record's state is `new` ("Fallback"). The titles are listed in
   offset order, joined with ", ". `<consequence>` is the tier sentence ("Tiers
   and limits") and is left out in the full tier. The search sentence is in
   "Several tables or no table".
3. One line per app slot whose image fails verification (`image.status`
   `invalid`; level bad, the octagon icon): "The image in `<label>` fails
   verification (`<fault>`)." A bootloader that fails verification shows as
   its own status ("Invalid") and has no line here.
4. On a fallback, one line (level warn, the triangle): "The otadata selects
   `<selected label>`, but the bootloader failed to load it and started
   `<running label>`."
5. Other storage devices, one muted line each (12 px): "External flash: not
   supported by this firmware." for a device with status `not_supported`.

#### Rows

- The list is a `<ul>` labelled "Regions and gaps", padding 6 px 8px, rows
  1 px apart, in the strip's order.
- A row is a button across the full width: a grid of `34px`, `minmax(0, 1fr)`
  and `auto` columns, gaps 1 px (rows) and 10 px (columns), padding 7 px 8 px,
  radius 7 px (`--radius-control`), left aligned. Clicking anywhere on it
  selects the region.
- Column 1, spanning both lines: the swatch, 34 × 34 px, radius 6 px, the code
  centred in the monospace font (700, 11 px), background `--t`, text `--ink`,
  an inset 1.5 px `--c` ring, and the region's texture. A gap's swatch shows
  "–" in `--muted`.
- Line 1: the title (600) and, 6 px after it on the same baseline, the
  subtitle (12.5 px, `--muted`, cut with an ellipsis). One line, no wrap.
- Line 2 (11.5 px, `--muted`, items 8 px apart, wrapping): the size (formatted
  as below), then the role chips.
- Column 3, spanning both lines: the status mark, an icon of 14 px and the
  word, 11.5 px, 600, 4 px apart, in the level's colour. Its `title` is the
  reason.
- Not shown in a row: offsets, and any "secrets" or "contains secrets" tag. The
  lock lives on the strip block and the swatch's hatching.
- States: hover `--surface-2`; highlighted from elsewhere `color-mix(in oklab,
  var(--c) 12%, var(--surface))`; selected (`aria-current="true"`)
  `color-mix(in oklab, var(--c) 20%, var(--surface))` with an inset 1 px ring
  of `--c` at 55 %.
- A missing row has its swatch and title line at opacity 0.7 and its title
  struck through; its swatch has the missing texture. An extra row has the
  extra swatch (`+` on the extra tint) and the subtitle "Extra entry".

#### Role chips

Shown on line 2 of an app slot's row, from `RegionInfo.roles`, in this order;
each is 12 px, 600, `--fg`, icon and word 5 px apart:

| Role                                     | Chip                                                                                                                                                                                     |
|------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `running`                                | An 8 px dot in `--booted`, then "Booted".                                                                                                                                                |
| `selected`, on the running slot          | The next-boot icon (14 px), then "Next boot".                                                                                                                                            |
| `selected`, on a slot that isn't running, an activated update (record state `new`) | A solid pill: background `--s-pending` (orange), text `--on-pending` (white in the light theme, `#1a1a1a` in the dark), padding 1 px 8 px 1 px 6 px, radius 999 px; the icon and "Next boot". |
| `selected`, on a slot that isn't running, a fallback | The next-boot icon and "Next boot", as on the running slot, followed by the warning icon (14 px, `--x-warn`) with the tooltip "The bootloader failed to load this slot at the last boot." |
| `rollback_target`                        | The rollback icon (14 px), then "Rollback target".                                                                                                                                       |

### The region inspector

The inspector pane is `--bg`. Its content (`article`, labelled "Inspector:
`<title>`") is padded 14 px 24 px 20 px, at most 1080 px wide, its parts 10 px
apart. Its colour variables (`--c`, `--t`, `--t2`, `--ink`) are the selected
region's. Selecting a region scrolls the inspector to its top.

#### Header

A grid of `auto`, `minmax(0, 1fr)` and `auto`, gaps 4 px and 14 px, centred
vertically:

- the large swatch, 46 × 46 px, radius 8 px, code at 14 px;
- the title, 22 px, line height 1.15, weight 650, letter spacing −0.01 em,
  then 10 px after it on the same baseline the subtitle, 14 px, 400,
  `--muted`, wrapping as needed;
- the status pill: a button, 600, 13 px, padding 5 px 11 px 5 px 9 px, radius
  999 px, a 1 px border in the level's colour (`currentColor`), background
  `--surface`, text in the level's colour: the icon, the word, then "Why?"
  (400, 12 px, `--muted`, a 1 px `--border` line at its left, 7 px padding
  after it, 2 px margin before it; `--fg` on hover and while open). The word
  is "As expected" when the level is ok, else the status word ("Differs",
  "Missing", "Extra", "Unknown", "Invalid", "MD5 differs", "Used",
  "Overlap"). Its accessible name is "`<word>`. Show why".
- Clicking the pill opens the Why popover: a status line (icon, then
  "**`<word>`.** `<reason>`", the word "As expected" when ok) and under it a
  muted 12.5 px note: "Whether the partition table on the board has this
  entry exactly as the expected layout does. If not, the firmware restricts
  what it does with the flash."

On a phone the header is two columns (`auto`, `1fr`), the pill on its own row
under them, aligned left, and the title 20 px.

#### Intro

Under the header, 4 px apart:

- the region's explanation (`--fg`, at most 95 characters wide), from the
  table in "Copy";
- for an app slot, the boot line (13 px, `--muted`, at most 120 characters
  wide), from "Boot line".

#### Actions

A wrapping row of download buttons, 8 px apart, all the same height:

- A download button has the download icon, then a title (600, 13 px, line
  height 1.2) over a sublabel (11.5 px, opacity 0.78, line height 1.2); a 1 px
  `--border` border, background `--surface`, radius 7 px, padding 6 px 12 px
  6 px 10 px, at least 40 px tall, icon and text 9 px apart. Hover: `--fg`
  border.
- The primary button (the first) is `--fg` with `--bg` text and border.
- A disabled button has opacity 0.55 and the not-allowed cursor.
- After a click the button's title reads "Added to downloads" and its border
  turns `--s-done` for 1.6 s; the header's Downloads button pulses (scale 1 to
  1.08 to 1 over 360 ms, not with reduced motion).

| Region                          | Buttons: title, sublabel                                                                                                                                       |
|---------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------|
| App slot                        | "Download image", "`<image size>`, SHA-256 appended inside the file" (primary); "Whole region", "`<region size>`, raw bytes"                                    |
| App slot, no valid image        | "Download image", "No valid image in this slot" (primary, disabled); "Whole region", "`<region size>`, raw bytes"                                              |
| App slot, image being checked   | "Download image", "The image check has not finished" (primary, disabled); "Whole region", "`<region size>`, raw bytes"                                          |
| Bootloader                      | "Download bootloader", "`<image size>`, SHA-256 appended inside the file" (primary); "Whole region", "`<region size>`, raw bytes"                               |
| Bootloader that doesn't verify  | "Download bootloader", "The bootloader image does not verify" (primary, disabled); "Whole region", "`<region size>`, raw bytes"                                 |
| Partition table                 | "Download table", "`<size>` (`<hex size>`): entries, MD5 entry and padding in one file" (primary), for example "3 KiB (0xc00): entries, MD5 entry and padding in one file" |
| otadata                         | "Download otadata", "`<size>`, both records" (primary)                                                                                                         |

The partition table region is the table's 0xC00 bytes (see "Decisions beyond
the prototype"), so the table download is the whole region and the prototype's
second button ("Whole region", "4 KiB, raw bytes, with the erased padding") is
not offered.

Regions without a download show a note in its place: a row of muted 13 px
text with an optional 16 px icon, padding 10 px 12 px, a 1 px dashed
`--border` border, radius 7 px:

| Region                               | Icon    | Note                                                                                                       |
|--------------------------------------|---------|------------------------------------------------------------------------------------------------------------|
| Sensitive                            | lock    | Raw region downloads are disabled because the region potentially contains secrets, such as cryptographic keys. |
| Reserved                             | none    | Reserved for future firmware. The region is not read, so no download is offered.                           |
| Gap                                  | none    | No data is stored in a gap, so no download is offered.                                                     |
| Public, not available this boot      | none    | The firmware does not read this region in the untrusted tier, so no download is offered.                   |
| Missing                              | warning | (see "Missing region")                                                                                     |
| Extra                                | lock    | Downloads for this region are disabled because the firmware does not know what it contains.               |

#### Usage summary panel

A panel (`--surface`, 1 px `--border` border, radius 10 px, padding 11 px
16 px) titled "Usage summary" (13 px, 600, 8 px below). It holds the ruler,
its ticks and the span table. Every region but a missing or extra one has it.

**Ruler.** 40 px tall, radius 5 px, background `--t`, an inset 1.5 px `--c`
ring, `overflow: hidden`, with the region's texture (hatched, dotted or dashed
as in the strip). The region's parts are drawn on it to scale, each
absolutely placed by its offset and size within the region (at least 1 px
wide):

| Region          | Parts, in order (label in the span table, short label on the ruler, fill)                                                                            |
|-----------------|-------------------------------------------------------------------------------------------------------------------------------------------------------|
| Bootloader      | "Bootloader image", "Image", used; "Unused, erased", "Unused", unused                                                                                 |
| App slot        | "Image", "Image", used; "Unused, erased", "Unused", unused                                                                                             |
| otadata         | "Sector 0 record", "Sector 0", used (4 KiB); "Sector 1 record", "Sector 1", alternate (4 KiB)                                                       |
| Partition table | "`<n>` entries", "Entries", used (n × 32 bytes); "MD5 entry", "MD5", alternate (32 bytes); "Unused, erased", "Unused", unused (the rest)          |

- Fills: used `--t2`; alternate `color-mix(in oklab, var(--c) 28%,
  var(--surface))`; unused a vertical rule every 10 px
  (`repeating-linear-gradient(90deg, transparent 0 9px, color-mix(in oklab,
  var(--c) 22%, transparent) 9px 10px)`). A part after another has a 1 px
  `--c` line at its left edge.
- A part wider than 18 % of the ruler carries a label inside it (10 px from
  its left, 6 px from its right, centred vertically, 12 px, `--fg`, one line
  with an ellipsis): "**`<short label>`** `<size>`", and for an app image
  ", `<percent>`% of the region" with one decimal (for example "**Image**
  1.13 MiB, 37.6% of the region"). On a phone the percentage is left out and
  the label sits 6 px from the left.
- A part's tooltip: "`<label>`: `<start>` to `<end>`, `<size>`".
- A region without parts shows one label over the whole ruler, "`<text>`,
  `<size>`":

| Region                           | Text                                                                                                                                                         |
|----------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------|
| App slot, erased                 | Erased, no image                                                                                                                                             |
| App slot, image not valid        | No valid image                                                                                                                                               |
| App slot, image being checked    | Image check running                                                                                                                                          |
| App slot not read this boot      | Not read                                                                                                                                                     |
| Sensitive                        | factory: "Factory data written during manufacturing"; running config: "Saved settings of the running app"; other config: "Saved settings of the other app"; diag: "Diagnostic crash and event records"; any other: "Protected data" |
| Reserved                         | Reserved, unused                                                                                                                                             |
| Gap                              | Nothing stored                                                                                                                                               |

**Ticks.** Under the ruler, the offset of every boundary: the region's start,
the end of each part, and the region's end, without duplicates, in the
monospace font (11 px, line height 1.2, `--muted`). Each tick is a 5 px
`--faint` line above its label, 6 px of padding over the label. The first
tick's label starts at its line, the last ends at it, the others are centred.
A tick less than 16 % of the ruler's width after the previous tick of the
first row goes to a second row, 15 px lower with a 20 px line; one that also
crowds the previous tick of the second row is left out, except the last. The
tick area is 22 px tall, 36 px with a second row.

**Span table.** A table (`role="table"`, labelled "Spans of `<title>`") with
four columns in a grid of `minmax(120px, 0.9fr)`, `1fr`, `1fr` and `1.6fr`,
13 px, 4 px below the ticks:

- Header row (11.5 px, `--muted`, padding 2 px 8 px 4 px): "What", "Start",
  "End, exclusive" with the help button (label "What does end, exclusive
  mean?"), "Size".
- Rows, each with a 1 px `--border` line above it and cells padded 5 px 8 px:
  first "Region" (the whole region), then one row per part. The first cell is
  600, with a 12 px key square (radius 3 px, an inset 1 px `--c` ring; `--t`
  for the region, `--t2` for used, `color-mix(in oklab, var(--c) 28%,
  var(--surface))` for alternate, a 2 px/1 px vertical stripe of `--surface`
  and `--c` at 30 % for unused) 7 px before the label.
- Start and End, exclusive: the hex offset (monospace, tabular, 13 px) and,
  8 px after it, the decimal (11.5 px, `--muted`, tabular, grouped with
  commas).
- Size: the hex size, then "`<decimal>` bytes, `<KiB or MiB>`" (the KiB or
  MiB part only from 1 KiB), for example "0x300000  3,145,728 bytes, 3 MiB" and
  "0x000020  32 bytes".
- A hovered row is tinted `color-mix(in oklab, var(--c) 9%, var(--surface))`
  and emphasises the same part on the ruler and the strip.
- On a phone the table has three columns; the "What" header is hidden; each
  row's label spans the full width (with the line above it, 8 px top padding)
  and the three values sit under it, each value's hex over its decimal.

The same span value (Start, End, exclusive with its help button, Size, in hex
and decimal) is used wherever a range is shown, including the downloads tray
(as three cells, labels 11.5 px `--muted`).

#### Details panel

A panel titled "Details". Its content, by region:

**App slot.**

1. The role boxes: three equal columns 6 px apart, each a box with a 1 px
   `--border` border, radius 6 px, padding 5 px 8 px, at least 46 px tall, a
   label (11 px, `--muted`) over a value (12 px). In order "Booted", "Next
   boot", "Rollback target". A box whose role the slot has shows its icon (the
   dot, the next-boot icon or the rollback icon) and "Yes" (600), and has a
   `--fg` border; one it doesn't have shows "No" (400, `--muted`). When the
   boot state isn't read (untrusted tier), "Next boot" and "Rollback target"
   show "Not known" (400, `--muted`).
2. Facts as cells: a wrapping row, 8 px between lines and 30 px between cells,
   each a label (11.5 px, `--muted`) over a value (13 px):

| Image state            | Cells                                                                                                                                                                                                                   |
|------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `valid`                | "Image": ok icon and "Valid" (ok colour, 600), then muted "checked `<n>` s after boot"; "Project and version": project name and the version in bold; "Built": "`<date>`, `<time>`"; "Built with": the ESP-IDF version; "Secure version" |
| `empty`                | "Image": "None. The slot is erased."                                                                                                                                                                                      |
| `invalid`              | "Image": bad icon and "Not valid" (bad colour, 600), then muted "(`<fault>`)". When the region is shifted (its board entry has another offset): a note, "The data at the expected offset is not an image header. The board's table places `<label>` at a different offset, so the image may be located elsewhere. This does not indicate damage." |
| `checking`             | "Image": muted "Checking. The background check has not finished."                                                                                                                                                       |
| slot not read          | "Image": muted "Not read. The firmware does not read the other slot in the untrusted tier."                                                                                                                             |

3. The hashes (valid image only), below a 1 px `--border` line, padded 10 px
   on top: "Hashes" (600, 13 px, inline) followed by the note "The SHA-256
   appended in the image is part of it and is verified by the bootloader. The
   SHA-256 of the image file is the value that file hashing tools print for
   the downloaded file." (12.5 px, `--muted`), then the hash rows:

| Label                       | Help                  | Under the label                                         | Value                          |
|-----------------------------|-----------------------|---------------------------------------------------------|--------------------------------|
| SHA-256 of the image file   | yes                   | `<size>` bytes, including the appended SHA-256           | `image.file_sha256`            |
| SHA-256 appended in image   | yes                   | validation hash, over the first `<size − 32>` bytes      | `image.digest`                 |
| ELF SHA-256                 | no                    | of the ELF file the image was built from                 | `image.descriptor.elf_sha256`  |

**Bootloader.** Cells: "Status" (the status mark), "Known as" (the name in
monospace, or muted "Not in the firmware's list"), "ESP-IDF" (the ESP-IDF
version; left out when the name already contains it), "Bootloader version",
"Built". Then the hashes as for an app, with "verified by the ROM" in place
of "verified by the bootloader", and without the ELF row.

**Partition table.** Cells: "Entries" ("`<n>`, matching the expected layout"
or "`<n>`, differing from the expected layout"), "MD5 check" (ok icon and
"Matches", or bad icon and "Differs", then muted "stored MD5 against the
entries"; muted "No MD5 entry" when the table has none), "Read from" (the
board table's offset in monospace hex, then its decimal, muted). When the
table is longer than the device lists: a note, "The table has more entries
than the device lists (16)." Then the hashes section: "Hashes" with one row,
"Stored MD5" (help), "over the `<n>` entries, kept in the MD5 entry", the
stored MD5; and the note "The table download includes this MD5. The device
does not hash the downloaded file as a whole."

**otadata.** A table (12.5 px, header 11.5 px `--muted` 500, cells padded
5 px 6 px, 1 px `--border` lines, no wrapping, scrolling sideways in its own
container when narrow) with the columns "Sector", "CRC", "Seq", "State",
"Selects", one row per record in sector order:

- CRC: ok icon and "CRC match" (ok colour, 600) for a record whose CRC
  matches (`validity` `valid`, `invalid` or `aborted`); muted "Blank, erased"
  for `blank`; muted "CRC mismatch" for `corrupt`; muted "Unreadable" for
  `unreadable`.
- Seq, State and Selects: the value, or "–" when `null`. The state is written
  as the API names it (`valid`, `new`, `pending_verify`, …).
- The record the bootloader boots (validity `valid`, highest sequence number)
  is bold, and its Selects cell ends with muted " (next boot)".

Under the table, 8 px below, 3 px apart: for each state the records use, a
line "**`<state>`**: `<text>`" (texts in "Copy"), then the muted line "The
bootloader boots the slot named by the record with a matching CRC and the
highest sequence number (marked next boot)." Then the hashes section, with no
rows and the note "Each record carries its own CRC, which the bootloader
verifies. The device does not hash the region as a whole, so the download has
no hash." When the otadata isn't read (untrusted tier), the panel holds only
the muted line "The firmware does not read the otadata in the untrusted tier."

Sensitive and reserved regions and gaps have no Details panel.

#### Expected state panel

A panel titled "Expected state" with the help button (label "What does
expected mean?"), shown only when the region's level isn't ok. It holds the
status line: the level's icon, then "**`<word>`.** `<reason>`" in `--fg`.

For a region whose board entry differs, two more parts follow:

1. A comparison table (12.5 px, 8 px below; header 11.5 px `--muted` 500,
   cells padded 4 px 6 px, 1 px `--border` lines) with the columns "Field",
   "Expected layout" and "Board's table has", and the rows `label`, `type`,
   `subtype`, `offset`, `size` and `flags`. Values: the label as text; the
   type as a number; the subtype and the flags as `0x` and lowercase hex
   without padding; the offset as its hex (monospace) over its decimal
   (muted); the size as its hex over its human size. A row whose values
   differ is in `--x-warn`, 600.
2. The placement bars (10 px below, 4 px apart, 11.5 px): the muted caption
   "Placement of the region, to scale (`<lowest start>` to `<highest end>`)",
   then a bar (18 px, `--surface-2`, radius 3 px) with a block at the expected
   span (fill `--t2`, an inset 1 px `--c` ring, radius 3 px) and the caption
   "Expected layout: `<start>` to `<end>`", then a bar with a hatched block
   (`repeating-linear-gradient(135deg, var(--t) 0 3px, var(--t2) 3px 6px)`)
   at the board's span and the caption "Board's table has `<start>` to
   `<end>`".

When both this panel and the Details panel show, they sit side by side on a
desktop (columns `minmax(0, 0.9fr)` and `minmax(0, 1.1fr)`, 14 px apart,
aligned to the top), Expected state on the left; below 1100 px they stack.

When the region overlaps another, the Overlap panel follows ("Overlaps and
shifted entries").

#### Missing region

The header as usual (the pill reads "Missing"), then:

- the intro "`<title>` is missing from the board's partition table. The
  expected layout has it at this range.";
- the note box with the warning icon: "The board's partition table has no
  `<label>` entry, so nothing is described at this range and there is nothing
  to read or download.`<consequence>`" (`<consequence>` with a leading
  space, as in "Tiers and limits");
- a panel titled "Expected layout" holding the span table with the "Region"
  row only (the expected span).

No usage summary, no Details, no Expected state panel: the pill's Why popover
holds the reason. An Overlap panel follows when another entry lies in the
expected span.

#### Extra entry

The header (the extra swatch, the board's label as title, "Extra entry" as
subtitle, the pill "Extra"), then:

- the intro "The board's partition table has a `<label>` entry that the
  expected layout does not have. Its contents are unknown.";
- the note box with the lock: "Downloads for this region are disabled because
  the firmware does not know what it contains.";
- a panel titled "Board's table entry": cells "Name" (the label), "Type",
  "Subtype" and "Flags" (each `0x` and two lowercase hex digits, for example
  "0x01", "0x40", "0x00"), then the span table with the "Region" row (the
  board's span).

An Overlap panel follows when the entry overlaps another region.

#### Boot line

For an app slot, built from its roles and the boot state:

| Case                                     | Text                                                                                                                                                                                       |
|------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Running and selected                     | Booted and selected for the next boot. Followed, when the OTA state is `valid`, by: OTA state valid: the app is confirmed, so the bootloader does not roll back.                             |
| Running and selected, needs confirmation | Booted and selected for the next boot. OTA state pending_verify: the app has not confirmed itself, so the bootloader rolls back at the next boot.                                        |
| Running, another slot selected           | Booted. The next boot starts `<other label>`.                                                                                                                                             |
| Running, boot state not read             | Booted. The otadata is not read in the untrusted tier, so the next boot is not known.                                                                                                     |
| Selected, not running, an activated update | Selected for the next boot (state `<state>`). The slot has not been booted. After booting, the app must confirm itself, otherwise the bootloader rolls back to `<other label>`.           |
| Selected, not running, a fallback        | Selected for the next boot by the otadata. The bootloader failed to load this slot at the last boot and started `<other label>` instead.                                                 |
| Running, after a fallback                | Booted as a fallback. The otadata selects `<other label>`, which the bootloader failed to load.                                                                                         |
| Rollback target (added)                  | The bootloader rolls back to this slot if the app in the other slot fails to confirm itself.                                                                                              |
| No role                                  | Not selected for booting.                                                                                                                                                                 |

The "(state `<state>`)" part shows only when a record with validity `valid`
selects the slot, and names its state. The rollback sentence follows the
others with a space. "Running, another slot selected" is for an activated
update.

**Fallback.** `BootState.selected` naming a slot other than `running` is
either an update activated for the next boot, whose selecting record (validity
`valid`, highest sequence number) has the state `new`, or a fallback: the
bootloader started the running slot because the selected one failed to load.
The page tells them apart by that record's state. A fallback gets the
fallback rows above, the plain Next boot chip with its warning, and line 4 of
the summary header; it never shows the orange pill or "An OTA update in … is
selected for the next boot."

#### Phone inspector

- The list and the inspector are each absolutely placed over the whole panes
  area. The inspector starts off-screen to the right (`translateX(100%)`)
  with the shadow; selecting a region slides it in (`transform: none`) while
  the list moves to `translateX(-22%)`, over 0.24 s
  (`cubic-bezier(.3,.7,.2,1)`). Going back reverses it. The hidden pane is
  `inert`.
- The inspector's first child is a sticky bar at its top: `--bg`, a 1 px
  `--border` bottom border, padding 6 px 16 px 6 px 10 px (with the safe
  areas), holding the Back button (the back chevron and "Back", 600, padding
  6 px 8px, radius 6 px) and the muted 12 px crumb "Regions". Under it the
  content has 16 px side margins, its parts 12 px apart, 28 px of bottom
  padding.
- Selecting moves focus to Back; Back (or Escape) slides the inspector out and
  focuses the selected row. On a desktop the bar isn't shown.

#### Interactions

- **Selection.** Clicking a strip block or a row selects the region: the
  strip block and the row get `aria-current="true"`, the readout names it, the
  inspector shows it from its top, and on a desktop the row scrolls into view
  (nearest). A click made with the keyboard leaves focus on the row. The
  default selection is the running app's region; after a refresh the
  selection is kept by its key (a partition's label, `bootloader`,
  `partition_table`, a gap's start offset, an extra entry's table index), and
  falls back to the running app's region when the key is gone.
- **Keyboard.** Up and Down move the focus between rows. Escape closes an open
  popover (focus back to its trigger), else the open tray (focus back to the
  Downloads button), else on a phone the inspector.
- **Hover and focus emphasis.** Pointing (not touch) at or focusing anything
  that stands for a region (a strip block, a row, the inspector header, a
  swatch in the tray) highlights that region everywhere: the strip dims the
  other blocks and rings this one, its row takes the highlight tint, and the
  readout names it. Pointing at a part (a span table row, a ruler part, the
  strip's usage bar) rings that part on the ruler and the strip with an inset
  2 px `--fg` line and tints its span table row; the "Region" row outlines the
  whole ruler (2 px `--fg`, 2 px outside). Leaving the document clears it.
  Emphasis toggles classes and attributes and never re-renders. Popovers and
  the tray don't emphasise.
- **Popovers.** One at a time, fixed, above everything (`z-index` 60):
  `--surface`, `--fg` text, a 1 px `--border` border, radius 10 px, the
  shadow, padding 12 px 14 px, 13 px text, at most `min(360px, 100vw − 32px)`
  wide, paragraphs 6 px apart. Placed 6 px under the trigger, aligned to its
  left edge and kept 16 px from the viewport's sides; above the trigger when
  it doesn't fit below; never closer than 8 px to the top or bottom. A popover
  closes on a pointer press outside it and its trigger, on Escape, when
  another opens, when its trigger is pressed again, and when anything but the
  popover scrolls. The trigger carries `aria-expanded`.
- **Help buttons.** An 18 px circle with a 1 px `--faint` border, "?" in
  600 11 px `--muted`; `--fg` border and text on hover and while open. The
  popover shows the title in bold, then the text; text in backticks is set in
  the monospace font.

| Key           | Title                     | Text                                                                                                                                                                                                                                        |
|---------------|---------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| span          | Ranges                    | A range of flash given by an offset and a size. It covers the bytes from the offset up to, but not including, offset + size (end exclusive). Example: a 4 KiB region at 0x009000 ends at 0x00a000, and its last byte is at 0x009fff.     |
| expected      | Expected                  | Whether the partition table on the board has this entry exactly as the expected layout does. If not, the firmware restricts what it does with the flash. For the bootloader, the state means the bootloader is in the firmware's list of known bootloaders. For a gap, it means the board's table has no entry in that space. |
| file_sha256   | SHA-256 of the image file | The SHA-256 of the whole image file, including the appended SHA-256. This is the value `sha256sum`, `Get-FileHash` and `shasum -a 256` print for the downloaded file.                                                                    |
| stored_digest | SHA-256 appended in image | The SHA-256 appended to the image, calculated over all preceding bytes. The bootloader and `esptool image_info` call it the validation hash. If it matches, the image is intact.                                                         |
| md5           | Stored MD5                | The MD5 entry stored after the partition table entries, calculated over those entries. If it matches, the table is intact.                                                                                                                 |
| strip         | The flash map             | (above, under "Help popover")                                                                                                                                                                                                               |

  The hash help buttons are labelled "What is the `<label>`?".
- **Hash fields.** A read-only one-line text area that grows to its content
  (`field-sizing: content`, with a script fallback): monospace 11 px, line
  height 1.5, `--surface-2`, a 1 px `--border` border, radius 6 px, padding
  5 px 8px, at least 30 px tall, breaking anywhere, text cursor; a click
  selects all of it; focus draws a 2 px `--accent` outline. Its accessible
  name is "`<label>`, `<text under the label>`". The Copy button beside it
  (a 1 px `--border` border, `--surface`, radius 6 px, padding 0 10 px,
  12.5 px, at least 66 × 30 px, the copy icon and "Copy"; `--fg` border on
  hover) writes the value to the clipboard inside the click handler and reads
  "Copied" for 1.5 s; where the clipboard is unavailable or refuses, it
  focuses and selects the field and reads "Selected" for 2.5 s.
- **Hash rows.** Inside a container 640 px wide or more, a row is a grid of
  `minmax(170px, 270px)`, `minmax(0, 1fr)` and `auto`, 10 px apart, centred:
  label, field, Copy. Narrower, the label takes the full width and the field
  and Copy share the next line (6 px apart, 3 px below). The label is 600
  12.5 px with the help button 5 px after it, over the muted 11.5 px text.
  Rows are 8 px apart.

### Downloads button and tray

#### Header button

At the header's right end: a pill button 34 px tall (padding 5 px 12 px 5 px
8px, a 1 px `--header-line` border, `--header-fg` text; `--header-muted`
border on hover; `--header-line` background while the tray is open), holding
8 px apart:

- a 22 px progress ring: a track circle (radius 9, stroke 2.4,
  `--header-line`) and a progress arc in `--hdr-accent` starting at the top
  and filling clockwise with the running download's share (0.12 s linear
  transition), around the 13 px download icon;
- the label "Downloads" (13 px; hidden on a phone);
- the badge: the number of active (pending and downloading) downloads, at
  least 18 px wide, 18 px tall, radius 9 px, `--hdr-accent` with
  `--header-bg` text, 700 11 px, padding 0 5 px; hidden when none is active.

The button is always present, also with no downloads. Its accessible name is
"Downloads, `<n>` active, `<p>`% of `<file name>`" while one runs, "Downloads,
`<n>` active" while some wait, and "Downloads, none active" otherwise. It has
`aria-expanded` and controls the tray; pressing it toggles the tray.

#### Tray

- A panel fixed at the right, from under the header to the bottom (with the
  safe areas), `min(420px, 100%)` wide (the full width on a phone, without its
  left border), `--surface`, a 1 px `--border` left border, the shadow,
  above the page (`z-index` 40). It slides in from the right
  (`translateX(105%)` to none, 0.2 s ease) and is `inert` and hidden while
  closed. Labelled "Downloads".
- It closes on a pointer press outside it (except in a popover and on the
  Downloads button), on Escape and on its close button; focus returns to the
  Downloads button.
- Head (padding 12 px 12 px 10 px 16 px, a 1 px `--border` bottom border):
  "Downloads" (15 px), the muted 12 px count "`<a>` active, `<f>` finished",
  then at the right the ghost buttons "Clear finished" and the close button
  (an ×, labelled "Close downloads").
- The list (padding 10 px 12px 16 px, items 8 px apart, scrolling on its
  own, scroll anchoring off) shows every download, newest first by the time
  it was queued. With none: centred muted 13 px text, 40 px padding, "No
  downloads. Downloads are started from a region's inspector."
- **New items** are inserted at the top. When the list is scrolled to the top
  (within 2 px) the new item simply appears there. When it is scrolled away,
  the list keeps what it shows (its scroll position grows by the new item's
  height) and, while the tray is open, a pill "New download above" (12 px,
  `--fg` with `--bg` text, radius 999 px, padding 5 px 12 px, the shadow)
  floats centred 64 px from the tray's top; clicking it scrolls the list to
  the top, and reaching the top hides it.
- **Item**: a card (1 px `--border` border, radius 10 px, `--surface`; `--faint`
  border while open). Its head is a button across the card (padding 10 px
  12px; a grid of `20px`, `minmax(0, 1fr)` and `auto`, gaps 2 px and 10 px)
  with `aria-expanded`:
  - the status icon (18 px, in the status colour) spanning two lines;
  - the file name, monospace 600 13 px, one line with an ellipsis;
  - what it is (12 px, `--muted`, one line with an ellipsis);
  - at the right, spanning two lines and right aligned: the status word (600
    12 px, status colour) over the amount (12 px, `--muted`, tabular);
  - while downloading, a progress bar across the text columns, 6 px below:
    4 px tall, radius 2 px, `--surface-2` track, `--s-active` fill (0.12 s
    linear).
- Clicking the head opens or closes the item's body (padding 0 12 px 12 px
  42 px, parts 10 px apart, 12.5 px):
  - for a failed item, the reason, in `--x-bad`;
  - facts (label `--muted`, value): "Queued" "`<HH:MM:SS>`, `<age>`"; "From"
    the small swatch (22 px, radius 4 px, code 9 px), the region's title and
    its muted subtitle; "Progress" ("`<done>` of `<total>`" while
    downloading or failed, "`<total>` saved" when completed, else the size);
  - the span value of the downloaded range (Start, End, exclusive, Size);
  - the hash field, when the download has one ("Downloads per region");
  - the note, muted;
  - the actions, 8 px apart: "Retry" (with the rollback icon) for failed and
    cancelled items, "Cancel" (with the cancelled icon) for pending and
    downloading ones, and the ghost "Remove" for completed, failed and
    cancelled ones. Unverified items also have "Save anyway".
- The age is "just now" under 10 s, "`<n>` s ago" under a minute, "`<n>` min
  ago" under an hour, then "`<n>` h ago", refreshed every 15 s; the clock is
  24-hour with seconds.
- **Clear finished** removes every item that is completed or cancelled. Failed
  items stay, since a failed download keeps its bytes for a resume; Remove
  takes one away.
- **Retry** puts the item back at the top as pending, with a new queue time.
- Downloads run one at a time, oldest pending first. No byte-range download is
  offered; a range is an API client's job.

#### Statuses

The five statuses and how the download service's states map onto them:

| Status      | Colour token             | Icon                       | Service states                                                       | Amount                                         |
|-------------|--------------------------|----------------------------|----------------------------------------------------------------------|------------------------------------------------|
| Pending     | `--s-pending` (orange)   | clock                      | `queued`; `waiting` (another client holds the device's download)    | the size; "Waiting" while `waiting`            |
| Downloading | `--s-active` (blue)      | arrow down to a line       | `downloading` (also `retrying`); `verifying`                         | "Preparing" before the size is known, "`<p>`%", "Checking" while verifying |
| Completed   | `--s-done` (green)       | check in a circle          | `done` with `checked` or `unchecked`                                 | the size                                       |
| Failed      | `--s-failed` (red)       | × in a circle              | `failed`; `done` with `mismatch` (saved unverified)                  | "at `<p>`%"; "Unverified" for a mismatch       |
| Cancelled   | `--s-cancel` (grey)      | slashed circle             | `cancelled`                                                          | the size                                       |

- While `waiting`, the body's note reads "Another client is downloading from
  the device. Asking again in `<countdown>`."
- While resuming (`retrying`), the word stays "Downloading" and the note reads
  "The connection broke; the download resumes from `<done>`."
- A `failed` item whose bytes didn't match the digest offers "Save anyway",
  which saves the file as unverified; such an item then shows the Failed
  status with the word "Unverified" and the note "Saved as unverified: the
  SHA-256 of the received bytes does not match the device's."
- "Retry" is the service's `resume` for a resumable failure, and adds the same
  target again otherwise. "Remove" is `dismiss`. The service's `up` and
  `down` actions are not offered.
- The page checks each download's SHA-256 where it can (a secure context); it
  says nothing about the check where it can't.

#### Downloads per region

| Download          | What                                       | Hash label, text, value                                                                   | Note                                                     |
|-------------------|--------------------------------------------|-------------------------------------------------------------------------------------------|----------------------------------------------------------|
| App image         | App image, `<label>`                       | "SHA-256 of the image file", "the value for the file you receive", `file_sha256`         | none                                                     |
| Bootloader image  | Bootloader image                           | "SHA-256 of the image file", "the value for the file you receive", `file_sha256`         | none                                                     |
| Partition table   | Partition table, entries and MD5 entry     | "Stored MD5", "over the entries; the MD5 entry is part of the file", `TableRead.md5.stored` at plan time | The device does not hash the file as a whole.            |
| otadata           | Boot selection (otadata)                   | none                                                                                      | No hash: the device does not hash whole regions.         |
| Whole region      | Whole region, `<title>`                    | none                                                                                      | No hash: the device does not hash whole regions.         |

The file name is the one the download plan gives (`web/src/storage/filenames.ts`,
the same name the device sends in `Content-Disposition`), for example
`eveningstar-0kd7-9c6-app_current-ota_0-image-1a2b3c4d.bin`; the prototype's
short names (`ota_0-image.bin`) were placeholders.

### Copy

All user-facing text is datasheet style: declarative third-person sentences,
complete and short, naming the real mechanism (OTA data region, OTA state,
rollback target, validation hash). No narration, no coaching, no "simply" or
"just", no fragments, no em-dash flourishes, no exclamation.

#### Terminology

| Term                        | Meaning and use                                                                                                                         |
|-----------------------------|-----------------------------------------------------------------------------------------------------------------------------------------|
| region                      | Any extent of the flash: the bootloader, the partition table, a partition, a gap, an extra entry. "Whole region" for a raw download.      |
| slot                        | Only an OTA app slot (`ota_0`, `ota_1`). Never a configuration or any other partition.                                                 |
| expected layout             | The partition layout the firmware is built for. Never "the layout" alone in user-facing text, nor "compiled layout".                   |
| board's table               | The partition table the bootloader read, as it is on the board.                                                                         |
| OTA updates                 | Installing a new app image into the other slot. "OTA updates are unavailable" when the tier is below full.                             |
| next boot                   | The slot the otadata selects. "Next boot" as a chip, a role box and the otadata marker "(next boot)".                                 |
| rollback target             | The slot the bootloader rolls back to.                                                                                                  |
| CRC match                   | An otadata record whose CRC matches.                                                                                                    |
| SHA-256 appended in image   | The digest an ESP-IDF image stores at its end (the validation hash). Never "stored digest" in user-facing text.                       |
| SHA-256 of the image file   | The SHA-256 of the image file, appended digest included.                                                                                |
| Usage summary               | The panel of a region's ruler and span table.                                                                                           |
| End, exclusive              | The end of a range, the first byte after it.                                                                                            |
| Unallocated                 | A gap: space no entry covers, at least a whole 4 KiB sector or crossing a sector boundary.                                              |
| Extra entry                 | A board table entry the expected layout doesn't have.                                                                                   |

Fixed sentences:

- Secrets note: "Raw region downloads are disabled because the region
  potentially contains secrets, such as cryptographic keys."
- Extra entry: "The board's partition table has a `<label>` entry that the
  expected layout does not have. Its contents are unknown." and "Downloads for
  this region are disabled because the firmware does not know what it
  contains."

#### Regions

Identity follows the partition's label, so a slot keeps its code and colour
when the other slot boots; the subtitle follows the role this boot.

| Region (`RegionInfo.region`, label)       | Code | Colour  | Title            | Subtitle                  | Explanation (intro)                                                                                                                                                                         |
|-------------------------------------------|------|---------|------------------|---------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `bootloader`                              | BL   | `--r-bl` | Bootloader       | Second-stage bootloader   | The second-stage bootloader handles partition table parsing, boot selection and automatic app rollback. It is loaded by the first-stage bootloader, which is burned into the chip's ROM.    |
| `partition_table`                         | PT   | `--r-pt` | Partition table  | Map of the flash          | The partition table lists the flash partitions, each with a name, type, offset and size. The second-stage bootloader reads it on every boot.                                               |
| `factory`, factory                        | FD   | `--r-fd` | factory          | Factory data              | Factory data written during manufacturing.                                                                                                                                                  |
| `otadata`, otadata                        | OD   | `--r-od` | otadata          | Boot selection            | The OTA data region enables app slot selection with automatic rollback. It holds two boot selection records, each with a sequence number, the selected app slot and an OTA state.          |
| gap                                       | (–)  | `--r-gap` | Unallocated     | Gap between entries       | Not covered by any partition table entry. Nothing is stored here.                                                                                                                           |
| `app_current`, ota_0 or ota_1             | A0/A1 | `--r-a0`/`--r-a1` | the label | Running app       | The OTA app slot the device is currently running from. It holds one app image; the remainder of the slot is unused.                                                                         |
| `app_alternate`, ota_0 or ota_1           | A0/A1 | `--r-a0`/`--r-a1` | the label | Other app         | The second OTA app slot. OTA updates are written here and selected for the next boot.                                                                                                       |
| `config_current`, config_0 or config_1    | C0/C1 | `--r-c0`/`--r-c1` | the label | Running config    | Settings saved by the running firmware.                                                                                                                                                     |
| `config_alternate`, config_0 or config_1  | C0/C1 | `--r-c0`/`--r-c1` | the label | Other config      | Settings for the other app slot, kept so that the slot can roll back with its own configuration.                                                                                            |
| `diag`, diag                              | DG   | `--r-dg` | diag             | Diagnostics               | Diagnostic crash and event records written by the firmware.                                                                                                                                 |
| `reserved`, reserved                      | RS   | `--r-rs` | reserved         | Reserved                  | Space reserved for future firmware. It is not read.                                                                                                                                         |
| extra entry                               | +    | `--x-warn` | its label      | Extra entry               | (the extra entry sentences)                                                                                                                                                                 |
| `app_current` without a label (booted from no slot) | RA | `--r-a0` | Running app | Not an expected slot  | The app the bootloader started, outside the expected layout's OTA app slots.                                                                                                                |
| an unknown region or label                | ??   | `--r-rs` | the label, else the region name | Unknown region | A region this page does not know.                                                                                                                          |

#### Statuses and reasons

`<consequence>` is the tier sentence with a leading space, empty in the full
tier ("Tiers and limits").

| Subject          | Case                                     | Level | Word        | Reason                                                                                                                                                                         |
|------------------|------------------------------------------|-------|-------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Bootloader       | `known`                                  | ok    | Expected    | The bootloader (`<name>`) is in the firmware's list of known bootloaders and supports rollback.                                                                                |
| Bootloader       | `unlisted`                               | warn  | Unknown     | The bootloader image verifies, but the bootloader is not in the firmware's list of known bootloaders. Rollback support cannot be assumed. OTA updates are unavailable.         |
| Bootloader       | `invalid`                                | bad   | Invalid     | The bootloader image fails verification (`<fault>`).                                                                                                                           |
| Partition table  | MD5 doesn't match                        | bad   | MD5 differs | The stored MD5 does not match the partition table entries. The table may be damaged.                                                                                         |
| Partition table  | matches                                  | ok    | Expected    | Every entry matches the expected layout, and the stored MD5 matches the entries.                                                                                              |
| Partition table  | differs                                  | warn  | Differs     | The table is intact (the stored MD5 matches the entries), but it differs from the expected layout: `<findings>`.`<consequence>`                                              |
| Partition table  | not known                                | warn  | Unknown     | The partition table the bootloader read is not known. `<search sentence>`                                                                                                     |
| Partition        | matches                                  | ok    | Expected    | The `<label>` entry in the board's table matches the expected layout in type, subtype, offset, size and flags.                                                               |
| Partition        | differs                                  | warn  | Differs     | The `<label>` entry in the board's table differs from the expected layout in `<fields>`.`<consequence>`                                                                       |
| Partition        | missing                                  | warn  | Missing     | The board's table has no `<label>` entry. `<label>` is missing from the board, and its expected space is unallocated.`<consequence>` (with an entry in the space: "…, and the board's table places `<other label>` in its expected space.") |
| Partition        | overlapped, own entry matches            | warn  | Overlap     | The `<label>` entry in the board's table matches the expected layout, but `<other title>` overlaps it from `<start>` to `<end>`, `<size>`.`<consequence>`                    |
| Partition        | board's table not known                  | warn  | Unknown     | The partition table the bootloader read is not known, so this entry cannot be compared.                                                                                       |
| Gap              | no entry in it                           | ok    | Expected    | The expected layout leaves this space unallocated. The board's table has no entry here.                                                                                       |
| Gap              | a shifted entry lies in it               | warn  | Used        | The board's table places `<labels>` in this space.                                                                                                                             |
| Gap              | board's table not known                  | warn  | Unknown     | The partition table the bootloader read is not known, so this space cannot be compared.                                                                                       |
| Extra entry      | any                                      | warn  | Extra       | The board's table has a `<label>` entry (type `<0xNN>`, subtype `<0xNN>`) that the expected layout does not have.`<overlap sentence>``<consequence>`                        |

- `<fields>`: the differing fields by name (`type`, `subtype`, `offset`,
  `size`, `flags`) joined as a list: "offset", "offset and size", "type,
  offset and size".
- `<findings>`: in offset order, "`<title>` is missing", "`<title>` differs",
  "`<title>` overlaps `<other title>`", "`<label>` is extra", joined with ", ";
  then, when they apply, "the entries are in a different order than in the
  expected layout", "the bootloader read the table at `<offset>`, not at
  `<expected offset>`" and "the table ends at an entry that is not valid".
- `<overlap sentence>`: " It overlaps `<other title>` from `<start>` to
  `<end>`, `<size>`." per overlap.

#### OTA states

| State            | Text                                                                                                                                          |
|------------------|-----------------------------------------------------------------------------------------------------------------------------------------------|
| `valid`          | Confirmed. The app has booted and marked itself valid. The bootloader does not roll back from a confirmed app.                               |
| `new`            | Written by an OTA update and selected for the next boot. The app has not been booted.                                                       |
| `pending_verify` | Booted once and awaiting confirmation by the app. If the board restarts before the app confirms, the bootloader rolls back.                 |
| `invalid`        | Rejected. The app did not confirm, and the bootloader rolled back.                                                                            |
| `aborted`        | Rejected. The app did not confirm, and the bootloader rolled back.                                                                            |
| `undefined`      | No OTA state recorded. The bootloader treats the app as confirmed.                                                                           |
| `other`          | A value ESP-IDF does not define.                                                                                                              |

#### Image faults

`<fault>` in a reason or a cell is the short description of the API's
`ImageFault`, as the web client's labels already have them: "Reading the flash
failed", "The image runs past the end of the region", "No image header",
"Built for another chip", "More segments than an image can have", "A
segment's length isn't a multiple of 4", "A segment runs past the end of the
region", "A segment can't be mapped from where it lies", "The checksum doesn't
match", "No SHA-256 digest appended", "The SHA-256 digest doesn't match", "No
app descriptor". Those contractions ("isn't", "can't", "doesn't") are reworded
to "is not", "cannot" and "does not" in the redesign.

#### Formats

- Offsets and hex sizes: `0x` and six lowercase hex digits, zero padded
  (`0x010000`), in the monospace font with tabular figures. Wider values
  keep all their digits.
- Decimals: grouped with commas (`3,211,264`), tabular figures.
- Human sizes: at least 1 MiB, the value in MiB with up to two decimals,
  trailing zeros dropped ("3 MiB", "1.13 MiB", "1.44 MiB"); at least 1 KiB,
  the same in KiB ("22.14 KiB", "4 KiB"); below, "`<n>` bytes".
- Byte counts in text: grouped decimal and "bytes" ("1,152,096 bytes").
- Hashes: lowercase hex, whole, never shortened.

### Icons

Stroked SVG on a 16 × 16 view box, 16 px by default, `fill: none`, stroke
`currentColor` 1.8 px wide, round caps and joins:

| Icon                          | Paths                                                                                                                       |
|-------------------------------|-----------------------------------------------------------------------------------------------------------------------------|
| pending (clock)               | `circle cx=8 cy=8 r=6.2`; `M8 4.6V8l2.4 1.6`                                                                                |
| downloading, download         | `M8 2v8.2M4.6 7 8 10.4 11.4 7M3 13.6h10`                                                                                    |
| completed, ok                 | `circle cx=8 cy=8 r=6.2`; `m5.2 8.2 1.9 1.9 3.8-4`                                                                          |
| failed                        | `circle cx=8 cy=8 r=6.2`; `m5.8 5.8 4.4 4.4M10.2 5.8l-4.4 4.4`                                                              |
| cancelled                     | `circle cx=8 cy=8 r=6.2`; `m3.7 12.3 8.6-8.6`                                                                               |
| warn (triangle)               | `M8 2.2 14.3 13H1.7z`; `M8 6.4v3M8 11.1v.1`                                                                                 |
| bad (octagon)                 | `M5.4 1.8h5.2l3.6 3.6v5.2l-3.6 3.6H5.4l-3.6-3.6V5.4z`; `M8 5v3.6M8 10.8v.1`                                                 |
| lock                          | `rect x=3.2 y=7 width=9.6 height=6.8 rx=1.2`; `M5.4 7V5.2a2.6 2.6 0 0 1 5.2 0V7`                                            |
| next boot                     | `M2.5 8h9M8.5 4.5 12 8l-3.5 3.5M13.8 3.5v9`                                                                                 |
| rollback, retry               | `M3.2 7.5A5 5 0 1 1 4.8 11.8`; `M2.8 3.8v3.9h3.9`                                                                           |
| back                          | `M10 3 5 8l5 5`                                                                                                             |
| copy                          | `rect x=5.5 y=5.5 width=8 height=8 rx=1.3`; `M10.5 3.2V3a1 1 0 0 0-1-1H3.4a1 1 0 0 0-1 1v6.1a1 1 0 0 0 1 1h.4`              |
| close                         | `M4 4l8 8M12 4l-8 8`                                                                                                        |

The header's ring holds the download icon at 13 px with
`M8 2.5v8M4.5 7.5 8 11l3.5-3.5M3 13.5h10`. Icons beside text are hidden from
assistive technology; a status mark's word carries its meaning.

### Theming

The page uses the app's tokens (`web/src/styles/tokens.css`). The light theme
is the default; the dark theme replaces the values. The prototype was designed
in both, and every state on this page has to be checked in both: a missing
block and row, an extra block and row, the next-boot pill, the hover emphasis
and dimming, the selected block and row (also missing, extra and gap), the
textures, the status pill and the Why popover, the disabled download button,
the tray's five statuses.

Colour roles:

| Role                                | Token                                                         |
|-------------------------------------|---------------------------------------------------------------|
| Page and inspector background       | `--bg`                                                        |
| Panes, panels, buttons, tray        | `--surface`; hover row and hash field `--surface-2`          |
| Lines                               | `--border`; dashed gap outline, help ring, tick lines `--faint` |
| Text                                | `--fg`; secondary `--muted`                                   |
| Focus ring                          | 2 px `--accent`, 2 px outside                                 |
| Header                              | `--header-bg`, `--header-fg`, `--header-muted`, `--header-line` |
| Header ring progress and badge      | `--hdr-accent`                                                |
| Status ok, warn, bad                | `--x-ok` = `--st-normal`; `--x-warn` = `--st-caution`; `--x-bad` = `--st-critical` |
| Booted dot                          | `--booted` = `--st-normal`                                    |
| Download pending, downloading, completed, failed, cancelled | `--s-pending` = `--st-serious`; `--s-active` = `--st-standby`; `--s-done` = `--st-normal`; `--s-failed` = `--st-critical`; `--s-cancel` = `--st-off` |
| Next-boot pill                      | `--s-pending` with `--on-pending` text                        |
| Region identity                     | `--r-*` (below), with `--t`, `--t2` and `--ink` derived       |
| Shadow (popovers, tray, phone inspector) | `--shadow`                                               |

Status colours are for status only. Region colours are a separate palette that
avoids the five status hues; green means "booted" only with the dot and the
word.

New tokens (in both themes):

| Token            | Light                              | Dark                              |
|------------------|------------------------------------|-----------------------------------|
| `--faint`        | `#8d8d8d`                          | `#8d8d8d`                         |
| `--hdr-accent`   | `#4589ff`                          | `#4589ff`                         |
| `--on-pending`   | `#ffffff`                          | `#1a1a1a`                         |
| `--shadow`       | `0 12px 40px rgba(22, 22, 22, 0.18)` | `0 12px 40px rgba(0, 0, 0, 0.55)` |
| `--r-bl`         | `#5753b0`                          | `#a09cf5`                         |
| `--r-pt`         | `#8b47c9`                          | `#c08cf5`                         |
| `--r-od`         | `#b0459a`                          | `#e88ad8`                         |
| `--r-a0`         | `#00807e`                          | `#3ccac6`                         |
| `--r-c0`         | `#4f9d9b`                          | `#93dcd8`                         |
| `--r-a1`         | `#a8346b`                          | `#f27aae`                         |
| `--r-c1`         | `#c9789f`                          | `#f6b3d0`                         |
| `--r-fd`         | `#9a6b78`                          | `#d9a6b4`                         |
| `--r-dg`         | `#7a7426`                          | `#cfc56e`                         |
| `--r-rs`         | `#6b7385`                          | `#9aa2b1`                         |
| `--r-gap`        | `#8d8d8d`                          | `#8d8d8d`                         |

Per region, `--c` is its colour and:

- `--t` = `color-mix(in oklab, var(--c) 17%, var(--surface))` (tint);
- `--t2` = `color-mix(in oklab, var(--c) 40%, var(--surface))` (stronger
  tint);
- `--ink` = `color-mix(in oklab, var(--c) 62%, var(--fg))` (text on a tint).

Existing token values the page relies on (light / dark): `--bg` `#f4f4f4` /
`#161616`, `--surface` `#ffffff` / `#262626`, `--surface-2` `#ececec` /
`#303030`, `--border` `#e0e0e0` / `#393939`, `--fg` `#161616` / `#f4f4f4`,
`--muted` `#525252` / `#c6c6c6`, `--accent` `#0f62fe` / `#78a9ff`,
`--st-normal` `#198038` / `#42be65`, `--st-caution` `#8e6a00` / `#f1c21b`,
`--st-serious` `#ba4e00` / `#ff832b`, `--st-critical` `#da1e28` / `#fa4d56`,
`--st-standby` `#0072c3` / `#4589ff`, `--st-off` `#8d8d8d` both. Radii:
`--radius-panel` 10 px, `--radius-control` 7 px, `--radius-pill` 999 px. Fonts:
`--font-ui` and `--font-mono`; the body text is 14 px, line height 1.45.

With reduced motion, no transition or animation runs.

### Data

#### Endpoints

The page reads, in order, `GetStorage` (`/storage`), `GetInternalFlash`
(`/storage/internal`), `GetRegions` (`/storage/internal/regions`) and
`GetPartitionTable` (`/storage/internal/table`), through the storage service
(`web/src/storage/service.ts`), which polls the flash and the regions every
3 s while the checks run and reads everything again when the device restarts.
Downloads go through `GetRegionContent`
(`/storage/internal/regions/{region}/content`) via the download service.

#### What supplies each part

| Page part                                   | Source                                                                                                                                                                  |
|---------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Flash size ("8 MiB flash", strip end)       | `InternalFlash.capacity`; when 0, `InternalFlash.layout_size` and the summary title reads "`<size>` flash (expected size; the chip's size is not known)". The strip spans the larger of that and the highest end of any block. |
| Expected regions and their order            | `GetRegions.regions`: `region`, `label`, `partition` (type, subtype, flags), `contents`, `available`, `roles`.                                                         |
| Expected span of a region                   | The layout's span, which the API has to supply (see "API the page needs"). Today `RegionInfo.offset` and `size`, which are the layout's span except where the firmware holds a handle with another span (the untrusted tier's bootloader, partition table and running app). |
| Gaps                                        | Derived: the space no expected region and no extra entry covers ("Overlaps and shifted entries"), except a remainder inside one sector.                               |
| Board entries                               | `GetPartitionTable`: `BoardTable.table.entries` (label, type, subtype, offset, size, flags), `table.offset`, `table.truncated`, `table.end`, `search`, `offsets`.     |
| Missing, Differs, Extra, Used, Overlap      | Derived from the board entries against the expected regions ("Matching entries"). `RegionInfo.layout` is not used for it.                                             |
| Partition table status and MD5              | `TableRead.md5` of the board's table (`stored`, `matches`); `TableSummary.md5` when the board's table is not known; `TableSummary.matches` and `offset`; the derived findings. |
| Bootloader status, name, version, built     | `InternalFlash.bootloader`: `status`, `name`, `error`, `version`, `idf_version`, `built`.                                                                              |
| Bootloader image size and hashes            | `bootloader.size`, `bootloader.file_sha256`, `bootloader.digest`.                                                                                                      |
| App image facts and hashes                  | `InternalFlash.apps[]` matched by `region`: `slot`, `image.status`, `checked_uptime_s`, `error`, `size`, `digest`, `file_sha256`, `descriptor` (`project_name`, `version`, `date`, `time`, `idf_version`, `secure_version`, `elf_sha256`). |
| Role chips and role boxes                   | `RegionInfo.roles` (`running`, `selected`, `rollback_target`).                                                                                                         |
| Boot line                                   | `RegionInfo.roles`, `InternalFlash.boot` (`state`, `needs_confirmation`; `null` in the untrusted tier) and the otadata record that selects the slot.                   |
| otadata table                               | `InternalFlash.otadata[]`: `sector`, `validity`, `seq`, `state`, `selects`; empty in the untrusted tier.                                                              |
| Tier sentence                               | `InternalFlash.tier`, `limit`, `updates`, `persistent_config`.                                                                                                          |
| Image check running                         | `InternalFlash.checks.status` and `image.status` `checking`.                                                                                                           |
| Download offered                            | `RegionInfo.contents` `public` and `available`; the image download also needs `image.status` `valid` (app) or `bootloader.size` (bootloader).                         |
| Download hash shown in the tray             | An image: the plan's `expected.sha256` (`file_sha256`). The table: `TableRead.md5.stored` read when the download is planned; `DownloadPlan` has no field for it yet and needs one (for example `md5`). |
| Other storage devices                       | `GetStorage.devices` other than `internal`.                                                                                                                             |

#### API the page needs

Two additions to `eveningstar-api` (`v1::storage`):

- **Partition table MD5**, being added by another change: `md5: { stored,
  matches }` on `TableSummary` (the table at the compiled offset) and on
  `TableRead` (the board's table). `stored` is the MD5 the table's MD5 entry
  holds, as 32 lowercase hexadecimal digits; `matches` says whether it equals
  the MD5 of the entries before it. The field is `null` when the table has no
  MD5 entry. The page's "MD5 check", "Stored MD5" hash row, the table
  download's hash and the partition table's "MD5 differs" status come from
  it.
- **The expected span of each region.** `RegionInfo.offset` and `size` are a
  handle's span when the firmware holds one. In the untrusted tier those are
  the board's spans: the bootloader up to the board's table, the table at the
  board's offset, and the running app cut to its image. The strip draws the
  expected span, so the page needs the layout's span of every region, for
  as `RegionInfo.expected: { offset, size }`, `null` for the running app
  listed without a slot. A change adds it before the page depends on it.

- **The rollback target during an update.** The API reports the role
  `rollback_target` on the slot the bootloader would roll back to from the
  running app, which is the other slot. While an update is pending, the running
  app is confirmed and the other slot holds the update, so no slot has the role.
  The page then derives it: when the selected slot isn't the running one, its
  otadata record's state is `new` and the running app's state is `valid`, the
  running slot shows "Rollback target" (the bootloader returns to it if the
  selected app fails to confirm). The firmware's definition of the role is
  unchanged, as the OTA planner relies on it.

The whole-file SHA-256 of an image is already in the API: `file_sha256` on
`AppImage` and on `BootloaderInfo`. The page needs nothing else from the API.

#### Matching entries

- A board entry belongs to the expected region with the same label; the first
  such entry in table order does, and a later one with the same label is an
  extra entry.
- An expected partition with no board entry is **missing**. One whose board
  entry has another type, subtype, offset, size or flags **differs**, in
  those fields. An entry whose label no expected region has is **extra**.
- The firmware's own comparison (`RegionInfo.layout`, `TableSummary.matches`)
  goes position by position, so one missing entry makes every later one
  "differ". The page matches by label instead, so only the missing entry is
  reported. It still reports a table that the firmware rejects: when every
  entry matches by label but `TableSummary.matches` is false or the entries
  are in another order, the partition table is "Differs" with the finding
  "the entries are in a different order than in the expected layout". The
  partition table and the summary line never read "Expected" or "Everything
  matches" while `TableSummary.matches` is false.
- The bootloader and the partition table are compared through their own
  fields (`bootloader.status`; the md5, the findings and the table's offset
  against `TableSummary.offset`).

### Overlaps and shifted entries

These rules are not in the prototype; they keep the strip readable whatever
the board's table holds.

#### Places

Each region has a place on the board for this comparison: an expected region
on the board, its board entry's span; a missing region, its expected span; an
extra entry, its span; the bootloader and the partition table, their expected
spans. Two places **overlap** when they share at least one byte; the overlap
is the range from the later start to the earlier end (end exclusive).

#### Shifted regions

- A region whose board entry has another offset or size is "Differs", as any
  other differing field.
- The strip, the list order and the inspector's usage summary use the
  **expected** span. The board's span appears only in the Expected state
  panel's comparison table and placement bars.
- A gap of the expected layout that a shifted entry reaches into is "Used",
  naming the entries.

#### Overlaps

- The strip never draws one block over another. Every block keeps its own
  place in offset order, and an overlapping extra entry is its own block,
  with the warning icon, after the region it overlaps when they start
  together.
- Gaps are the space covered by no expected region and no extra entry, each
  at least a whole 4 KiB sector or crossing a sector boundary; an uncovered
  remainder inside one sector is not a gap. An extra entry that covers a whole
  gap of the expected layout so replaces it; one that covers part of it leaves
  the uncovered parts as gaps, under the same rule.
- Both sides of an overlap state it. A region that is otherwise as expected
  gets the status "Overlap"; a missing, differing or extra one keeps its word
  and its reason gains the overlap.
- The inspector of each side shows a panel titled "Overlap" after the Expected
  state panel (or after the missing or extra panels): one status line per
  other region, "`<other title>` (`<other subtitle>`) overlaps this region
  from `<start>` to `<end>`, `<size>`." (for a missing region: "The board's
  table places `<other title>` (`<other subtitle>`) in the expected range of
  this region, from `<start>` to `<end>`, `<size>`."), then a span table
  whose rows are "Region" and one "Overlap with `<other title>`" per overlap,
  its key square in the other region's colour. Pointing at an overlap row
  emphasises the other region.
- The summary line counts every overlapping region as not as expected.

#### Ordering

Blocks and rows are ordered by the start of their drawn span. At the same
start, the expected region comes before an extra entry, and extra entries
keep their table order.

#### Several tables or no table

`BoardTable.search` says how the firmware found the board's table:

| `search`   | `table`  | Page                                                                                                                                                                                                                                        |
|------------|----------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `compiled` | present  | The table at the expected offset matches and the bootloader is known to read it. Every comparison runs as above.                                                                                                                         |
| `found`    | present  | The board's table is the one found; comparisons run against it. When its offset is not `TableSummary.offset`, the partition table differs with the finding "the bootloader read the table at `<offset>`, not at `<expected offset>`". |
| `several`  | `null`   | No comparison: the partition table, every partition and every gap are "Unknown". Search sentence: "Partition tables were found at `<offset>` and `<offset>`, and either could be the one the bootloader read."                         |
| `none`     | `null`   | As `several`. Search sentence: "No partition table on the flash has an app entry at the offset the app booted from (`<booted offset>`)."                                                                                                 |
| other      | any      | As `found` with a table, as `several` without one; the search sentence is left out.                                                                                                                                                        |

Without a board table there are no extra entries and no overlaps; the
partition table's Details show "Read from" as "Not known" and the MD5 check of
the table at the expected offset, labelled "MD5 check (table at `<offset>`)".

#### Tiers and limits

The consequence sentence of the summary line and of the reasons:

| Tier        | Sentence                                                                    |
|-------------|-----------------------------------------------------------------------------|
| `full`      | (none)                                                                      |
| `data`      | OTA updates are unavailable because `<limit>`.                             |
| `untrusted` | OTA updates are unavailable and settings are not saved because `<limit>`. |
| other       | The firmware limits what it does with the flash.                            |

| `limit`               | `<limit>`                                                                                                                       |
|-----------------------|----------------------------------------------------------------------------------------------------------------------------------|
| `table`               | the board's partition table differs from the expected layout                                                                     |
| `flash_size`          | the flash is smaller than the expected layout                                                                                    |
| `not_a_slot`          | the app did not boot from one of the expected layout's OTA app slots                                                             |
| `other_table`         | the bootloader is not in the firmware's list of known bootloaders, and another partition table on the flash could be the one it read |
| `bootloader_invalid`  | the bootloader image fails verification                                                                                          |
| `bootloader_unlisted` | the bootloader is not in the firmware's list of known bootloaders                                                                |
| `app_descriptor`      | the running app's descriptor cannot be read                                                                                      |
| other, or `null` below the full tier | the firmware limits what it does with the flash                                                                   |

The sentence appears whenever the tier is below full, also when every region
is as expected.

In the untrusted tier the firmware reads only the boot chain, so: the otadata
table is replaced by its note; the other slot shows "Not read"; the role boxes
"Next boot" and "Rollback target" read "Not known"; the running slot's boot
line is the "boot state not read" one; public regions without a handle offer
no download. When the app booted from no slot (`not_a_slot`), `GetRegions`
lists the running app without a label and leaves the slots and their
configurations out; the page then shows the running app's block ("Running
app", code RA) at its span, and no slot or configuration blocks. As the
expected layout is incomplete then, the page derives no Missing, Differs,
Extra, Used or Overlap status in that case: the board's `ota_*` and
`config_*` entries would otherwise all read as extra. Every partition region
and gap is "Unknown" with the reason "The app did not boot from one of the
expected layout's OTA app slots, so the board's table is not compared." `RegionInfo.expected` does not lift this limit: the regions list leaves out
the slots and configurations then, because a region is named by its role and
the app is not in a role.

### Decisions beyond the prototype

Each rule here settles a case the prototype doesn't show, or replaces what it
shows because the API differs:

- **Untrusted tier.** The prototype's differing-table scenarios show the data
  tier with otadata, both apps and boot roles. A differing table (`limit`
  `table`) is the untrusted tier, so the page shows what "Tiers and limits"
  says.
- **Partition table region size.** The prototype draws the partition table
  as a 4 KiB region with a "Whole region, 4 KiB, raw bytes, with the erased
  padding" download. The firmware's region is the table's 0xC00 bytes
  (`table::MAX_LEN`), so the table download is the whole region. The space
  from 0x008c00 to 0x009000 lies inside the table's sector, so it is not a
  gap: the strip and the list show nothing there.
- **Gap explanation.** The prototype's "…; the gap comes from alignment." is
  not true of every gap (a gap left by a shifted or missing entry), so the
  explanation stops at "Nothing is stored here."
- **Sector remainders.** Uncovered space inside one 4 KiB sector, such as the
  1 KiB after the partition table, is not a gap ("Overview").
- **Label-matched comparison** ("Matching entries") in place of the API's
  positional `layout`.
- **Identity by label**, so colours and codes don't swap when the other slot
  boots.
- **Overlap**, **Used** from shifted entries, and ordering ("Overlaps and
  shifted entries").
- **Several tables, no table, tiers and limits**, and the summary line's
  consequence when every region is as expected. No comparison when the app
  booted from no slot.
- **Fallback.** A selected slot that isn't running is an update only when its
  record's state is `new`; a fallback has its own copy ("Fallback").
- **Selected missing and extra blocks** get a 2 px `--fg` outline; in the
  prototype their own outline hid the selection.
- **Strip minimum share** shrinks with many blocks.
- **Comparison table** gains the `flags` row; the "matches" reason names
  flags; "differing from the layout" became "differing from the expected
  layout".
- **Refresh.** The "Refresh" button is in the shell's header (see "Title
  bar"), disabled and reading "Refreshing…" while it runs. When a reload
  fails, the last line of the region list's summary, with a warning icon,
  reads "Not refreshed: `<failure title>`. Values from `<time>`.".
- **Image checks.** While a slot's image is `checking`, its Details, ruler and
  download read as "Usage summary" and "Details" say; the page polls until
  the checks finish.
- **Invalid images** are named in the summary header.
- **needs_confirmation** is stated in the boot line.
- **Unknown values** from a newer firmware (region, label, state, fault) show
  as the API names them.
- **Other storage devices** are one muted line each in the summary header.
- **The phone crumb** reads "Regions" (the prototype's "Partitions" named a
  tab that no longer exists).
- **Downloads**: the five statuses map the service's richer states as
  "Statuses" says; file names come from the plan; "Clear finished" keeps
  failed items; "Save anyway" stays; "Move up" and "Move down" go.

### Verification on a board

Each case on the dev board (flashed with `flash-all`, then the partition
table replaced with esptool `write-flash 0x8000 <table.bin>`), in the light
and the dark theme, on a desktop and a phone width:

| Board's partition table                                      | What the page shows                                                                                                                                                                                                                                                                                        |
|--------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| As the expected layout                                       | "Everything matches the expected layout."; every row Expected; the strip in offset order with usage bars in BL, PT, OD and both slots; locks on FD, C0, C1 and DG; RS dotted; Booted, Next boot and Rollback target where the board has them; table MD5 "Matches"; full tier, no consequence.             |
| A partition missing (for example `reserved` dropped)        | Untrusted tier. `reserved` alone is "Missing" (dashed orange outline, struck name, row at 0.7 opacity); every other partition Expected; the partition table "Differs" with "reserved is missing"; the summary "2 items are not as expected: Partition table, reserved. OTA updates are unavailable and settings are not saved because the board's partition table differs from the expected layout."; the otadata and the other slot not read. |
| A partition missing in the middle (`ota_1` dropped)         | Only `ota_1` Missing and the partition table Differs; `config_0` and later Expected (the label match, not the positional cascade).                                                                                                                                                                         |
| An extra partition in the gap (`scratch` at 0x00c000, 16 KiB) | The gap block replaced by a `+` block "scratch, Extra entry"; Extra with type and subtype in the reason; the partition table Differs with "scratch is extra"; no download.                                                                                                                                 |
| A shifted partition (`ota_1` at 0x320000, 2 MiB)            | `ota_1` Differs in offset and size; the strip and usage summary at 0x310000 to 0x610000; the comparison table and placement bars with the board's 0x320000 to 0x520000; nothing drawn at the board's position.                                                                                            |
| An extra partition overlapping an expected one (`scratch` at 0x300000, 64 KiB, overlapping the end of `ota_0`) | A separate `+` block after `ota_0` in the strip, both with the warning icon; `ota_0` "Overlap", `scratch` "Extra"; each inspector's Overlap panel with 0x300000 to 0x310000, 64 KiB, naming the other; nothing drawn over another block.                                                          |
| A missing partition and an extra one in its space (`ota_1` dropped, `storage` at 0x310000, 1 MiB) | `ota_1` Missing, its reason "…, and the board's table places storage in its expected space."; `storage` Extra; both Overlap panels with 0x310000 to 0x410000, 1 MiB; the missing block then the extra block, side by side.                                                                       |
| The table at another offset (`found` elsewhere than 0x8000)  | The partition table Differs with "the bootloader read the table at …"; "Read from" the board's offset. A known bootloader reads 0x8000, so this needs a bootloader built with another `CONFIG_PARTITION_TABLE_OFFSET` and the table at 0x8000 erased or different. Every differing-table row above is already `found` at 0x8000. |
| A damaged MD5 entry                                          | The partition table "MD5 differs" (bad, octagon). An ESP-IDF bootloader built with `CONFIG_PARTITION_TABLE_MD5` (the default) refuses such a table and boots nothing, so on a board this needs a bootloader built without the MD5 check; otherwise the state is covered by host tests. |
| A `cargo run` board (probe-rs writes its own bootloader and table) | Untrusted tier; the bootloader "Unknown" or "Expected" as `known.rs` says; the board's own entries (for example `nvs`, `phy_init`, `factory`) compared by label: the expected partitions without a board entry Missing, the board's other entries Extra, overlaps stated on both sides; the otadata and the other slot not read; "Next boot" and "Rollback target" "Not known". |
| A fallback (an erased `ota_1` selected by the otadata, the board booting `ota_0`) | `ota_1` with the plain Next boot chip and its warning, no orange pill; the summary line 4 "The otadata selects ota_1, but the bootloader failed to load it and started ota_0."; the fallback boot lines on both slots.                                                                   |
| Two candidate tables (`several`)                             | Every partition and gap "Unknown"; the summary "The partition table the bootloader read is not known. Partition tables were found at … and …, …".                                                                                                                                                         |

`gen_esp32part` refuses overlapping partitions in a CSV (also with
`--no-verify`), so the overlapping tables are built by editing the entry of a
generated table and writing a new MD5 entry over the entries. Whether the
ESP-IDF bootloader boots with an overlapping table is to be confirmed on the
board first; if it refuses, the two overlap rows are covered by host tests of
the page's derivation only.

Downloads: every public region and image, downloaded through the page,
compared byte for byte with `esptool read-flash` of the same range (`cmp`,
and `sha256sum` against the SHA-256 the page shows): the bootloader image
against its first `bootloader.size` bytes, an app image against its slot's
first `image.size` bytes, the table, the otadata and each whole slot.

Also on the board: a download failing (the device reset mid-download) and
retried; the tray's auto-scroll with the list at the top and scrolled away;
Escape and outside clicks on every popover and the tray; keyboard selection
with Up and Down; hover emphasis from the strip, the list and the span table.

## Later: wiping

Wiping is wanted, but each wipe belongs to the service that owns the region,
none of which exists yet, and a wipe can't be undone, so it waits for
enforced authentication and needs the `configure` scope:

| Region             | What a wipe means                                     | Owner          | Conditions and effects                                                                                                                                                  |
|--------------------|-------------------------------------------------------|----------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `config_alternate` | A secure wipe of the previous firmware's settings     | Config store   | Only after the store has migrated from it. The handle already allows erasing and not programming (`OtherConfig`). A rollback then boots on defaults.                      |
| `app_alternate`    | Removing the rollback target, e.g. a vulnerable image | OTA service    | Through `Ota`, the only writer of the other slot: first both otadata entries select the running slot, so no pending activation points at an erased slot. Full tier, running image `Valid`. No rollback until the next update. |
| `config_current`   | A factory reset of the settings                       | Config service | Erase and reboot, as erasing under a running store leaves its state wrong. The same action as the board's button. Loses the Wi-Fi credentials, so the device may leave the network. |

Erasing a 3 MiB slot is 768 sector erases, about 35 s with interrupts off for
tens of milliseconds (up to about 400 ms) at a time. Each wipe is a long job
under its own watchdog handle, fed per sector and yielding between them.

## Left to build

- **Listing every layout entry when the app booted from no slot.** The slots
  and configurations are left out of `/storage/internal/regions` then, so the
  page compares nothing in that case. Naming a slot without a role needs a new
  region name, which touches the path parameter, the content endpoint and the
  web client's lookup by region name.
- **Re-checks** on request, or periodically to catch flash going bad. Both
  would reuse the check task.
- **The OTA service's writes.** It is the first writer of a public region. The
  generations, the re-check after a write and the download cut were exercised
  only with temporary hooks; a real update should show `checking`, then the
  new image's digests, and cut a download of the slot in progress.
- **A boot identifier in `/storage/internal`**, or the entity tag of each
  region in `/storage/internal/regions`, would let the web UI plan a download
  without reading the storage view again first.

## Not verified yet

Verified on the dev board (full tier after `flash-all`, untrusted tier after
`cargo run`): every endpoint; both digests of the bootloader and both slots
against `esptool image-info` and `sha256sum`; downloads of every public region
byte for byte against `esptool read-flash`; ranges, `416`, resume with
`If-Range`, the tag changing across a reboot; `403`, `404`, `405`, `409` and
`503` with `Retry-After`; a stalled and a killed client; a temporary write
during a download cutting it short; injected hangs naming `storage_check` and
`storage_download`; HTTP latency during checks (about 12 ms) and downloads
(about 21 ms); and the earlier web page served by the device in headless
Chromium, over HTTP and in a secure context through a local proxy, including a
busy wait across a route change.

Not verified:

- The contended case of the large-response mutex: the JSON responses fit the
  TCP buffers, so a stalled client never held it on the board.
- Stack use measured on the board; the frames are bounded by clippy's limit
  at compile time only.
- The web UI over HTTPS on the device, and Chrome's handling of downloads from
  a plain-HTTP page on a real desktop.
- The `Invalid` image path and the data tier on the board (host tests only).
- The redesigned page on a board with: a table at another offset, a damaged
  MD5 (needs a bootloader built without the MD5 check), several tables, a
  `cargo run` board and a fallback to the other slot.

Verified on the dev board with the redesigned page, served by the device and
rendered in headless Chromium (light and dark, 1280 px and 400 px): the
matching layout, a missing `ota_1`, an extra entry in the gap and in the
reserved space, an extra entry over `ota_0` and `ota_1`, a shifted `ota_1`,
two expected entries overlapping each other, and a missing `ota_1` with an
extra entry on its space. For a missing `reserved`, `config_1`, a shifted
`diag`, a resized `config_0`, a renamed `diag` and a retyped `config_0` only
the API's answers were read on the board, and the page's handling of those is
covered by the model's tests. The board booted with every one of those tables
(the bootloader doesn't check overlaps), always in the untrusted tier, and the
strip drew no block over another. The downloads of every public region, by the
API and by the tray, equal `esptool read-flash` byte for byte; the images'
SHA-256 equal the device's `file_sha256`, and the table's stored MD5 equals the
MD5 of its entries.
