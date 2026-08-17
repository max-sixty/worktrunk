#!/usr/bin/env bash
# Probe body for star-history-token-probe.yaml. Reports rather than gates: it
# exits 0 whatever it finds, and the verdict is the job summary. Delete with
# the workflow once the answer is recorded.
#
# "Denied" and "the API is having a bad day" have to stay distinguishable: a
# GitHub incident makes these endpoints 503, and reading that as a denial is
# how you conclude a token lacks access it actually has. Only an authorization
# status decides the verdict; anything else is reported as inconclusive.
set -uo pipefail

OWNER="${GITHUB_REPOSITORY%%/*}"
NAME="${GITHUB_REPOSITORY##*/}"

# REST status, retried while the answer looks transient rather than final.
rest_status() {
  local status=""
  for _ in 1 2 3; do
    status=$(gh api "$1" --include 2>/dev/null | head -1 | grep -oE '[0-9]{3}' | tail -1)
    case "$status" in
      200 | 401 | 403 | 404) break ;;
    esac
    sleep 5
  done
  printf '%s' "${status:-error}"
}

own_rest=$(rest_status "repos/${GITHUB_REPOSITORY}/stargazers?per_page=1")

# The control. Nobody here collaborates on this repo, so a 404 confirms the
# restriction is in force — without it a 200 above would prove nothing, since
# it could just mean GitHub had stopped restricting entirely.
control_rest=$(rest_status "repos/rust-lang/rust/stargazers?per_page=1")

gql_out=""
gql_count=""
for _ in 1 2 3; do
  gql_out=$(gh api graphql -f query="
{
  repository(owner: \"${OWNER}\", name: \"${NAME}\") {
    stargazers(first: 1) { totalCount }
  }
}" 2>&1)
  gql_count=$(printf '%s' "$gql_out" | grep -oE '"totalCount":[0-9]+' | grep -oE '[0-9]+')
  [ -n "$gql_count" ] && break
  printf '%s' "$gql_out" | grep -qiE "no server is currently available|502|503|timeout" || break
  sleep 5
done

case "$own_rest" in
  200)
    verdict="**Yes** — \`github.token\` reads this repo's stargazers. Drop \`STARGAZERS_TOKEN\` and the \`star-history\` environment, and give the workflow this permission set."
    ;;
  401 | 403 | 404)
    verdict="**No** — \`github.token\` is refused (\`${own_rest}\`). It is not treated as an admin or collaborator, so \`STARGAZERS_TOKEN\` stays."
    ;;
  *)
    verdict="**Inconclusive** — REST answered \`${own_rest}\`, which is neither success nor a refusal. Re-run; do not read this as a denial."
    ;;
esac

if [ "$control_rest" != "404" ]; then
  verdict="${verdict}

⚠️ Control returned \`${control_rest}\` rather than 404, so the restriction may not be in force. Treat the verdict as unproven until a re-run shows 404 here."
fi

{
  echo "## ${LABEL}"
  echo
  echo "$verdict"
  echo
  echo "| check | result |"
  echo "| --- | --- |"
  echo "| REST \`/stargazers\` (this repo) | \`${own_rest}\` |"
  echo "| REST \`/stargazers\` (rust-lang/rust, control) | \`${control_rest}\` |"
  echo "| GraphQL \`repository.stargazers\` | ${gql_count:-no count returned} |"
} >> "$GITHUB_STEP_SUMMARY"

echo "::notice title=${LABEL}::REST=${own_rest} control=${control_rest} graphql=${gql_count:-failed}"
printf '%s\n' "--- GraphQL response ---" "$gql_out"
