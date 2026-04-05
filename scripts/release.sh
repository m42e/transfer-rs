#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/release.sh <version>

Prepare a local release by:
  1. Verifying the repository is clean
  2. Updating the crate version in Cargo.toml and Cargo.lock
  3. Regenerating the demo media
  4. Creating a release commit
  5. Creating an annotated tag

The script does not push the commit or tag.
EOF
}

require_command() {
  local command_name="$1"

  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "required command not found: $command_name" >&2
    exit 1
  fi
}

require_clean_repo() {
  if ! git diff --quiet --ignore-submodules --; then
    echo "repository has unstaged changes" >&2
    exit 1
  fi

  if ! git diff --cached --quiet --ignore-submodules --; then
    echo "repository has staged but uncommitted changes" >&2
    exit 1
  fi

  if [[ -n "$(git ls-files --others --exclude-standard)" ]]; then
    echo "repository has untracked files" >&2
    exit 1
  fi
}

validate_version() {
  local version="$1"
  local semver_regex='^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'

  if [[ ! "$version" =~ $semver_regex ]]; then
    echo "version must look like semantic versioning, for example 1.2.3 or 1.2.3-rc.1" >&2
    exit 1
  fi
}

current_version() {
  python3 - <<'PY'
from pathlib import Path
import re

content = Path("Cargo.toml").read_text(encoding="utf-8")
match = re.search(r"(?ms)^\[package\]\n.*?^version = \"([^\"]+)\"$", content)
if match is None:
    raise SystemExit("failed to determine current crate version from Cargo.toml")
print(match.group(1))
PY
}

update_versions() {
  local old_version="$1"
  local new_version="$2"

  OLD_VERSION="$old_version" NEW_VERSION="$new_version" python3 - <<'PY'
from pathlib import Path
import os
import re

old_version = os.environ["OLD_VERSION"]
new_version = os.environ["NEW_VERSION"]

cargo_toml = Path("Cargo.toml")
toml_content = cargo_toml.read_text(encoding="utf-8")
toml_pattern = re.compile(r"(?ms)^(\[package\]\n.*?^version = \")([^\"]+)(\"$)")
toml_content, toml_count = toml_pattern.subn(
  lambda match: f"{match.group(1)}{new_version}{match.group(3)}",
  toml_content,
  count=1,
)
if toml_count != 1:
    raise SystemExit("failed to update package version in Cargo.toml")
cargo_toml.write_text(toml_content, encoding="utf-8")

cargo_lock = Path("Cargo.lock")
lock_content = cargo_lock.read_text(encoding="utf-8")
lock_pattern = re.compile(
    r'(?ms)^(\[\[package\]\]\nname = "transfer-rs"\nversion = ")([^"]+)("$)'
)
lock_content, lock_count = lock_pattern.subn(
  lambda match: f'{match.group(1)}{new_version}{match.group(3)}',
  lock_content,
  count=1,
)
if lock_count != 1:
    raise SystemExit("failed to update root package version in Cargo.lock")
cargo_lock.write_text(lock_content, encoding="utf-8")

if old_version == new_version:
    raise SystemExit("new version must differ from current version")
PY
}

main() {
  if [[ $# -ne 1 ]]; then
    usage
    exit 1
  fi

  local version="$1"
  local tag="v$version"
  local script_dir
  local repo_root
  local old_version

  script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  repo_root="$(cd "$script_dir/.." && pwd)"

  require_command git
  require_command python3
  require_command cargo
  require_command ffmpeg

  cd "$repo_root"

  validate_version "$version"
  require_clean_repo

  if git rev-parse --verify --quiet "refs/tags/$tag" >/dev/null; then
    echo "tag already exists: $tag" >&2
    exit 1
  fi

  old_version="$(current_version)"
  if [[ "$old_version" == "$version" ]]; then
    echo "crate is already at version $version" >&2
    exit 1
  fi

  update_versions "$old_version" "$version"

  python3 scripts/generate_usage_video.py

  git add Cargo.toml Cargo.lock demo/usage-demo.mp4 demo/usage-demo.gif
  git commit -m "chore(release): cut $tag"
  git tag -a "$tag" -m "$tag"

  echo "Created release commit and tag $tag"
  echo "Next steps:"
  echo "  git push origin HEAD"
  echo "  git push origin $tag"
}

main "$@"