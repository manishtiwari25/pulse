# Test report - dark mode toggle

Branch `pulse/chat-a1b2c3`, two commits plus one uncommitted stylesheet change.

## Summary

| Suite | Tests | Passed | Failed | Time |
|-------|------:|-------:|-------:|-----:|
| unit | 38 | 38 | 0 | 1.9 s |
| settings page | 4 | 4 | 0 | 0.6 s |
| **total** | **42** | **42** | **0** | **2.5 s** |

## What was checked

- The **Theme** row renders three choices (system, light, dark) as a radio group.
- Picking a choice writes `theme` to local storage and flips the `dark` class on `<html>`.
- The inline script in `layout.tsx` applies the stored choice before first paint, so there is no flash on reload.
- The muted label uses `text-muted-foreground` at 70%, as the mockup draws it.

## Still open

- [ ] `theme.css` is not committed yet - it waits on a check against the light theme.
- [ ] A screenshot test for the light theme would make the contrast check repeatable.

```text
npm test
> demo-app@0.4.0 test
> vitest run

 Test Files  7 passed (7)
      Tests  42 passed (42)
   Duration  2.51s
```
