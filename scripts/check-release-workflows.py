#!/usr/bin/env python3
"""check-release-workflows.py — keep the three release workflows honest about the
release gate (r2.s3.w1, spine A12/A13).

Each release workflow must wait on the reusable release gate before it publishes
and must not smuggle a publish path past it. This checker reads a workflow's YAML
and prints every way it breaks that contract, one named line each, so a failure
names the rule and the workflow — the release runbook maps each to an action.

    --workflow <path> --environment <env> [--oidc]   ->  release workflow ok: <path>
    --all                                            ->  release workflows: ok
    --self-test                                      ->  self-test: ok

`--all` means crates-release.yml --environment crates-io --oidc, npm-release.yml
--environment npm, and python-release.yml --environment pypi --oidc, resolved
from the repository root (this script's parent directory).

The publish job is the one job whose `environment` (a string, or a mapping's
`name`) equals <env>; exactly one must exist. Invariants, each failing with
`check-release-workflows: ERROR: <path>: <message>`:

 1. `on` includes `push` with a `tags` list containing `v*`, and `workflow_dispatch`.
 2. A job `gate` exists with `uses: ./.github/workflows/release-gate.yml`, and its
    `if` contains `startsWith(github.ref, 'refs/tags/v')`.
 3. The publish job's `needs` includes `gate`.
 4. The publish job's `if` contains `startsWith(github.ref, 'refs/tags/v')`; it
    does not contain `github.event_name == 'push'` (a dispatch on a tag must reach
    it), and none of `always()`, `cancelled()`, `failure()` or `||` (a job runs
    after its `needs` failed when its condition says so).
 5. Every secret reference sits inside the publish job; `secrets: inherit` is
    rejected outright. Workflow- and job-level `env` count.
 6. With `--oidc`: the publish job's `permissions` has `id-token: write`, and the
    file has no secret reference at all.
 7. No job other than the publish job grants `id-token: write`, at job or
    workflow level.
 8. Every `uses:` in the publish job's steps is pinned to a 40-hex commit SHA;
    local `./` actions are exempt.
 9. A top-level `concurrency` exists with `cancel-in-progress: false` (A14).

Every invariant is evaluated and every violation printed before a non-zero exit,
so a caller can assert any one message regardless of order.
"""

import json
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path

import yaml

PROG = "check-release-workflows"
ROOT = Path(__file__).resolve().parents[1]
GATE_PATH = "./.github/workflows/release-gate.yml"
TAG_GATE = "startsWith(github.ref, 'refs/tags/v')"
SECRET_RE = re.compile(r"""secrets\s*\.\s*[A-Za-z_][A-Za-z0-9_]*|secrets\s*\[\s*['"][^'"]+['"]\s*\]""")
PINNED_RE = re.compile(r"[^@\s]+@[0-9a-f]{40}")
ALL_WORKFLOWS = (
    (".github/workflows/crates-release.yml", "crates-io", True),
    (".github/workflows/npm-release.yml", "npm", False),
    (".github/workflows/python-release.yml", "pypi", True),
)


def usage() -> None:
    print(f"usage:\n  {PROG} --workflow <path> --environment <env> [--oidc]\n"
          f"  {PROG} --all\n"
          f"  {PROG} --self-test", file=sys.stderr)


def usage_error(message: str) -> int:
    print(f"{PROG}: {message}", file=sys.stderr)
    usage()
    return 2


def violation(path: str, message: str) -> None:
    print(f"{PROG}: ERROR: {path}: {message}", file=sys.stderr)


# --------------------------------------------------------------------------
# The workflow checks
# --------------------------------------------------------------------------


def environment_of(job: dict):
    """A job's environment as a plain name: a string, or a mapping's `name`."""
    environment = job.get("environment")
    if isinstance(environment, str):
        return environment
    if isinstance(environment, dict):
        return environment.get("name")
    return None


def resolve_publish(jobs: dict, environment: str):
    """(publish job name or None, violations) — exactly one job must match."""
    matches = [
        name
        for name, job in jobs.items()
        if isinstance(job, dict) and environment_of(job) == environment
    ]
    if len(matches) == 1:
        return matches[0], []
    if not matches:
        return None, [f"no job declares the environment {environment}"]
    names = ", ".join(sorted(str(name) for name in matches))
    return None, [f"more than one job declares the environment {environment}: {names}"]


def find_reference(node, pattern: re.Pattern) -> bool:
    """True when <node> — dumped to one JSON line — holds a match for <pattern>."""
    return pattern.search(json.dumps(node, default=str)) is not None


def find_inherit(node) -> bool:
    """True when any mapping in <node> declares `secrets: inherit`."""
    if isinstance(node, dict):
        if node.get("secrets") == "inherit":
            return True
        return any(find_inherit(value) for value in node.values())
    if isinstance(node, list):
        return any(find_inherit(item) for item in node)
    return False


def check_triggers(doc: dict) -> list:
    # YAML 1.1 reads a bare `on` as the boolean True, so both spellings count.
    triggers = doc.get("on", doc.get(True))
    if not isinstance(triggers, dict):
        return ["on must be a mapping with push and workflow_dispatch"]
    violations = []
    push = triggers.get("push")
    tags = push.get("tags") if isinstance(push, dict) else None
    if not isinstance(tags, list) or "v*" not in tags:
        violations.append("on.push.tags must contain 'v*'")
    if "workflow_dispatch" not in triggers:
        violations.append("on.workflow_dispatch is required")
    return violations


def check_gate_job(jobs: dict) -> list:
    gate = jobs.get("gate")
    if not isinstance(gate, dict) or gate.get("uses") != GATE_PATH:
        return [f"no job named gate calls {GATE_PATH}"]
    if TAG_GATE not in str(gate.get("if", "")):
        return ["gate job if does not gate on startsWith(github.ref, 'refs/tags/v')"]
    return []


def check_publish_needs(publish) -> list:
    if publish is None:
        return []
    needs = publish.get("needs")
    names = [needs] if isinstance(needs, str) else needs
    if not isinstance(names, list) or "gate" not in names:
        return ["publish job does not need the release gate"]
    return []


def check_publish_if(publish) -> list:
    if publish is None:
        return []
    violations = []
    condition = str(publish.get("if", ""))
    if TAG_GATE not in condition:
        violations.append("publish job if does not gate on startsWith(github.ref, 'refs/tags/v')")
    if "github.event_name == 'push'" in condition:
        violations.append("publish job if excludes a workflow_dispatch run (github.event_name == 'push')")
    for token in ("always()", "cancelled()", "failure()", "||"):
        if token in condition:
            violations.append(f"publish job if contains {token}")
    return violations


def check_secrets(doc: dict, jobs: dict, publish_name, oidc: bool) -> list:
    violations = []
    if find_inherit(doc):
        violations.append("secrets: inherit is not allowed")
    workflow_level = {key: value for key, value in doc.items() if key != "jobs"}
    if find_reference(workflow_level, SECRET_RE):
        violations.append("secret referenced outside the publish job: workflow-level")
    for name, job in jobs.items():
        if not find_reference(job, SECRET_RE):
            continue
        if oidc:
            violations.append(f"secret referenced in an OIDC workflow: {name}")
        elif name != publish_name:
            violations.append(f"secret referenced outside the publish job: {name}")
    return violations


def check_id_token(doc: dict, jobs: dict, publish_name, oidc: bool) -> list:
    violations = []
    workflow_permissions = doc.get("permissions")
    if isinstance(workflow_permissions, dict) and workflow_permissions.get("id-token") == "write":
        violations.append("id-token: write granted outside the publish job: workflow level")
    for name, job in jobs.items():
        if name == publish_name or not isinstance(job, dict):
            continue
        permissions = job.get("permissions")
        if isinstance(permissions, dict) and permissions.get("id-token") == "write":
            violations.append(f"id-token: write granted outside the publish job: {name}")
    if oidc and publish_name is not None:
        permissions = jobs[publish_name].get("permissions")
        if not (isinstance(permissions, dict) and permissions.get("id-token") == "write"):
            violations.append("OIDC publish job does not grant id-token: write")
    return violations


def check_pinned_actions(publish) -> list:
    if publish is None:
        return []
    violations = []
    for step in publish.get("steps") or []:
        if not isinstance(step, dict):
            continue
        action = step.get("uses")
        if not isinstance(action, str) or action.startswith("./"):
            continue
        if not PINNED_RE.fullmatch(action):
            violations.append(f"publish job uses an unpinned action: {action}")
    return violations


def check_concurrency(doc: dict) -> list:
    concurrency = doc.get("concurrency")
    if not isinstance(concurrency, dict) or concurrency.get("cancel-in-progress") is not False:
        return ["top-level concurrency with cancel-in-progress: false is required"]
    return []


def check_workflow(path: str, environment: str, oidc: bool) -> list:
    """Every violation the workflow at <path> carries; [] means it passes."""
    try:
        with open(path, encoding="utf-8") as handle:
            doc = yaml.safe_load(handle)
    except OSError as exc:
        return [f"cannot read the workflow: {exc}"]
    except yaml.YAMLError as exc:
        return [f"does not parse: {exc}"]
    if not isinstance(doc, dict):
        return ["does not parse as a mapping"]

    jobs = doc.get("jobs")
    if not isinstance(jobs, dict):
        jobs = {}
    publish_name, violations = resolve_publish(jobs, environment)
    publish = jobs.get(publish_name) if publish_name is not None else None

    violations = list(violations)
    violations += check_triggers(doc)
    violations += check_gate_job(jobs)
    violations += check_publish_needs(publish)
    violations += check_publish_if(publish)
    violations += check_secrets(doc, jobs, publish_name, oidc)
    violations += check_id_token(doc, jobs, publish_name, oidc)
    violations += check_pinned_actions(publish)
    violations += check_concurrency(doc)
    return violations


# --------------------------------------------------------------------------
# --self-test
#
# Each fixture is a workflow that breaks exactly one invariant; every case
# asserts that it is rejected with exactly its own message, and the passing
# fixture is OIDC-clean (no secrets at all), the strongest form of the
# contract.
# --------------------------------------------------------------------------

FIXTURE_ENVIRONMENT = "fixture-env"

HEADER = """name: Fixture Release
on:
  push:
    tags: ["v*"]
  workflow_dispatch:
permissions:
  contents: read
concurrency:
  group: fixture-release
  cancel-in-progress: false
jobs:
"""

CONCURRENCY_BLOCK = """concurrency:
  group: fixture-release
  cancel-in-progress: false
"""

GATE_JOB = """  gate:
    if: startsWith(github.ref, 'refs/tags/v')
    uses: ./.github/workflows/release-gate.yml
"""

PUBLISH_JOB = """  publish:
    name: Publish
    runs-on: ubuntu-latest
    environment: fixture-env
    needs: [gate]
    if: startsWith(github.ref, 'refs/tags/v')
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4
      - name: Publish
        run: echo publish
"""

PUBLISH_IF = "    if: startsWith(github.ref, 'refs/tags/v')\n"
PINNED_CHECKOUT = "actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683"

SECRET_VERIFY_JOB = """  verify:
    runs-on: ubuntu-latest
    env:
      TOKEN: ${{ secrets.FIXTURE_TOKEN }}
    steps:
      - run: echo verify
"""

BRACKET_SECRET_VERIFY_JOB = """  verify:
    runs-on: ubuntu-latest
    env:
      TOKEN: ${{ secrets['FIXTURE_TOKEN'] }}
    steps:
      - run: echo verify
"""

INHERIT_DELEGATE_JOB = """  delegate:
    uses: ./.github/workflows/release-gate.yml
    secrets: inherit
"""

ID_TOKEN_PROBE_JOB = """  probe:
    runs-on: ubuntu-latest
    permissions:
      id-token: write
    steps:
      - run: echo probe
"""

OIDC_SECRET_PUBLISH_JOB = PUBLISH_JOB.replace(
    "        run: echo publish\n",
    "        run: echo publish\n        env:\n          TOKEN: ${{ secrets.FIXTURE_TOKEN }}\n",
)

WORKFLOW_LEVEL_SECRET_HEADER = HEADER.replace(
    "permissions:\n",
    "env:\n  TOKEN: ${{ secrets.FIXTURE_TOKEN }}\npermissions:\n",
)

WORKFLOW_LEVEL_ID_TOKEN_HEADER = HEADER.replace(
    "  contents: read\n",
    "  contents: read\n  id-token: write\n",
)

CASES = (
    # label, workflow text, --oidc, expected message (None = must be accepted)
    ("good-workflow", HEADER + GATE_JOB + PUBLISH_JOB, True, None),
    (
        "no-tag-push",
        HEADER.replace('    tags: ["v*"]\n', "") + GATE_JOB + PUBLISH_JOB,
        False,
        "on.push.tags must contain 'v*'",
    ),
    (
        "no-workflow-dispatch",
        HEADER.replace("  workflow_dispatch:\n", "") + GATE_JOB + PUBLISH_JOB,
        False,
        "on.workflow_dispatch is required",
    ),
    ("no-gate-job", HEADER + PUBLISH_JOB, False, "no job named gate"),
    (
        "publish-not-needing-gate",
        HEADER + GATE_JOB + PUBLISH_JOB.replace("    needs: [gate]\n", ""),
        False,
        "publish job does not need the release gate",
    ),
    (
        "push-only-publish-if",
        HEADER + GATE_JOB + PUBLISH_JOB.replace(PUBLISH_IF, "    if: github.event_name == 'push' && " + TAG_GATE + "\n"),
        False,
        "github.event_name == 'push'",
    ),
    (
        "always-in-publish-if",
        HEADER + GATE_JOB + PUBLISH_JOB.replace(PUBLISH_IF, "    if: always() && " + TAG_GATE + "\n"),
        False,
        "publish job if contains always()",
    ),
    (
        "or-in-publish-if",
        HEADER + GATE_JOB + PUBLISH_JOB.replace(PUBLISH_IF, "    if: " + TAG_GATE + " || github.event_name == 'workflow_dispatch'\n"),
        False,
        "publish job if contains ||",
    ),
    (
        "secret-outside-publish",
        HEADER + GATE_JOB + PUBLISH_JOB + SECRET_VERIFY_JOB,
        False,
        "secret referenced outside the publish job: verify",
    ),
    (
        "bracket-secret",
        HEADER + GATE_JOB + PUBLISH_JOB + BRACKET_SECRET_VERIFY_JOB,
        False,
        "secret referenced outside the publish job: verify",
    ),
    (
        "workflow-level-secret",
        WORKFLOW_LEVEL_SECRET_HEADER + GATE_JOB + PUBLISH_JOB,
        False,
        "secret referenced outside the publish job: workflow-level",
    ),
    (
        "secrets-inherit",
        HEADER + GATE_JOB + PUBLISH_JOB + INHERIT_DELEGATE_JOB,
        False,
        "secrets: inherit is not allowed",
    ),
    (
        "oidc-with-secret",
        HEADER + GATE_JOB + OIDC_SECRET_PUBLISH_JOB,
        True,
        "secret referenced in an OIDC workflow: publish",
    ),
    (
        "id-token-outside-publish",
        HEADER + GATE_JOB + PUBLISH_JOB + ID_TOKEN_PROBE_JOB,
        False,
        "id-token: write granted outside the publish job: probe",
    ),
    (
        "id-token-at-workflow-level",
        WORKFLOW_LEVEL_ID_TOKEN_HEADER + GATE_JOB + PUBLISH_JOB,
        False,
        "id-token: write granted outside the publish job: workflow level",
    ),
    (
        "unpinned-action",
        HEADER + GATE_JOB + PUBLISH_JOB.replace(PINNED_CHECKOUT, "actions/checkout@v7"),
        False,
        "publish job uses an unpinned action: actions/checkout@v7",
    ),
    (
        "no-concurrency",
        HEADER.replace(CONCURRENCY_BLOCK, "") + GATE_JOB + PUBLISH_JOB,
        False,
        "top-level concurrency with cancel-in-progress: false is required",
    ),
)


def self_test_failed(label: str, detail: str) -> None:
    print(f"self-test: FAILED [{label}]: {detail}", file=sys.stderr)
    sys.exit(1)


def self_test() -> None:
    tmp = tempfile.mkdtemp(prefix="check-release-workflows-selftest.")
    try:
        for label, workflow, oidc, expected in CASES:
            path = os.path.join(tmp, label + ".yml")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(workflow)
            violations = check_workflow(path, FIXTURE_ENVIRONMENT, oidc)
            if expected is None:
                if violations:
                    self_test_failed(label, "; ".join(violations))
                print(f"case {label}: accepted")
                continue
            if not violations:
                self_test_failed(label, "expected a rejection, but the workflow passed")
            if len(violations) != 1 or expected not in violations[0]:
                self_test_failed(label, f"expected exactly '{expected}', got: {'; '.join(violations)}")
            print(f"case {label}: rejected")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print("self-test: ok")


def main(argv: list) -> int:
    workflow = None
    environment = None
    oidc = False
    run_all = False
    want_self_test = False

    index = 0
    while index < len(argv):
        argument = argv[index]
        if argument == "--workflow":
            index += 1
            if index >= len(argv):
                return usage_error("--workflow needs a path")
            workflow = argv[index]
        elif argument == "--environment":
            index += 1
            if index >= len(argv):
                return usage_error("--environment needs a name")
            environment = argv[index]
        elif argument == "--oidc":
            oidc = True
        elif argument == "--all":
            run_all = True
        elif argument == "--self-test":
            want_self_test = True
        elif argument in ("-h", "--help"):
            usage()
            return 0
        else:
            return usage_error(f"unknown argument '{argument}'")
        index += 1

    if want_self_test:
        self_test()
        return 0

    if run_all:
        failures = 0
        for relative, environment_name, oidc_required in ALL_WORKFLOWS:
            for message in check_workflow(str(ROOT / relative), environment_name, oidc_required):
                violation(relative, message)
                failures += 1
        if failures:
            return 1
        print("release workflows: ok")
        return 0

    if workflow is None or environment is None:
        return usage_error("either --self-test, --all, or both --workflow and --environment are required")

    messages = check_workflow(workflow, environment, oidc)
    for message in messages:
        violation(workflow, message)
    if messages:
        return 1
    print(f"release workflow ok: {workflow}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
