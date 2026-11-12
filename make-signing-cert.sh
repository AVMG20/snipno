#!/usr/bin/env bash
# One-time setup: create a self-signed code-signing certificate in your login keychain.
#
# Why: macOS ties the Screen Recording permission to the app's code signature. An ad-hoc
# signature changes on every build, so each rebuild would need the permission granted again.
# Signing with this fixed certificate keeps the identity stable, so the grant survives rebuilds.
# bundle.sh uses it automatically when it exists.
#
# Undo: delete "Snipno Local Signing" in Keychain Access (login keychain → My Certificates).
set -euo pipefail

NAME="Snipno Local Signing"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if security find-certificate -c "$NAME" "$KEYCHAIN" >/dev/null 2>&1; then
  echo "\"$NAME\" already exists."
  exit 0
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/cert.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
CNF

# macOS's own (LibreSSL) openssl writes a PKCS#12 that `security import` accepts.
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout "$TMP/key.pem" -out "$TMP/cert.pem" -config "$TMP/cert.cnf" 2>/dev/null
/usr/bin/openssl pkcs12 -export -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
  -name "$NAME" -out "$TMP/id.p12" -passout pass:snipno

security import "$TMP/id.p12" -k "$KEYCHAIN" -P snipno -T /usr/bin/codesign >/dev/null
echo "Created \"$NAME\". Run ./bundle.sh to sign with it."
