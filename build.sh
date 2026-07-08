#!/usr/bin/env bash
set -euo pipefail

COMPONENT_NAME="com.mbreissi.edgecommons.ConfigComponent"
COMPONENT_VERSION="$(sed -n '0,/"version"[[:space:]]*:/s/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' gdk-config.json)"
if [[ -z "${COMPONENT_VERSION}" ]]; then
  echo "error: failed to read component version from gdk-config.json" >&2
  exit 1
fi
BIN_NAME="config-component"
FEATURES="${EDGECOMMONS_FEATURES:-greengrass}"
TARGET="${EDGECOMMONS_TARGET:-}"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"

echo "Building ${BIN_NAME} (release, features=${FEATURES})${TARGET:+ for ${TARGET}}..."
if [[ -n "${TARGET}" ]]; then
  cargo build --release --no-default-features --features "${FEATURES}" --target "${TARGET}"
  BIN_DIR="${TARGET_DIR}/${TARGET}/release"
else
  cargo build --release --no-default-features --features "${FEATURES}"
  BIN_DIR="${TARGET_DIR}/release"
fi

BIN_PATH="${BIN_DIR}/${BIN_NAME}"
[[ -f "${BIN_PATH}" ]] || BIN_PATH="${BIN_DIR}/${BIN_NAME}.exe"
if [[ ! -f "${BIN_PATH}" ]]; then
  echo "error: built binary not found in ${BIN_DIR}" >&2
  exit 1
fi

ARTIFACT_DIR="greengrass-build/artifacts/${COMPONENT_NAME}/${COMPONENT_VERSION}"
RECIPE_DIR="greengrass-build/recipes"
mkdir -p "${ARTIFACT_DIR}" "${RECIPE_DIR}"

cp "${BIN_PATH}" "${ARTIFACT_DIR}/${BIN_NAME}"
chmod +x "${ARTIFACT_DIR}/${BIN_NAME}" || true
cp recipe.yaml "${RECIPE_DIR}/recipe.yaml"

echo "Staged artifact -> ${ARTIFACT_DIR}/${BIN_NAME}"
echo "Staged recipe   -> ${RECIPE_DIR}/recipe.yaml"
