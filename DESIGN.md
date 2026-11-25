# Design Language & UI Patterns

BlueBubbles Native is a GNOME-first desktop client built directly on **GTK 4** and **libadwaita**. It follows GNOME Human Interface Guidelines for layout, keyboard accessibility, and native windowing while defining a cohesive visual language for messaging surfaces, icons, shapes, and colorways.

Run the offline pattern kitchen sink to inspect and interact with all widgets without connecting to a server:

```sh
cargo run --locked -- --patterns
```

## Layout & Navigation

- **Adaptive split workspace**: Desktop widths (`> 680sp`) use `GtkPaned` with a draggable, keyboard-adjustable divider and a single continuous `wide-header-background` behind transparent `AdwHeaderBar` toolbars. Below `680sp`, an `AdwBreakpoint` reparents the sidebar and thread into an `AdwNavigationView` with a native back button.
- **Sidebar header geometry**: `GtkSearchEntry` shares a `34px`-high row with square `34×34px` action buttons (`Preferences`, `Refresh`, and narrow-mode window controls) separated by `8px` gaps and `8px` panel insets. Sync animations run in-place inside the Refresh button's stack so the toolbar never shifts.
- **Virtualized lists**: Both the conversation sidebar and the message timeline use `GtkListView` with `GtkSignalListItemFactory` and typed widget handles (`ConversationRowWidgets`, `MessageRowWidgets`) for deterministic row recycling. Sidebar highlights reflect only the currently visible thread.
- **Preferences**: A floating `740×560` `AdwDialog` with a `200px` left tab rail (**Server**, **Archived Chats**, **Desktop**, **Appearance**) and a right `GtkStack` pane. Forms remain scrollable without visible vertical scrollbars, and pointer clicks suppress focus rings while keyboard navigation preserves `:focus-visible`.

## Shapes, Icons & Motion

- **Continuous-corner squircles** (`src/squircle.rs`): Message bubbles, media contours, and sidebar selection/hover pills use a custom `GskPath` widget (`bb-squircle`) whose mirrored cubic Bézier corners maintain tangent and curvature continuity and cache per allocation.
- **App-owned iconography** (`src/icons.rs`): All UI controls draw directly via Cairo on a uniform `17px` canvas (`20×20` logical space, `1.65` rounded stroke) so icons stay visually consistent and immune to desktop icon-theme mismatches.
- **Tasteful motion**:
  - Three-dot orbital indicator (`src/loading.rs`) during sync/loading, automatically static when GTK animations are disabled.
  - Subtle shimmer sweep on optimistic sending bubbles (`src/squircle.rs`) and a short overshoot-settle scale animation on send confirmation (`src/bubble_lane.rs`).
  - Expandable layered Tapback reaction stacks (`src/reaction_stack.rs`) rendered with alpha-masked drop shadows.

## Colorways & Typography

Colorways (`src/theme.rs`, `src/style.css`) bind 15 semantic GTK `@define-color` roles (`canvas`, `sidebar`, `header`, `surface`, `text`, `muted`, `border`, `divider`, `accent`, `accent_text`, `selected`, `outgoing`, `incoming`, `error`, `error_surface`) plus timestamp and link tokens:

| Colorway | Mode | Character |
| --- | --- | --- |
| **Follow System** | Adaptive | Slate-blue light and dark palettes synced to `AdwStyleManager` |
| **Graphite and Ice Blue** | Dark | Neutral deep charcoal with crisp ice-blue accents |
| **Warm Charcoal and Champagne** | Dark | Warm espresso surfaces with soft champagne highlights |
| **Ink and Periwinkle** | Dark | Deep indigo-ink canvas with periwinkle accents |
| **Black and Neon** | Dark | Near-black surfaces with electric cyan accent and violet bubbles |
| **Porcelain and Cobalt** | Light | Cool porcelain surfaces with cobalt accents |
| **Ivory and Copper** | Light | Warm paper surfaces with copper accents |

High-contrast desktop themes automatically fall back to native libadwaita system tokens (`@window_bg_color`, `@accent_bg_color`, `@borders`).

Text scaling (`Ctrl++`, `Ctrl+-`, `Ctrl+0`, `80%–160%`) scales message and chrome typography while keeping fixed-geometry controls (text-size stepper buttons, circular window controls) static so buttons never drift under the cursor.

## Messaging Patterns

- **Composer**: Multiline `GtkTextView` inside a single rounded surface that promotes controls below full-width text when wrapping; `Enter` sends, `Shift+Enter` inserts a newline. Supports native file picking and drag-and-drop.
- **Links**: Embedded URLs render as clickable inline Pango links inside the message bubble; preview cards stay hidden unless rich metadata (title/description) is available. Standalone URLs render as clickable preview cards.
- **Media**: EXIF-normalized image decoding and FFmpeg video posters run off the GTK main thread; clicking opens an in-window lightbox that displays the thumbnail immediately while loading the full-resolution asset.
- **Context menus**: Right-click (or `Menu` / `Shift+F10`) opens a neutral-focus popover with arrow-key navigation; message menus include the 6-emoji Tapback bar, Reply, Copy Text / Copy URL, and Restore Draft for failed sends.
