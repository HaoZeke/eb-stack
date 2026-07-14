# Campaign agent prompt

Render instead of hand-editing:

```bash
skills/new-package/render-full-drive \
  --work "$WORK" --repo "$REPO" --robot "$ROBOT" \
  --build-backend podman-rocky9 \
  --recipe path/A.eb [--oracle …] [--stem a] \
  --recipe path/B.eb …

# produces WORK/residuals/{full-drive.sh,campaign-full-drive.md}
```

Default backend is Rocky 9 Podman for `eb --robot`.
