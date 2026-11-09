#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

fail() {
  echo "error: $*" >&2
  exit 1
}

http_status_from() {
  sed -n '1s#^HTTP/[^ ]* \([0-9][0-9][0-9]\).*#\1#p' <<<"${1:-}"
}

validate_remote_tag() {
  local release_tag="${RELEASE_TAG:-}"
  local target_sha="${RELEASE_TARGET_SHA:-}"
  local require_existing_tag="${REQUIRE_EXISTING_TAG:-0}"
  local remote_refs tag_ref tag_object_sha peeled_sha tag_commit_sha

  [[ "${release_tag}" == v* ]] || fail "release tag must start with v"
  [[ -n "${target_sha}" ]] || fail "release target commit is required"
  [[ "${require_existing_tag}" == 0 || "${require_existing_tag}" == 1 ]] \
    || fail "REQUIRE_EXISTING_TAG must be 0 or 1"
  git check-ref-format "refs/tags/${release_tag}" \
    || fail "invalid release tag: ${release_tag}"

  remote_refs="$(git ls-remote --tags origin "refs/tags/${release_tag}" "refs/tags/${release_tag}^{}")" \
    || fail "could not inspect remote tag ${release_tag}"
  tag_ref="refs/tags/${release_tag}"
  tag_object_sha="$(awk -v ref="${tag_ref}" '$2 == ref { print $1 }' <<<"${remote_refs}")"
  peeled_sha="$(awk -v ref="${tag_ref}^{}" '$2 == ref { print $1 }' <<<"${remote_refs}")"

  if [[ -z "${tag_object_sha}" ]]; then
    [[ -z "${peeled_sha}" ]] || fail "remote tag ${release_tag} has a peeled ref without a tag ref"
    [[ "${require_existing_tag}" == 0 ]] \
      || fail "pushed tag ${release_tag} is missing from origin"
    printf 'missing\n'
    return
  fi

  [[ "${tag_object_sha}" =~ ^[0-9a-fA-F]{40}$ ]] \
    || fail "remote tag ${release_tag} has an invalid object ID"
  if [[ -n "${peeled_sha}" ]]; then
    [[ "${peeled_sha}" =~ ^[0-9a-fA-F]{40}$ ]] \
      || fail "annotated tag ${release_tag} has an invalid peeled object ID"
    tag_commit_sha="${peeled_sha}"
  else
    tag_commit_sha="${tag_object_sha}"
  fi

  [[ "${tag_commit_sha}" == "${target_sha}" ]] \
    || fail "tag ${release_tag} resolves to ${tag_commit_sha}, not the build commit ${target_sha}"
  printf 'existing\n'
}

create_release_tag() {
  local repository="$1"
  local release_tag="$2"
  local target_sha="$3"
  local api_response api_exit http_status
  local endpoint="repos/${repository}/git/refs"

  if api_response="$(gh api --include --method POST "${endpoint}" \
    -f "ref=refs/tags/${release_tag}" \
    -f "sha=${target_sha}" 2>&1)"; then
    api_exit=0
  else
    api_exit=$?
  fi
  http_status="$(http_status_from "${api_response:-}")"

  case "${http_status}:${api_exit}" in
    201:0)
      ;;
    422:*)
      # A concurrent workflow may have created the ref after our initial lookup.
      # Re-read and accept it only if it resolves to the exact build commit.
      RELEASE_TAG="${release_tag}" RELEASE_TARGET_SHA="${target_sha}" \
        REQUIRE_EXISTING_TAG=1 validate_remote_tag >/dev/null
      ;;
    *)
      printf '%s\n' "${api_response:-gh api returned no response}" >&2
      fail "could not create release tag ${release_tag} at ${target_sha} (HTTP ${http_status:-unknown}, exit ${api_exit:-unknown})"
      ;;
  esac
}

prepare_release_ref() {
  local event_name="${GITHUB_EVENT_NAME:-}"
  local ref="${GITHUB_REF:-}"
  local ref_name="${GITHUB_REF_NAME:-}"
  local target_sha="${GITHUB_SHA:-}"
  local output_file="${GITHUB_OUTPUT:-}"
  local package_version expected_tag release_tag require_existing_tag tag_state

  [[ -n "${target_sha}" ]] || fail "GitHub event commit is missing"
  [[ -n "${output_file}" ]] || fail "GITHUB_OUTPUT is required"
  package_version="$(cd "${ROOT}" && node -p "require('./package.json').version")" \
    || fail "could not read package.json version"
  expected_tag="v${package_version}"

  case "${event_name}" in
    push)
      [[ "${ref}" == refs/tags/v* ]] \
        || fail "push release runs must be triggered by a v-prefixed tag (got ${ref})"
      release_tag="${ref_name}"
      require_existing_tag=1
      ;;
    workflow_dispatch)
      [[ "${ref}" == refs/heads/main ]] \
        || fail "manual releases must be run from refs/heads/main (got ${ref})"
      release_tag="${expected_tag}"
      require_existing_tag=0
      ;;
    *)
      fail "unsupported release event: ${event_name}"
      ;;
  esac

  [[ "${release_tag}" == "${expected_tag}" ]] \
    || fail "release tag ${release_tag} does not match package.json version ${package_version}"
  tag_state="$(RELEASE_TAG="${release_tag}" RELEASE_TARGET_SHA="${target_sha}" \
    REQUIRE_EXISTING_TAG="${require_existing_tag}" validate_remote_tag)" \
    || fail "release tag validation failed"

  {
    echo "release_tag=${release_tag}"
    echo "target_sha=${target_sha}"
    echo "require_existing_tag=${require_existing_tag}"
    echo "tag_state=${tag_state}"
  } >>"${output_file}"
}

publish_release() {
  local release_tag="${RELEASE_TAG:-}"
  local target_sha="${RELEASE_TARGET_SHA:-}"
  local dmg_path="${DMG_PATH:-}"
  local artifact_dir="${RELEASE_ARTIFACT_DIR:-artifacts}"
  local notes_path="${RELEASE_NOTES_PATH:-release-notes.md}"
  local repository="${GITHUB_REPOSITORY:-}"
  local asset_name asset_path tag_state api_endpoint api_response api_exit http_status release_exists

  [[ -n "${release_tag}" ]] || fail "release tag is required"
  [[ -n "${target_sha}" ]] || fail "release target commit is required"
  [[ -n "${dmg_path}" ]] || fail "DMG path is required"
  [[ "${repository}" == */* && "${repository}" != /* && "${repository}" != *..* ]] \
    || fail "GITHUB_REPOSITORY must be owner/repo"
  [[ -f "${notes_path}" ]] || fail "release notes are missing: ${notes_path}"

  asset_name="${dmg_path##*/}"
  asset_path="${artifact_dir}/${asset_name}"
  [[ -f "${asset_path}" ]] || fail "release DMG is missing: ${asset_path}"

  tag_state="$(RELEASE_TAG="${release_tag}" RELEASE_TARGET_SHA="${target_sha}" \
    REQUIRE_EXISTING_TAG="${REQUIRE_EXISTING_TAG:-0}" validate_remote_tag)" \
    || fail "release tag validation failed"

  api_endpoint="repos/${repository}/releases/tags/${release_tag}"
  if api_response="$(gh api --include "${api_endpoint}" 2>&1)"; then
    api_exit=0
  else
    api_exit=$?
  fi
  http_status="$(http_status_from "${api_response:-}")"

  case "${http_status}:${api_exit}" in
    200:0)
      release_exists=1
      ;;
    404:*)
      release_exists=0
      ;;
    *)
      printf '%s\n' "${api_response:-gh api returned no response}" >&2
      fail "could not determine whether release ${release_tag} already exists (HTTP ${http_status:-unknown}, exit ${api_exit:-unknown})"
      ;;
  esac

  if [[ "${release_exists}" == 1 ]]; then
    [[ "${tag_state}" == existing ]] \
      || fail "release ${release_tag} exists but its remote tag is missing; refusing to attach an asset"
    tag_state="$(RELEASE_TAG="${release_tag}" RELEASE_TARGET_SHA="${target_sha}" \
      REQUIRE_EXISTING_TAG=1 validate_remote_tag)" \
      || fail "release tag validation failed immediately before upload"
    gh release upload "${release_tag}" "${asset_path}"
    gh release edit "${release_tag}" --notes-file "${notes_path}"
  else
    if [[ "${tag_state}" == missing ]]; then
      [[ "${REQUIRE_EXISTING_TAG:-0}" == 0 ]] \
        || fail "pushed tag ${release_tag} disappeared before publication"
      create_release_tag "${repository}" "${release_tag}" "${target_sha}"
    fi
    tag_state="$(RELEASE_TAG="${release_tag}" RELEASE_TARGET_SHA="${target_sha}" \
      REQUIRE_EXISTING_TAG=1 validate_remote_tag)" \
      || fail "release tag validation failed immediately before creation"
    gh release create "${release_tag}" "${asset_path}" \
      --title "VoiceFlow ${release_tag}" \
      --notes-file "${notes_path}" \
      --latest \
      --verify-tag
  fi
}

case "${1:-}" in
  prepare)
    prepare_release_ref
    ;;
  publish)
    publish_release
    ;;
  *)
    fail "usage: $0 {prepare|publish}"
    ;;
esac
