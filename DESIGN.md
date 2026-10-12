---
name: Lavdeck
description: A native macOS menu bar console for a DJI Mic receiver; two channel strips and a master section on an anodized deck.
colors:
  deck: "#e3e5e8"
  deck-dark: "#1c1e21"
  face: "#f7f8f9"
  face-dark: "#26292d"
  well: "#ffffff"
  well-dark: "#16181b"
  hairline: "#cdd2d7"
  hairline-dark: "#3a3f45"
  hairline-hc: "#6b737b"
  hairline-hc-dark: "#9aa3ac"
  ink: "#121416"
  ink-dark: "#f1f3f5"
  on-ink: "#ffffff"
  on-ink-dark: "#121416"
  mute: "#5a636b"
  mute-dark: "#a9b1b9"
  plate: "#121416"
  plate-text: "#ffffff"
  plate-sub: "#c9d0d6"
  led-off: "#d5d9dd"
  led-off-dark: "#34393f"
  signal-green: "#18a957"
  signal-amber: "#e4a100"
  signal-red: "#d32f2f"
  rec: "#c4261f"
  rec-text: "#c4261f"
  rec-text-dark: "#ff6b61"
  warn-fill: "#fbf0dc"
  warn-fill-dark: "#3a2e14"
  warn-text: "#6b4300"
  warn-text-dark: "#ffd27a"
typography:
  display:
    fontFamily: "SF Pro (system), -apple-system"
    fontSize: "20pt"
    fontWeight: 600
    fontFeature: "tnum"
  title:
    fontFamily: "SF Pro (system), -apple-system"
    fontSize: "13pt"
    fontWeight: 600
  body:
    fontFamily: "SF Pro (system), -apple-system"
    fontSize: "12pt"
    fontWeight: 400
  body-strong:
    fontFamily: "SF Pro (system), -apple-system"
    fontSize: "12pt"
    fontWeight: 600
  label:
    fontFamily: "SF Pro (system), -apple-system"
    fontSize: "11pt"
    fontWeight: 700
    letterSpacing: "0.6-0.9pt"
  mono:
    fontFamily: "SF Mono (system monospaced), ui-monospace"
    fontSize: "11pt"
    fontWeight: 500
rounded:
  segment: "1.5pt"
  cap: "4pt"
  plate: "5pt"
  arm: "6pt"
  switch: "7pt"
  module: "9pt"
spacing:
  tick: "2pt"
  inset: "4pt"
  gutter: "6pt"
  strip: "8pt"
  deck: "10pt"
components:
  channel-strip:
    backgroundColor: "{colors.face}"
    rounded: "{rounded.module}"
    padding: "8pt"
    width: "100pt"
  master-section:
    backgroundColor: "{colors.face}"
    rounded: "{rounded.module}"
    padding: "8pt"
  name-plate:
    backgroundColor: "{colors.plate}"
    textColor: "{colors.plate-text}"
    typography: "{typography.body-strong}"
    rounded: "{rounded.plate}"
    padding: "5pt 7pt"
  name-plate-recording:
    backgroundColor: "{colors.rec}"
    textColor: "{colors.plate-text}"
    rounded: "{rounded.plate}"
  name-plate-hollow:
    backgroundColor: "{colors.well}"
    textColor: "{colors.ink}"
    rounded: "{rounded.plate}"
  arm-button:
    backgroundColor: "{colors.well}"
    textColor: "{colors.mute}"
    typography: "{typography.label}"
    rounded: "{rounded.arm}"
    padding: "4pt 0"
  arm-button-armed:
    backgroundColor: "{colors.well}"
    textColor: "{colors.ink}"
    rounded: "{rounded.arm}"
  arm-button-live:
    backgroundColor: "{colors.well}"
    textColor: "{colors.rec-text}"
    rounded: "{rounded.arm}"
  mode-switch:
    backgroundColor: "{colors.well}"
    textColor: "{colors.mute}"
    typography: "{typography.label}"
    rounded: "{rounded.switch}"
    padding: "6pt 0"
  mode-switch-selected:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.on-ink}"
  led-meter:
    backgroundColor: "{colors.well}"
    rounded: "{rounded.plate}"
    padding: "4pt"
  fader-cap:
    backgroundColor: "{colors.plate}"
    rounded: "{rounded.cap}"
    width: "34pt"
    height: "16pt"
  note:
    backgroundColor: "{colors.warn-fill}"
    textColor: "{colors.warn-text}"
    typography: "{typography.body}"
    rounded: "{rounded.switch}"
    padding: "7pt 9pt"
---

# Design System: Lavdeck

## Overview

**Creative North Star: "The Channel Strip"**

The popover is a small mixing console, not a settings list. Each transmitter is a vertical channel strip (name plate, arm button, link LED, LED meter ladder, battery cells, readout); the receiver is the master section (gain fader, MONO | STEREO switch, capture switches, Record). The strips and master sit as raised faces on an anodized deck, each edged by a 0.5pt hairline. Every element is a piece of hardware vocabulary: scribble strip, LED, detented fader, two-position switch, transport button. This is the world's refusal of the category default of Control Center rows and toggles.

Density is console density: 11-13pt type, 2pt segment gaps, 6-10pt gutters, a fixed 372pt-wide deck. Colour is almost entirely neutral grey; signal colour appears only where hardware would light up (LEDs, meter segments, the record state, warning notes). Truth over decoration is a visual rule here: values the receiver does not report show as an ellipsis or "not linked", and the provisional battery reading always carries its "est" tag.

This is a native macOS surface. Type is the system face (SF Pro, with SF Mono for machine data), icons in the popover footer and notes are SF Symbols, the menu bar glyph is drawn in code at 18x14pt, and the Settings window is a stock grouped `Form`. Light, dark and Increase Contrast appearances resolve per token through `NSColor` dynamic providers.

**Key Characteristics:**
- Three-column deck: two 100pt channel strips plus a flexible master section, 6pt apart.
- Raised faces on a darker deck, 9pt corners, 0.5pt hairline on every module.
- Black scribble-strip name plates that turn record-red and name their file during a take.
- Signal colours (green, amber, red, rec) are fixed across appearances; only the neutrals adapt.
- SF Mono is reserved for machine data: Bluetooth IDs, firmware, dBFS, file names.
- Status is never colour alone: LEDs switch solid/hollow, glyph states change shape.

## Colors

A cool anodized-aluminium neutral family with four fixed signal lamps.

### Primary
- **Record Red** (rec): the live take, and only the live take. Fills name plates while recording, and colours the armed strip's REC dot and border, the header REC dot, the Record button's idle disc and the recording menu bar glyph.
- **Record Red Text** (rec-text / rec-text-dark): the "REC" caps on the live arm button and in the header. Identical to Record Red in light appearance; lifted in dark appearance so 11pt bold caps clear 4.5:1 on the dark well and deck (6.38:1 and 5.99:1).

### Secondary
- **Signal Green** (signal-green): linked LED and the lower eight meter segments.
- **Signal Amber** (signal-amber): meter segments 9-10, the hollow dropped-link LED, and amber marks in the menu bar glyph (dropped mic, low battery, error triangle).
- **Signal Red** (signal-red): top two meter segments (clip zone). Distinct from Record Red; never used for recording state.

### Tertiary
- **Warning Parchment** (warn-fill / warn-text, with dark variants): the note banners below the deck and the outdated-firmware chip in a strip readout.

### Neutral
- **Anodized Deck** (deck / deck-dark): the popover background.
- **Strip Face** (face / face-dark): channel strip, master section and the receiver-absent panel.
- **Well** (well / well-dark): recessed surfaces: meter housing, arm button, switch track, hollow name plate.
- **Hairline** (hairline / hairline-dark; Increase Contrast: hairline-hc / hairline-hc-dark): every 0.5pt module edge and the rule above Record. The Increase Contrast variants are defined in Theme.swift but not yet proven in a capture; treat them as provisional until a high-contrast screenshot shows them resolving.
- **Console Ink** (ink / ink-dark) and **On Ink** (on-ink / on-ink-dark): primary text, filled battery cells, the selected switch segment and the Stop square.
- **Silkscreen Grey** (mute / mute-dark): secondary text, caps labels, unselected scale and switch labels.
- **Scribble Plate** (plate, plate-text, plate-sub): the name plate is black in both appearances; its sub-line is a pale grey.
- **Unlit LED** (led-off / led-off-dark): unlit meter segments, empty battery cells, the fader track, the off link LED.

### Named Rules
**The Red Means Live Rule.** Record Red appears only while a take is running. Armed-but-idle is neutral ink (filled dot, "ARMED"); unarmed is a hollow ring and "OFF". If something is red and nothing is recording, it is a bug.

**The Fixed Lamps Rule.** Signal green, amber, signal red and Record Red are single values in every appearance when used as fills. Only neutrals, the well and the warning pair adapt to dark and Increase Contrast. A lamp used as text gets its own text token that must clear 4.5:1 on its ground, as Record Red Text does.

## Typography

**Display Font:** SF Pro (system)
**Body Font:** SF Pro (system)
**Label/Mono Font:** SF Mono (system monospaced design)

**Character:** The system face reads as the Mac it lives on; SF Mono plays the part of the printed data on a console scribble strip.

### Hierarchy
- **Display** (600, 20pt, tabular figures): the elapsed-time counter in the master section while recording.
- **Title** (600, 13pt): the "Wireless Mic Rx" header.
- **Body** (400, 12pt): footer, notes, fader scale, secondary readouts. Fader scale and firmware readout use monospaced digits.
- **Body Strong** (600, 12pt): name plate titles (tracked 0.5pt), the selected fader step, "Record" / "Stop", footer values.
- **Label** (700, 11pt, all caps, tracking 0.6-0.9pt): console silkscreen: GAIN, MONO / STEREO, REC / ARMED / OFF.
- **Mono** (500, 11pt): Bluetooth IDs, file names, dBFS, firmware, "not rec".

### Named Rules
**The Mono Is Data Rule.** SF Mono is for values the receiver or the filesystem produced. Human labels are never set in mono.

**The Silkscreen Rule.** Caps labels name a control that sits directly beneath or beside them, the way panel printing does. They never introduce a section of prose.

## Layout

A fixed 372pt-wide popover with 10pt outer padding and 10pt vertical rhythm between rows: header, deck, footer, notes. The deck is a top-aligned three-column row with 6pt gutters: TX1 strip (100pt), TX2 strip (100pt), master section (fills the remainder). Strips stack their parts at 8pt spacing inside 8pt padding; the master stacks at 10pt. Header and footer are inset a further 4pt to align with module content.

The fader is 132pt tall at rest and shrinks to 84pt while recording so the elapsed counter fits without growing the deck. Notes stack full-width below the footer; they are the only element whose height varies with content. When no receiver is connected, the deck is replaced by a single face panel (12pt padding) with a bold title and one explanatory line.

Small-scale rhythm: 2pt between meter segments and battery cells, 4pt meter housing inset.

## Elevation & Depth

Depth is tonal, not shadowed: a darker deck, raised lighter faces, and recessed white wells, each separated by 0.5pt hairlines. In dark appearance the order holds (deck #1c1e21, faces lighter, wells darkest).

### Shadow Vocabulary
- **Fader cap** (black at 30% opacity, 2pt radius, 2pt down): the only shadow in the system, on the only object that physically stands proud of the panel. The cap also carries a top-to-bottom dark gradient and a white index line.
- **Link LED halo** (signal green at 18% opacity, 13pt disc behind a 9pt lamp): a glow, not a shadow; linked state only.

### Named Rules
**The Hardware-Only Relief Rule.** Shadows and gradients belong to physical controls that would cast them (the fader cap). Modules, plates and notes stay flat.

## Shapes

Softly squared hardware. Radii step with object size: 1.5pt meter segments and battery cells, 4pt fader cap and firmware chip, 5pt name plates and meter housing, 6pt arm buttons and the Stop square, 7pt switch and notes, 9pt modules. Round forms are reserved for lamps and transport: link LED, record disc, arm dot, header REC dot, and the capsule fader track. Every module, plate, housing and switch carries a 0.5pt hairline border; the armed-and-live arm button thickens to 1pt in Record Red at 60%.

The menu bar glyph (18x14pt) is two vertical channel bars and a short master stub with 1pt corner radii. States change shape as well as colour: recording replaces the stub with a dot, a dropped mic hollows its bar in amber, low battery draws an amber battery, error draws an amber triangle, absent draws all three parts hollow.

## Components

### Buttons
- **Record / Stop:** a transport button under a hairline rule at the base of the master section. Idle: a 26pt Record Red disc with a 15% black edge, "Record" in body strong, "⌥⌘R" in body mute. Recording: a 22pt ink square (6pt radius), "Stop". Disabled (nothing armed) drops to 45% opacity with a tooltip.
- **Arm button:** full strip width, well fill, 6pt radius, 8pt dot plus label caps. OFF: hollow mute ring, mute text. ARMED: ink dot and ink text. Live: Record Red dot, "REC" in Record Red Text, and a Record Red border at 60%. Locked during a take.
- **Capture switches:** a row of independent SCREEN | CAMERA buttons under the MONO | STEREO switch, one per capture source a plugin offers, 6pt apart, 7pt radius, caps label at 10pt. Off: well fill, mute text. On: ink fill, on-ink text, like a selected mode segment. While recording they lock: a source capturing in the take shows Record Red Text with a Record Red border at 60% (like a live arm button), and an off source drops to 45%. The row only appears when a capture plugin is installed; the gain fader gives up its height (132pt to 96pt) so the master section still ends level with the strips.
- **Footer actions:** plain SF Symbol buttons (folder, gear) in mute; the format readout is itself the Reveal in Finder button.

### Chips
- **Firmware chip:** the strip's mono firmware readout gains a warning-parchment fill (4pt radius, 5x2pt padding) only when that transmitter is behind the other.

### Cards / Containers
- **Corner Style:** 9pt.
- **Background:** strip face on the deck.
- **Shadow Strategy:** none; see Elevation & Depth.
- **Border:** 0.5pt hairline.
- **Internal Padding:** 8pt (strips, master), 12pt (absent panel).

### Inputs / Fields
- **Mode switch:** two equal segments in a 7pt-radius well with a hairline. Selected segment is an ink fill with on-ink caps; unselected is mute on well. Disabled while recording or while a change is pending; the unselected side fades to 45%.
- **Gain fader:** 6pt led-off capsule track, 34x16pt cap, five detents (+12, +6, 0 dB, −6, −12) labelled to the right with true minus signs. Drag snaps between detents with an alignment haptic; the cap position follows the receiver's confirmed gain and dims to 55% while a change is pending. Scale labels are also click targets.
- **Settings:** the stock macOS grouped Form at 460pt; no custom styling.

### Navigation
Not applicable: a single popover with a Settings link in the footer.

### Channel Strip (signature)
Top to bottom: name plate, arm button, link LED, LED meter, battery cells, mono readout. The **name plate** is a black scribble strip with the transmitter name and its Bluetooth ID; during a take it fills Record Red and shows "→ file name" (stereo adds "· L" / "· R"); a strip left out of the take is drawn hollow (well fill, ink title, "off") so it stays readable; an unlinked strip dims to 45%. The **LED meter** is a 12-segment ladder of 26x7pt segments (8 green, 2 amber, 2 red) in a well housing, with a 1.2s peak hold. The **link LED** is solid green with halo when linked, a hollow amber ring when a known mic has dropped, and an unlit dot otherwise. **Battery cells** are seven 5x10pt ink cells followed by "est".

### Note
Full-width warning-parchment banner, 7pt radius, an 11pt SF Symbol leading 12pt body text. Used for errors, dropped transmitters, low battery, solo-arm explanation and firmware mismatch.

### Motion
Meter segments ease out over 80ms; the recording state change cross-fades over 160ms (ease in-out). Both are removed under Reduce Motion.

## Do's and Don'ts

### Do:
- **Do** keep Record Red for the live take only; armed-but-idle is ink (The Red Means Live Rule). Set red type in Record Red Text, never in the fill token.
- **Do** give every module, plate, housing and switch a 0.5pt hairline, and define Increase Contrast variants for any new hairline-weight token.
- **Do** pair every colour state with a shape or text change: hollow vs solid LED, dot vs triangle in the glyph, REC / ARMED / OFF labels.
- **Do** set receiver- and file-produced values in SF Mono and mark provisional values (battery "est").
- **Do** add new controls as hardware vocabulary inside a strip or the master section, using the existing well / face / deck layering.
- **Do** honour Reduce Motion on every animation.

### Don't:
- **Don't** use Signal Red (the meter clip colour) for recording state, or Record Red for the meter.
- **Don't** put shadows or gradients on modules, plates or notes; relief belongs to the fader cap.
- **Don't** make the signal lamp fills appearance-dynamic; adapt neutrals instead, and give a lamp a separate text token when it sets type.
- **Don't** set human labels in SF Mono or machine values in caps.
- **Don't** fabricate DJI branding or logos anywhere in the surface.
