# Custom installation and update record

This fork is the source of truth for the custom OpenCode root-tab sidebar,
selection, alignment, and lifecycle integration. Installed files are deployment
copies, not the only backup. No OpenCode core patch is required.

## Validated installation: 2026-10-20

| Component | Validated version or artifact |
| --- | --- |
| Herdr | 0.9.5, custom fork build |
| OpenCode | 2.0.32; newer versions require a separate compatibility check |
| Radar | 1.4.8 with the local animation-deadline fix; 159 tests and invariant checks passed |
| Radar `lib/frame.js` SHA256 | `9209b1888f7daea6c0b91f420352a7d8366325521e0434334a9488889eb4121a` |
| Herdr binary SHA256 | `e42d19a5e26c943fc18d6fab26a5e04c248169e7711888a99e966e8f8ab5be8e` |
| OpenCode hook SHA256 | `607b7668a2c5c2fad537b6b38c3ca80484bbb94fab89e2a6435656d6ea2a5bce` |

Installed targets:

- `~/.local/bin/herdr`
- `~/.config/opencode/herdr-tui-session.js`

Pre-install backup:
`~/.local/state/herdr/backups/status-alignment-20261020-130502.U3BvHj`.

Validation passed: 4,141 Rust tests (14 skipped), 115 integration asset tests,
150 Python maintenance tests, five release-workflow tests, six architecture
tests, seven docs tests, formatting, Clippy, release build, and independent
hook review. The user subsequently reported correct live behavior.

Earlier runs reproduced a startup SIGTERM readiness race on both unchanged
HEAD and the candidate. The final full run passed; that does not fix the
pre-existing race. On list-only OpenCode hosts, reply tombstones expire after
15 seconds; a permanently stale host cache can then restore blocked status.

## Read-only installation check

From this checkout:

```sh
python3 scripts/check-custom-installation.py
python3 -m unittest scripts.test_check_custom_installation
```

The checker compares deployed bytes against the local release artifact and
maintained hook. Missing files or mismatches return nonzero. It never updates,
copies, restarts, or contacts a network service. A match does not prove that
the release artifact was built from current source or that running processes
have loaded it. Rebuild and test after source changes.

## Safe update procedure

1. Commit and push local fixes before updating. Keep the tested commit IDs,
   component versions, and SHA256 values with the deployment record.
2. Finish running agents. Back up the installed files and relevant configuration
   before changing a component. Do not commit credentials or session data.
3. Update only one component. Do not combine an OpenCode upgrade, Herdr rebuild,
   and Radar update into a single untested change.
4. For OpenCode, review release notes and confirm OMO-Slim compatibility
   separately. Preserve the custom hook; do not blindly reinstall an older
   Herdr integration over it. Test the new version before declaring support.
5. For Herdr, incorporate upstream changes into this fork, run its checks, and
   build the fork. An official binary update replaces the custom implementation.
6. For Radar, retain the spinner fix in `/home/nitro/development/herdr-radar`
   until an upstream release includes it. Plugin updates may replace the patched
   installed checkout. Verify the released code before dropping the patch.
7. Run the drift checker and smoke tests. Verify exact tab selection, rapid
   switching, sibling alignment, spinner cadence, running descendants, genuine
   permission/form blocking, and reply clearing on active and inactive tabs.
8. Record the new tested versions, commits, hashes, and rollback backup. Proceed
   to the next component only after these checks pass.

Restarting Herdr terminates all pane processes and agents. Obtain explicit
restart approval and finish active work first. Restart OpenCode to load a
changed hook; restarting OpenCode alone does not load a new Herdr server binary.

For rollback, restore the explicitly chosen backed-up binary and hook while
preserving any newer user changes, then restart only with approval. Do not
delete OpenCode databases or session storage. Retest after rollback.
