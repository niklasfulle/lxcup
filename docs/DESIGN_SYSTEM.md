# Frontend design system

This document describes the visual and interaction conventions of the lxcup
control-plane UI. It is intentionally grounded in the current React/Tailwind
implementation rather than a separate design-token framework.

## Layout

- The application uses a persistent left navigation, a fixed top header, a
  scrollable central content region, and an optional right activity sidebar.
- Keep the header stationary while the page body scrolls. The activity sidebar
  remains full-height and independently scrollable while visible; its
  collapsed state is persisted in the browser.
- At narrow widths, reduce secondary breadcrumb detail and let content grids
  collapse without horizontal page scrolling. Keep essential controls usable.
- Resource pages should share a clear title/description, a consistent add
  action, and a dedicated inventory/list area. Resource creation forms stay
  collapsed until the user chooses to add a resource.

## Styling approach

- Tailwind utility classes live on the JSX elements they style. Use the existing
  conditional class helper where needed; do not add a centralized `ui.ts`
  catalogue for component styles.
- Reuse CSS custom properties from `frontend/src/styles.css` for semantic
  surfaces, lines, text, primary color, and status colors. `data-theme` selects
  the active theme.
- Keep global CSS limited to reset/base behavior, theme variables, and cases
  that are awkward or impossible to express with the current Tailwind setup.
- Prefer the existing square-edged, compact operational-console language.
  Establish hierarchy with spacing, restrained surface contrast, borders, and
  typography rather than oversized banners or ornamental gradients.

## Components and interaction

- Use consistent button height, padding, text size, and state treatment for
  equivalent actions. Primary actions should be visually prominent without
  stretching across a whole card unless they submit a full-width form by
  design.
- Give icon-only controls an accessible name and title. Preserve keyboard
  focus, native button/link semantics, and a visible focus indicator.
- Status chips use the shared job-status labels/styles in
  `frontend/src/jobStatus.ts` where applicable. Keep user-facing status terms
  consistent across workflow lists, details, notifications, and the activity
  sidebar.
- Every data surface should distinguish loading, empty, error, and populated
  states. Job failure views should link to complete execution logs rather than
  hiding diagnostics behind a generic error label.
- Tooltips should supplement, not replace, visible labels for important
  controls. Do not rely on hover alone for essential guidance.

## Validation for UI changes

- Test important states and interactions with Vitest and Testing Library.
- Run the production build to catch TypeScript and Tailwind errors.
- Check desktop and narrow layouts when changing shared shell, navigation,
  forms, tables, notifications, or sidebars. Prefer existing project visual
  conventions and reusable behavior over one-off styling systems.
