#!/usr/bin/env bash
# Exercise native signing and replacement on macOS, using an isolated identity.
set -euo pipefail
umask 077
installer=${1:?installer script required}
work=$(mktemp -d)
cleanup() {
  /usr/bin/security delete-keychain "$work/state/signing.keychain-db" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

printf '%s\n' 'int main(void) { return REVISION; }' > "$work/main.c"
/usr/bin/clang -DREVISION=0 "$work/main.c" -o "$work/first"
/usr/bin/clang -DREVISION=1 "$work/main.c" -o "$work/second"
install_revision() {
  /bin/bash "$installer" "$work/state" "$work/apps" "$1" "$1"
}
identity() {
  /usr/bin/codesign -d -r- "$1" 2>&1 | /usr/bin/awk '/^designated =>/ { print }'
}
code_hash() {
  /usr/bin/codesign -dv --verbose=4 "$1" 2>&1 | /usr/bin/awk '/^CDHash=/ { print }'
}

install_revision "$work/first"
client="$work/apps/Hidpipe.app"
agent="$work/apps/OpenLogi Agent.app"
client_identity=$(identity "$client")
agent_identity=$(identity "$agent")
client_hash=$(code_hash "$client")
agent_hash=$(code_hash "$agent")
[ -n "$client_identity" ]
[ -n "$agent_identity" ]
[ -n "$client_hash" ]
[ -n "$agent_hash" ]
[ "$client_identity" != "$agent_identity" ]
"$client/Contents/MacOS/hidpipe-client"

# Repeat activation must leave the bundle in place, without resigning it.
inode=$(/usr/bin/stat -f '%i' "$client")
install_revision "$work/first"
[ "$inode" = "$(/usr/bin/stat -f '%i' "$client")" ]

# Both rebuilt executables change code hashes but retain their original DRs.
install_revision "$work/second"
[ "$client_identity" = "$(identity "$client")" ]
[ "$agent_identity" = "$(identity "$agent")" ]
[ "$client_hash" != "$(code_hash "$client")" ]
[ "$agent_hash" != "$(code_hash "$agent")" ]
/usr/bin/codesign --verify --strict "$client"
/usr/bin/codesign --verify --strict "$agent"
if "$client/Contents/MacOS/hidpipe-client"; then
  echo 'Updated executable did not run' >&2
  exit 1
fi

# Rollback uses the original code without rotating the signing identity.
install_revision "$work/first"
[ "$client_identity" = "$(identity "$client")" ]
[ "$agent_identity" = "$(identity "$agent")" ]
"$client/Contents/MacOS/hidpipe-client"

# An incomplete existing identity must fail without replacing the apps.
/bin/mv "$work/state/signing.crt" "$work/certificate-backup"
if install_revision "$work/second"; then
  echo 'Incomplete signing state was accepted' >&2
  exit 1
fi
[ "$client_identity" = "$(identity "$client")" ]
/bin/mv "$work/certificate-backup" "$work/state/signing.crt"

echo 'macOS signing lifecycle passed: install, no-op, upgrade, rollback, incomplete identity'
