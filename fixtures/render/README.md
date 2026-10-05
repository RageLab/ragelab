# Render regression fixtures

`cases.json` declares synthetic, redistributable render cases.

The renderer metadata sidecar is part of the regression record. Pixel baselines should be generated per CI GPU backend/adapter rather than committed as a single universal truth, because different drivers may produce small sampling/rasterization differences.

For exact determinism on the same backend, render the same case twice and run:

```bash
ragelab render compare first.png second.png --tolerance 0 --json
```

For cross-backend regression, use the smallest explicitly justified channel tolerance and keep the metadata sidecars with the CI artifacts.
