# View Example — Plan

## Done

- Introduced `View` trait, `ComponentView` trait, and `ViewElement` struct as a unification of `Component`, `RenderOnce`, `AnyView`, and `Render`
- Initialized example of the composition this can achieve with the editor
- Made `ExampleInput` a proper `View` with its own `ExampleInputState` entity — demonstrates independent caching boundaries, focus tracking, and separation of concerns
- Made `ExampleTextArea` a `ComponentView` — demonstrates stateless wrapper pattern where the inner `EntityView` does the caching
- Analyzed `use_state` observer behavior — confirmed it's necessary and correct for component-local state (non-view entities aren't in the dispatch tree, so `mark_view_dirty` can't propagate)

## Next

- Add a render log showing coarse-grained caching — log `render()` calls on ExampleInput, ExampleTextArea, and ExampleEditor to show that sibling isolation works and FlashState only re-renders chrome
- RootView type — a window root that takes an `Entity<V>` directly. Introduce in a backwards-compatible way alongside `open_window`. Use in this example.
- Move focus handles out to the input and textarea, stop blinking when not focused
- Tab index support so the demo doesn't need mouse movement
- De-fluff LLM generated code

## Design Decisions

### View as a separation-of-concerns boundary

The `ExampleInput` View demonstrates the key principle: a View should own its concerns and delegate everything else.

- `ExampleInputState` creates the editor internally (`cx.new(|cx| ExampleEditor::new(cx))`) — the parent never sees it
- Focus is tracked via `on_focus`/`on_blur` listeners on the state entity — render never reads the editor
- The editor is passed as a `ViewElement` child and trusted to manage its own rendering (blink, text, cursor)
- `render()` only reads from `self.state` (ExampleInputState), giving it a clean reactive boundary

The parent just does:
```
let input_state = window.use_state(cx, |window, cx| ExampleInputState::new(window, cx));
ExampleInput::new(input_state).width(px(320.)).color(input_color)
```

### `use_state` as the allocation pattern for View entities

`use_state` in the parent creates the View's backing entity. The View's `new()` is a pure constructor that takes the entity handle. The entity's `new()` can internally chain to `cx.new()` to create child entities (like the editor), keeping allocation hierarchical and self-contained.

### `use_state` observer — keep as-is

`use_state` creates an observer that notifies the parent view when the state entity changes. This is correct because:

1. Non-view entities aren't in the dispatch tree, so `mark_view_dirty` can't propagate their notifications upward
2. `use_state` entities are component-local (single owner), so notifying the parent is always the right granularity
3. The alternative (fixing the tracking layer to auto-propagate all entity reads) would be catastrophic for large shared entities like `Project`, where dozens of views read it but only care about specific changes

### Entity taxonomy for reactivity

| Entity type | On notification | Mechanism |
|---|---|---|
| View entity (ExampleEditor) | Own ViewElement cache invalidates | `dirty_views` + dispatch tree |
| Component-local (use_state) | Parent view re-renders | `use_state` observer |
| Shared, universally relevant (Theme) | All readers re-render | Explicit `observe` today; `use_global` (v2) |
| Shared, selectively relevant (Project) | Only subscribers re-render | Explicit `subscribe` |

### View vs ComponentView vs EntityView

- **View** = has its own entity, gets caching, creates a reactive boundary. The component owns its state and delegates child rendering. Use when the component has state or when caching its render output matters for performance (ExampleInput).
- **ComponentView** = stateless wrapper, no caching, always re-renders when parent does. Use when the component is cheap and the inner child has its own cache (ExampleTextArea).
- **EntityView** = the entity IS the view. The data-owning entity renders itself directly. Use for entities that need full control over their element tree (ExampleEditor).

### V2: `use_global` for universal reactivity

A future `cx.use_global::<ThemeSettings>()` API that auto-wires reactive dependencies for globally-shared "value" entities (theme, settings). Would eliminate dozens of boilerplate `observe_global` + `cx.notify()` callbacks throughout the codebase.
