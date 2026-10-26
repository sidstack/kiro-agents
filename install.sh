#!/bin/sh
# Install verified GitHub release binaries. No daemon or Kiro state is changed.
set -eu
repository='https://github.com/sidstack/kiro-agents'
requested_version=''
install_dir=''
uninstall=0
usage() {
    cat <<'HELP'
Usage: install.sh [--version VERSION] [--install-dir DIRECTORY] [--uninstall]
Defaults: latest stable release, $HOME/.local/bin. Explicit beta tags are supported.
Requires Linux x86_64 with glibc 2.35+, curl, tar and coreutils. Never uses sudo.
HELP
}
fail() { printf 'kiro-agents installer: %s\n' "$*" >&2; exit 1; }
while [ "$#" -gt 0 ]; do
    case "$1" in
        --version|--install-dir)
            [ "$#" -ge 2 ] && [ -n "$2" ] || fail "$1 requires a value"
            case "$1" in --version) requested_version=$2 ;; --install-dir) install_dir=$2 ;; esac
            shift 2 ;;
        --uninstall) uninstall=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) fail "Unknown option: $1 (see --help)" ;;
    esac
done
if [ -z "$install_dir" ]; then
    [ -n "${HOME:-}" ] || fail 'HOME is unset; pass --install-dir'
    install_dir="$HOME/.local/bin"
fi
case "$install_dir" in /*) ;; *) install_dir="$(pwd)/$install_dir" ;; esac
if [ "$uninstall" -eq 1 ]; then
    [ -z "$requested_version" ] || fail '--uninstall cannot be combined with --version'
    rm -f -- "$install_dir/kiro-agents"
    printf 'Removed %s/kiro-agents; existing hosts and sessions are unchanged.\n' "$install_dir"
    exit 0
fi
for dependency in curl tar sha256sum mktemp install mv cmp sort awk grep uname getconf cat sleep; do
    command -v "$dependency" >/dev/null 2>&1 || fail "Missing required command: $dependency"
done
[ "$(uname -s)" = Linux ] || fail 'Only Linux release binaries are available'
case "$(uname -m)" in x86_64|amd64) ;; *) fail 'Only Linux x86_64 release binaries are available' ;; esac
glibc_version=$(getconf GNU_LIBC_VERSION 2>/dev/null) || fail 'A glibc-based Linux system is required (musl is unsupported)'
glibc_version=${glibc_version#glibc }
glibc_major=${glibc_version%%.*}
glibc_minor=${glibc_version#*.}; glibc_minor=${glibc_minor%%.*}
case "$glibc_major:$glibc_minor" in *[!0-9:]*|:|*:|:*) fail 'Cannot determine glibc version' ;; esac
[ "$glibc_major" -gt 2 ] || { [ "$glibc_major" -eq 2 ] && [ "$glibc_minor" -ge 35 ]; } || fail 'glibc 2.35 or newer is required; build from source on older systems'
workspace=$(mktemp -d)
staged_binary=''
cleanup() {
    rm -rf -- "$workspace"
    if [ -n "$staged_binary" ]; then rm -f -- "$staged_binary"; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
fetch() {
    curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
        --retry 3 --connect-timeout 10 --max-time 120 "$@"
}
# GitHub can publish a release record before CI attaches its assets. Retry only
# missing assets; other download failures keep the existing executable intact.
fetch_release_asset() {
    asset_url=$1
    asset_file=$2
    asset_attempt=0
    while :; do
        if download_status=$(fetch --output "$asset_file" --write-out '%{http_code}' "$asset_url" 2> "$workspace/download-error"); then
            return 0
        fi
        if [ "$download_status" != 404 ] || [ "$asset_attempt" -ge 18 ]; then
            cat "$workspace/download-error" >&2
            return 1
        fi
        if [ "$asset_attempt" -eq 0 ]; then
            printf 'Release assets are not attached yet; waiting up to three minutes for this asset...\n' >&2
        fi
        asset_attempt=$((asset_attempt + 1))
        sleep 10
    done
}
if [ -z "$requested_version" ]; then
    latest_url=$(fetch --head --output /dev/null --write-out '%{url_effective}' "$repository/releases/latest") || fail 'Cannot resolve latest stable release'
    release_tag=${latest_url##*/}
else
    case "$requested_version" in v*) release_tag=$requested_version ;; *) release_tag="v$requested_version" ;; esac
fi
printf '%s\n' "$release_tag" | LC_ALL=C grep -Eq '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?$' || fail 'Invalid release version'
if [ -n "$requested_version" ]; then
    fetch --head --output /dev/null "$repository/releases/tag/$release_tag" || fail "Release $release_tag is unavailable"
fi
archive="kiro-agents-$release_tag-x86_64-unknown-linux-gnu.tar.gz"
prefix=${archive%.tar.gz}
fetch_release_asset "$repository/releases/download/$release_tag/$archive" "$workspace/$archive" || fail "Cannot download $release_tag binary"
fetch_release_asset "$repository/releases/download/$release_tag/SHA256SUMS" "$workspace/SHA256SUMS" || fail 'Cannot download release checksums'
digest=$(awk -v name="$archive" '$2 == name { n++; digest=$1 } END { if (n != 1) exit 1; print digest }' "$workspace/SHA256SUMS") || fail 'Missing or duplicate archive checksum'
printf '%s\n' "$digest" | LC_ALL=C grep -Eq '^[0-9a-f]{64}$' || fail 'Malformed release checksum'
(cd "$workspace" && printf '%s  %s\n' "$digest" "$archive" | sha256sum --check --status -) || fail 'Release checksum verification failed'
tar -tzf "$workspace/$archive" > "$workspace/members" || fail 'Cannot inspect archive'
printf '%s\n' "$prefix/kiro-agents" "$prefix/LICENSE" "$prefix/README.md" "$prefix/CHANGELOG.md" | LC_ALL=C sort > "$workspace/expected"
LC_ALL=C sort "$workspace/members" > "$workspace/actual"
cmp -s "$workspace/expected" "$workspace/actual" || fail 'Archive contains unexpected, missing or duplicate files'
tar -tvzf "$workspace/$archive" | awk 'substr($0,1,1) != "-" { exit 1 }' || fail 'Archive must contain regular files only'
mkdir "$workspace/extracted"
tar --extract --gzip --file "$workspace/$archive" --directory "$workspace/extracted" --no-same-owner --no-same-permissions || fail 'Cannot extract archive'
binary="$workspace/extracted/$prefix/kiro-agents"
[ -f "$binary" ] && [ ! -L "$binary" ] || fail 'Archive has no regular executable'
chmod 0755 "$binary"
[ "$("$binary" --version)" = "kiro-agents ${release_tag#v}" ] || fail 'Downloaded executable version does not match the requested release'
mkdir -p "$install_dir"
staged_binary=$(mktemp "$install_dir/.kiro-agents.XXXXXX")
install -m 0755 "$binary" "$staged_binary"
mv -fT -- "$staged_binary" "$install_dir/kiro-agents"
staged_binary=''
printf 'Installed %s to %s/kiro-agents\n' "$release_tag" "$install_dir"
case ":${PATH:-}:" in *":$install_dir:"*) ;; *) printf 'Add %s to your PATH.\n' "$install_dir" ;; esac
printf 'Run kiro-agents doctor. Install/authenticate Kiro CLI separately. Existing hosts are unchanged.\n'
