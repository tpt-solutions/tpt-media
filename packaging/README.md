# Package manager templates

These are templates for publishing `tpt` through package managers. **Nothing
here has been submitted anywhere.** Submitting publishes outside this
repository, so it waits for a tagged release.

To use a template after a release `vX.Y.Z`:

1. Replace `VERSION` with `X.Y.Z`.
2. Replace each `SHA256_*` placeholder with the matching line from that
   release's `SHA256SUMS` file.
3. Test on the target platform (`brew install --build-from-source`, `scoop
   install`, `winget validate`) before submitting.

| Target | Template | Submit to |
|---|---|---|
| Homebrew | `homebrew/tpt.rb` | a tap, or homebrew-core after it qualifies |
| Scoop | `scoop/tpt.json` | a bucket |
| winget | `winget/*.yaml` | microsoft/winget-pkgs |

The release assets are single binaries named `tpt-<platform>`, so each
manifest downloads one file and installs it as `tpt`.
