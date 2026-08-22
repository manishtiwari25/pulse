# Workflow - Release the Go Code-Context Binary

Use this workflow to turn the standard-library Go helper into downloadable
executables. Source users can always use `go run`; release binaries are the
toolchain-free path for everyone else.

## 1. Prepare the Release

1. Start from a clean, reviewed commit.
2. Choose a tag in the form `code-context-vX.Y.Z`.
3. Run the skill's Go tests and vet checks.
4. Confirm the index schema did not change without a matching schema-version
   update.

## 2. Build Every Supported Target

From `docs/skills/pulse-code-context/`, build with `CGO_ENABLED=0`:

```bash
version=vX.Y.Z
output_dir=dist
mkdir -p "$output_dir"

for target in \
  "darwin amd64" \
  "darwin arm64" \
  "linux amd64" \
  "linux arm64" \
  "windows amd64" \
  "windows arm64"
do
  set -- $target
  goos=$1
  goarch=$2
  suffix=
  if [ "$goos" = windows ]; then
    suffix=.exe
  fi
  CGO_ENABLED=0 GOOS="$goos" GOARCH="$goarch" \
    go build -trimpath -ldflags="-s -w" \
    -o "$output_dir/pulse-code-context_${version}_${goos}_${goarch}${suffix}" .
done
```

Do not commit `dist/` or generated binaries.

## 3. Create Checksums

On macOS:

```bash
(cd dist && shasum -a 256 pulse-code-context_* > SHA256SUMS)
```

On Linux:

```bash
(cd dist && sha256sum pulse-code-context_* > SHA256SUMS)
```

Review `SHA256SUMS` and keep it beside the binaries.

## 4. Publish and Verify

1. Create the approved Git tag.
2. Publish the six binaries and `SHA256SUMS` in one GitHub release.
3. Download one artifact from the release, verify its checksum, and run
   `status`, `index`, `search`, and `related` against a temporary repository.
4. Record the tested tag and platforms in the release notes.

## 5. Recover Safely

- Before publication, discard only the generated `dist/` output and rebuild.
- After publication, do not silently replace assets under the same tag. Mark
  the release as affected and publish a corrected version.
- A bad binary cannot corrupt source code: generated indexes remain external
  and disposable. Still verify that the previous helper version can read its
  own index or rebuild a fresh one.

In technical terms, the release is a reproducible `CGO_ENABLED=0` build matrix
with SHA-256 integrity metadata and an immutable-version recovery path.
