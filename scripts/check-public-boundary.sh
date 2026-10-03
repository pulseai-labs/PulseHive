#!/usr/bin/env bash
# check-public-boundary.sh — execute PUBLIC_BOUNDARY.md's machine-checkable rules
# (ADR-019).
#
# PUBLIC_BOUNDARY.md carries three blocks: the machine-checkable rules the
# release-close boundary audit executes, the working-tree hygiene allowlist, and
# the prose rules. This script runs the first two against the tracked tree, so a
# prohibited file is caught at pull-request time instead of at a release close.
#
#   bash scripts/check-public-boundary.sh
#       parse the rules block and match every `never-tracked:` pattern against
#       `git ls-files`; prints `public-boundary: ok (<r> rules, <n> tracked
#       files)` and exits 0 when the tree obeys every rule.
#   bash scripts/check-public-boundary.sh --deny-terms <file>
#       the above, plus a case-insensitive search of the tracked tree for each
#       non-comment line of <file>. CHANGELOG.md is searched only above its
#       first released `## [x.y.z]` heading — its published release history is
#       exempt. The term itself is never printed.
#   bash scripts/check-public-boundary.sh --hygiene
#       list `git ls-files --others` (without --exclude-standard) and classify
#       each path against the hygiene allowlist as `allowlisted: <path>` or
#       `unlisted: <path>`; report only, always exits 0.
#   bash scripts/check-public-boundary.sh --self-test
#       hermetic proof, under mktemp -d, that every rejection, inconclusive
#       verdict and acceptance fires; prints `self-test: ok` last.
#
# Failure lines exit non-zero and every match is printed, not just the first:
#   public-boundary: FAIL: <path> matches never-tracked '<pattern>'
#   public-boundary: FAIL: <path>:<line> contains a denied term
# A block that is absent, empty, unparseable, carries an unknown directive or is
# missing a template rule is INCONCLUSIVE, named, and never clean:
#   public-boundary: INCONCLUSIVE: missing template rule '<line>'
# `fixtures-must-be: synthetic` is a judgment rule: it is reported as a note and
# checked by the release-close audit, not here.
#
# Matching is fnmatch-style with explicit expansion: a leading `**/` matches zero
# or more path segments, a trailing `/**` matches the directory itself as well as
# everything below it, a pattern with no `/` matches at any depth, and any other
# pattern is anchored at the tree root. Where a pattern and a path are arguably a
# match they match — an over-match is a finding a user rejects in one sentence, an
# under-match is a leak.
#
# PUBLIC_BOUNDARY_ROOT overrides the tree root the checks read (internal seam: the
# self-test points the same checks at throwaway trees under mktemp -d).
set -u

PROG="public-boundary"
ROOT="${PUBLIC_BOUNDARY_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)}"
SELF="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/$(basename -- "${BASH_SOURCE[0]}")"
POLICY="PUBLIC_BOUNDARY.md"
DENY_TERMS=""
MODE="check"

# The self-test's throwaway tree; global so the EXIT trap still sees it.
SELFTEST_TMP=""

usage() {
  cat <<EOF
usage:
  $PROG [--deny-terms <file>]
  $PROG --hygiene
  $PROG --self-test
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --deny-terms)
      [ $# -ge 2 ] || { echo "$PROG: ERROR: --deny-terms needs a file path" >&2; exit 2; }
      DENY_TERMS="$2"
      shift 2
      ;;
    --hygiene) MODE="hygiene"; shift ;;
    --self-test) MODE="self-test"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "$PROG: ERROR: unknown argument '$1'" >&2; usage >&2; exit 2 ;;
  esac
done

# run_engine <mode> <deny-terms-file> — parse the policy from the index and run
# the requested checks. The engine owns every verdict line; its exit code is the
# script's (0 clean, 1 finding or INCONCLUSIVE, 2 malformed expectation).
run_engine() {
  python3 - "$ROOT" "$1" "$2" <<'PY'
import os
import re
import subprocess
import sys

root, mode, deny_file = sys.argv[1], sys.argv[2], sys.argv[3]
PROG = "public-boundary"
POLICY = "PUBLIC_BOUNDARY.md"

# The rules the template ships (start/references/posture-block.md §6). A block
# missing one of them is INCONCLUSIVE, never clean: a rule that vanished checks
# nothing, and a check that reads nothing looks exactly like a check that passed.
TEMPLATE = (
    "never-tracked: **/.env, **/.env.*, **/*.pem, **/*.key, **/id_rsa*",
    "never-tracked: **/secrets/**, **/credentials.json",
    "never-tracked: **/SPEC.md, docs/planning/**",
    "fixtures-must-be: synthetic",
)

FIXTURE_NOTE = (
    "public-boundary: note: fixtures-must-be is a judgment rule "
    "\u2014 checked by the release-close audit, not here"
)


def git(*args):
    return subprocess.run(("git", "-C", root) + args, capture_output=True, text=True)


def inconclusive(msg):
    print("%s: INCONCLUSIVE: %s" % (PROG, msg))
    sys.exit(1)


def glob_to_regex(pattern):
    """Translate one pattern to a conservative regex (header comment)."""
    trailing_dir = pattern.endswith("/**")
    if trailing_dir:
        pattern = pattern[:-3]
    out, i = [], 0
    while i < len(pattern):
        c = pattern[i]
        if c == "*":
            if pattern.startswith("**/", i):
                out.append("(?:.*/)?")
                i += 3
                continue
            if pattern.startswith("**", i):
                out.append(".*")
                i += 2
                continue
            out.append("[^/]*")
        elif c == "?":
            out.append("[^/]")
        elif c == "[":
            j = pattern.find("]", i + 1)
            if j == -1:
                out.append(re.escape(c))
            else:
                out.append(pattern[i:j + 1])
                i = j
        else:
            out.append(re.escape(c))
        i += 1
    body = "".join(out)
    if "/" not in pattern:
        body = "(?:.*/)?" + body
    if trailing_dir:
        body += "(?:/.*)?"
    return re.compile("^(?:" + body + ")$")


def blank_comments(lines):
    """Blank HTML comment spans, keeping the line count (headings stay put)."""
    out, in_comment = [], False
    for line in lines:
        keep, i = "", 0
        while i < len(line):
            if in_comment:
                j = line.find("-->", i)
                if j == -1:
                    i = len(line)
                    break
                in_comment = False
                i = j + 3
            else:
                j = line.find("<!--", i)
                if j == -1:
                    keep += line[i:]
                    i = len(line)
                else:
                    keep += line[i:j]
                    i = j + 4
                    in_comment = True
        out.append(keep)
    return out


def find_section(lines, heading):
    """(body_start, body_end) for one `## ` section, or None."""
    for i, line in enumerate(lines):
        if line.strip() == heading:
            end = len(lines)
            for j in range(i + 1, len(lines)):
                if lines[j].startswith("## "):
                    end = j
                    break
            return i + 1, end
    return None


def read_policy_from_index():
    entry = git("ls-files", "-s", "--", POLICY)
    rows = [r for r in entry.stdout.splitlines() if r.strip()]
    if not rows:
        if os.path.lexists(os.path.join(root, POLICY)):
            inconclusive("%s is untracked (present but not in the index)" % POLICY)
        inconclusive("%s is missing (not tracked)" % POLICY)
    mode_bits = rows[0].split()[0]
    if not re.match(r"^100(644|755)$", mode_bits):
        if mode_bits == "120000":
            inconclusive("%s is a symlink (mode 120000)" % POLICY)
        if mode_bits == "160000":
            inconclusive("%s is a gitlink (mode 160000)" % POLICY)
        inconclusive("%s is tracked with unexpected mode %s" % (POLICY, mode_bits))
    show = git("show", ":" + POLICY)
    if show.returncode != 0:
        inconclusive("%s is unreadable from the index" % POLICY)
    return show.stdout


def parse_rules(policy_text):
    lines = blank_comments(policy_text.splitlines())
    section = find_section(lines, "## Machine-checkable rules")
    if section is None:
        inconclusive("missing '## Machine-checkable rules' block")
    body = lines[section[0]:section[1]]
    seen, rules, fixture = set(), [], None
    for line in body:
        text = line.strip()
        if not text:
            continue
        seen.add(text)
        if ":" not in text:
            inconclusive("unparseable line '%s'" % text)
        name, _, value = text.partition(":")
        name, value = name.strip(), value.strip()
        if name == "never-tracked":
            items = [p.strip() for p in value.split(",") if p.strip()]
            if not items:
                inconclusive("unparseable line '%s'" % text)
            rules.append(items)
        elif name == "fixtures-must-be":
            fixture = value
        else:
            inconclusive("unknown directive '%s'" % name)
    if not rules:
        inconclusive("the machine-checkable rules block is empty")
    for required in TEMPLATE:
        if required not in seen:
            inconclusive("missing template rule '%s'" % required)
    return rules, fixture


def tracked_paths():
    return [p for p in git("ls-files").stdout.splitlines() if p]


def check_rules(rules, tracked):
    findings = 0
    for items in rules:
        for pattern in items:
            rx = glob_to_regex(pattern)
            for path in tracked:
                if rx.match(path):
                    print("%s: FAIL: %s matches never-tracked '%s'" % (PROG, path, pattern))
                    findings += 1
    return findings


def check_deny_terms(tracked):
    findings = 0
    try:
        with open(deny_file, encoding="utf-8", errors="replace") as fh:
            raw = fh.read()
    except OSError:
        inconclusive("cannot read the deny-terms file: %s" % deny_file)
    terms = [l.strip() for l in raw.splitlines() if l.strip() and not l.strip().startswith("#")]
    if not terms:
        inconclusive("the deny-terms file carries no terms")
    for path in tracked:
        try:
            with open(os.path.join(root, path), encoding="utf-8", errors="replace") as fh:
                lines = fh.read().splitlines()
        except OSError:
            continue
        if path == "CHANGELOG.md":
            for i, line in enumerate(lines):
                if re.match(r"^## \[[0-9]+\.[0-9]+\.[0-9]+\]", line):
                    lines = lines[:i]
                    break
        hits = set()
        for term in terms:
            needle = term.lower()
            for n, line in enumerate(lines, 1):
                if needle in line.lower():
                    hits.add(n)
        for n in sorted(hits):
            print("%s: FAIL: %s:%d contains a denied term" % (PROG, path, n))
            findings += 1
    return findings


def run_hygiene(policy_text):
    lines = blank_comments(policy_text.splitlines())
    section = find_section(lines, "## Working-tree hygiene allowlist")
    patterns = []
    if section is not None:
        patterns = re.findall(r"`([^`]+)`", "\n".join(lines[section[0]:section[1]]))
    else:
        print("%s: note: no '## Working-tree hygiene allowlist' section "
              "\u2014 every untracked path is unlisted" % PROG)
    untracked = git("ls-files", "--others").stdout.splitlines()
    allowlisted = unlisted = 0
    for path in untracked:
        if any(glob_to_regex(p).match(path) for p in patterns):
            print("allowlisted: %s" % path)
            allowlisted += 1
        else:
            print("unlisted: %s" % path)
            unlisted += 1
    print("%s hygiene: %d allowlisted, %d unlisted" % (PROG, allowlisted, unlisted))
    return 0


def main():
    policy_text = read_policy_from_index()
    if mode == "hygiene":
        return run_hygiene(policy_text)
    rules, _fixture = parse_rules(policy_text)
    tracked = tracked_paths()
    findings = check_rules(rules, tracked)
    if deny_file:
        findings += check_deny_terms(tracked)
    if findings:
        return 1
    print(FIXTURE_NOTE)
    count = sum(len(items) for items in rules)
    print("%s: ok (%d rules, %d tracked files)" % (PROG, count, len(tracked)))
    return 0


sys.exit(main())
PY
}

# run_self_test — hermetic proof under mktemp -d. Every case builds a throwaway
# git repo, runs this same script against it through PUBLIC_BOUNDARY_ROOT, and
# records the verdict it observed; the case fails when that verdict is not the
# one expected. `git add -f` is load-bearing: the case repos carry a .gitignore,
# and the whole point of a `tracked-env` case is a file tracked despite it.
run_self_test() {
  local cases=0 failed=0
  SELFTEST_TMP="$(mktemp -d "${TMPDIR:-/tmp}/public-boundary-selftest.XXXXXX")"
  local tmp="$SELFTEST_TMP"
  trap 'rm -rf "$SELFTEST_TMP" >/dev/null 2>&1' EXIT
  CASE_DIR=""

  write_policy() {
    cat > "$1" <<'POLICY'
# Public Boundary

## Machine-checkable rules
never-tracked: **/.env, **/.env.*, **/*.pem, **/*.key, **/id_rsa*
never-tracked: **/secrets/**, **/credentials.json
never-tracked: **/SPEC.md, docs/planning/**
never-tracked: ROADMAP.md, docs/01-PRD.md, docs/02-SRS.md, docs/12-Backlog.md, docs/13-Project-Plan.md
never-tracked: MASTER-SPEC.md, .claude/memory-bank/**, docs/specs/**, docs/handoffs/**
never-tracked: .gitleaks.toml, .gitleaksignore
fixtures-must-be: synthetic

## Working-tree hygiene allowlist
- `.env`, `.env.*` (untracked, gitignored)

## Never here (prose rules)
- No secrets, tokens, or credentials of any kind.
POLICY
  }

  new_case() {
    CASE_DIR="$tmp/$1"
    mkdir -p "$CASE_DIR"
    git -c init.defaultBranch=main init -q "$CASE_DIR"
    printf '%s\n' '.env' '.env.*' '*.pem' '*.key' 'id_rsa' 'secrets/' 'target/' > "$CASE_DIR/.gitignore"
    write_policy "$CASE_DIR/PUBLIC_BOUNDARY.md"
    printf 'synthetic case tree\n' > "$CASE_DIR/README.md"
    git -C "$CASE_DIR" add -f .gitignore PUBLIC_BOUNDARY.md README.md >/dev/null 2>&1
  }

  run_case() {
    local label="$1" expected="$2"
    shift 2
    local out rc verdict
    out="$(PUBLIC_BOUNDARY_ROOT="$CASE_DIR" bash "$SELF" "$@" 2>&1)"
    rc=$?
    if [ "$rc" -ne 0 ] && printf '%s\n' "$out" | grep -q '^public-boundary: FAIL:'; then
      verdict="rejected"
    elif [ "$rc" -ne 0 ] && printf '%s\n' "$out" | grep -q '^public-boundary: INCONCLUSIVE:'; then
      verdict="inconclusive"
    elif [ "$rc" -eq 0 ] && printf '%s\n' "$out" | grep -q '^public-boundary: ok'; then
      verdict="accepted"
    else
      verdict="error (rc=$rc)"
    fi
    printf 'case %s: %s\n' "$label" "$verdict"
    cases=$((cases + 1))
    if [ "$verdict" != "$expected" ]; then
      failed=$((failed + 1))
      printf '%s\n' "$out" | sed 's/^/    | /'
    fi
  }

  # One rejection per never-tracked pattern, plus the root-versus-nested case.
  reject_case() {
    new_case "$1"
    mkdir -p "$CASE_DIR/$(dirname "$2")"
    printf 'synthetic case file\n' > "$CASE_DIR/$2"
    git -C "$CASE_DIR" add -f -- "$2" >/dev/null 2>&1
    run_case "$1" rejected
  }
  reject_case tracked-env .env
  reject_case tracked-env-production .env.production
  reject_case tracked-pem key.pem
  reject_case tracked-key key.key
  reject_case tracked-id-rsa id_rsa
  reject_case tracked-secrets-dir secrets/x.txt
  reject_case tracked-credentials-json credentials.json
  reject_case tracked-spec-md SPEC.md
  reject_case tracked-planning-dir docs/planning/x.md
  reject_case tracked-roadmap ROADMAP.md
  reject_case tracked-prd docs/01-PRD.md
  reject_case tracked-srs docs/02-SRS.md
  reject_case tracked-backlog docs/12-Backlog.md
  reject_case tracked-project-plan docs/13-Project-Plan.md
  reject_case tracked-master-spec MASTER-SPEC.md
  reject_case tracked-memory-bank .claude/memory-bank/x.md
  reject_case tracked-docs-specs docs/specs/x.md
  reject_case tracked-docs-handoffs docs/handoffs/x.md
  reject_case tracked-gitleaks-toml .gitleaks.toml
  reject_case tracked-gitleaksignore .gitleaksignore
  reject_case nested-env a/b/.env

  # The synthetic deny term, in a file and in a --deny-terms list.
  printf 'AcmeWidget\n' > "$tmp/deny-terms.txt"
  DENY="$tmp/deny-terms.txt"
  new_case denied-term
  printf 'AcmeWidget review notes\n' > "$CASE_DIR/notes.txt"
  git -C "$CASE_DIR" add -f notes.txt >/dev/null 2>&1
  run_case denied-term rejected --deny-terms "$DENY"

  # A deny term above CHANGELOG.md's first released heading is not exempt.
  new_case changelog-unreleased-term
  cat > "$CASE_DIR/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

- AcmeWidget alignment work
EOF
  git -C "$CASE_DIR" add -f CHANGELOG.md >/dev/null 2>&1
  run_case changelog-unreleased-term rejected --deny-terms "$DENY"

  # Inconclusive: the policy cannot be read, or the block cannot be executed.
  new_case missing-template-rule
  grep -vF 'never-tracked: **/secrets/**, **/credentials.json' \
    "$CASE_DIR/PUBLIC_BOUNDARY.md" > "$CASE_DIR/policy.new"
  mv "$CASE_DIR/policy.new" "$CASE_DIR/PUBLIC_BOUNDARY.md"
  git -C "$CASE_DIR" add -f PUBLIC_BOUNDARY.md >/dev/null 2>&1
  run_case missing-template-rule inconclusive

  new_case empty-block
  printf '# Public Boundary\n\n## Machine-checkable rules\n\n<!-- no rules -->\n' \
    > "$CASE_DIR/PUBLIC_BOUNDARY.md"
  git -C "$CASE_DIR" add -f PUBLIC_BOUNDARY.md >/dev/null 2>&1
  run_case empty-block inconclusive

  new_case unknown-directive
  sed -i '/^never-tracked: .gitleaks.toml/a never-committed: secrets' \
    "$CASE_DIR/PUBLIC_BOUNDARY.md"
  git -C "$CASE_DIR" add -f PUBLIC_BOUNDARY.md >/dev/null 2>&1
  run_case unknown-directive inconclusive

  new_case policy-missing
  rm -f "$CASE_DIR/PUBLIC_BOUNDARY.md"
  git -C "$CASE_DIR" rm -q --cached PUBLIC_BOUNDARY.md
  run_case policy-missing inconclusive

  new_case policy-untracked
  git -C "$CASE_DIR" rm -q --cached PUBLIC_BOUNDARY.md
  run_case policy-untracked inconclusive

  new_case policy-symlink
  git -C "$CASE_DIR" rm -q --cached PUBLIC_BOUNDARY.md
  mv "$CASE_DIR/PUBLIC_BOUNDARY.md" "$CASE_DIR/real-policy.md"
  ln -s real-policy.md "$CASE_DIR/PUBLIC_BOUNDARY.md"
  git -C "$CASE_DIR" add -f PUBLIC_BOUNDARY.md >/dev/null 2>&1
  run_case policy-symlink inconclusive

  new_case policy-gitlink
  git -C "$CASE_DIR" rm -q --cached PUBLIC_BOUNDARY.md
  rm -f "$CASE_DIR/PUBLIC_BOUNDARY.md"
  mkdir -p "$CASE_DIR/PUBLIC_BOUNDARY.md"
  git -c init.defaultBranch=main init -q "$CASE_DIR/PUBLIC_BOUNDARY.md"
  printf 'inner\n' > "$CASE_DIR/PUBLIC_BOUNDARY.md/inner.txt"
  git -C "$CASE_DIR/PUBLIC_BOUNDARY.md" add -f inner.txt >/dev/null 2>&1
  git -C "$CASE_DIR/PUBLIC_BOUNDARY.md" -c user.email=case@example.invalid \
    -c user.name=case commit -qm inner >/dev/null 2>&1
  git -C "$CASE_DIR" add -f PUBLIC_BOUNDARY.md >/dev/null 2>&1
  run_case policy-gitlink inconclusive

  # Accepted: a clean tree, the renamed env template, released history, and a
  # working-tree edit the index does not carry.
  new_case clean-tree
  run_case clean-tree accepted

  new_case env-example-renamed
  printf 'KEY=\n' > "$CASE_DIR/env.example"
  git -C "$CASE_DIR" add -f env.example >/dev/null 2>&1
  run_case env-example-renamed accepted

  new_case changelog-released-exempt
  cat > "$CASE_DIR/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

- clean

## [1.0.0] - 2026-01-01

- AcmeWidget work, as published
EOF
  git -C "$CASE_DIR" add -f CHANGELOG.md >/dev/null 2>&1
  run_case changelog-released-exempt accepted --deny-terms "$DENY"

  new_case policy-unstaged-edit
  grep -vF 'never-tracked: **/.env, **/.env.*, **/*.pem, **/*.key, **/id_rsa*' \
    "$CASE_DIR/PUBLIC_BOUNDARY.md" > "$CASE_DIR/policy.new"
  mv "$CASE_DIR/policy.new" "$CASE_DIR/PUBLIC_BOUNDARY.md"
  run_case policy-unstaged-edit accepted

  if [ "$failed" -eq 0 ]; then
    echo "self-test: ok"
    return 0
  fi
  echo "self-test: FAILED ($failed of $cases cases)"
  return 1
}

case "$MODE" in
  self-test) run_self_test ;;
  *) run_engine "$MODE" "$DENY_TERMS" ;;
esac
