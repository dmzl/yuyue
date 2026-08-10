#!/usr/bin/env bash

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_PATH="${PROJECT_ROOT}/src-tauri/target/release/bundle/macos/屿阅.app"
OUTPUT_DIR="${PROJECT_ROOT}/src-tauri/target/release/bundle/dmg"
mkdir -p "${OUTPUT_DIR}"

if [[ ! -d "${APP_PATH}" ]]; then
  echo "Missing unsigned app bundle: ${APP_PATH}" >&2
  exit 1
fi

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "${APP_PATH}/Contents/Info.plist")"
MACH_O_PATH="${APP_PATH}/Contents/MacOS/tauri-app"
ARCHES="$(lipo -archs "${MACH_O_PATH}" 2>/dev/null)" || {
  echo "Unable to inspect app architecture: ${MACH_O_PATH}" >&2
  exit 1
}
case "${ARCHES}" in
  arm64) ARCH="arm64" ;;
  x86_64) ARCH="x86_64" ;;
  "arm64 x86_64"|"x86_64 arm64") ARCH="universal2" ;;
  *) echo "Unsupported app architecture: ${ARCHES}" >&2; exit 1 ;;
esac

OUTPUT_PATH="${OUTPUT_DIR}/屿阅_${VERSION}_${ARCH}.dmg"
STAGE_DIR="$(mktemp -d -t mdreader-dmg)"
cleanup() {
  rm -rf "${STAGE_DIR}"
}
trap cleanup EXIT

rm -f "${OUTPUT_PATH}"
ditto "${APP_PATH}" "${STAGE_DIR}/屿阅.app"
ln -s /Applications "${STAGE_DIR}/Applications"

hdiutil create \
  -ov \
  -fs HFS+ \
  -format UDZO \
  -volname "屿阅 ${VERSION}" \
  -srcfolder "${STAGE_DIR}" \
  "${OUTPUT_PATH}"

echo "Unsigned macOS DMG: ${OUTPUT_PATH}"
