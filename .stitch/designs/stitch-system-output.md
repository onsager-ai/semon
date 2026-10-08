> Tool-generated Stitch design-system evidence. The authoritative specification is ../DESIGN.md. Generated Material palette entries below were inspected and rejected in favor of existing semantic roles.

---
name: Semon Session Inspection
colors:
  surface: '#f9f9f9'
  surface-dim: '#dadada'
  surface-bright: '#f9f9f9'
  surface-container-lowest: '#ffffff'
  surface-container-low: '#f3f3f4'
  surface-container: '#eeeeee'
  surface-container-high: '#e8e8e8'
  surface-container-highest: '#e2e2e2'
  on-surface: '#1a1c1c'
  on-surface-variant: '#434845'
  inverse-surface: '#2f3131'
  inverse-on-surface: '#f0f1f1'
  outline: '#747875'
  outline-variant: '#c4c7c4'
  surface-tint: '#5b5f5d'
  primary: '#000101'
  on-primary: '#ffffff'
  primary-container: '#191d1b'
  on-primary-container: '#818582'
  inverse-primary: '#c4c7c4'
  secondary: '#59605c'
  on-secondary: '#ffffff'
  secondary-container: '#dde4de'
  on-secondary-container: '#5f6662'
  tertiary: '#030101'
  on-tertiary: '#ffffff'
  tertiary-container: '#221b1a'
  on-tertiary-container: '#8d8280'
  error: '#b53232'
  on-error: '#ffffff'
  error-container: '#ffdad6'
  on-error-container: '#93000a'
  primary-fixed: '#e0e3e0'
  primary-fixed-dim: '#c4c7c4'
  on-primary-fixed: '#181c1a'
  on-primary-fixed-variant: '#444845'
  secondary-fixed: '#dde4de'
  secondary-fixed-dim: '#c1c8c3'
  on-secondary-fixed: '#161d1a'
  on-secondary-fixed-variant: '#414844'
  tertiary-fixed: '#eedfdd'
  tertiary-fixed-dim: '#d1c4c2'
  on-tertiary-fixed: '#211a19'
  on-tertiary-fixed-variant: '#4e4543'
  background: '#f9f9f9'
  on-background: '#1a1c1c'
  surface-variant: '#e2e2e2'
  ground: '#ffffff'
  side: '#f7f7f5'
  sunken: '#f1f2ef'
  ink: '#191d1b'
  muted: '#5d6561'
  line: '#e5e7e3'
  lineStrong: '#d3d7d2'
  errorSurface: '#fbeaea'
  working: '#1d6db0'
  waiting: '#8f5700'
typography:
  headline-lg:
    fontFamily: Instrument Sans
    fontSize: 20px
    fontWeight: '600'
    lineHeight: 28px
  headline-lg-mobile:
    fontFamily: Instrument Sans
    fontSize: 17px
    fontWeight: '600'
    lineHeight: 24px
  headline-md:
    fontFamily: Instrument Sans
    fontSize: 16px
    fontWeight: '600'
    lineHeight: 22px
  body-base:
    fontFamily: Instrument Sans
    fontSize: 14px
    fontWeight: '400'
    lineHeight: 20px
  body-base-mobile:
    fontFamily: Instrument Sans
    fontSize: 15px
    fontWeight: '400'
    lineHeight: 22px
  body-reading:
    fontFamily: Instrument Sans
    fontSize: 16px
    fontWeight: '400'
    lineHeight: 24px
  body-secondary:
    fontFamily: Instrument Sans
    fontSize: 13px
    fontWeight: '400'
    lineHeight: 18px
  caption:
    fontFamily: Instrument Sans
    fontSize: 12px
    fontWeight: '400'
    lineHeight: 18px
  code:
    fontFamily: JetBrains Mono
    fontSize: 13px
    fontWeight: '400'
    lineHeight: 18px
rounded:
  sm: 0.125rem
  DEFAULT: 0.25rem
  md: 0.375rem
  lg: 0.5rem
  xl: 0.75rem
  full: 9999px
spacing:
  gutter: 32px
  margin: 24px
  space-xs: 4px
  space-sm: 8px
  space-md: 12px
  space-lg: 16px
  space-xl: 24px
  space-2xl: 32px
---

## Brand & Style

This design system embodies a **Minimalism** and **Corporate / Modern** fusion. It is engineered for a quiet, compact record reader where content takes absolute priority over decoration. The atmosphere is calm, functional, and utilitarian—prioritizing metadata search, readable labeled filters, and discoverable recovery actions without visual noise.

## Colors

The palette is anchored by neutral surfaces and strong ink actions. The light theme utilizes crisp ground surfaces (`#ffffff`) contrasted against structural sidebars and sunken wells. Accent color is strictly ink in both themes. State colors (`working`, `waiting`, `error`) appear exclusively alongside their explicit state text to maintain cognitive clarity. Neutral borders cleanly separate regions without unnecessary decoration.

## Typography

Instrument Sans serves as the primary typeface for interface text and long-form messages, while JetBrains Mono is designated for commands, code blocks, and system identifiers. Row names and headings utilize 500–600 font weights. Text sizes strictly avoid dropping below 12px. Long prose wraps cleanly, and code blocks allow horizontal scrolling within their bounded regions without triggering page-level overflow.

## Layout & Spacing

The layout follows a structured grid system featuring a 296px application sidebar, a 56px toolbar, and generous desktop reading column gutters set to 32px. Phone viewports scale gutters down to 16px, adapting the shell into a responsive drawer at 760px. Spacing relies on a strict 4/8/12/16/24/32px rhythm. All interactive controls on coarse-pointer targets maintain a minimum dimension of 44px for accessibility compliance.

## Elevation & Depth

Visual hierarchy relies primarily on flat list rows and tonal surface separation (`ground`, `side`, `sunken`). Panels and floating overlays utilize precise, subtle shadows to establish distinct layer separation. Low-contrast neutral outlines cleanly delineate interactive regions without visual clutter. Focus states are communicated consistently via a high-visibility 2px ink outline.

## Shapes

The shape language employs controlled, modest corner radii (6px, 10px, and 14px) to maintain a crisp, utilitarian aesthetic. UI containers, inputs, and interactive components feature soft, restrained curves that prioritize structural integrity over excessive playfulness.

## Components

### Buttons & Actions
Compact buttons dominate the interface, utilizing clear ink borders or solid ink fills. Recovery actions (such as Retry and Clear filters) are explicitly discoverable. Destructive or error states use designated error surface treatments.

### Inputs & Search
Search functions as the primary full-width field, paired with secondary labeled fields for Harness and Repository filtering. Inputs feature persistent labels, clear focus outlines, and immediate state feedback.

### Lists & Rows
Session rows and list items remain flat and structured for high-density reading. Row names use medium font weights (500-600) for scannability, accompanied by discrete, state-bound status indicators.

### Disclosures & Tool Steps
Tool-step disclosures cleanly expose invocation names, commands/titles, recorded exit statuses, and expansion states. Expanded outputs clearly distinguish input and output labels, offer direct copy actions, and provide explicit truncation notices.

### Feedback & States
Loading states preserve useful historical records; empty filtered states provide direct access to clear filters; error states retain history and offer actionable recovery prompts without erasing retained data.