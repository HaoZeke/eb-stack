# Hermes full-drive prompt

**Do not hand-fill this file for campaigns.** Render instead:

```bash
skills/new-package/render-full-drive \
  --work "$WORK" --repo "$REPO" --robot "$ROBOT" \
  --recipe path/to/A.eb [--oracle fixtures/.../A.eb] [--stem a] \
  --recipe path/to/B.eb ...

# writes:
#   $WORK/residuals/full-drive.sh
#   $WORK/residuals/hermes-full-drive.md
```

Templates live in `templates/full-drive.sh.tmpl` and
`templates/hermes-full-drive.md.tmpl`. Example packaging:

```bash
skills/new-package/examples/render-eon-qmcpack.sh
```

Then start herdr with `-q "$(cat $WORK/residuals/hermes-full-drive.md)"`.
