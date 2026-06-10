# Git Hooks

This directory contains project-level git hooks. Git does not use them automatically — you must configure git to look here after cloning:

```sh
git config core.hooksPath .githooks
```

Run this once per clone. No other setup is needed.

## Hooks

### pre-push

Fires before any `git push`. It inspects every ref being pushed and **aborts** if a `v*` tag is being pushed whose version does not match the `version` field in `Cargo.toml`.

**Example — blocked push:**

```
$ git push origin v0.3.0
pre-push: ERROR: pushing tag 'v0.3.0' but Cargo.toml version is '0.2.2' (expected tag 'v0.2.2').
pre-push: Update Cargo.toml to match the tag, or push 'v0.2.2' instead.
error: failed to push some refs to 'origin'
```

**Correct workflow for a release:**

1. Bump the version in `Cargo.toml`.
2. Commit the change.
3. Create the matching tag: `git tag v<version>`
4. Push both: `git push origin main v<version>`

Non-tag refs (branches, HEAD, etc.) are never blocked by this hook.
