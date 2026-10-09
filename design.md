# Design — brie

Locked design system. Future Hallmark runs read this file first; pages defer
to it. Amend intentionally — the file is the rule. First applied to the GitHub
Pages site in `docs/`.

## System
- Genre · modern-minimal, friendly voice
- Macrostructure · Narrative Workflow (hero: H2 split diptych, 5/7, transcript right)
- Theme · custom (vibe: "friendly terminal at night, one amber signal"). The palette is brie's own TUI theme, brand-locked.
- Axes · dark / geometric-sans / chromatic-amber ~87°
- Nav · N1b, frost on scroll · Footer · Ft5 statement

## Tokens (canonical · `docs/tokens.css` is the source of truth)
```css
:root {
  --color-paper:      oklch(13.5% 0.009 273);   /* #07080c */
  --color-paper-2:    oklch(17.4% 0.013 271);   /* #0e1016 */
  --color-paper-3:    oklch(21.5% 0.015 272);
  --color-rule:       oklch(29.4% 0.015 274);   /* #2a2c34 */
  --color-ink:        oklch(97.0% 0.004 271);   /* #f4f5f8 */
  --color-muted:      oklch(75.6% 0.022 264);   /* #a9b0be, secondary text */
  --color-accent:     oklch(85.0% 0.174 87);    /* #ffc400, amber, the one signal; also the working status */
  --color-accent-ink: oklch(16.5% 0.035 85);    /* #140e00 */
  --color-focus:      oklch(85.0% 0.174 87);
  --color-bad:        oklch(65.5% 0.231 26);    /* #ff3b3b, blocked */
  --color-info:       oklch(79.6% 0.111 245);   /* #7cc4ff */

  --font-display: "Bricolage Grotesque", ui-sans-serif, sans-serif;  /* 800 headings, tight tracking */
  --font-body:    "Geist", ui-sans-serif, sans-serif;                /* 350 on dark */
  --font-mono:    "JetBrains Mono", ui-monospace, monospace;         /* code and TUI captures only */

  /* 4-pt spacing scale, named: --space-3xs … --space-4xl. See tokens.css.   */
  /* Type scale, 1.25 (major-third) ratio: --text-sm … --text-3xl, --text-display (≤ 4.5rem). */

  --ease-out: cubic-bezier(0.16, 1, 0.3, 1);
  --dur-micro: 120ms;  --dur-short: 220ms;  --dur-long: 420ms;

  --radius-sm: 0.375rem;  --radius-md: 0.75rem;  --radius-pill: 999px;
}
```

## CTA voice
- Primary · amber fill, `--color-accent-ink` text · pill · 44 px tall, 24 px side padding · weight 600 · one per fold
- Secondary · outline pill (nav) or text link with a trailing arrow · same height, never wraps

## Motion stance
- Silent. One entrance on load (8 px rise, 420 ms, 70 ms stagger), a frosted nav on scroll, a 3 px arrow nudge on hover.
- Reduced-motion fallback · ≤150 ms opacity crossfade.

## Voice and content rules
- Friendly, specific, declarative. Name the command, the file, the number. No "seamless", "unleash", "supercharge".
- Every figure comes from the code or from real output. No invented stats, testimonials or logos.
- Show the product as it is: screenshots are real renders of the TUI (ratatui test buffer to HTML, cell classes mapped to tokens). Never draw fake window chrome.
- Headings are roman. No eyebrows, except the numbered stage labels of the workflow.
- Mono is for code and captures. Glyphs outside the font (○ ◐ ● ✕ braille) sit in a 1ch box so the grid holds.

## Exports
`docs/tokens.css` is the source of truth. For Tailwind v4 `@theme`, DTCG
`tokens.json`, or shadcn/ui CSS variables, ask *"extend design.md with
Tailwind exports"* (or the format you want) — Hallmark will append them per
`export-formats.md`.
