#!/usr/bin/env bash
# release-settings-check.sh — read-only check of the GitHub settings ADR-018
# assumes (r2.s3.w5). It answers one question: does this repository still match
# the posture the publish authorization was written against?
#
# Every item is a `gh api` GET. Nothing here writes, and nothing here needs a
# token with more than read access to the repository's settings.
#
# Usage
#   release-settings-check.sh [--repo <owner/name>]
#       Default repo: pulseai-labs/PulseHive. Prints one `ok:` or `MISSING:`
#       line per item, then `settings: ok` when every item passes; exits
#       non-zero when any item is MISSING.
#   release-settings-check.sh --self-test
#       Hermetic: a `gh` PATH shim answers from fixture JSON, one fully-set
#       fixture passes and every single-item gap is reported by name. Prints
#       `case <label>: rejected` per gap, `case all-set: accepted` for the full
#       fixture, and `self-test: ok` last. No network, no credentials.
#
# The items, and where each is read from:
#   - no repository secret named NPM_TOKEN or CARGO_REGISTRY_TOKEN
#       GET repos/<repo>/actions/secrets
#   - NPM_TOKEN present in the `npm` environment
#       GET repos/<repo>/environments/npm/secrets
#   - each environment's required reviewer is draco28, prevent_self_review is
#     false, and its deployment policy admits only `v*` tags
#       GET repos/<repo>/environments
#       GET repos/<repo>/environments/<env>/deployment-branch-policies
#   - the `v*` tag ruleset is active and includes creation, deletion and
#     non_fast_forward
#       GET repos/<repo>/rulesets, GET repos/<repo>/rulesets/<id>
#   - the required status checks include the Node 22 and Node 24 checks and
#     exclude Node 20
#       GET repos/<repo>/branches/main/protection
#
# Trusted-publisher registrations on crates.io and PyPI cannot be read through
# `gh`; docs/RELEASING.md carries them as manual evidence rows, and this check
# says nothing about them.
set -u

PROG="release-settings-check"
REPO="pulseai-labs/PulseHive"
DEFAULT_BRANCH="main"
ENVIRONMENTS=("crates-io" "npm" "pypi")
REVIEWER="draco28"
TAG_POLICY_NAME="v*"
RULESET_REQUIRED_RULES=("creation" "deletion" "non_fast_forward")
NODE_CHECKS_REQUIRED=("Node.js Tests (Node 22)" "Node.js Tests (Node 24)")
NODE_CHECK_FORBIDDEN="Node.js Tests (Node 20)"

# The items that did not pass; the exit status is theirs.
FAILED=0

usage() {
  cat <<EOF
usage:
  $PROG [--repo <owner/name>]
  $PROG --self-test
EOF
}

report_ok() {
  printf 'ok: %s\n' "$*"
}

report_missing() {
  printf 'MISSING: %s\n' "$*"
  FAILED=$((FAILED + 1))
}

# ---------------------------------------------------------------- gh access --

# gh_body — the JSON body of one `gh api` GET. stderr is folded in so a
# transport failure (no auth, no network, no `gh`) travels into the caller's
# `MISSING:` line instead of the terminal. The shim's exit code is the same
# shape, so --self-test exercises this path too.
gh_body() { # <api-path>
  gh api "$1" 2>&1
}

# py <program> [args...] — parse stdin with python3. Kept to one call per item
# so a fixture that stops parsing fails loudly instead of passing vacuously.
py() {
  python3 -c "$1" "${@:2}"
}

# --------------------------------------------------------------- the items --

check_repo_secrets() {
  local body names
  body="$(gh_body "repos/$REPO/actions/secrets")" || {
    report_missing "cannot read repository secrets ($(printf '%s' "$body" | head -1))"
    return 0
  }
  names="$(printf '%s' "$body" | py '
import json, sys
print("\n".join(s.get("name", "") for s in json.load(sys.stdin).get("secrets", [])))
')" || {
    report_missing "cannot parse the repository secrets response"
    return 0
  }
  local found=""
  local name
  while IFS= read -r name; do
    case "$name" in
      NPM_TOKEN | CARGO_REGISTRY_TOKEN) found="$found $name" ;;
    esac
  done <<<"$names"
  if [ -n "$found" ]; then
    report_missing "repository secret(s) still present:$found — move NPM_TOKEN to the npm environment and delete both"
  else
    report_ok "no repository secret NPM_TOKEN or CARGO_REGISTRY_TOKEN"
  fi
}

check_npm_environment_secret() {
  local body names
  body="$(gh_body "repos/$REPO/environments/npm/secrets")" || {
    report_missing "cannot read the npm environment's secrets ($(printf '%s' "$body" | head -1))"
    return 0
  }
  names="$(printf '%s' "$body" | py '
import json, sys
print("\n".join(s.get("name", "") for s in json.load(sys.stdin).get("secrets", [])))
')" || {
    report_missing "cannot parse the npm environment secrets response"
    return 0
  }
  if printf '%s\n' "$names" | grep -qx 'NPM_TOKEN'; then
    report_ok "NPM_TOKEN is present in the npm environment"
  else
    report_missing "NPM_TOKEN is not in the npm environment"
  fi
}

# env_field <env> <python expression over `e`> — the environment object from the
# environments list, with the expression evaluated against it. Prints nothing and
# fails when the environment or the field is absent.
env_field() { # <env> <expression>
  local body
  body="$(gh_body "repos/$REPO/environments")" || return 1
  printf '%s' "$body" | py "
import json, sys
envs = json.load(sys.stdin).get('environments', [])
e = next((x for x in envs if x.get('name') == '$1'), None)
if e is None:
    sys.exit(3)
print($2)
" 2>/dev/null
}

check_environment() { # <env>
  local env="$1" rule logins flag
  rule="$(env_field "$env" "next(('%s|%s' % (str(bool(r.get('prevent_self_review'))).lower(), ','.join((rv.get('reviewer') or {}).get('login','') for rv in r.get('reviewers', []) or [])) for r in (e.get('protection_rules') or []) if r.get('type') == 'required_reviewers'), 'norule')")" || {
    report_missing "$env: no environment found (or the environments response did not parse)"
    return 0
  }
  if [ "$rule" = "norule" ]; then
    report_missing "$env: no required reviewer"
  else
    flag="${rule%%|*}"
    logins="${rule#*|}"
    if printf '%s' ",$logins," | grep -qF ",$REVIEWER,"; then
      report_ok "$env: reviewer is $REVIEWER"
    else
      report_missing "$env: reviewer is '${logins:-none}', expected $REVIEWER"
    fi
    if [ "$flag" = "false" ]; then
      report_ok "$env: prevent_self_review is false"
    else
      report_missing "$env: prevent_self_review is true (self-approval is explicitly permitted by ADR-018)"
    fi
  fi

  local policy policies
  policy="$(env_field "$env" "str(bool((e.get('deployment_branch_policy') or {}).get('custom_branch_policies'))).lower()")" ||
    policy="false"
  if [ "$policy" != "true" ]; then
    report_missing "$env: no deployment policy — set it to admit only the $TAG_POLICY_NAME tag"
    return 0
  fi
  policies="$(gh_body "repos/$REPO/environments/$env/deployment-branch-policies")" || {
    report_missing "$env: cannot read its deployment branch policies"
    return 0
  }
  local lines count entries
  lines="$(printf '%s' "$policies" | py '
import json, sys
d = json.load(sys.stdin)
print(d.get("total_count", 0))
for p in d.get("branch_policies", []):
    print("%s:%s" % (p.get("type", ""), p.get("name", "")))
')" || {
    report_missing "$env: cannot parse its deployment branch policies"
    return 0
  }
  count="$(printf '%s\n' "$lines" | head -1)"
  entries="$(printf '%s\n' "$lines" | tail -n +2 | paste -sd, -)"
  if [ "$count" = "1" ] && [ "$entries" = "tag:$TAG_POLICY_NAME" ]; then
    report_ok "$env: deployment policy admits only the $TAG_POLICY_NAME tag"
  else
    report_missing "$env: deployment policy admits '${entries:-nothing}', expected only tag:$TAG_POLICY_NAME"
  fi
}

check_tag_ruleset() {
  local list listed
  list="$(gh_body "repos/$REPO/rulesets")" || {
    report_missing "cannot read the repository rulesets ($(printf '%s' "$list" | head -1))"
    return 0
  }
  # The list endpoint carries id/target/enforcement but NOT `conditions`; only
  # each ruleset's detail does. So: list the tag-targeted rulesets here, then
  # decide from every one's detail below.
  listed="$(printf '%s' "$list" | py '
import json, sys
for r in json.load(sys.stdin):
    if r.get("target") == "tag":
        print("%s|%s" % (r.get("id"), r.get("enforcement", "")))
')" || {
    report_missing "cannot parse the repository rulesets response"
    return 0
  }
  local id enforcement detail shape seen="" active="" active_rules=""
  while IFS='|' read -r id enforcement; do
    [ -n "$id" ] || continue
    detail="$(gh_body "repos/$REPO/rulesets/$id")" || {
      report_missing "cannot read ruleset $id's detail ($(printf '%s' "$detail" | head -1))"
      return 0
    }
    # enf|cover|rules — cover is tags when the detail's conditions include a v*
    # tag pattern, other otherwise. The detail is the only source for both.
    shape="$(printf '%s' "$detail" | py '
import json, sys
d = json.load(sys.stdin)
inc = ((d.get("conditions") or {}).get("ref_name") or {}).get("include") or []
cover = "tags" if any(i == "refs/tags/v*" or i.startswith("refs/tags/v") for i in inc) else "other"
print("%s|%s|%s" % (d.get("enforcement", ""), cover,
                    ",".join(sorted(r.get("type", "") for r in d.get("rules", [])))))
')" || {
      report_missing "cannot parse ruleset $id's detail"
      return 0
    }
    seen="$seen $id:${shape%%|*}:$(printf '%s' "$shape" | cut -d'|' -f2)"
    if [ -z "$active" ] && [ "$enforcement" = "active" ] && [ "$(printf '%s' "$shape" | cut -d'|' -f1)" = "active" ] &&
      [ "$(printf '%s' "$shape" | cut -d'|' -f2)" = "tags" ]; then
      active="$id"
      active_rules="$(printf '%s' "$shape" | cut -d'|' -f3)"
    fi
  done <<<"$listed"
  if [ -z "$active" ]; then
    report_missing "no active ruleset protects $TAG_POLICY_NAME tags (found:${seen:- none})"
    return 0
  fi
  local missing="" want
  for want in "${RULESET_REQUIRED_RULES[@]}"; do
    printf '%s' ",$active_rules," | grep -qF ",$want," || missing="$missing $want"
  done
  if [ -n "$missing" ]; then
    report_missing "the active $TAG_POLICY_NAME ruleset is missing rule(s):$missing"
  else
    report_ok "the $TAG_POLICY_NAME ruleset is active with creation, deletion and non_fast_forward"
  fi
}

check_required_checks() {
  local body contexts want missing="" forbidden=""
  body="$(gh_body "repos/$REPO/branches/$DEFAULT_BRANCH/protection")" || {
    report_missing "cannot read $DEFAULT_BRANCH's branch protection ($(printf '%s' "$body" | head -1))"
    return 0
  }
  contexts="$(printf '%s' "$body" | py '
import json, sys
print("\n".join((json.load(sys.stdin).get("required_status_checks") or {}).get("contexts") or []))
')" || {
    report_missing "cannot parse $DEFAULT_BRANCH's branch protection response"
    return 0
  }
  for want in "${NODE_CHECKS_REQUIRED[@]}"; do
    printf '%s\n' "$contexts" | grep -qxF "$want" || missing="$missing '$want'"
  done
  if [ -n "$missing" ]; then
    report_missing "required checks are missing:$missing"
  else
    report_ok "required checks include ${NODE_CHECKS_REQUIRED[0]} and ${NODE_CHECKS_REQUIRED[1]}"
  fi
  if printf '%s\n' "$contexts" | grep -qxF "$NODE_CHECK_FORBIDDEN"; then
    report_missing "required checks still include '$NODE_CHECK_FORBIDDEN'"
  else
    report_ok "required checks exclude '$NODE_CHECK_FORBIDDEN'"
  fi
}

run_checks() {
  check_repo_secrets
  check_npm_environment_secret
  local env
  for env in "${ENVIRONMENTS[@]}"; do
    check_environment "$env"
  done
  check_tag_ruleset
  check_required_checks
}

# ---------------------------------------------------------------- self-test --
#
# A `gh` shim first on PATH answers `gh api <path>` from a fixture directory.
# One fully-set fixture must pass; each mutation is applied to a fresh copy of
# it and must be rejected, naming the item.

SELFTEST_TMP=""
SELFTEST_SHIM=""
SELFTEST_FIXTURE=""
SELFTEST_OUT=""
SELFTEST_RC=0
SELFTEST_GEN=0

selftest_fail() { # <label> <detail>
  printf 'self-test: FAILED [%s]: %s\n' "$1" "$2" >&2
  exit 1
}

# selftest_write <relative path> — stdin becomes a fixture file.
selftest_write() {
  mkdir -p "$SELFTEST_FIXTURE/$(dirname "$1")"
  cat >"$SELFTEST_FIXTURE/$1"
}

# selftest_edit <relative path> <python program> — mutates a fixture file. The
# program reads the JSON, mutates it and prints the new body.
selftest_edit() {
  local file="$SELFTEST_FIXTURE/$1" body
  body="$(py "$2" <"$file")" || selftest_fail "fixture-edit" "cannot apply the mutation to $1"
  printf '%s' "$body" >"$file"
}

selftest_base_fixture() {
  SELFTEST_GEN=$((SELFTEST_GEN + 1))
  SELFTEST_FIXTURE="$SELFTEST_TMP/fixture-$SELFTEST_GEN"
  mkdir -p "$SELFTEST_FIXTURE"

  selftest_write secrets.json <<'JSON'
{"total_count": 0, "secrets": []}
JSON
  selftest_write env-npm-secrets.json <<'JSON'
{"total_count": 1, "secrets": [{"name": "NPM_TOKEN", "created_at": "2026-10-02T00:00:00Z"}]}
JSON
  selftest_write environments.json <<'JSON'
{
  "total_count": 3,
  "environments": [
    {
      "id": 1, "name": "crates-io",
      "deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true},
      "protection_rules": [
        {"id": 11, "node_id": "RA_1", "type": "required_reviewers", "prevent_self_review": false,
         "reviewers": [{"type": "User", "reviewer": {"login": "draco28", "id": 7, "type": "User"}}]}
      ]
    },
    {
      "id": 2, "name": "npm",
      "deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true},
      "protection_rules": [
        {"id": 12, "node_id": "RA_2", "type": "required_reviewers", "prevent_self_review": false,
         "reviewers": [{"type": "User", "reviewer": {"login": "draco28", "id": 7, "type": "User"}}]}
      ]
    },
    {
      "id": 3, "name": "pypi",
      "deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true},
      "protection_rules": [
        {"id": 13, "node_id": "RA_3", "type": "required_reviewers", "prevent_self_review": false,
         "reviewers": [{"type": "User", "reviewer": {"login": "draco28", "id": 7, "type": "User"}}]}
      ]
    }
  ]
}
JSON
  local env
  for env in "${ENVIRONMENTS[@]}"; do
    selftest_write "env-$env-policies.json" <<'JSON'
{"total_count": 1, "branch_policies": [{"id": 1, "node_id": "BP_1", "name": "v*", "type": "tag"}]}
JSON
  done
  selftest_write rulesets.json <<'JSON'
[
  {"id": 18326988, "name": "Protect release tags (v*)", "target": "tag", "enforcement": "active"}
]
JSON
  selftest_write ruleset-18326988.json <<'JSON'
{"id": 18326988, "name": "Protect release tags (v*)", "target": "tag", "enforcement": "active",
 "conditions": {"ref_name": {"include": ["refs/tags/v*"], "exclude": []}},
 "rules": [{"type": "creation"}, {"type": "deletion"}, {"type": "non_fast_forward"}]}
JSON
  selftest_write protection.json <<'JSON'
{"required_status_checks": {"strict": true, "contexts": [
  "Check (ubuntu-latest)", "Python Tests", "Release workflows",
  "Node.js Tests (Node 22)", "Node.js Tests (Node 24)"
]}}
JSON
}

selftest_shim() {
  SELFTEST_SHIM="$SELFTEST_TMP/bin"
  mkdir -p "$SELFTEST_SHIM"
  cat >"$SELFTEST_SHIM/gh" <<'SHIM'
#!/usr/bin/env bash
# Fixture shim for release-settings-check.sh --self-test: answers `gh api <path>`
# from $RELEASE_SETTINGS_FIXTURE. Any other invocation fails loudly.
set -u
path=""
for arg in "$@"; do
  case "$arg" in
    api | -*) continue ;;
    *) path="$arg" ;;
  esac
done
fix="${RELEASE_SETTINGS_FIXTURE:?}"
file=""
case "$path" in
  repos/*/*/actions/secrets) file="$fix/secrets.json" ;;
  repos/*/*/environments) file="$fix/environments.json" ;;
  repos/*/*/environments/*/deployment-branch-policies)
    env="${path#*/environments/}"
    file="$fix/env-${env%%/*}-policies.json"
    ;;
  repos/*/*/environments/*/secrets)
    env="${path#*/environments/}"
    file="$fix/env-${env%%/*}-secrets.json"
    ;;
  repos/*/*/rulesets) file="$fix/rulesets.json" ;;
  repos/*/*/rulesets/*) file="$fix/ruleset-${path##*/}.json" ;;
  repos/*/*/branches/*/protection) file="$fix/protection.json" ;;
  *)
    printf 'gh-shim: unhandled path %s\n' "$path" >&2
    exit 1
    ;;
esac
[ -f "$file" ] || {
  printf 'gh-shim: no fixture for %s\n' "$path" >&2
  exit 1
}
cat "$file"
SHIM
  chmod +x "$SELFTEST_SHIM/gh"
}

selftest_run() { # runs the checker against the current fixture
  SELFTEST_OUT="$(PATH="$SELFTEST_SHIM:$PATH" RELEASE_SETTINGS_FIXTURE="$SELFTEST_FIXTURE" bash "$SELF" 2>&1)"
  SELFTEST_RC=$?
}

selftest_expect_rejected() { # <label> <substring the output must name>
  [ "$SELFTEST_RC" -ne 0 ] ||
    selftest_fail "$1" "expected a rejection, got rc=0: $SELFTEST_OUT"
  case "$SELFTEST_OUT" in
    *"$2"*) ;;
    *) selftest_fail "$1" "rejected without naming '$2': $SELFTEST_OUT" ;;
  esac
  printf 'case %s: rejected\n' "$1"
}

selftest_expect_accepted() { # <label>
  [ "$SELFTEST_RC" -eq 0 ] ||
    selftest_fail "$1" "expected acceptance, got rc=$SELFTEST_RC: $SELFTEST_OUT"
  case "$SELFTEST_OUT" in
    *"settings: ok"*) ;;
    *) selftest_fail "$1" "accepted without 'settings: ok': $SELFTEST_OUT" ;;
  esac
  printf 'case %s: accepted\n' "$1"
}

selftest_expect_accepted_naming() { # <label> <substring the output must name>
  [ "$SELFTEST_RC" -eq 0 ] ||
    selftest_fail "$1" "expected acceptance, got rc=$SELFTEST_RC: $SELFTEST_OUT"
  case "$SELFTEST_OUT" in
    *"$2"*) ;;
    *) selftest_fail "$1" "accepted without naming '$2': $SELFTEST_OUT" ;;
  esac
  printf 'case %s: accepted\n' "$1"
}

self_test() {
  SELFTEST_TMP="$(mktemp -d "${TMPDIR:-/tmp}/release-settings-check-selftest.XXXXXX")" ||
    selftest_fail "setup" "cannot create a temp dir"
  trap 'rm -rf "${SELFTEST_TMP:-}"' EXIT
  selftest_shim

  # 1. all-set: the full fixture passes.
  selftest_base_fixture
  selftest_run
  selftest_expect_accepted "all-set"

  # 2. wrong-reviewer: an environment's reviewer is not draco28.
  selftest_base_fixture
  selftest_edit environments.json '
import json, sys
d = json.load(sys.stdin)
for e in d["environments"]:
    if e["name"] == "pypi":
        e["protection_rules"][0]["reviewers"][0]["reviewer"]["login"] = "someone-else"
print(json.dumps(d))
'
  selftest_run
  selftest_expect_rejected "wrong-reviewer" "reviewer is 'someone-else'"

  # 3. self-review-flag: prevent_self_review is true on one environment.
  selftest_base_fixture
  selftest_edit environments.json '
import json, sys
d = json.load(sys.stdin)
for e in d["environments"]:
    if e["name"] == "npm":
        e["protection_rules"][0]["prevent_self_review"] = True
print(json.dumps(d))
'
  selftest_run
  selftest_expect_rejected "self-review-flag" "prevent_self_review is true"

  # 4. no-tag-policy: an environment has no deployment policy.
  selftest_base_fixture
  selftest_edit environments.json '
import json, sys
d = json.load(sys.stdin)
for e in d["environments"]:
    if e["name"] == "crates-io":
        e["deployment_branch_policy"] = None
print(json.dumps(d))
'
  selftest_run
  selftest_expect_rejected "no-tag-policy" "no deployment policy"

  # 5. ruleset-inactive: the v* ruleset is not enforced.
  selftest_base_fixture
  selftest_edit rulesets.json '
import json, sys
d = json.load(sys.stdin)
d[0]["enforcement"] = "evaluate"
print(json.dumps(d))
'
  selftest_run
  selftest_expect_rejected "ruleset-inactive" "no active ruleset protects v* tags"

  # 6. ruleset-missing-creation: the active ruleset does not restrict creation.
  selftest_base_fixture
  selftest_edit ruleset-18326988.json '
import json, sys
d = json.load(sys.stdin)
d["rules"] = [r for r in d["rules"] if r["type"] != "creation"]
print(json.dumps(d))
'
  selftest_run
  selftest_expect_rejected "ruleset-missing-creation" "missing rule(s): creation"

  # 7. node22-24-missing: the Node 24 check is not required.
  selftest_base_fixture
  selftest_edit protection.json '
import json, sys
d = json.load(sys.stdin)
d["required_status_checks"]["contexts"] = [
    c for c in d["required_status_checks"]["contexts"] if c != "Node.js Tests (Node 24)"
]
print(json.dumps(d))
'
  selftest_run
  selftest_expect_rejected "node22-24-missing" "required checks are missing: 'Node.js Tests (Node 24)'"

  # 8. node20-required: the retired Node 20 check is still required.
  selftest_base_fixture
  selftest_edit protection.json '
import json, sys
d = json.load(sys.stdin)
d["required_status_checks"]["contexts"].append("Node.js Tests (Node 20)")
print(json.dumps(d))
'
  selftest_run
  selftest_expect_rejected "node20-required" "still include 'Node.js Tests (Node 20)'"

  # 9. repo-secret-present: NPM_TOKEN is still a repository secret.
  selftest_base_fixture
  selftest_edit secrets.json '
import json, sys
d = json.load(sys.stdin)
d["total_count"] = 1
d["secrets"] = [{"name": "NPM_TOKEN", "created_at": "2026-10-02T00:00:00Z"}]
print(json.dumps(d))
'
  selftest_run
  selftest_expect_rejected "repo-secret-present" "repository secret(s) still present: NPM_TOKEN"

  # 10. npm-token-missing: the npm environment holds no NPM_TOKEN.
  selftest_base_fixture
  selftest_edit env-npm-secrets.json '
import json, sys
d = json.load(sys.stdin)
d["total_count"] = 0
d["secrets"] = []
print(json.dumps(d))
'
  selftest_run
  selftest_expect_rejected "npm-token-missing" "NPM_TOKEN is not in the npm environment"

  # 11. ruleset-list-shape (regression): the rulesets LIST endpoint does not
  # return `conditions` — only each ruleset's detail does. The item must still
  # pass, decided from the detail. With conditions read from the list, this case
  # fails (`no active ruleset protects v* tags`) and the live item can never
  # turn ok:.
  selftest_base_fixture
  selftest_run
  selftest_expect_accepted_naming "ruleset-list-shape" \
    "ok: the v* ruleset is active with creation, deletion and non_fast_forward"

  # 12. ruleset-detail-not-tags: the list says target=tag/enforcement=active, but
  # the detail's conditions cover branches only. The detail decides, so no active
  # v* ruleset is found.
  selftest_base_fixture
  selftest_edit ruleset-18326988.json '
import json, sys
d = json.load(sys.stdin)
d["conditions"]["ref_name"]["include"] = ["refs/heads/**"]
print(json.dumps(d))
'
  selftest_run
  selftest_expect_rejected "ruleset-detail-not-tags" "no active ruleset protects v* tags"

  printf 'self-test: ok\n'
}

# -------------------------------------------------------------------- main --

main() {
  local self_test_flag=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --repo)
        [ $# -ge 2 ] || {
          usage >&2
          exit 2
        }
        REPO="$2"
        shift 2
        ;;
      --self-test)
        self_test_flag=1
        shift
        ;;
      -h | --help)
        usage
        exit 0
        ;;
      *)
        usage >&2
        printf '%s: unknown argument %s\n' "$PROG" "$1" >&2
        exit 2
        ;;
    esac
  done
  if [ "$self_test_flag" -eq 1 ]; then
    self_test
    exit 0
  fi
  run_checks
  if [ "$FAILED" -eq 0 ]; then
    printf 'settings: ok\n'
  else
    printf 'settings: not ok — %d item(s) MISSING\n' "$FAILED"
    exit 1
  fi
}

SELF="$0"
main "$@"
