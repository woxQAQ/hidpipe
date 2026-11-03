#!/usr/bin/env bash
# Install locally signed applications without exposing signing keys to Nix builds.
set -euo pipefail
umask 077

if [ "$#" -ne 4 ]; then
  echo "usage: install-apps.sh STATE_DIR APPLICATIONS_DIR HIDPIPE_BINARY OPENLOGI_BINARY_OR_EMPTY" >&2
  exit 2
fi
state_dir=$1
applications_dir=$2
hidpipe_binary=$3
openlogi_binary=$4
openssl=${OPENSSL:-openssl}
security=/usr/bin/security
codesign=/usr/bin/codesign

mkdir -p "$state_dir" "$applications_dir"
chmod 700 "$state_dir"
# Never follow an existing app symlink into the immutable Nix store.
for app in "Hidpipe.app" "OpenLogi Agent.app"; do
  if [ -L "$applications_dir/$app" ]; then
    echo "Refusing to replace symlink: $applications_dir/$app" >&2
    exit 1
  fi
done

keychain="$state_dir/signing.keychain-db"
password_file="$state_dir/keychain-password"
certificate="$state_dir/signing.crt"
work_dir=$(mktemp -d "$state_dir/work.XXXXXX")
lock_dir="$state_dir/install.lock"
if ! mkdir "$lock_dir" 2>/dev/null; then
  rm -rf "$work_dir"
  echo "Another installer holds $lock_dir; remove it only if that installer has exited." >&2
  exit 1
fi
created_identity=false
identity_ready=false
cleanup() {
  "$security" lock-keychain "$keychain" >/dev/null 2>&1 || true
  if [ "$created_identity" = true ] && [ "$identity_ready" = false ]; then
    "$security" delete-keychain "$keychain" >/dev/null 2>&1 || true
    rm -f "$password_file" "$certificate"
  fi
  rm -rf "$work_dir"
  rmdir "$lock_dir"
}
trap cleanup EXIT

if [ ! -e "$keychain" ]; then
  if [ -e "$password_file" ] || [ -e "$certificate" ]; then
    echo "Incomplete signing identity in $state_dir; restore it from backup before continuing." >&2
    exit 1
  fi
  created_identity=true
  "$openssl" rand -base64 48 > "$password_file"
  chmod 600 "$password_file"
  password=$(< "$password_file")
  "$openssl" req -x509 -newkey rsa:3072 -nodes -sha256 -days 3650 \
    -subj '/CN=hidpipe Local Code Signing/' \
    -addext 'basicConstraints=critical,CA:FALSE' \
    -addext 'keyUsage=critical,digitalSignature' \
    -addext 'extendedKeyUsage=critical,codeSigning' \
    -keyout "$work_dir/signing.key" -out "$certificate" 2> "$work_dir/openssl.log"
  # Security.framework cannot import OpenSSL 3's default PKCS#12 encryption.
  # The temporary container is encrypted with the same local-only password.
  "$openssl" pkcs12 -export -inkey "$work_dir/signing.key" -in "$certificate" \
    -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 \
    -name 'hidpipe Local Code Signing' -passout "file:$password_file" -out "$work_dir/signing.p12"
  "$security" create-keychain -p "$password" "$keychain"
  "$security" unlock-keychain -p "$password" "$keychain"
  "$security" import "$work_dir/signing.p12" -k "$keychain" -f pkcs12 -P "$password" \
    -x -T "$codesign" >/dev/null
  "$security" set-key-partition-list -S 'apple-tool:,apple:,codesign:' -s \
    -k "$password" "$keychain" >/dev/null
else
  if [ ! -f "$password_file" ] || [ ! -f "$certificate" ]; then
    echo "Signing identity is incomplete in $state_dir; refusing to rotate it." >&2
    exit 1
  fi
  password=$(< "$password_file")
  "$security" unlock-keychain -p "$password" "$keychain"
fi
identity_ready=true

fingerprint=$("$openssl" x509 -in "$certificate" -noout -fingerprint -sha1)
fingerprint=${fingerprint#*=}
fingerprint=${fingerprint//:/}
installer_hash=$(/usr/bin/shasum -a 256 "$0")
installer_hash=${installer_hash%% *}

install_app() {
  local source=$1 name=$2 executable=$3 bundle_id=$4
  local target="$applications_dir/$name.app"
  local staged="$work_dir/$name.app"
  local requirement="identifier \"$bundle_id\" and certificate leaf = H\"$fingerprint\""
  local source_hash
  source_hash=$(/usr/bin/shasum -a 256 "$source")
  source_hash=${source_hash%% *}
  if [ -f "$target/Contents/Resources/source-sha256" ] \
    && [ -f "$target/Contents/Resources/installer-sha256" ] \
    && [ "$(< "$target/Contents/Resources/source-sha256")" = "$source_hash" ] \
    && [ "$(< "$target/Contents/Resources/installer-sha256")" = "$installer_hash" ] \
    && "$codesign" --verify --strict -R "=$requirement" "$target" 2>/dev/null; then
    echo "Unchanged: $target"
    return
  fi
  mkdir -p "$staged/Contents/MacOS" "$staged/Contents/Resources"
  /usr/bin/install -m 755 "$source" "$staged/Contents/MacOS/$executable"
  printf '%s\n' "$source_hash" > "$staged/Contents/Resources/source-sha256"
  printf '%s\n' "$installer_hash" > "$staged/Contents/Resources/installer-sha256"
  printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>' \
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
    '<plist version="1.0"><dict>' \
    "<key>CFBundleIdentifier</key><string>$bundle_id</string>" \
    "<key>CFBundleExecutable</key><string>$executable</string>" \
    "<key>CFBundleName</key><string>$name</string>" \
    '<key>CFBundlePackageType</key><string>APPL</string>' \
    '<key>CFBundleVersion</key><string>1</string>' \
    '<key>LSUIElement</key><true/>' \
    '</dict></plist>' > "$staged/Contents/Info.plist"
  "$codesign" --force --sign "$fingerprint" --keychain "$keychain" --timestamp=none \
    --identifier "$bundle_id" --requirements "=designated => $requirement" "$staged"
  "$codesign" --verify --strict -R "=$requirement" "$staged"
  # Stage on the destination filesystem before replacing the previous bundle.
  local pending
  pending=$(mktemp -d "$applications_dir/.hidpipe-install.XXXXXX")
  /bin/cp -R "$staged" "$pending/$name.app"
  if [ -e "$target" ]; then
    /bin/mv "$target" "$pending/previous.app"
  fi
  if ! /bin/mv "$pending/$name.app" "$target"; then
    if [ -e "$pending/previous.app" ]; then
      /bin/mv "$pending/previous.app" "$target"
    fi
    rm -rf "$pending"
    return 1
  fi
  rm -rf "$pending"
  echo "Installed: $target"
}

install_app "$hidpipe_binary" 'Hidpipe' 'hidpipe-client' 'io.github.woxqaq.hidpipe.client'
if [ -n "$openlogi_binary" ]; then
  install_app "$openlogi_binary" 'OpenLogi Agent' 'openlogi-agent' 'io.github.woxqaq.hidpipe.openlogi-agent'
fi
