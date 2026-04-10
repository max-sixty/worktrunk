#!/usr/bin/env bash
#
# TEMPORARY: This E2E test is included for reviewer validation only.
# It will be removed before merging.
#
# E2E test for `wt sync` — full workflow with 5-PR stack.
#
# Tests the complete stacked-PR workflow including:
#   1. Initial sync + auto stack file creation
#   2. Main advances → rebase all
#   3. Mid-stack commit → rebase downstream
#   4. PR1 squash-merged to main → reparent with --onto
#   5. PR3 squash-merged into PR2 (non-default branch merge) → detected!
#   6. --push with deleted remote branches
#
# Stack: main <- pr1 <- pr2 <- pr3 <- pr4 <- pr5
#
# Requires: gh (authenticated), git, wt (in PATH or via --wt-path)
#
# Usage:
#   ./test-sync-e2e.sh                     # uses `wt` from PATH
#   ./test-sync-e2e.sh --wt-path ./target/release/wt
#   ./test-sync-e2e.sh --keep              # don't prompt for cleanup
#   ./test-sync-e2e.sh --auto-cleanup      # delete repo without prompting

set -euo pipefail

# ─── Parse arguments ──────────────────────────────────────────────────────

WT_PATH=""
CLEANUP_MODE="prompt"  # prompt | keep | auto

while [[ $# -gt 0 ]]; do
    case "$1" in
        --wt-path)
            WT_PATH="$2"
            shift 2
            ;;
        --keep)
            CLEANUP_MODE="keep"
            shift
            ;;
        --auto-cleanup)
            CLEANUP_MODE="auto"
            shift
            ;;
        *)
            echo "Unknown option: $1" >&2
            echo "Usage: $0 [--wt-path PATH] [--keep | --auto-cleanup]" >&2
            exit 1
            ;;
    esac
done

# Resolve wt binary
if [[ -n "${WT_PATH}" ]]; then
    WT_BIN="$(cd "$(dirname "${WT_PATH}")" && pwd)/$(basename "${WT_PATH}")"
    if [[ ! -x "${WT_BIN}" ]]; then
        echo "Error: wt binary not found or not executable: ${WT_BIN}" >&2
        exit 1
    fi
else
    WT_BIN="$(command -v wt 2>/dev/null || true)"
    if [[ -z "${WT_BIN}" ]]; then
        echo "Error: wt not found in PATH. Use --wt-path to specify location." >&2
        exit 1
    fi
fi

echo "Using wt: ${WT_BIN}"
"${WT_BIN}" --version

# ─── Verify prerequisites ────────────────────────────────────────────────

for cmd in gh git; do
    if ! command -v "${cmd}" &>/dev/null; then
        echo "Error: ${cmd} is required but not found in PATH." >&2
        exit 1
    fi
done

if ! gh auth status &>/dev/null; then
    echo "Error: gh is not authenticated. Run 'gh auth login' first." >&2
    exit 1
fi

# ─── Setup ────────────────────────────────────────────────────────────────

GH_USER=$(gh api user -q .login)
REPO_NAME="wt-sync-e2e-$$"  # unique per invocation via PID
REPO_FULL="${GH_USER}/${REPO_NAME}"
WORK_DIR=$(mktemp -d)
REPO_DIR="${WORK_DIR}/${REPO_NAME}"

bold='\033[1m'
green='\033[32m'
red='\033[31m'
yellow='\033[33m'
cyan='\033[36m'
reset='\033[0m'

PASS=0
FAIL=0

info()  { echo -e "${cyan}${bold}==> ${1}${reset}"; }
ok()    { echo -e "${green}${bold}  ✓ ${1}${reset}"; PASS=$((PASS + 1)); }
fail()  { echo -e "${red}${bold}  ✗ ${1}${reset}"; FAIL=$((FAIL + 1)); }
separator() { echo "────────────────────────────────────────"; }

check() {
    local description="$1"
    local condition="$2"
    if eval "$condition"; then
        ok "$description"
    else
        fail "$description"
    fi
}

show_branches() {
    info "Branch state:"
    cd "${REPO_DIR}"
    for branch in pr1 pr2 pr3 pr4 pr5; do
        if git rev-parse --verify "${branch}" &>/dev/null; then
            local count
            count=$(git rev-list --count main.."${branch}" 2>/dev/null || echo "?")
            echo "  ${branch}: ${count} commits ahead of main"
        else
            echo "  ${branch}: (not present)"
        fi
    done
}

show_stack_file() {
    local stack_file="${REPO_DIR}/.git/wt/stack"
    if [[ -f "${stack_file}" ]]; then
        info "Stack file (.git/wt/stack):"
        cat "${stack_file}"
        echo ""
    else
        info "No stack file"
    fi
}

# Alias wt to our resolved binary
wt() { "${WT_BIN}" "$@"; }

cleanup() {
    echo ""
    echo -e "${bold}═══ Cleanup ═══${reset}"
    echo "Work directory: ${WORK_DIR}"
    echo "GitHub repo:    ${REPO_FULL}"
    echo ""

    case "${CLEANUP_MODE}" in
        keep)
            echo "Kept (--keep). Clean up manually:"
            echo "  gh repo delete ${REPO_FULL} --yes"
            echo "  rm -rf ${WORK_DIR}"
            ;;
        auto)
            gh repo delete "${REPO_FULL}" --yes 2>/dev/null && ok "Deleted GitHub repo" || echo "  Could not delete repo"
            rm -rf "${WORK_DIR}" && ok "Deleted ${WORK_DIR}" || echo "  Could not delete work dir"
            ;;
        prompt)
            read -rp "Delete the GitHub repo and local files? [y/N] " confirm
            if [[ "${confirm}" =~ ^[Yy]$ ]]; then
                gh repo delete "${REPO_FULL}" --yes 2>/dev/null && ok "Deleted GitHub repo" || echo "  Could not delete repo"
                rm -rf "${WORK_DIR}" && ok "Deleted ${WORK_DIR}" || echo "  Could not delete work dir"
            else
                echo "Kept. Clean up manually:"
                echo "  gh repo delete ${REPO_FULL} --yes"
                echo "  rm -rf ${WORK_DIR}"
            fi
            ;;
    esac
}

# Clean up on exit (including failures)
trap 'cleanup' EXIT

# ─── Create repo ──────────────────────────────────────────────────────────

info "Creating private repo: ${REPO_FULL}"
cd "${WORK_DIR}"
gh repo create "${REPO_NAME}" --private --clone --add-readme -d "wt sync e2e testing"
cd "${REPO_DIR}"

echo "# E2E test" > README.md
echo "module main" > go.mod
git add -A && git commit -m "Initial project setup"
git push
ok "Repo created at ${REPO_FULL}"

# ─── Create 5-PR stack ───────────────────────────────────────────────────

info "Creating stacked worktrees: main <- pr1 <- pr2 <- pr3 <- pr4 <- pr5"

git worktree add -b pr1 "${WORK_DIR}/repo.pr1"
cd "${WORK_DIR}/repo.pr1"
echo "func Auth() {}" > auth.go
git add -A && git commit -m "pr1: add auth"
git push -u origin pr1 --force
cd "${REPO_DIR}"

git worktree add -b pr2 "${WORK_DIR}/repo.pr2" pr1
cd "${WORK_DIR}/repo.pr2"
echo "func Login() {}" > login.go
git add -A && git commit -m "pr2: add login"
git push -u origin pr2 --force
cd "${REPO_DIR}"

git worktree add -b pr3 "${WORK_DIR}/repo.pr3" pr2
cd "${WORK_DIR}/repo.pr3"
echo "func Session() {}" > session.go
git add -A && git commit -m "pr3: add session"
git push -u origin pr3 --force
cd "${REPO_DIR}"

git worktree add -b pr4 "${WORK_DIR}/repo.pr4" pr3
cd "${WORK_DIR}/repo.pr4"
echo "func Middleware() {}" > middleware.go
git add -A && git commit -m "pr4: add middleware"
git push -u origin pr4 --force
cd "${REPO_DIR}"

git worktree add -b pr5 "${WORK_DIR}/repo.pr5" pr4
cd "${WORK_DIR}/repo.pr5"
echo "func Handler() {}" > handler.go
git add -A && git commit -m "pr5: add handler"
git push -u origin pr5 --force
cd "${REPO_DIR}"

ok "Worktrees created"

info "Creating GitHub PRs"
gh pr create --head pr1 --base main \
    --title "pr1: auth" --body "Auth." --repo "${REPO_FULL}" 2>/dev/null || true
gh pr create --head pr2 --base pr1 \
    --title "pr2: login" --body "Login. Stacked on pr1." --repo "${REPO_FULL}" 2>/dev/null || true
gh pr create --head pr3 --base pr2 \
    --title "pr3: session" --body "Session. Stacked on pr2." --repo "${REPO_FULL}" 2>/dev/null || true
gh pr create --head pr4 --base pr3 \
    --title "pr4: middleware" --body "Middleware. Stacked on pr3." --repo "${REPO_FULL}" 2>/dev/null || true
gh pr create --head pr5 --base pr4 \
    --title "pr5: handler" --body "Handler. Stacked on pr4." --repo "${REPO_FULL}" 2>/dev/null || true
ok "PRs created"

echo ""
info "Setup complete: https://github.com/${REPO_FULL}/pulls"
show_branches

# ═══════════════════════════════════════════════════════════════════════════
# Test 1: Initial sync creates stack file automatically
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${bold}═══ Test 1: Initial sync + auto stack file ═══${reset}"

cd "${REPO_DIR}"
info "Running: wt sync --dry-run"
separator
wt sync --dry-run 2>&1
separator

check "Stack file exists after dry-run" "[[ -f ${REPO_DIR}/.git/wt/stack ]]"
check "Stack file contains pr1" "grep -q pr1 ${REPO_DIR}/.git/wt/stack"
check "Stack file contains pr5" "grep -q pr5 ${REPO_DIR}/.git/wt/stack"
show_stack_file

# ═══════════════════════════════════════════════════════════════════════════
# Test 2: Main advances → rebase all branches
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${bold}═══ Test 2: Main advances → rebase all ═══${reset}"

cd "${REPO_DIR}"
echo 'version = "1.0"' > version.txt
git add -A && git commit -m "chore: add version file"
git push

info "Running: wt sync --push"
separator
wt sync --push 2>&1
separator

check "pr1 has version.txt after rebase" "[[ -f ${WORK_DIR}/repo.pr1/version.txt ]]"
check "pr5 has version.txt after rebase" "[[ -f ${WORK_DIR}/repo.pr5/version.txt ]]"
show_branches

# ═══════════════════════════════════════════════════════════════════════════
# Test 3: Mid-stack commit → downstream rebases
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${bold}═══ Test 3: Mid-stack commit → downstream rebases ═══${reset}"

cd "${WORK_DIR}/repo.pr1"
echo "func Logout() {}" >> auth.go
git add -A && git commit -m "pr1: add logout"
git push

cd "${REPO_DIR}"
info "Running: wt sync --push"
separator
wt sync --push 2>&1
separator

check "pr2 has logout after mid-stack rebase" "grep -q Logout ${WORK_DIR}/repo.pr2/auth.go"
check "pr5 has logout after mid-stack rebase" "grep -q Logout ${WORK_DIR}/repo.pr5/auth.go"
show_branches

# ═══════════════════════════════════════════════════════════════════════════
# Test 4: PR1 squash-merged to main → reparent with --onto
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${bold}═══ Test 4: PR1 squash-merged to main ═══${reset}"

info "Squash-merging PR1 via GitHub CLI"
gh pr merge pr1 --squash --repo "${REPO_FULL}"

cd "${REPO_DIR}"
info "Running: wt sync --fetch --push"
separator
wt sync --fetch --push 2>&1
separator

show_stack_file
show_branches

check "Stack file does NOT contain pr1" "! grep -q '^pr1$' ${REPO_DIR}/.git/wt/stack"
check "pr2 still has login.go" "[[ -f ${WORK_DIR}/repo.pr2/login.go ]]"
check "pr2 has auth.go (from merged pr1)" "[[ -f ${WORK_DIR}/repo.pr2/auth.go ]]"

# ═══════════════════════════════════════════════════════════════════════════
# Test 5: PR3 squash-merged into PR2 (non-default branch merge!)
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${bold}═══ Test 5: PR3 squash-merged into PR2 (non-default branch!) ═══${reset}"
echo -e "${yellow}This is the key test — PR3 merged into its parent PR2 (not main).${reset}"

# Simulate squash-merge of PR3 into PR2 locally.
# Stack before: main ← pr2 ← pr3 ← pr4 ← pr5
# After merge:  main ← pr2 (contains pr3's changes) ← pr4 ← pr5
info "Simulating squash-merge of PR3 into PR2 (locally)"
cd "${WORK_DIR}/repo.pr2"
echo "func Session() {}" > session.go
git add -A && git commit -m "squash-merge pr3 into pr2"
git push --force-with-lease

cd "${REPO_DIR}"
info "Running: wt sync"
separator
wt sync 2>&1
separator

show_stack_file
show_branches

check "Stack file does NOT contain pr3" "! grep -q '^[[:space:]]*pr3' ${REPO_DIR}/.git/wt/stack"
check "pr4 has session.go (from pr3 via pr2)" "[[ -f ${WORK_DIR}/repo.pr4/session.go ]]"
check "pr5 has middleware.go" "[[ -f ${WORK_DIR}/repo.pr5/middleware.go ]]"

# ═══════════════════════════════════════════════════════════════════════════
# Test 6: --push with pruned remote branches
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${bold}═══ Test 6: --push with pruned remote branches ═══${reset}"

cd "${REPO_DIR}"
info "Deleting remote pr1 and pr3 branches"
git push origin --delete pr1 2>/dev/null || true
git push origin --delete pr3 2>/dev/null || true
git fetch --prune

info "Running: wt sync --push"
separator
wt sync --push 2>&1
separator

check "Sync with --push completes without error" "true"

# ═══════════════════════════════════════════════════════════════════════════
# Summary
# ═══════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${bold}═══════════════════════════════════════════${reset}"
echo -e "${bold}  Results: ${green}${PASS} passed${reset}, ${red}${FAIL} failed${reset}"
echo -e "${bold}═══════════════════════════════════════════${reset}"
echo ""
echo "Test 1: Auto stack file creation"
echo "Test 2: Main advances → rebase all"
echo "Test 3: Mid-stack commit → rebase downstream"
echo "Test 4: PR1 squash-merged to main → reparent"
echo "Test 5: PR3 merged into PR2 (non-default branch detection)"
echo "Test 6: --push with deleted remote branches"
echo ""

# Exit with failure if any tests failed
if [[ "${FAIL}" -gt 0 ]]; then
    exit 1
fi
